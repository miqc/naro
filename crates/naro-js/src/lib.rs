//! A real JavaScript runtime bound to NARO's own DOM.
//!
//! One `JsSession` lives for the lifetime of a document. It owns a QuickJS
//! runtime and the live `Document`; the engine reads a snapshot after every
//! interaction. Navigations drop the whole session, so timers, listeners and
//! node handles from the previous page cannot reach the next one.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use naro_html::{Document, NodeData, NodeId};
use rquickjs::{Context, Function, Runtime};

#[derive(Debug, Default, Clone)]
pub struct ScriptOutcome {
    pub logs: Vec<String>,
    pub errors: Vec<String>,
}

impl ScriptOutcome {
    pub fn merge(&mut self, other: ScriptOutcome) {
        self.logs.extend(other.logs);
        self.errors.extend(other.errors);
    }
}

/// Wall-clock budget for one synchronous script or callback batch.
const SCRIPT_BUDGET: Duration = Duration::from_millis(250);
/// Hard cap on QuickJS heap, so a runaway allocation becomes a JS error.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

thread_local! {
    /// Deadline for the currently running script. QuickJS is single-threaded,
    /// so a thread-local avoids any shared-state or unsafe trickery.
    static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Interrupt handler: QuickJS calls this periodically while executing bytecode.
fn deadline_exceeded() -> bool {
    DEADLINE.with(|d| match d.get() {
        Some(at) if Instant::now() >= at => {
            HIT_DEADLINE.with(|hit| hit.set(true));
            true
        }
        _ => false,
    })
}

thread_local! {
    static HIT_DEADLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn deadline_was_hit() -> bool {
    HIT_DEADLINE.with(|hit| {
        let v = hit.get();
        hit.set(false);
        v
    })
}

/// Run `f` with a hard wall-clock deadline installed.
fn with_deadline<T>(f: impl FnOnce() -> T) -> T {
    DEADLINE.with(|d| d.set(Some(Instant::now() + SCRIPT_BUDGET)));
    let out = f();
    DEADLINE.with(|d| d.set(None));
    out
}

/// Run `f` with the script deadline suspended.
///
/// A host function that waits on the network is not running bytecode, so the
/// interrupt handler cannot fire anyway; clearing the deadline is what keeps
/// the *next* instruction from being interrupted for time the script did not
/// spend. The previous deadline is restored on the way out.
fn without_deadline<T>(f: impl FnOnce() -> T) -> T {
    DEADLINE.with(|d| {
        let saved = d.get();
        d.set(None);
        let out = f();
        d.set(saved);
        out
    })
}

/// What a DOM mutation made stale.
///
/// Produced by the JavaScript side through `__naro_invalidate`, consumed once
/// per task by the engine. The three flags respect their dependencies: a style
/// change always repaints, a layout change always repaints, and a paint-only
/// change never triggers layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Invalidation {
    /// Computed styles must be recalculated (DOM, attributes or CSS changed).
    pub style: bool,
    /// Boxes must be rebuilt (geometry-affecting properties changed).
    pub layout: bool,
    /// The frame must be redrawn.
    pub paint: bool,
}

impl Invalidation {
    pub fn mark_style(&mut self) {
        self.style = true;
        self.paint = true;
    }

    pub fn mark_layout(&mut self) {
        self.layout = true;
        self.paint = true;
    }

    pub fn mark_paint(&mut self) {
        self.paint = true;
    }

    pub fn merge(&mut self, other: Invalidation) {
        self.style |= other.style;
        self.layout |= other.layout;
        self.paint |= other.paint;
    }

    pub fn is_empty(&self) -> bool {
        !self.style && !self.layout && !self.paint
    }

    pub fn clear(&mut self) {
        *self = Invalidation::default();
    }
}

/// Border box of one element in page coordinates, plus its border widths.
///
/// The engine fills this from the real layout and hands it to the session, so
/// `getBoundingClientRect` and friends read measured geometry instead of
/// re-deriving it from CSS. `naro-js` deliberately does not depend on
/// `naro-layout`: the engine converts, the session stores.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub border_top: f32,
    pub border_right: f32,
    pub border_bottom: f32,
    pub border_left: f32,
}

/// A history change requested by page script.
///
/// `naro-js` cannot touch the engine, so `history.pushState` and friends queue
/// an op that the engine applies on its next [`Engine::flush`], which is where
/// the URL and the session's own history vector are already rewritten.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryOp {
    /// `history.pushState` (`replace = false`) or `replaceState`.
    Push { url: String, replace: bool },
    Back,
    Forward,
    /// `history.go(n)`: negative goes back, positive forward.
    Go(i64),
}

/// A form action requested by page script.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FormOp {
    /// `form.submit()`.
    Submit,
    /// `form.reset()`.
    Reset,
    /// A `<select>` was asked to open (`element.click()` on a select).
    OpenSelect,
    /// `element.click()`: script already dispatched the `click` event, so the
    /// engine only has to run the default action the browser would.
    DefaultClick,
}

struct Dom {
    doc: Document,
    logs: Vec<String>,
    pending_errors: Vec<String>,
    /// Controls whose `value` property was written from JavaScript.
    value_dirty: Vec<usize>,
    /// Mutations since the engine last rebuilt styles and layout.
    invalidation: Invalidation,
    /// Measured border boxes, indexed by `NodeId`. `None` means "not laid out".
    geometry: Vec<Option<ElementBox>>,
    /// Computed styles indexed by `NodeId`, mirroring the engine's `styles`, so
    /// `getComputedStyle` reads the cascade the renderer actually used.
    computed: Vec<naro_css::ComputedStyle>,
    /// `document.readyState`, driven by the load sequence.
    ready_state: &'static str,
    /// History changes queued by script, drained by the engine.
    history_ops: Vec<HistoryOp>,
    /// Form actions queued by script, drained by the engine.
    form_ops: Vec<(usize, FormOp)>,
    /// Selected text, published by the engine after every selection change.
    selection_text: String,
    /// Index of the current entry in the engine's history vector.
    history_index: i64,
    /// Viewport the page was laid out at, for `matchMedia`.
    viewport: (f32, f32),
    /// Document URL, backing `location`.
    url: String,
}

impl Dom {
    /// `[target, parent, ..., root]`; QuickJS receives these as plain integers.
    fn ancestors(&self, node: NodeId) -> Vec<i64> {
        let mut out = vec![node as i64];
        let mut current = self.doc.node(node).parent;
        while let Some(parent) = current {
            out.push(parent as i64);
            current = self.doc.node(parent).parent;
        }
        out
    }
}

const BOOTSTRAP: &str = r##"
(function () {
  const listeners = [];
  const timers = [];
  const microtasks = [];
  let nextTimer = 1;
  let currentEvent = null;

  const report = (e) => __naro_error(String(e && e.message ? e.message : e));

  // One object per node, for as long as the node exists. Node ids come from a
  // Vec<Node> that never recycles slots, so a cached wrapper can only ever
  // describe the node it was made for; a removed node keeps its identity
  // instead of silently pointing at whatever took its place.
  //
  // The cache holds a WeakRef, not the object: identity is observable only
  // while some script still references the wrapper, which is exactly the
  // browser contract, and a long-lived page that churns nodes does not grow a
  // permanent table. FinalizationRegistry drops the dead key.
  const wrappers = new Map();
  const hasWeakRef = typeof WeakRef === "function";
  const finalizer = typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry((id) => {
        const ref = wrappers.get(id);
        if (ref !== undefined && ref.deref() === undefined) wrappers.delete(id);
      })
    : null;
  const wrap = (id) => {
    if (id === undefined || id === null || id < 0) return null;
    const cached = wrappers.get(id);
    if (cached !== undefined) {
      const live = cached.deref();
      if (live !== undefined) return live;
      wrappers.delete(id);
    }
    const el = {
      __id: id,
      get id() { return __naro_get(id, "id") || ""; },
      set id(v) { __naro_set(id, "id", String(v)); __naro_mutation("attributes", id, "id", -1, -1); },
      get className() { return __naro_get(id, "class") || ""; },
      set className(v) { __naro_set(id, "class", String(v)); __naro_mutation("attributes", id, "class", -1, -1); },
      get classList() {
        const read = () => (__naro_get(id, "class") || "").split(/\s+/).filter(Boolean);
        const write = (list) => __naro_set(id, "class", list.join(" "));
        return {
          add: (...names) => { const l = read(); for (const n of names) if (!l.includes(n)) l.push(n); write(l); __naro_mutation("attributes", id, "class", -1, -1); },
          remove: (...names) => { write(read().filter((c) => !names.includes(c))); __naro_mutation("attributes", id, "class", -1, -1); },
          toggle: (n) => { const l = read(); const has = l.includes(n); write(has ? l.filter((c) => c !== n) : l.concat(n)); __naro_mutation("attributes", id, "class", -1, -1); return !has; },
          contains: (n) => read().includes(n),
          get length() { return read().length; },
          item: (i) => read()[i] || null
        };
      },
      get style() {
        // `el.style.backgroundColor` is camelCase; the cascade reads
        // `background-color`. Without this every camelCase write was stored
        // under a name the parser ignores.
        const cssName = (k) => (k.startsWith("--") ? k : k.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase()));
        const current = () => {
          const out = {};
          for (const decl of (__naro_get(id, "style") || "").split(";")) {
            const i = decl.indexOf(":");
            if (i > 0) out[decl.slice(0, i).trim()] = decl.slice(i + 1).trim();
          }
          return out;
        };
        const flush = (map) => {
          __naro_set(id, "style", Object.keys(map).map((k) => k + ":" + map[k]).join(";"));
          __naro_mutation("attributes", id, "style", -1, -1);
        };
        const api = {
          setProperty: (k, v) => { const m = current(); m[cssName(String(k))] = String(v); flush(m); },
          getPropertyValue: (k) => current()[cssName(String(k))] || "",
          removeProperty: (k) => { const m = current(); delete m[cssName(String(k))]; flush(m); }
        };
        return new Proxy(api, {
          get: (t, k) => (typeof k === "string" && !(k in t)) ? (current()[cssName(k)] || "") : t[k],
          set: (t, k, v) => { if (typeof k === "string") { const m = current(); m[cssName(k)] = String(v); flush(m); } return true; }
        });
      },
      get textContent() { return __naro_text(id); },
      set textContent(v) { __naro_set_text(id, String(v)); __naro_mutation("childList", id, null, -1, -1); },
      get innerText() { return __naro_text(id); },
      set innerText(v) { __naro_set_text(id, String(v)); __naro_mutation("childList", id, null, -1, -1); },
      get innerHTML() { return __naro_inner_html(id); },
      set innerHTML(v) {
        __naro_set_inner_html(id, String(v));
        ceUpgradeTree(id);
        __naro_mutation("childList", id, null, -1, -1);
      },
      get parentNode() { const p = __naro_parent(id); return p < 0 ? null : wrap(p); },
      removeChild(child) {
        ceDisconnectTree(child.__id);
        __naro_remove_child(id, child.__id);
        __naro_mutation("childList", id, null, -1, child.__id);
        return child;
      },
      get value() { const v = __naro_value(id); return v === null ? "" : v; },
      set value(v) { __naro_set(id, "value", String(v)); __naro_invalidate("paint"); __naro_value_changed(id); },
      getAttribute(n) {
        const v = __naro_get(id, String(n));
        // `__naro_get` maps a missing attribute to `undefined`; the DOM API
        // returns `null`, and pages test for that explicitly.
        return v === null || v === undefined ? null : v;
      },
      setAttribute(n, v) { __naro_set(id, String(n), String(v)); __naro_mutation("attributes", id, String(n), -1, -1); },
      hasAttribute(n) { return __naro_get(id, String(n)) != null; },
      toggleAttribute(n, force) {
        const name = String(n);
        const has = __naro_get(id, name) !== null;
        const want = force === undefined ? !has : !!force;
        if (want && !has) __naro_set(id, name, "");
        if (!want && has) __naro_remove_attr(id, name);
        __naro_mutation("attributes", id, name, -1, -1);
        return want;
      },
      removeAttribute(n) { __naro_remove_attr(id, String(n)); __naro_mutation("attributes", id, String(n), -1, -1); },
      appendChild(child) {
        __naro_append(id, child.__id);
        ceUpgradeTree(child.__id);
        __naro_mutation("childList", id, null, child.__id, -1);
        return child;
      },
      insertBefore(child, ref) {
        __naro_insert_before(id, child.__id, ref ? ref.__id : -1);
        ceUpgradeTree(child.__id);
        __naro_mutation("childList", id, null, child.__id, -1);
        return child;
      },
      // ParentNode.append/prepend take nodes or strings; strings become text
      // nodes, as in the spec.
      append(...nodes) {
        for (const n of nodes) {
          const node = typeof n === "string" ? document.createTextNode(n) : n;
          __naro_append(id, node.__id);
          ceUpgradeTree(node.__id);
          __naro_mutation("childList", id, null, node.__id, -1);
        }
      },
      prepend(...nodes) {
        const first = __naro_child_nodes(id)[0];
        for (const n of nodes) {
          const node = typeof n === "string" ? document.createTextNode(n) : n;
          __naro_insert_before(id, node.__id, first === undefined ? -1 : first);
          ceUpgradeTree(node.__id);
          __naro_mutation("childList", id, null, node.__id, -1);
        }
      },
      addEventListener(type, fn, opts) { __naro_add(id, String(type), fn, !!(opts && opts.capture)); },
      removeEventListener(type, fn) { __naro_remove(id, String(type), fn); },
      dispatchEvent(ev) {
        const path = [];
        let cur = id;
        while (cur >= 0) { path.push(cur); cur = __naro_parent(cur); }
        __naro_dispatch(id, ev, path);
        return !(ev && ev.defaultPrevented);
      },
      click() {
        const path = [];
        let cur = id;
        while (cur >= 0) { path.push(cur); cur = __naro_parent(cur); }
        const ev = new MouseEvent("click", { bubbles: true, cancelable: true });
        __naro_dispatch(id, ev, path);
        // A programmatic click runs the same default action a real click does.
        // A toggle is applied here, synchronously, so `el.checked` is already
        // true on the next statement; anything that needs the engine (submit,
        // reset, a select popup) is queued for its next flush.
        if (ev.defaultPrevented) return;
        if (__naro_toggle(id)) {
          const inputEv = new InputEvent("input", { bubbles: true });
          __naro_dispatch(id, inputEv, path);
          const changeEv = new Event("change", { bubbles: true });
          __naro_dispatch(id, changeEv, path);
          return;
        }
        __naro_form_op(id, "click");
      },
      get tagName() { return __naro_tag(id); },
      get nodeName() {
        if (__naro_is_fragment(id)) return "#document-fragment";
        if (__naro_is_shadow_root(id)) return "#document-fragment";
        if (__naro_tag(id) === "") return "#text";
        return __naro_parent(id) < 0 ? "#document" : __naro_tag(id);
      },
      get children() { return __naro_children(id).map(wrap); },
      get childNodes() { return __naro_child_nodes(id).map(wrap); },
      get firstChild() { const c = __naro_child_nodes(id); return c.length ? wrap(c[0]) : null; },
      get lastChild() { const c = __naro_child_nodes(id); return c.length ? wrap(c[c.length - 1]) : null; },
      get nextSibling() {
        const p = __naro_parent(id);
        if (p < 0) return null;
        const list = __naro_child_nodes(p);
        const i = list.indexOf(id);
        return i >= 0 && i + 1 < list.length ? wrap(list[i + 1]) : null;
      },
      get previousSibling() {
        const p = __naro_parent(id);
        if (p < 0) return null;
        const list = __naro_child_nodes(p);
        const i = list.indexOf(id);
        return i > 0 ? wrap(list[i - 1]) : null;
      },
      get parentElement() {
        const p = __naro_parent(id);
        // The document node is not an element, so it is not a parent element.
        if (p < 0 || __naro_tag(p) === "") return null;
        return wrap(p);
      },
      get nodeType() {
        // 11 = shadow root or fragment, 1 = element, 3 = text, 9 = document.
        if (__naro_is_shadow_root(id) || __naro_is_fragment(id)) return 11;
        if (__naro_tag(id) === "") return 3;
        return __naro_parent(id) < 0 ? 9 : 1;
      },
      get isConnected() { return __naro_parent(id) >= 0; },
      // --- Shadow DOM -----------------------------------------------------
      // A closed root exists in the tree but is not reachable from the host,
      // which is the whole point of `mode: "closed"`.
      get shadowRoot() {
        const root = __naro_shadow_root(id);
        if (root < 0 || __naro_shadow_mode(root) === "closed") return null;
        return wrap(root);
      },
      attachShadow(init) {
        if (__naro_shadow_root(id) >= 0) {
          throw new Error("NotSupportedError: shadow root already attached");
        }
        const mode = init && init.mode === "closed" ? "closed" : "open";
        const root = __naro_attach_shadow(id, mode);
        __naro_mutation("childList", id, null, -1, -1);
        return wrap(root);
      },
      // The host of a shadow root; `null` for anything else.
      get host() {
        if (!__naro_is_shadow_root(id)) return null;
        const p = __naro_parent(id);
        return p < 0 ? null : wrap(p);
      },
      get mode() {
        const mode = __naro_shadow_mode(id);
        return mode === "" ? null : mode;
      },
      getRootNode() {
        let cur = id;
        for (;;) {
          if (__naro_is_shadow_root(cur)) return wrap(cur);
          const p = __naro_parent(cur);
          if (p < 0) return wrap(cur);
          cur = p;
        }
      },
      remove() {
        const p = __naro_parent(id);
        if (p < 0) return;
        ceDisconnectTree(id);
        __naro_remove_child(p, id);
        __naro_mutation("childList", p, null, -1, id);
      },
      querySelector(selector) { return wrap(__naro_query_within(id, String(selector))); },
      querySelectorAll(selector) { return __naro_query_all_within(id, String(selector)).map(wrap); },
      matches(selector) { return __naro_matches(id, String(selector)); },
      closest(selector) {
        const sel = String(selector);
        let cur = id;
        while (cur >= 0) {
          if (__naro_tag(cur) !== "" && __naro_matches(cur, sel)) return wrap(cur);
          cur = __naro_parent(cur);
        }
        return null;
      },
      insertAdjacentHTML(position, html) {
        const nodes = __naro_insert_adjacent(id, String(position), String(html));
        if (nodes.length === 0) return;
        const host = position.toLowerCase() === "beforebegin" || position.toLowerCase() === "afterend"
          ? __naro_parent(id)
          : id;
        if (host < 0) return;
        ceUpgradeTree(host);
        for (const node of nodes) __naro_mutation("childList", host, null, node, -1);
      },
      // --- traversal and mutation (A1) -----------------------------------
      // Element-only navigation reads `__naro_element_children`, which skips
      // text nodes exactly like the DOM's element axes.
      get firstElementChild() {
        const c = __naro_element_children(id);
        return c.length ? wrap(c[0]) : null;
      },
      get lastElementChild() {
        const c = __naro_element_children(id);
        return c.length ? wrap(c[c.length - 1]) : null;
      },
      get childElementCount() { return __naro_element_children(id).length; },
      get nextElementSibling() {
        const p = __naro_parent(id);
        if (p < 0) return null;
        const list = __naro_element_children(p);
        const i = list.indexOf(id);
        return i >= 0 && i + 1 < list.length ? wrap(list[i + 1]) : null;
      },
      get previousElementSibling() {
        const p = __naro_parent(id);
        if (p < 0) return null;
        const list = __naro_element_children(p);
        const i = list.indexOf(id);
        return i > 0 ? wrap(list[i - 1]) : null;
      },
      getElementsByClassName(name) {
        const wanted = String(name).split(/\s+/).filter(Boolean);
        if (wanted.length === 0) return [];
        const first = __naro_elements_by_class(id, wanted[0]).map(wrap);
        if (wanted.length === 1) return first;
        // Multiple class names intersect, as in the DOM.
        return first.filter((el) => wanted.every((c) => el.classList.contains(c)));
      },
      getElementsByTagName(name) {
        return __naro_elements_by_tag_name(id, String(name)).map(wrap);
      },
      cloneNode(deep) {
        const clone = __naro_clone(id, !!deep);
        if (clone < 0) return null;
        return wrap(clone);
      },
      replaceChild(newNode, oldNode) {
        if (!oldNode || oldNode.__id === undefined) return null;
        __naro_insert_before(id, newNode.__id, oldNode.__id);
        ceUpgradeTree(newNode.__id);
        __naro_remove_child(id, oldNode.__id);
        __naro_mutation("childList", id, null, newNode.__id, oldNode.__id);
        return oldNode;
      },
      get outerHTML() { return __naro_outer_html(id); },
      set outerHTML(v) {
        const p = __naro_parent(id);
        __naro_set_outer_html(id, String(v));
        if (p >= 0) ceUpgradeTree(p);
        __naro_mutation("childList", p < 0 ? id : p, null, -1, id);
      },
      insertAdjacentText(position, text) {
        const node = __naro_insert_adjacent_text(id, String(position), String(text));
        if (node < 0) return;
        const pos = String(position).toLowerCase();
        const host = pos === "beforebegin" || pos === "afterend" ? __naro_parent(id) : id;
        if (host < 0) return;
        __naro_mutation("childList", host, null, node, -1);
      },
      // `form.submit()` skips validation and the `submit` event, as in the
      // DOM; `requestSubmit()` would fire it, which this engine does not model.
      submit() { __naro_form_op(id, "submit"); },
      reset() { __naro_form_op(id, "reset"); },
      get elements() {
        return __naro_elements_by_tag_name(id, "input")
          .concat(__naro_elements_by_tag_name(id, "select"))
          .concat(__naro_elements_by_tag_name(id, "textarea"))
          .concat(__naro_elements_by_tag_name(id, "button"))
          .map(wrap);
      },
      // A `<select>` opens its popup through the same path a click takes.
      get selectedIndex() {
        const options = __naro_elements_by_tag_name(id, "option").map(wrap);
        return options.findIndex((o) => o.hasAttribute("selected"));
      },
      get options() { return __naro_elements_by_tag_name(id, "option").map(wrap); },
      get value() {
        const v = __naro_value(id);
        if (v !== null && v !== "") return v;
        const options = __naro_elements_by_tag_name(id, "option").map(wrap);
        const chosen = options.find((o) => o.hasAttribute("selected")) || options[0];
        return chosen ? (chosen.getAttribute("value") || chosen.textContent) : "";
      },
      get checked() { return __naro_get(id, "checked") != null; },
      set checked(v) {
        if (v) __naro_set(id, "checked", "checked");
        else __naro_remove_attr(id, "checked");
        __naro_mutation("attributes", id, "checked", -1, -1);
      },
      get dataset() {
        // `data-user-id` <-> `userId`, per the DOMStringMap rules.
        const toProp = (name) => name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
        const toAttr = (prop) => "data-" + prop.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase());
        const read = () => {
          const out = {};
          const flat = __naro_attrs(id);
          for (let i = 0; i + 1 < flat.length; i += 2) {
            const name = flat[i];
            if (name.length > 5 && name.startsWith("data-")) out[toProp(name)] = flat[i + 1];
          }
          return out;
        };
        // A Proxy handler, not an object of same-named methods: the trap
        // signature is (target, key, value), so passing `api` directly made
        // `dataset.userId = x` store the property name instead of the value.
        return new Proxy({}, {
          get: (_, k) => read()[k],
          set: (_, k, v) => {
            const name = toAttr(String(k));
            __naro_set(id, name, String(v));
            __naro_mutation("attributes", id, name, -1, -1);
            return true;
          },
          deleteProperty: (_, k) => {
            const name = toAttr(String(k));
            __naro_remove_attr(id, name);
            __naro_mutation("attributes", id, name, -1, -1);
            return true;
          },
          has: (_, k) => Object.prototype.hasOwnProperty.call(read(), k),
          ownKeys: () => Object.keys(read()),
          getOwnPropertyDescriptor: (_, k) => {
            const map = read();
            return Object.prototype.hasOwnProperty.call(map, k)
              ? { value: map[k], writable: true, enumerable: true, configurable: true }
              : undefined;
          }
        });
      },
      getBoundingClientRect() {
        const b = __naro_box(id);
        const x = b.length ? b[0] : 0;
        const y = b.length ? b[1] : 0;
        const w = b.length ? b[2] : 0;
        const h = b.length ? b[3] : 0;
        return {
          x: x, y: y, left: x, top: y, right: x + w, bottom: y + h, width: w, height: h,
          toJSON() { return { x: x, y: y, left: x, top: y, right: x + w, bottom: y + h, width: w, height: h }; }
        };
      },
      get offsetWidth() { const b = __naro_box(id); return b.length ? b[2] : 0; },
      get offsetHeight() { const b = __naro_box(id); return b.length ? b[3] : 0; },
      get offsetLeft() {
        const b = __naro_box(id);
        if (!b.length) return 0;
        const p = __naro_parent(id);
        const pb = p < 0 ? [] : __naro_box(p);
        return b[0] - (pb.length ? pb[0] : 0);
      },
      get offsetTop() {
        const b = __naro_box(id);
        if (!b.length) return 0;
        const p = __naro_parent(id);
        const pb = p < 0 ? [] : __naro_box(p);
        return b[1] - (pb.length ? pb[1] : 0);
      },
      get clientWidth() {
        // Padding box: the border box minus its borders (no scrollbars here).
        const b = __naro_box(id);
        return b.length ? Math.max(0, b[2] - b[7] - b[5]) : 0;
      },
      get clientHeight() {
        const b = __naro_box(id);
        return b.length ? Math.max(0, b[3] - b[4] - b[6]) : 0;
      }
    };
    if (hasWeakRef) {
      wrappers.set(id, new WeakRef(el));
      if (finalizer) finalizer.register(el, id);
    } else {
      // No WeakRef: fall back to a strong cache. Identity still holds; the
      // table only grows while the document lives.
      wrappers.set(id, { deref: () => el });
    }
    return el;
  };

  globalThis.__wrap = wrap;
  globalThis.document = {
    getElementById: (id) => wrap(__naro_query_id(String(id))),
    querySelector: (sel) => wrap(__naro_query(String(sel))),
    querySelectorAll: (sel) => __naro_query_all(String(sel)).map(wrap),
    createElement: (tag) => {
      const id = __naro_create(String(tag));
      ceUpgrade(id);
      return wrap(id);
    },
    createTextNode: (text) => wrap(__naro_create_text(String(text))),
    createDocumentFragment: () => wrap(__naro_create_fragment()),
    getElementsByClassName: (name) => {
      const wanted = String(name).split(/\s+/).filter(Boolean);
      if (wanted.length === 0) return [];
      const first = __naro_elements_by_class(-1, wanted[0]).map(wrap);
      return wanted.length === 1
        ? first
        : first.filter((el) => wanted.every((c) => el.classList.contains(c)));
    },
    getElementsByTagName: (name) => __naro_elements_by_tag_name(-1, String(name)).map(wrap),
    get body() { return wrap(__naro_body()); },
    get head() { return wrap(__naro_head()); },
    get documentElement() { return wrap(__naro_root()); },
    get title() { return __naro_doc_title(); },
    set title(v) { __naro_set_doc_title(String(v)); },
    // `loading` until the parser is done, `interactive` while the
    // `DOMContentLoaded` handlers run, `complete` afterwards.
    get readyState() { return __naro_ready_state(); }
  };
  globalThis.DocumentFragment = function DocumentFragment() {};
  globalThis.Node = { ELEMENT_NODE: 1, TEXT_NODE: 3, DOCUMENT_NODE: 9, DOCUMENT_FRAGMENT_NODE: 11 };
  globalThis.console = {
    log: (...a) => __naro_log(a.map(String).join(" ")),
    error: (...a) => __naro_log(a.map(String).join(" "))
  };
  // `window`, `self` and `globalThis` are the same object in a browser.
  globalThis.window = globalThis;
  globalThis.self = globalThis;
  globalThis.top = globalThis;
  globalThis.parent = globalThis;
  globalThis.frames = globalThis;
  // Feature-detection stub: enough for scripts that branch on the browser
  // language without pretending to be a full `navigator`.
  globalThis.navigator = globalThis.navigator || {
    language: "pt-BR",
    languages: ["pt-BR", "pt", "en"],
    userAgent: "NARO",
    platform: "",
    onLine: true
  };
  // `location` is a live view of the document URL: `pushState` rewrites the
  // URL, so every field is re-read instead of captured once at bootstrap.
  const urlParts = (input, base) => {
    const p = __naro_url_parts(
      String(input === undefined || input === null ? "" : input),
      String(base === undefined || base === null ? "" : base)
    );
    return {
      href: p[0], origin: p[1], protocol: p[2], host: p[3], hostname: p[4],
      port: p[5], pathname: p[6], search: p[7], hash: p[8]
    };
  };
  globalThis.__urlParts = urlParts;

  globalThis.location = (function () {
    const read = () => urlParts(__naro_url());
    // `assign`/`replace`/`reload` are navigation, which the engine owns; a
    // no-op keeps a page that branches on `location.href` from dying.
    const api = {
      reload: function () {},
      assign: function () {},
      replace: function () {},
      toString: function () { return read().href; }
    };
    return new Proxy(api, {
      get: (t, k) => (typeof k === "string" && !(k in t)) ? read()[k] : t[k],
      has: (t, k) => (k in t) || (typeof k === "string" && k in read())
    });
  })();

  // --- Event classes -----------------------------------------------------
  // One prototype chain, so `instanceof` works and a page can build its own
  // event objects. Dispatch always walks the whole ancestor path: `bubbles`
  // is recorded and reported but not used to cut propagation, because the
  // engine's own synthetic events rely on reaching the document.
  class Event {
    constructor(type, init) {
      init = init || {};
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.timeStamp = Date.now();
      this.isTrusted = false;
      this.__stop = false;
      this.__stopImmediate = false;
      this.__path = [];
    }
    preventDefault() { this.defaultPrevented = true; }
    stopPropagation() { this.__stop = true; }
    stopImmediatePropagation() { this.__stop = true; this.__stopImmediate = true; }
    composedPath() { return this.__path.slice(); }
    get returnValue() { return !this.defaultPrevented; }
    set returnValue(v) { if (!v) this.defaultPrevented = true; }
    get srcElement() { return this.target; }
  }
  class CustomEvent extends Event {
    constructor(type, init) {
      super(type, init);
      this.detail = init && init.detail !== undefined ? init.detail : null;
    }
  }
  class UIEvent extends Event {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.detail = Number(init.detail) || 0;
      this.view = globalThis;
    }
  }
  class MouseEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.clientX = Number(init.clientX) || 0;
      this.clientY = Number(init.clientY) || 0;
      this.screenX = Number(init.screenX) || 0;
      this.screenY = Number(init.screenY) || 0;
      this.pageX = this.clientX;
      this.pageY = this.clientY;
      this.button = Number(init.button) || 0;
      this.buttons = Number(init.buttons) || 0;
      this.altKey = !!init.altKey;
      this.ctrlKey = !!init.ctrlKey;
      this.shiftKey = !!init.shiftKey;
      this.metaKey = !!init.metaKey;
      this.relatedTarget = init.relatedTarget || null;
    }
  }
  class KeyboardEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.key = init.key === undefined ? "" : String(init.key);
      this.code = init.code === undefined ? "" : String(init.code);
      this.repeat = !!init.repeat;
      this.altKey = !!init.altKey;
      this.ctrlKey = !!init.ctrlKey;
      this.shiftKey = !!init.shiftKey;
      this.metaKey = !!init.metaKey;
      this.keyCode = Number(init.keyCode) || 0;
      this.which = this.keyCode;
    }
  }
  class FocusEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      this.relatedTarget = (init && init.relatedTarget) || null;
    }
  }
  class InputEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.data = init.data === undefined ? null : init.data;
      this.inputType = init.inputType === undefined ? "" : String(init.inputType);
      this.isComposing = !!init.isComposing;
    }
  }
  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.UIEvent = UIEvent;
  globalThis.MouseEvent = MouseEvent;
  globalThis.KeyboardEvent = KeyboardEvent;
  globalThis.FocusEvent = FocusEvent;
  globalThis.InputEvent = InputEvent;

  // --- computed style ----------------------------------------------------
  // `getComputedStyle` reports the cascade the renderer actually used: the
  // engine hands its own `ComputedStyle` vector to the session, so a page and
  // the paint pass can never disagree.
  const camelToKebab = (k) => (k.startsWith("--") ? k : k.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase()));
  globalThis.getComputedStyle = function (el, pseudoElement) {
    const id = el && el.__id !== undefined ? el.__id : -1;
    const read = (prop) => {
      const v = __naro_computed(id, String(prop));
      return v === null || v === undefined ? "" : v;
    };
    const api = {
      getPropertyValue: (prop) => read(camelToKebab(String(prop))),
      get cssText() { return ""; }
    };
    return new Proxy(api, {
      get: (t, k) => {
        if (typeof k !== "string") return t[k];
        if (k in t) return t[k];
        return read(camelToKebab(k));
      }
    });
  };

  // --- media queries -----------------------------------------------------
  // `(min-width: Npx)` / `(max-width: Npx)` (and the height pair) are
  // evaluated against the layout viewport; any other feature never matches,
  // which is what a browser does for a feature it does not know. The result is
  // a snapshot: there is no re-evaluation on resize.
  globalThis.matchMedia = function (query) {
    const text = String(query === undefined || query === null ? "" : query);
    const viewport = __naro_viewport();
    const evaluate = () => {
      const clauses = text.toLowerCase().replace(/^\s*only\s+/, "").split(/\s+and\s+/);
      for (const raw of clauses) {
        const clause = raw.trim();
        if (clause === "" || clause === "all" || clause === "screen") continue;
        const m = /^\(\s*(min|max)-(width|height)\s*:\s*([\d.]+)px\s*\)$/.exec(clause);
        if (!m) return false;
        const limit = parseFloat(m[3]);
        const actual = m[2] === "width" ? viewport[0] : viewport[1];
        if (m[1] === "min" ? actual < limit : actual > limit) return false;
      }
      return true;
    };
    const listeners = [];
    return {
      media: text,
      matches: evaluate(),
      onchange: null,
      addEventListener(type, fn) { if (type === "change" && typeof fn === "function") listeners.push(fn); },
      removeEventListener(type, fn) {
        const i = listeners.indexOf(fn);
        if (i >= 0) listeners.splice(i, 1);
      },
      addListener(fn) { this.addEventListener("change", fn); },
      removeListener(fn) { this.removeEventListener("change", fn); },
      dispatchEvent() { return true; }
    };
  };

  // --- URL and URLSearchParams -------------------------------------------
  const percentEncode = (s) => encodeURIComponent(String(s));
  const percentDecode = (s) => {
    try { return decodeURIComponent(String(s).replace(/\+/g, " ")); }
    catch (e) { return String(s); }
  };

  class URLSearchParams {
    constructor(init) {
      this.__pairs = [];
      if (init === undefined || init === null) return;
      if (typeof init === "string") {
        const text = init.startsWith("?") ? init.slice(1) : init;
        if (!text) return;
        for (const chunk of text.split("&")) {
          if (!chunk) continue;
          const eq = chunk.indexOf("=");
          this.__pairs.push([
            percentDecode(eq < 0 ? chunk : chunk.slice(0, eq)),
            eq < 0 ? "" : percentDecode(chunk.slice(eq + 1))
          ]);
        }
      } else if (init instanceof URLSearchParams) {
        for (const pair of init.__pairs) this.__pairs.push([pair[0], pair[1]]);
      } else if (Array.isArray(init)) {
        for (const pair of init) this.__pairs.push([String(pair[0]), String(pair[1])]);
      } else if (typeof init === "object") {
        for (const key of Object.keys(init)) this.__pairs.push([key, String(init[key])]);
      }
    }
    get size() { return this.__pairs.length; }
    append(k, v) { this.__pairs.push([String(k), String(v)]); }
    set(k, v) {
      const key = String(k);
      const i = this.__pairs.findIndex((p) => p[0] === key);
      if (i < 0) { this.__pairs.push([key, String(v)]); return; }
      // `set` keeps the first position and drops the duplicates.
      this.__pairs[i] = [key, String(v)];
      this.__pairs = this.__pairs.filter((p, idx) => idx <= i || p[0] !== key);
    }
    get(k) {
      const key = String(k);
      const hit = this.__pairs.find((p) => p[0] === key);
      return hit ? hit[1] : null;
    }
    getAll(k) {
      const key = String(k);
      return this.__pairs.filter((p) => p[0] === key).map((p) => p[1]);
    }
    has(k) {
      const key = String(k);
      return this.__pairs.some((p) => p[0] === key);
    }
    delete(k) {
      const key = String(k);
      this.__pairs = this.__pairs.filter((p) => p[0] !== key);
    }
    sort() { this.__pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); }
    forEach(fn, thisArg) { for (const pair of this.__pairs) fn.call(thisArg, pair[1], pair[0], this); }
    keys() { return this.__pairs.map((p) => p[0])[Symbol.iterator](); }
    values() { return this.__pairs.map((p) => p[1])[Symbol.iterator](); }
    entries() { return this.__pairs.map((p) => p.slice())[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() {
      return this.__pairs.map((p) => percentEncode(p[0]) + "=" + percentEncode(p[1])).join("&");
    }
  }

  class URL {
    constructor(url, base) {
      const raw = String(url);
      const hasBase = base !== undefined && base !== null;
      // A relative URL without a base is a `TypeError`, as in the DOM.
      if (!hasBase && !/^[a-z][a-z0-9+.-]*:/i.test(raw)) {
        throw new TypeError("URL inválida sem base: " + raw);
      }
      const parts = urlParts(raw, hasBase ? String(base) : "");
      if (!parts.href) throw new TypeError("URL inválida: " + raw);
      this.__p = parts;
    }
    static canParse(url, base) {
      try { new URL(url, base); return true; } catch (e) { return false; }
    }
    get href() { return this.__p.href; }
    set href(v) { this.__p = urlParts(String(v), ""); }
    get origin() { return this.__p.origin; }
    get protocol() { return this.__p.protocol; }
    get host() { return this.__p.host; }
    get hostname() { return this.__p.hostname; }
    get port() { return this.__p.port; }
    get pathname() { return this.__p.pathname; }
    set pathname(v) {
      const text = String(v);
      const path = text.startsWith("/") ? text : "/" + text;
      this.__p = urlParts(this.__p.origin + path + this.__p.search + this.__p.hash, "");
    }
    get search() { return this.__p.search; }
    set search(v) {
      const text = String(v);
      const query = text === "" ? "" : (text.startsWith("?") ? text : "?" + text);
      this.__p = urlParts(this.__p.href.split("#")[0].split("?")[0] + query + this.__p.hash, "");
    }
    get hash() { return this.__p.hash; }
    set hash(v) {
      const text = String(v);
      const fragment = text === "" ? "" : (text.startsWith("#") ? text : "#" + text);
      this.__p = urlParts(this.__p.href.split("#")[0] + fragment, "");
    }
    get searchParams() { return new URLSearchParams(this.__p.search); }
    toString() { return this.__p.href; }
    toJSON() { return this.__p.href; }
  }
  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;

  // --- text encoding -----------------------------------------------------
  class TextEncoder {
    constructor() { this.encoding = "utf-8"; }
    encode(text) {
      return new Uint8Array(__naro_utf8_encode(text === undefined ? "" : String(text)));
    }
  }
  class TextDecoder {
    constructor(label, options) {
      this.encoding = String(label === undefined || label === null ? "utf-8" : label).toLowerCase();
      this.fatal = !!(options && options.fatal);
      this.ignoreBOM = !!(options && options.ignoreBOM);
    }
    decode(input) {
      if (input === undefined || input === null) return "";
      return __naro_utf8_decode(Array.from(input));
    }
  }
  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;

  // --- base64 ------------------------------------------------------------
  // Latin-1 only, like `btoa`: a code unit above 255 throws.
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  globalThis.btoa = function (input) {
    const text = String(input);
    for (let i = 0; i < text.length; i++) {
      if (text.charCodeAt(i) > 255) throw new Error("InvalidCharacterError: btoa");
    }
    let out = "";
    for (let i = 0; i < text.length; i += 3) {
      const c1 = text.charCodeAt(i);
      const has2 = i + 1 < text.length;
      const has3 = i + 2 < text.length;
      const c2 = has2 ? text.charCodeAt(i + 1) : 0;
      const c3 = has3 ? text.charCodeAt(i + 2) : 0;
      const n = (c1 << 16) | (c2 << 8) | c3;
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63]
        + (has2 ? B64[(n >> 6) & 63] : "=")
        + (has3 ? B64[n & 63] : "=");
    }
    return out;
  };
  globalThis.atob = function (input) {
    const text = String(input).replace(/[^A-Za-z0-9+/=]/g, "");
    let out = "";
    for (let i = 0; i < text.length; i += 4) {
      const c1 = B64.indexOf(text[i]);
      const c2 = B64.indexOf(text[i + 1] || "=");
      const c3 = text[i + 2] || "=";
      const c4 = text[i + 3] || "=";
      const n = ((c1 < 0 ? 0 : c1) << 18)
        | ((c2 < 0 ? 0 : c2) << 12)
        | ((c3 === "=" ? 0 : B64.indexOf(c3)) << 6)
        | (c4 === "=" ? 0 : B64.indexOf(c4));
      out += String.fromCharCode((n >> 16) & 255);
      if (c3 !== "=") out += String.fromCharCode((n >> 8) & 255);
      if (c4 !== "=") out += String.fromCharCode(n & 255);
    }
    return out;
  };

  // --- history -----------------------------------------------------------
  // The engine owns the history vector; script queues a request and the engine
  // applies it on its next flush. The states array is indexed by the engine's
  // own index, so entries created by navigation read back as `null`.
  globalThis.history = (function () {
    const states = [];
    const popState = () => {
      setTimeout(function () {
        const index = __naro_history_index();
        const ev = new Event("popstate");
        ev.state = index >= 0 && index < states.length ? states[index] : null;
        const root = __naro_root();
        __naro_dispatch(root, ev, [root]);
      }, 0);
    };
    return {
      get length() { return Math.max(states.length, __naro_history_index() + 1); },
      get state() {
        const index = __naro_history_index();
        return index >= 0 && index < states.length ? states[index] : null;
      },
      pushState(state, title, url) {
        const index = __naro_history_index() + 1;
        __naro_history_push(url === undefined || url === null ? "" : String(url), false);
        states[index] = state === undefined ? null : state;
      },
      replaceState(state, title, url) {
        const index = __naro_history_index();
        __naro_history_push(url === undefined || url === null ? "" : String(url), true);
        if (index >= 0) states[index] = state === undefined ? null : state;
      },
      back() { __naro_history_travel(-1); popState(); },
      forward() { __naro_history_travel(1); popState(); },
      go(delta) { __naro_history_travel(Number(delta) || 0); popState(); }
    };
  })();

  // `getSelection()` reports the engine's selection as one string. A
  // `Range` object is not modelled: pages that only read `toString()` — the
  // overwhelming majority — work, and a page that wants `anchorNode` gets
  // `null` instead of a wrong answer.
  globalThis.getSelection = function () {
    const text = () => __naro_selection();
    return {
      toString: text,
      isCollapsed: () => text().length === 0,
      rangeCount: 0,
      anchorNode: null,
      focusNode: null,
      anchorOffset: 0,
      focusOffset: 0,
      removeAllRanges: function () {},
      addRange: function () {},
      getRangeAt: function () { return null; },
      collapse: function () {},
      selectAllChildren: function () {}
    };
  };
  globalThis.document.getSelection = globalThis.getSelection;

  // `JSON` cannot carry functions, Dates or cyclic graphs; documented
  // limitation of the only deep clone available without a structured-clone
  // implementation in QuickJS.
  globalThis.structuredClone = function (value) {
    if (value === undefined) return undefined;
    return JSON.parse(JSON.stringify(value));
  };

  // --- fetch and XMLHttpRequest ------------------------------------------
  // Both are synchronous under the hood: `__naro_fetch` blocks until the
  // response arrives, and the `Promise` that `fetch` returns is already
  // settled. A page that awaits it therefore makes progress, but a page that
  // starts a request and keeps working does not get concurrency — documented
  // limitation, chosen to keep one script thread and one ordering rule.
  const flattenHeaders = (headers) => {
    const out = [];
    if (!headers) return out;
    if (typeof headers.forEach === "function" && !Array.isArray(headers)) {
      headers.forEach((value, name) => { out.push(String(name), String(value)); });
    } else if (Array.isArray(headers)) {
      for (const pair of headers) out.push(String(pair[0]), String(pair[1]));
    } else if (typeof headers === "object") {
      for (const name of Object.keys(headers)) out.push(name, String(headers[name]));
    }
    return out;
  };

  const requestSync = (method, url, body, headers) => {
    const absolute = urlParts(url).href || String(url);
    const raw = __naro_fetch(String(method || "GET"), absolute, body === undefined || body === null ? "" : String(body), flattenHeaders(headers));
    const error = raw[4] || "";
    return {
      status: Number(raw[0]) || 0,
      statusText: raw[1] || "",
      text: raw[2] || "",
      url: raw[3] || absolute,
      error
    };
  };

  class Headers {
    constructor(init) {
      this.__map = new Map();
      const flat = flattenHeaders(init);
      for (let i = 0; i + 1 < flat.length; i += 2) this.__map.set(flat[i].toLowerCase(), flat[i + 1]);
    }
    get(name) { const v = this.__map.get(String(name).toLowerCase()); return v === undefined ? null : v; }
    set(name, value) { this.__map.set(String(name).toLowerCase(), String(value)); }
    append(name, value) { this.set(name, value); }
    has(name) { return this.__map.has(String(name).toLowerCase()); }
    delete(name) { this.__map.delete(String(name).toLowerCase()); }
    forEach(fn, thisArg) { this.__map.forEach((v, k) => fn.call(thisArg, v, k, this)); }
    keys() { return this.__map.keys(); }
    values() { return this.__map.values(); }
    entries() { return this.__map.entries(); }
    [Symbol.iterator]() { return this.__map.entries(); }
  }

  class Response {
    constructor(result, requestUrl) {
      this.status = result.status;
      this.statusText = result.statusText;
      this.ok = result.status >= 200 && result.status < 300;
      this.url = result.url || requestUrl;
      this.redirected = false;
      this.type = "basic";
      this.bodyUsed = false;
      this.headers = new Headers();
      this.__text = result.text;
      this.__used = false;
    }
    get body() { return null; }
    text() {
      this.__used = true;
      this.bodyUsed = true;
      return Promise.resolve(this.__text);
    }
    json() {
      this.__used = true;
      this.bodyUsed = true;
      try { return Promise.resolve(JSON.parse(this.__text)); }
      catch (e) { return Promise.reject(e); }
    }
    clone() {
      const copy = Object.create(Response.prototype);
      Object.assign(copy, this);
      return copy;
    }
  }

  globalThis.fetch = function (input, init) {
    init = init || {};
    const url = typeof input === "string" ? input : String(input && input.url ? input.url : input);
    const method = init.method || (input && input.method) || "GET";
    const body = init.body === undefined || init.body === null ? "" : init.body;
    const result = requestSync(method, url, body, init.headers);
    if (result.error) {
      return Promise.reject(new TypeError(result.error));
    }
    return Promise.resolve(new Response(result, url));
  };

  globalThis.Headers = Headers;
  globalThis.Response = Response;
  globalThis.Request = function Request(input, init) {
    init = init || {};
    this.url = typeof input === "string" ? input : String(input && input.url ? input.url : input);
    this.method = init.method || "GET";
    this.headers = new Headers(init.headers);
    this.body = init.body === undefined ? null : init.body;
  };

  // `XMLHttpRequest` reports through `readyState` and `onreadystatechange`;
  // `send` performs the request and fires the callbacks in order, which is
  // what a synchronous host call can honestly offer.
  const XHR_UNSENT = 0, XHR_OPENED = 1, XHR_HEADERS = 2, XHR_LOADING = 3, XHR_DONE = 4;
  class XMLHttpRequest {
    constructor() {
      this.readyState = XHR_UNSENT;
      this.status = 0;
      this.statusText = "";
      this.responseText = "";
      this.response = "";
      this.responseType = "";
      this.onload = null;
      this.onerror = null;
      this.onreadystatechange = null;
      this.onloadend = null;
      this.__method = "GET";
      this.__url = "";
      this.__headers = [];
      this.__listeners = [];
    }
    open(method, url) {
      this.__method = String(method || "GET").toUpperCase();
      this.__url = String(url);
      this.readyState = XHR_OPENED;
      this.__fire("readystatechange");
    }
    setRequestHeader(name, value) {
      this.__headers.push([String(name), String(value)]);
      this.readyState = XHR_HEADERS;
    }
    getResponseHeader() { return null; }
    getAllResponseHeaders() { return ""; }
    addEventListener(type, fn) {
      if (typeof fn === "function") this.__listeners.push({ type: String(type), fn });
    }
    removeEventListener(type, fn) {
      const i = this.__listeners.findIndex((l) => l.type === String(type) && l.fn === fn);
      if (i >= 0) this.__listeners.splice(i, 1);
    }
    __fire(type) {
      const ev = new Event(type);
      ev.target = this;
      ev.currentTarget = this;
      const handler = this["on" + type];
      if (typeof handler === "function") {
        try { handler.call(this, ev); } catch (e) { report(e); }
      }
      for (const l of this.__listeners.slice()) {
        if (l.type !== type) continue;
        try { l.fn.call(this, ev); } catch (e) { report(e); }
      }
    }
    abort() { this.readyState = XHR_UNSENT; }
    send(body) {
      const result = requestSync(this.__method, this.__url, body, this.__headers);
      if (result.error) {
        this.status = 0;
        this.readyState = XHR_DONE;
        this.__fire("readystatechange");
        this.__fire("error");
        this.__fire("loadend");
        return;
      }
      this.status = result.status;
      this.statusText = result.statusText;
      this.responseText = result.text;
      this.response = result.text;
      this.readyState = XHR_LOADING;
      this.__fire("readystatechange");
      this.readyState = XHR_DONE;
      this.__fire("readystatechange");
      this.__fire("load");
      this.__fire("loadend");
    }
  }
  XMLHttpRequest.UNSENT = XHR_UNSENT;
  XMLHttpRequest.OPENED = XHR_OPENED;
  XMLHttpRequest.HEADERS_RECEIVED = XHR_HEADERS;
  XMLHttpRequest.LOADING = XHR_LOADING;
  XMLHttpRequest.DONE = XHR_DONE;
  globalThis.XMLHttpRequest = XMLHttpRequest;

  // `document.cookie`: one string of `name=value` pairs, `HttpOnly` hidden.
  // Assigning replaces the cookie with that name at the document's path.
  Object.defineProperty(globalThis.document, "cookie", {
    get() { return __naro_cookie_get(); },
    set(value) { __naro_cookie_set(String(value)); },
    configurable: true
  });

  // --- Custom Elements (minimal) ---------------------------------------
  // `define` registers a constructor and upgrades the elements that already
  // exist; elements created or inserted later are upgraded on the way in.
  // Only `connectedCallback`/`disconnectedCallback` are delivered: there is no
  // `observedAttributes`, no `adoptedCallback` and no reaction queue, so the
  // callbacks run synchronously at the DOM change that caused them.
  globalThis.HTMLElement = function HTMLElement() {};
  const ceRegistry = new Map();
  const ceInstances = new Map();
  const ceName = /^[a-z][a-z0-9._]*-[a-z0-9._-]*$/;

  const ceUpgrade = (id) => {
    if (id < 0 || ceInstances.has(id)) return null;
    // `tagName` is upper-cased for HTML; the registry is keyed by the parsed
    // (lower-case) tag.
    const tag = String(__naro_tag(id) || "").toLowerCase();
    if (!tag) return null;
    const ctor = ceRegistry.get(tag);
    if (!ctor) return null;
    const el = wrap(id);
    // The element API lives on the wrapper object, so the instance inherits
    // it: `Object.create(ctor.prototype)` plus the wrapper's own descriptors
    // gives an object that is both an element and an instance of `ctor`.
    const base = Object.create(ctor.prototype);
    Object.defineProperties(base, Object.getOwnPropertyDescriptors(el));
    ceInstances.set(id, base);
    const cache = (value) => {
      if (hasWeakRef) {
        wrappers.set(id, new WeakRef(value));
        if (finalizer) finalizer.register(value, id);
      } else {
        wrappers.set(id, { deref: () => value });
      }
    };
    cache(base);
    // `Reflect.construct` is the only way to run a class body. `Bridge` makes
    // the new object inherit from `base`, so `this.__id` and the element API
    // are visible while the constructor runs.
    const Bridge = function () {};
    Bridge.prototype = base;
    let instance = base;
    try {
      instance = Reflect.construct(ctor, [], Bridge);
      ceInstances.set(id, instance);
      cache(instance);
    } catch (e) {
      report(e);
    }
    if (__naro_in_document(id) && typeof instance.connectedCallback === "function") {
      try { instance.connectedCallback(); } catch (e) { report(e); }
    }
    return instance;
  };

  const ceUpgradeTree = (id) => {
    if (id < 0) return;
    const stack = [id];
    while (stack.length) {
      const cur = stack.pop();
      ceUpgrade(cur);
      const kids = __naro_child_nodes(cur);
      for (let i = kids.length - 1; i >= 0; i--) stack.push(kids[i]);
    }
  };

  const ceDisconnectTree = (id) => {
    if (id < 0) return;
    const stack = [id];
    while (stack.length) {
      const cur = stack.pop();
      const inst = ceInstances.get(cur);
      if (inst && typeof inst.disconnectedCallback === "function") {
        try { inst.disconnectedCallback(); } catch (e) { report(e); }
      }
      const kids = __naro_child_nodes(cur);
      for (let i = kids.length - 1; i >= 0; i--) stack.push(kids[i]);
    }
  };

  globalThis.customElements = {
    define(name, ctor) {
      const tag = String(name).toLowerCase();
      if (!ceName.test(tag)) {
        throw new Error("SyntaxError: invalid custom element name: " + name);
      }
      if (ceRegistry.has(tag)) {
        throw new Error("NotSupportedError: already defined: " + name);
      }
      ceRegistry.set(tag, ctor);
      for (const id of __naro_elements_by_tag(tag)) ceUpgrade(id);
    },
    get(name) { return ceRegistry.get(String(name).toLowerCase()); },
    upgrade(el) { if (el && el.__id !== undefined) ceUpgrade(el.__id); },
    whenDefined(name) {
      const tag = String(name).toLowerCase();
      if (ceRegistry.has(tag)) return Promise.resolve(ceRegistry.get(tag));
      return new Promise((resolve) => {
        const tick = () => {
          if (ceRegistry.has(tag)) resolve(ceRegistry.get(tag));
          else setTimeout(tick, 0);
        };
        setTimeout(tick, 0);
      });
    }
  };
  globalThis.setTimeout = function (fn, delay, ...args) {
    const id = nextTimer++;
    timers.push({ id, due: Date.now() + (Number(delay) || 0), fn, args });
    return id;
  };
  globalThis.clearTimeout = function (id) {
    const i = timers.findIndex((t) => t.id === id);
    if (i >= 0) timers.splice(i, 1);
  };
  globalThis.queueMicrotask = function (fn) { microtasks.push(fn); };

  // Idle callbacks run as zero-delay timers under the same time budget.
  globalThis.requestIdleCallback = function (fn, opts) {
    return setTimeout(function () {
      try {
        fn({ didTimeout: false, timeRemaining: function () { return 8; } });
      } catch (e) { report(e); }
    }, 0);
  };
  globalThis.cancelIdleCallback = function (id) { clearTimeout(id); };

  // `Image` is a plain object: it records the requested source but does not
  // load anything yet, so feature detection does not throw.
  globalThis.Image = function () {
    this.src = "";
    this.width = 0;
    this.height = 0;
    this.onload = null;
    this.onerror = null;
    this.addEventListener = function () {};
    this.removeEventListener = function () {};
  };

  // Minimal PerformanceObserver: records nothing, never throws.
  globalThis.PerformanceObserver = function (cb) {
    this.observe = function () {};
    this.disconnect = function () {};
    this.takeRecords = function () { return []; };
    if (typeof cb === "function") { /* no entries to deliver */ }
  };
  globalThis.performance = globalThis.performance || {
    now: function () { return Date.now(); },
    getEntriesByType: function () { return []; },
    getEntriesByName: function () { return []; },
    mark: function () {},
    measure: function () {}
  };
  globalThis.requestAnimationFrame = function (fn) {
    return setTimeout(function () { fn(Date.now()); }, 16);
  };
  globalThis.cancelAnimationFrame = function (id) { clearTimeout(id); };

  // Host entry points.
  globalThis.__naro_add = function (id, type, fn, capture) {
    listeners.push({ id, type, fn, capture: !!capture });
  };
  globalThis.__naro_remove = function (id, type, fn) {
    const i = listeners.findIndex((l) => l.id === id && l.type === type && l.fn === fn);
    if (i >= 0) listeners.splice(i, 1);
  };
  globalThis.__naro_dispatch = function (id, event, path) {
    // `path` is [target, parent, ..., root]: capture, target, then bubbling.
    // A string is the engine's shorthand for a plain `Event`; a page that
    // built its own event object keeps its identity and its own fields.
    const ev = (event !== null && typeof event === "object") ? event : new Event(event);
    if (ev.target === undefined || ev.target === null) ev.target = wrap(id);
    ev.__path = path.map(wrap);
    if (ev.defaultPrevented === undefined) ev.defaultPrevented = false;
    if (ev.__stop === undefined) ev.__stop = false;
    if (ev.__stopImmediate === undefined) ev.__stopImmediate = false;
    if (typeof ev.preventDefault !== "function") ev.preventDefault = function () { this.defaultPrevented = true; };
    if (typeof ev.stopPropagation !== "function") ev.stopPropagation = function () { this.__stop = true; };
    if (typeof ev.stopImmediatePropagation !== "function") ev.stopImmediatePropagation = function () { this.__stop = true; this.__stopImmediate = true; };
    const type = ev.type;
    const snapshot = listeners.slice();
    const run = (nodeId, phase) => {
      ev.eventPhase = phase;
      ev.currentTarget = wrap(nodeId);
      ev.__stopImmediate = false;
      for (const l of snapshot) {
        if (ev.__stopImmediate) break;
        if (l.type !== type || l.id !== nodeId) continue;
        // Capture listeners only run on the way down; bubble listeners only
        // on the target and on the way up.
        if (phase === 1 && !l.capture) continue;
        if (phase !== 1 && l.capture) continue;
        called++;
        try { l.fn.call(ev.currentTarget, ev); } catch (e) { report(e); }
        if (ev.__stop) break;
      }
    };
    let called = 0;
    // Capture: root down to the parent of the target.
    for (let i = path.length - 1; i >= 1; i--) {
      if (ev.__stop) break;
      run(path[i], 1);
    }
    if (!ev.__stop) run(id, 2);
    for (let i = 1; i < path.length; i++) {
      if (ev.__stop) break;
      run(path[i], 3);
    }
    ev.eventPhase = 0;
    ev.currentTarget = null;
    return called;
  };
  globalThis.__naro_run_microtasks = function () {
    let ran = 0;
    while (microtasks.length && ran < 200) {
      const fn = microtasks.shift();
      try { fn(); } catch (e) { report(e); }
      ran++;
    }
    return ran;
  };
  globalThis.__naro_run_timers = function (budgetMs) {
    const start = Date.now();
    let ran = 0;
    while (ran < 100) {
      if (Date.now() - start > budgetMs) break;
      const now = Date.now();
      let idx = -1;
      let best = Infinity;
      for (let i = 0; i < timers.length; i++) {
        if (timers[i].due <= now && timers[i].due < best) { best = timers[i].due; idx = i; }
      }
      if (idx < 0) break;
      const t = timers.splice(idx, 1)[0];
      try { t.fn(...t.args); } catch (e) { report(e); }
      ran++;
      // A zero-delay chain must not spin forever inside one host call. The
      // wall-clock budget above and the `ran < 100` cap are what stop it; an
      // extra "same millisecond" break would silently drop a second timer
      // queued in the same millisecond as the first, which real pages do.
    }
    return ran;
  };
  // --- MutationObserver -------------------------------------------------
  // Records are produced by the wrappers above and delivered as a batch at
  // the next microtask checkpoint, never re-entrantly during the mutation.
  const observers = [];
  const mutationQueue = [];
  let deliveryScheduled = false;

  globalThis.__naro_mutation = function (kind, targetId, name, addedId, removedId) {
    // Invalidation is independent of observation: a page with no observer
    // still has to be restyled, relaid out and repainted.
    __naro_invalidate("style");
    if (observers.length === 0) return;
    mutationQueue.push({ kind, targetId, name, addedId, removedId });
    if (!deliveryScheduled) {
      deliveryScheduled = true;
      microtasks.push(function () { deliveryScheduled = false; __naro_deliver_mutations(); });
    }
  };

  globalThis.__naro_deliver_mutations = function () {
    if (observers.length === 0) { mutationQueue.length = 0; return; }
    const pending = mutationQueue.splice(0, mutationQueue.length);
    for (const obs of observers) {
      const records = [];
      for (const m of pending) {
        for (const t of obs.targets) {
          if (!t.attributes && !t.childList) continue;
          const same = t.node === m.targetId;
          const inSubtree = t.subtree && __naro_is_descendant(m.targetId, t.node);
          if (!same && !inSubtree) continue;
          if (m.kind === "attributes" && !t.attributes) continue;
          if (m.kind === "childList" && !t.childList) continue;
          if (m.kind === "attributes" && t.filter && !t.filter.includes(m.name)) continue;
          records.push({
            type: m.kind,
            target: wrap(m.targetId),
            attributeName: m.kind === "attributes" ? m.name : null,
            addedNodes: m.addedId >= 0 ? [wrap(m.addedId)] : [],
            removedNodes: m.removedId >= 0 ? [wrap(m.removedId)] : []
          });
          break;
        }
      }
      if (records.length) {
        obs.queue.push.apply(obs.queue, records);
        try { obs.callback(obs.queue.splice(0, obs.queue.length), obs); } catch (e) { report(e); }
      }
    }
  };

  globalThis.MutationObserver = function (callback) {
    this.callback = callback;
    this.targets = [];
    this.queue = [];
    const self = this;
    this.observe = function (node, options) {
      options = options || {};
      self.targets.push({
        node: node.__id,
        attributes: !!options.attributes,
        childList: !!options.childList,
        subtree: !!options.subtree,
        filter: options.attributeFilter ? options.attributeFilter.map(String) : null
      });
    };
    this.disconnect = function () { self.targets = []; self.queue = []; };
    this.takeRecords = function () { return self.queue.splice(0, self.queue.length); };
    observers.push(self);
  };

  globalThis.__naro_pending_timers = function () { return timers.length; };
  // Indirect eval keeps `function` declarations global while still allowing a
  // try/catch, so one failing script reports its real message and the next
  // scripts still run.
  globalThis.__naro_eval = function (src) {
    try { (0, eval)(src); } catch (e) {
      var m = (e && e.message) ? e.message : String(e);
      if (e && e.stack) m += ' @ ' + String(e.stack).split('\n')[0];
      __naro_error(m);
    }
  };
  // Registering on `window` means "document level".
  globalThis.addEventListener = function (type, fn) {
    __naro_add(__naro_root(), String(type), fn);
  };
  globalThis.removeEventListener = function (type, fn) {
    __naro_remove(__naro_root(), String(type), fn);
  };
})();
"##;

/// A live JavaScript realm bound to one document.
pub struct JsSession {
    /// Kept alive for the lifetime of the context.
    _runtime: Runtime,
    context: Context,
    dom: Rc<RefCell<Dom>>,
}

impl JsSession {
    /// Create a realm, run every `<script>` in document order, then fire
    /// `DOMContentLoaded` and drain microtasks and timers.
    pub fn start(doc: Document, scripts: &[(String, String)]) -> (Option<JsSession>, ScriptOutcome) {
        JsSession::start_with_geometry(doc, scripts, Vec::new())
    }

    /// Same as [`JsSession::start`], but the document is already laid out and
    /// the measured boxes are available to scripts that run during load.
    ///
    /// `DOMContentLoaded` handlers and the first timers are a common place to
    /// measure the page; without this the engine would still be styling the
    /// document and every rect would read as zero.
    pub fn start_with_geometry(
        doc: Document,
        scripts: &[(String, String)],
        geometry: Vec<Option<ElementBox>>,
    ) -> (Option<JsSession>, ScriptOutcome) {
        JsSession::start_measured(doc, scripts, geometry, Vec::new(), None, "")
    }

    /// Same as [`JsSession::start_with_geometry`], plus a callback the session
    /// calls once more before the first timers run, so a `DOMContentLoaded`
    /// handler that mutates and then measures sees the updated layout.
    ///
    /// `measure` receives the live document and returns one box per node; the
    /// engine builds it from the real layout, so `naro-js` stays free of
    /// layout code.
    pub fn start_measured(
        doc: Document,
        scripts: &[(String, String)],
        geometry: Vec<Option<ElementBox>>,
        computed: Vec<naro_css::ComputedStyle>,
        measure: Option<&mut dyn FnMut(&Document) -> (Vec<Option<ElementBox>>, Vec<naro_css::ComputedStyle>)>,
        url: &str,
    ) -> (Option<JsSession>, ScriptOutcome) {
        let dom = Rc::new(RefCell::new(Dom {
            doc,
            logs: Vec::new(),
            pending_errors: Vec::new(),
            value_dirty: Vec::new(),
            invalidation: Invalidation::default(),
            geometry,
            computed,
            ready_state: "loading",
            history_ops: Vec::new(),
            form_ops: Vec::new(),
            selection_text: String::new(),
            history_index: 0,
            viewport: (1000.0, 700.0),
            url: url.to_string(),
        }));
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(err) => {
                let state = Rc::try_unwrap(dom).ok().unwrap().into_inner();
                let mut out = ScriptOutcome::default();
                out.errors.push(format!("runtime JS indisponível: {err}"));
                return (None, out);
            }
        };
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_interrupt_handler(Some(Box::new(deadline_exceeded)));
        let context = match Context::full(&runtime) {
            Ok(ctx) => ctx,
            Err(err) => {
                let state = Rc::try_unwrap(dom).ok().unwrap().into_inner();
                let mut out = ScriptOutcome::default();
                out.errors.push(format!("contexto JS indisponível: {err}"));
                return (None, out);
            }
        };
        let session = JsSession { _runtime: runtime, context, dom };
        let mut out = ScriptOutcome::default();
        out.merge(session.eval_labeled("<bootstrap>", BOOTSTRAP));

        for (label, source) in scripts {
            // Each script is wrapped so one failure cannot stop the others.
            // QuickJS reports an interrupted script with a generic message, so
            // an explicit marker is added when the deadline fired.
            // Closure's `_._DumpException` is the site's own error sink. When it
            // is missing, its `catch` block throws a second time and hides the
            // original failure; install a reporter so the real cause surfaces.
            // `__naro_eval` uses indirect eval: top-level `function`
            // declarations stay global (so later scripts see them) and the
            // try/catch still reports the real message.
            let _ = source;
            let wrapped = source.clone();
            out.merge(session.eval_script(label, &wrapped));
        }

        let root = session.root_id();
        // `loading` while the parser runs, `interactive` for the
        // `DOMContentLoaded` handlers, `complete` once they and the
        // microtasks they queued are done.
        session.set_ready_state("interactive");
        out.merge(session.call_js("DOMContentLoaded", |ctx| {
            let f = ctx.globals().get::<_, Function>("__naro_dispatch")?;
            f.call::<_, ()>((root as i64, "DOMContentLoaded", vec![root as i64]))
        }));
        out.merge(session.drain_microtasks());
        session.set_ready_state("complete");
        if let Some(measure) = measure {
            // Re-measure once the load handlers have run: a timer that reads
            // geometry right after `DOMContentLoaded` must not see the tree as
            // it was before those handlers touched it.
            if session.has_pending_timers() {
                let (boxes, styles) = measure(&session.dom.borrow().doc);
                session.set_geometry(boxes);
                session.set_computed(styles);
            }
        }
        out.merge(session.run_timers(50));
        (Some(session), out)
    }

    fn eval_labeled(&self, label: &str, source: &str) -> ScriptOutcome {
        let result = with_deadline(|| {
            self.context.with(|ctx| {
                register(&ctx, &self.dom)?;
                ctx.eval::<(), _>(source)
            })
        });
        let mut out = ScriptOutcome::default();
        if let Err(err) = result {
            let message = err.to_string();
            let message = if message.contains("interrupted") || deadline_was_hit() {
                "script interrompido: limite de tempo excedido".to_string()
            } else {
                message.trim().to_string()
            };
            out.errors.push(format!("{label}: {message}"));
        }
        out.merge(self.take_errors(label));
        out
    }

    /// Evaluate a document script through the JS-side indirect-eval helper.
    fn eval_script(&self, label: &str, source: &str) -> ScriptOutcome {
        let result = with_deadline(|| {
            self.context.with(|ctx| {
                register(&ctx, &self.dom)?;
                let f = ctx.globals().get::<_, Function>("__naro_eval")?;
                f.call::<_, ()>((source,))
            })
        });
        let mut out = ScriptOutcome::default();
        if let Err(err) = result {
            let message = err.to_string();
            let message = if message.contains("interrupted") || deadline_was_hit() {
                "script interrompido: limite de tempo excedido".to_string()
            } else {
                message.trim().to_string()
            };
            out.errors.push(format!("{label}: {message}"));
        }
        out.merge(self.take_errors(label));
        out
    }

    fn call_js<F>(&self, label: &str, f: F) -> ScriptOutcome
    where
        F: FnOnce(&rquickjs::Ctx<'_>) -> rquickjs::Result<()>,
    {
        let result = with_deadline(|| {
            self.context.with(|ctx| {
                register(&ctx, &self.dom)?;
                f(&ctx)
            })
        });
        let mut out = ScriptOutcome::default();
        if let Err(err) = result {
            out.errors.push(format!("{label}: {}", err.to_string().trim()));
        }
        out.merge(self.take_errors(label));
        out
    }

    /// Errors recorded by `__naro_error`, prefixed with the current source.
    fn take_errors(&self, label: &str) -> ScriptOutcome {
        let mut dom = self.dom.borrow_mut();
        let mut out = ScriptOutcome::default();
        for line in dom.pending_errors.drain(..) {
            out.errors.push(format!("{label}: {line}"));
        }
        out.logs = std::mem::take(&mut dom.logs);
        out
    }

    /// Deliver an event to listeners registered on `node` (or document-level).
    /// Returns how many handlers actually ran.
    pub fn dispatch(&self, node: NodeId, event: &str) -> (ScriptOutcome, usize) {
        let path = self.dom.borrow().ancestors(node);
        let count = with_deadline(|| {
            self.context
                .with(|ctx| -> rquickjs::Result<i64> {
                    register(&ctx, &self.dom)?;
                    let f = ctx.globals().get::<_, Function>("__naro_dispatch")?;
                    f.call::<_, i64>((node as i64, event, path.clone()))
                })
                .unwrap_or(0)
        })
        .max(0) as usize;
        (self.take_errors(event), count)
    }

    pub fn drain_microtasks(&self) -> ScriptOutcome {
        // Two queues have to be drained: our own `queueMicrotask` array and
        // QuickJS's job queue, which is where native `Promise` reactions go.
        // Without the second one every `fetch(...).then(...)` would silently
        // never run.
        let mut out = self.call_js("microtasks", |ctx| {
            let f = ctx.globals().get::<_, Function>("__naro_run_microtasks")?;
            f.call::<_, ()>(())
        });
        let jobs = with_deadline(|| {
            self.context.with(|ctx| {
                let mut ran = 0;
                // Bounded: a promise chain that keeps re-queueing must not
                // hold the engine forever.
                while ran < 1000 {
                    if ctx.execute_pending_job() {
                        ran += 1;
                    } else {
                        break;
                    }
                }
                ran
            })
        });
        if jobs >= 1000 {
            out.errors
                .push("microtasks: cadeia de promessas não terminou".to_string());
        }
        out.merge(self.take_errors("microtasks"));
        out
    }

    pub fn run_timers(&self, budget_ms: i64) -> ScriptOutcome {
        self.call_js("timers", |ctx| {
            let f = ctx.globals().get::<_, Function>("__naro_run_timers")?;
            f.call::<_, ()>((budget_ms,))
        })
    }

    pub fn has_pending_timers(&self) -> bool {
        self.context
            .with(|ctx| -> rquickjs::Result<bool> {
                let f = ctx.globals().get::<_, Function>("__naro_pending_timers")?;
                Ok(f.call::<_, i64>(()).unwrap_or(0) > 0)
            })
            .unwrap_or(false)
    }

    /// Replace the measured geometry used by `getBoundingClientRect` and the
    /// `offset*`/`client*` properties. Called by the engine after every layout.
    pub fn set_geometry(&self, boxes: Vec<Option<ElementBox>>) {
        self.dom.borrow_mut().geometry = boxes;
    }

    /// Replace the computed styles behind `getComputedStyle`. Called by the
    /// engine next to [`JsSession::set_geometry`], so a page and the renderer
    /// can never disagree about a cascaded value.
    pub fn set_computed(&self, styles: Vec<naro_css::ComputedStyle>) {
        self.dom.borrow_mut().computed = styles;
    }

    /// Update `document.readyState`.
    pub fn set_ready_state(&self, state: &'static str) {
        self.dom.borrow_mut().ready_state = state;
    }

    /// Update the URL `location` reports, without reloading anything.
    pub fn set_url(&self, url: &str) {
        self.dom.borrow_mut().url = url.to_string();
    }

    /// Record the viewport the page was laid out at, so `matchMedia` can
    /// evaluate `min-width`/`max-width` the way the cascade already does.
    pub fn set_viewport(&self, width: f32, height: f32) {
        let mut dom = self.dom.borrow_mut();
        dom.viewport = (width, height);
    }

    /// Record the current entry index of the engine's history.
    pub fn set_history_index(&self, index: i64) {
        self.dom.borrow_mut().history_index = index;
    }

    /// History changes page script queued since the last drain.
    pub fn take_history_ops(&self) -> Vec<HistoryOp> {
        std::mem::take(&mut self.dom.borrow_mut().history_ops)
    }

    /// Publish the current selection text, for `window.getSelection()`.
    pub fn set_selection_text(&self, text: &str) {
        self.dom.borrow_mut().selection_text = text.to_string();
    }

    /// Form actions page script queued since the last drain.
    pub fn take_form_ops(&self) -> Vec<(usize, FormOp)> {
        std::mem::take(&mut self.dom.borrow_mut().form_ops)
    }

    /// Pending invalidation produced by DOM mutations.
    pub fn invalidation(&self) -> Invalidation {
        self.dom.borrow().invalidation
    }

    /// Consume the pending invalidation.
    pub fn take_invalidation(&self) -> Invalidation {
        let mut dom = self.dom.borrow_mut();
        std::mem::take(&mut dom.invalidation)
    }

    /// Mirror a host-side attribute change into the live JavaScript DOM.
    pub fn set_attribute(&self, node: NodeId, name: &str, value: &str) {
        self.dom.borrow_mut().doc.set_attr(node, name, value);
    }

    /// Read an attribute from the live JavaScript DOM (diagnostics).
    pub fn attribute(&self, node: NodeId, name: &str) -> Option<String> {
        self.dom
            .borrow()
            .doc
            .attr(node, name)
            .map(|v| v.to_string())
    }

    /// Replace the live document, keeping the realm alive.
    ///
    /// The engine keeps its own snapshot for layout; when it applies an action
    /// that changed the tree (a reset, a select choice), it hands the result
    /// back here so the next script statement reads the new tree.
    pub fn set_document(&self, doc: Document) {
        self.dom.borrow_mut().doc = doc;
    }

    /// Flip a checkbox or radio in the live DOM. Returns whether anything
    /// changed, so the caller can dispatch `input`/`change` exactly once.
    pub fn toggle_checked(&self, node: NodeId) -> bool {
        let mut dom = self.dom.borrow_mut();
        let changed = dom.doc.toggle_checked(node);
        if changed {
            dom.invalidation.mark_style();
        }
        changed
    }

    /// Mirror a host-side attribute removal into the live JavaScript DOM.
    pub fn remove_attribute(&self, node: NodeId, name: &str) {
        let mut dom = self.dom.borrow_mut();
        if let NodeData::Element { attrs, .. } = &mut dom.doc.nodes[node].data {
            attrs.retain(|(n, _)| n != name);
        }
    }

    fn root_id(&self) -> usize {
        let doc = self.dom.borrow();
        doc.doc.find_tag("html").unwrap_or(doc.doc.root)
    }

    pub fn logs(&self) -> Vec<String> {
        std::mem::take(&mut self.dom.borrow_mut().logs)
    }

    /// Cheap copy of the live DOM for styling, layout and painting.
    pub fn snapshot(&self) -> Document {
        self.dom.borrow().doc.clone()
    }

    pub fn document(&self) -> std::cell::Ref<'_, Document> {
        std::cell::Ref::map(self.dom.borrow(), |d| &d.doc)
    }
}

fn register(ctx: &rquickjs::Ctx<'_>, dom: &Rc<RefCell<Dom>>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    let d = dom.clone();
    globals.set(
        "__naro_query_id",
        Function::new(ctx.clone(), move |id: String| -> i64 {
            let dom = d.borrow();
            // Walk the tree instead of the arena: a removed node is still in
            // `nodes`, but it is no longer part of the document.
            let mut stack = vec![dom.doc.root];
            while let Some(node) = stack.pop() {
                if dom.doc.attr(node, "id") == Some(id.as_str()) {
                    return node as i64;
                }
                for &child in dom.doc.children(node).iter().rev() {
                    stack.push(child);
                }
            }
            -1
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_query",
        Function::new(ctx.clone(), move |sel: String| -> i64 {
            let dom = d.borrow();
            naro_css::query_selector(&dom.doc, &sel)
                .map(|n| n as i64)
                .unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_query_all",
        Function::new(ctx.clone(), move |sel: String| -> Vec<i64> {
            let dom = d.borrow();
            naro_css::query_selector_all(&dom.doc, &sel)
                .into_iter()
                .map(|n| n as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_children",
        Function::new(ctx.clone(), move |id: i64| -> Vec<i64> {
            let dom = d.borrow();
            if id < 0 {
                return Vec::new();
            }
            dom.doc
                .children(id as usize)
                .iter()
                .filter(|&&c| dom.doc.tag(c).is_some())
                .map(|&c| c as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_create",
        Function::new(ctx.clone(), move |tag: String| -> i64 {
            let mut dom = d.borrow_mut();
            let tag = tag.to_ascii_lowercase();
            let parent = dom.doc.root;
            dom.doc.push_detached_element(tag, parent) as i64
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_head",
        Function::new(ctx.clone(), move || -> i64 {
            d.borrow().doc.find_tag("head").map(|n| n as i64).unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_body",
        Function::new(ctx.clone(), move || -> i64 {
            d.borrow().doc.find_tag("body").map(|n| n as i64).unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_root",
        Function::new(ctx.clone(), move || -> i64 {
            let dom = d.borrow();
            dom.doc
                .find_tag("html")
                .or_else(|| dom.doc.find_tag("body"))
                .map(|n| n as i64)
                .unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_attach_shadow",
        Function::new(ctx.clone(), move |id: i64, mode: String| -> i64 {
            if id < 0 {
                return -1;
            }
            let mode = if mode.eq_ignore_ascii_case("closed") {
                naro_html::ShadowMode::Closed
            } else {
                naro_html::ShadowMode::Open
            };
            let mut dom = d.borrow_mut();
            let root = dom.doc.attach_shadow(id as usize, mode);
            dom.invalidation.mark_style();
            root as i64
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_shadow_root",
        Function::new(ctx.clone(), move |id: i64| -> i64 {
            if id < 0 {
                return -1;
            }
            d.borrow()
                .doc
                .shadow_root(id as usize)
                .map(|root| root as i64)
                .unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        // Reachable from the document node. A node built by `createElement`
        // has the document as a placeholder parent, so `parent >= 0` is not
        // enough to tell whether it is on the page — and `connectedCallback`
        // must not run for it.
        "__naro_in_document",
        Function::new(ctx.clone(), move |id: i64| -> bool {
            if id < 0 {
                return false;
            }
            let dom = d.borrow();
            let mut cur = id as usize;
            loop {
                if cur == dom.doc.root {
                    return true;
                }
                match dom.doc.node(cur).parent {
                    Some(parent) if parent != cur => cur = parent,
                    _ => return false,
                }
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_shadow_mode",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            match d.borrow().doc.shadow_mode(id as usize) {
                Some(naro_html::ShadowMode::Open) => "open".to_string(),
                Some(naro_html::ShadowMode::Closed) => "closed".to_string(),
                None => String::new(),
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_is_shadow_root",
        Function::new(ctx.clone(), move |id: i64| -> bool {
            id >= 0 && d.borrow().doc.shadow_host(id as usize).is_some()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_elements_by_tag",
        Function::new(ctx.clone(), move |tag: String| -> Vec<i64> {
            d.borrow()
                .doc
                .elements_by_tag(&tag)
                .into_iter()
                .map(|id| id as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_get",
        Function::new(ctx.clone(), move |id: i64, name: String| -> Option<String> {
            if id < 0 {
                return None;
            }
            d.borrow().doc.attr(id as usize, &name).map(|v| v.to_string())
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_set",
        Function::new(ctx.clone(), move |id: i64, name: String, value: String| {
            if id < 0 {
                return;
            }
            d.borrow_mut().doc.set_attr(id as usize, &name, &value);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_value",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            let dom = d.borrow();
            dom.doc.attr(id as usize, "value").unwrap_or("").to_string()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_invalidate",
        Function::new(ctx.clone(), move |scope: String| {
            let mut dom = d.borrow_mut();
            match scope.as_str() {
                "paint" => dom.invalidation.mark_paint(),
                "layout" => dom.invalidation.mark_layout(),
                _ => dom.invalidation.mark_style(),
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_value_changed",
        Function::new(ctx.clone(), move |id: i64| {
            // Property sets are recorded so layout can redraw the field.
            if id >= 0 {
                d.borrow_mut().value_dirty.push(id as usize);
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_remove_attr",
        Function::new(ctx.clone(), move |id: i64, name: String| {
            if id < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            if let NodeData::Element { attrs, .. } = &mut dom.doc.nodes[id as usize].data {
                attrs.retain(|(n, _)| n != &name);
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_text",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            text_of(&d.borrow().doc, id as usize)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_set_text",
        Function::new(ctx.clone(), move |id: i64, value: String| {
            if id < 0 {
                return;
            }
            d.borrow_mut().doc.set_text_content(id as usize, &value);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_is_descendant",
        Function::new(ctx.clone(), move |node: i64, ancestor: i64| -> bool {
            if node < 0 || ancestor < 0 {
                return false;
            }
            let dom = d.borrow();
            let mut current = dom.doc.node(node as usize).parent;
            while let Some(parent) = current {
                if parent as i64 == ancestor {
                    return true;
                }
                current = dom.doc.node(parent).parent;
            }
            false
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_parent",
        Function::new(ctx.clone(), move |id: i64| -> i64 {
            if id < 0 {
                return -1;
            }
            d.borrow().doc.node(id as usize).parent.map(|p| p as i64).unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_insert_before",
        Function::new(ctx.clone(), move |parent: i64, child: i64, reference: i64| {
            if parent < 0 || child < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            let reference = if reference >= 0 { Some(reference as usize) } else { None };
            insert_node(&mut dom.doc, parent as usize, child as usize, reference);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_remove_child",
        Function::new(ctx.clone(), move |parent: i64, child: i64| {
            if parent < 0 || child < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            dom.doc.detach(child as usize);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_inner_html",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            let dom = d.borrow();
            serialize_children(&dom.doc, id as usize)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_set_inner_html",
        Function::new(ctx.clone(), move |id: i64, value: String| {
            if id < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            let fragment = naro_html::Document::parse(&format!("<body>{value}</body>"));
            if let Some(body) = fragment.find_tag("body") {
                dom.doc.set_text_content(id as usize, "");
                let copied: Vec<NodeId> = fragment
                    .children(body)
                    .iter()
                    .map(|&child| graft(&mut dom.doc, &fragment, child, id as usize))
                    .collect();
                for node in copied {
                    dom.doc.attach(id as usize, node);
                }
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_append",
        Function::new(ctx.clone(), move |parent: i64, child: i64| {
            if parent < 0 || child < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            insert_node(&mut dom.doc, parent as usize, child as usize, None);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_query_within",
        Function::new(ctx.clone(), move |id: i64, sel: String| -> i64 {
            if id < 0 {
                return -1;
            }
            let dom = d.borrow();
            naro_css::query_selector_within(&dom.doc, id as usize, &sel)
                .map(|n| n as i64)
                .unwrap_or(-1)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_query_all_within",
        Function::new(ctx.clone(), move |id: i64, sel: String| -> Vec<i64> {
            if id < 0 {
                return Vec::new();
            }
            let dom = d.borrow();
            naro_css::query_selector_all_within(&dom.doc, id as usize, &sel)
                .into_iter()
                .map(|n| n as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_url",
        Function::new(ctx.clone(), move || -> String { d.borrow().url.clone() })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_box",
        Function::new(ctx.clone(), move |id: i64| -> Vec<f32> {
            if id < 0 {
                return Vec::new();
            }
            let dom = d.borrow();
            match dom.geometry.get(id as usize).copied().flatten() {
                Some(b) => vec![
                    b.x,
                    b.y,
                    b.w,
                    b.h,
                    b.border_top,
                    b.border_right,
                    b.border_bottom,
                    b.border_left,
                ],
                None => Vec::new(),
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_child_nodes",
        Function::new(ctx.clone(), move |id: i64| -> Vec<i64> {
            if id < 0 {
                return Vec::new();
            }
            d.borrow()
                .doc
                .children(id as usize)
                .iter()
                .map(|&c| c as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_create_text",
        Function::new(ctx.clone(), move |text: String| -> i64 {
            d.borrow_mut().doc.push_detached_text(&text) as i64
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_matches",
        Function::new(ctx.clone(), move |id: i64, selector: String| -> bool {
            if id < 0 {
                return false;
            }
            let dom = d.borrow();
            if dom.doc.tag(id as usize).is_none() {
                return false;
            }
            naro_css::parse_selector_list_text(&selector)
                .iter()
                .any(|s| s.matches(&dom.doc, id as usize))
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_attrs",
        Function::new(ctx.clone(), move |id: i64| -> Vec<String> {
            if id < 0 {
                return Vec::new();
            }
            let dom = d.borrow();
            match dom.doc.attrs(id as usize) {
                Some(attrs) => {
                    let mut out = Vec::with_capacity(attrs.len() * 2);
                    for (name, value) in attrs {
                        out.push(name.clone());
                        out.push(value.clone());
                    }
                    out
                }
                None => Vec::new(),
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_insert_adjacent",
        Function::new(ctx.clone(), move |id: i64, position: String, html: String| -> Vec<i64> {
            if id < 0 {
                return Vec::new();
            }
            let mut dom = d.borrow_mut();
            let target = id as usize;
            let parent = dom.doc.node(target).parent;
            let fragment = naro_html::Document::parse(&format!("<body>{html}</body>"));
            let body = match fragment.find_tag("body") {
                Some(body) => body,
                None => return Vec::new(),
            };
            let sources: Vec<NodeId> = fragment.children(body).to_vec();
            // Graft first, so a failed position leaves the tree untouched.
            let mut grafted: Vec<NodeId> = Vec::with_capacity(sources.len());
            for child in sources {
                let node = match &fragment.node(child).data {
                    NodeData::Text(text) => {
                        let text = text.clone();
                        dom.doc.push_detached_text(&text)
                    }
                    NodeData::Element { .. } => graft(&mut dom.doc, &fragment, child, target),
                    NodeData::Document | NodeData::ShadowRoot { .. } | NodeData::Fragment => {
                        continue
                    }
                };
                grafted.push(node);
            }
            match position.to_ascii_lowercase().as_str() {
                "beforebegin" => {
                    let Some(parent) = parent else { return Vec::new() };
                    for &node in &grafted {
                        dom.doc.detach(node);
                        dom.doc.attach_before(parent, node, target);
                    }
                }
                "afterbegin" => {
                    let first = dom.doc.children(target).first().copied();
                    for &node in &grafted {
                        dom.doc.detach(node);
                        match first {
                            Some(reference) => dom.doc.attach_before(target, node, reference),
                            None => dom.doc.attach(target, node),
                        }
                    }
                }
                "beforeend" => {
                    for &node in &grafted {
                        dom.doc.detach(node);
                        dom.doc.attach(target, node);
                    }
                }
                "afterend" => {
                    let Some(parent) = parent else { return Vec::new() };
                    // Each node goes after the previously inserted one, so the
                    // source order is preserved.
                    let mut reference = target;
                    for &node in &grafted {
                        dom.doc.detach(node);
                        dom.doc.attach_after(parent, node, reference);
                        reference = node;
                    }
                }
                _ => return Vec::new(),
            }
            grafted.into_iter().map(|n| n as i64).collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_tag",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            d.borrow().doc.tag(id as usize).unwrap_or("").to_uppercase()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_log",
        Function::new(ctx.clone(), move |line: String| {
            d.borrow_mut().logs.push(line);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_error",
        Function::new(ctx.clone(), move |message: String| {
            d.borrow_mut().pending_errors.push(message);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_doc_title",
        Function::new(ctx.clone(), move || -> String { d.borrow().doc.title.clone() })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_set_doc_title",
        Function::new(ctx.clone(), move |value: String| {
            d.borrow_mut().doc.title = value;
        })?,
    )?;

    // --- platform surface (A1) -------------------------------------------
    // Every one of these reads state the engine already keeps: computed
    // styles, the viewport, the URL and the history. `naro-js` never derives
    // a value the renderer would compute differently.

    let d = dom.clone();
    globals.set(
        "__naro_computed",
        Function::new(ctx.clone(), move |id: i64, prop: String| -> Option<String> {
            if id < 0 {
                return None;
            }
            let dom = d.borrow();
            let fallback;
            let style = match dom.computed.get(id as usize) {
                Some(style) => style,
                // A node the engine has not styled yet (a detached element)
                // reports the initial value, as a browser does.
                None => {
                    fallback = naro_css::ComputedStyle::default();
                    &fallback
                }
            };
            naro_css::computed_value(style, &prop)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_element_children",
        Function::new(ctx.clone(), move |id: i64| -> Vec<i64> {
            if id < 0 {
                return Vec::new();
            }
            d.borrow()
                .doc
                .element_children(id as usize)
                .map(|c| c as i64)
                .collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_is_fragment",
        Function::new(ctx.clone(), move |id: i64| -> bool {
            id >= 0 && d.borrow().doc.is_fragment(id as usize)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_create_fragment",
        Function::new(ctx.clone(), move || -> i64 {
            d.borrow_mut().doc.push_detached_fragment() as i64
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_clone",
        Function::new(ctx.clone(), move |id: i64, deep: bool| -> i64 {
            if id < 0 {
                return -1;
            }
            let mut dom = d.borrow_mut();
            let plan = {
                let doc = &dom.doc;
                clone_plan(doc, id as usize, deep)
            };
            match plan {
                Some(plan) => {
                    let node = materialize(&mut dom.doc, &plan);
                    // A clone is never in the tree, even though the arena
                    // needs a placeholder parent while it is built.
                    dom.doc.detach(node);
                    node as i64
                }
                None => -1,
            }
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_elements_by_class",
        Function::new(ctx.clone(), move |id: i64, name: String| -> Vec<i64> {
            let dom = d.borrow();
            let root = if id < 0 { dom.doc.root } else { id as usize };
            let mut out = Vec::new();
            collect_elements(&dom.doc, root, &mut out, &mut |doc, node| {
                doc.attr(node, "class")
                    .map(|classes| classes.split_whitespace().any(|c| c == name))
                    .unwrap_or(false)
            });
            out.into_iter().map(|n| n as i64).collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_elements_by_tag_name",
        Function::new(ctx.clone(), move |id: i64, name: String| -> Vec<i64> {
            let dom = d.borrow();
            let root = if id < 0 { dom.doc.root } else { id as usize };
            let wanted = name.to_ascii_lowercase();
            let any = wanted == "*";
            let mut out = Vec::new();
            collect_elements(&dom.doc, root, &mut out, &mut |doc, node| {
                any || doc.tag(node).map(|t| t == wanted).unwrap_or(false)
            });
            out.into_iter().map(|n| n as i64).collect()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_outer_html",
        Function::new(ctx.clone(), move |id: i64| -> String {
            if id < 0 {
                return String::new();
            }
            let dom = d.borrow();
            let mut out = String::new();
            serialize_node(&dom.doc, id as usize, &mut out);
            out
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_set_outer_html",
        Function::new(ctx.clone(), move |id: i64, html: String| {
            if id < 0 {
                return;
            }
            let mut dom = d.borrow_mut();
            let target = id as usize;
            let parent = match dom.doc.node(target).parent {
                Some(parent) => parent,
                None => return,
            };
            let fragment = naro_html::Document::parse(&format!("<body>{html}</body>"));
            let Some(body) = fragment.find_tag("body") else {
                return;
            };
            let sources: Vec<NodeId> = fragment.children(body).to_vec();
            let mut grafted: Vec<NodeId> = Vec::with_capacity(sources.len());
            for child in sources {
                match &fragment.node(child).data {
                    NodeData::Text(text) => {
                        let text = text.clone();
                        grafted.push(dom.doc.push_detached_text(&text));
                    }
                    NodeData::Element { .. } => {
                        grafted.push(graft(&mut dom.doc, &fragment, child, parent))
                    }
                    NodeData::Document | NodeData::ShadowRoot { .. } | NodeData::Fragment => {}
                }
            }
            // Insert the replacements first, then drop the old node, so the
            // order matches the source even for several top-level nodes.
            for &node in &grafted {
                dom.doc.detach(node);
                dom.doc.attach_before(parent, node, target);
            }
            dom.doc.detach(target);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_insert_adjacent_text",
        Function::new(ctx.clone(), move |id: i64, position: String, text: String| -> i64 {
            if id < 0 || text.is_empty() {
                return -1;
            }
            let mut dom = d.borrow_mut();
            let target = id as usize;
            let node = dom.doc.push_detached_text(&text);
            let position = position.to_ascii_lowercase();
            match position.as_str() {
                "beforebegin" | "afterend" => {
                    let Some(parent) = dom.doc.node(target).parent else {
                        return -1;
                    };
                    if position == "beforebegin" {
                        dom.doc.attach_before(parent, node, target);
                    } else {
                        dom.doc.attach_after(parent, node, target);
                    }
                }
                "afterbegin" => match dom.doc.children(target).first().copied() {
                    Some(reference) => dom.doc.attach_before(target, node, reference),
                    None => dom.doc.attach(target, node),
                },
                "beforeend" => dom.doc.attach(target, node),
                _ => return -1,
            }
            node as i64
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_ready_state",
        Function::new(ctx.clone(), move || -> String { d.borrow().ready_state.to_string() })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_viewport",
        Function::new(ctx.clone(), move || -> Vec<f32> {
            let dom = d.borrow();
            vec![dom.viewport.0, dom.viewport.1]
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_url_parts",
        Function::new(ctx.clone(), move |input: String, base: String| -> Vec<String> {
            // An empty `base` means "the document URL", which is what
            // `location` and a relative `fetch` need.
            let base = if base.is_empty() {
                d.borrow().url.clone()
            } else {
                base
            };
            url_parts(&base, &input)
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_history_push",
        Function::new(ctx.clone(), move |url: String, replace: bool| {
            let mut dom = d.borrow_mut();
            // The address bar changes the moment script asks for it, exactly
            // like a browser; the engine's own history vector catches up on
            // its next flush. The op carries the absolute URL, because the
            // engine should not have to resolve anything twice.
            let mut resolved_url = String::new();
            if !url.is_empty() {
                let resolved = url_parts(&dom.url, &url);
                if !resolved[0].is_empty() {
                    dom.url = resolved[0].clone();
                    resolved_url = resolved[0].clone();
                }
            }
            if !replace {
                dom.history_index += 1;
            }
            dom.history_ops.push(HistoryOp::Push {
                url: resolved_url,
                replace,
            });
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_history_travel",
        Function::new(ctx.clone(), move |delta: i64| {
            let op = match delta {
                d if d < 0 => HistoryOp::Back,
                d if d > 0 => HistoryOp::Forward,
                _ => HistoryOp::Go(0),
            };
            d.borrow_mut().history_ops.push(op);
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_history_index",
        Function::new(ctx.clone(), move || -> i64 { d.borrow().history_index })?,
    )?;

    globals.set(
        "__naro_utf8_encode",
        Function::new(ctx.clone(), move |text: String| -> Vec<u8> {
            text.into_bytes()
        })?,
    )?;

    globals.set(
        "__naro_utf8_decode",
        Function::new(ctx.clone(), move |bytes: Vec<u8>| -> String {
            String::from_utf8_lossy(&bytes).to_string()
        })?,
    )?;

    // --- network surface (A2) --------------------------------------------
    // `fetch` and `XMLHttpRequest` are synchronous underneath: the host call
    // blocks the JS thread until the response arrives. That keeps one script
    // thread and one ordering rule (a `fetch` before a `fetch`), at the cost
    // of a slow server freezing the page for up to the transport timeout.

    let d = dom.clone();
    globals.set(
        "__naro_fetch",
        Function::new(
            ctx.clone(),
            move |method: String,
                url: String,
                body: String,
                headers: Vec<String>| -> Vec<String> {
                let base = d.borrow().url.clone();
                let absolute = naro_net::resolve(&base, &url).unwrap_or(url);
                let mut pairs: Vec<(String, String)> = Vec::new();
                let mut i = 0usize;
                while i + 1 < headers.len() {
                    pairs.push((headers[i].clone(), headers[i + 1].clone()));
                    i += 2;
                }
                let method = if method.is_empty() {
                    "GET".to_string()
                } else {
                    method.to_ascii_uppercase()
                };
                let payload = if body.is_empty() { None } else { Some(body) };
                let result = without_deadline(|| {
                    naro_net::with_session(|net| {
                        net.request(&method, &absolute, payload.as_deref().map(str::as_bytes), &pairs)
                    })
                });
                match result {
                    Ok(response) => vec![
                        response.status.to_string(),
                        response.status_text.clone(),
                        response.text(),
                        response.url.clone(),
                        String::new(),
                    ],
                    Err(err) => vec![
                        "0".to_string(),
                        String::new(),
                        String::new(),
                        absolute,
                        err,
                    ],
                }
            },
        )?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_toggle",
        Function::new(ctx.clone(), move |id: i64| -> bool {
            if id < 0 {
                return false;
            }
            let mut dom = d.borrow_mut();
            let changed = dom.doc.toggle_checked(id as usize);
            if changed {
                // The engine restyles from its next snapshot; marking it here
                // is what makes `:checked` take effect without a second click.
                dom.invalidation.mark_style();
            }
            changed
        })?,
    )?;

    let d = dom.clone();
    let d = dom.clone();
    globals.set(
        "__naro_selection",
        Function::new(ctx.clone(), move || -> String { d.borrow().selection_text.clone() })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_form_op",
        Function::new(ctx.clone(), move |id: i64, op: String| {
            if id < 0 {
                return;
            }
            let op = match op.as_str() {
                "reset" => FormOp::Reset,
                "open-select" => FormOp::OpenSelect,
                "click" => FormOp::DefaultClick,
                _ => FormOp::Submit,
            };
            d.borrow_mut().form_ops.push((id as usize, op));
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_cookie_get",
        Function::new(ctx.clone(), move || -> String {
            let url = d.borrow().url.clone();
            naro_net::with_session(|net| Ok(net.cookie_string(&url))).unwrap_or_default()
        })?,
    )?;

    let d = dom.clone();
    globals.set(
        "__naro_cookie_set",
        Function::new(ctx.clone(), move |value: String| {
            let url = d.borrow().url.clone();
            let _ = naro_net::with_session(|net| {
                net.set_cookie_string(&url, &value);
                Ok(())
            });
        })?,
    )?;

    Ok(())
}

/// Insert `child` under `parent`, before `reference` when given.
///
/// A `DocumentFragment` is a container, not a node: inserting one moves its
/// children and leaves it empty, which is what makes the common
/// "build a fragment, append it once" pattern cheap.
fn insert_node(doc: &mut Document, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
    if doc.is_fragment(child) {
        let moved: Vec<NodeId> = doc.children(child).to_vec();
        doc.nodes[child].children.clear();
        for node in moved {
            doc.detach(node);
            match reference {
                Some(reference) => doc.attach_before(parent, node, reference),
                None => doc.attach(parent, node),
            }
        }
        return;
    }
    doc.detach(child);
    match reference {
        Some(reference) => doc.attach_before(parent, child, reference),
        None => doc.attach(parent, child),
    }
}

/// A detached copy of one subtree, captured before the destination document is
/// mutated. Cloning through a plan keeps `cloneNode` from borrowing the arena
/// it is writing into.
enum ClonePlan {
    Text(String),
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<ClonePlan>,
    },
    Fragment {
        children: Vec<ClonePlan>,
    },
}

fn clone_plan(doc: &Document, id: NodeId, deep: bool) -> Option<ClonePlan> {
    match &doc.node(id).data {
        NodeData::Text(text) => Some(ClonePlan::Text(text.clone())),
        NodeData::Element { tag, attrs } => Some(ClonePlan::Element {
            tag: tag.clone(),
            attrs: attrs.clone(),
            children: if deep {
                doc.children(id)
                    .iter()
                    .filter_map(|&child| clone_plan(doc, child, true))
                    .collect()
            } else {
                Vec::new()
            },
        }),
        NodeData::Fragment => Some(ClonePlan::Fragment {
            children: if deep {
                doc.children(id)
                    .iter()
                    .filter_map(|&child| clone_plan(doc, child, true))
                    .collect()
            } else {
                Vec::new()
            },
        }),
        // A shadow root is not a child of its host and cannot be cloned by
        // `cloneNode`; the document node is not a valid argument at all.
        NodeData::Document | NodeData::ShadowRoot { .. } => None,
    }
}

fn materialize(dest: &mut Document, plan: &ClonePlan) -> NodeId {
    match plan {
        ClonePlan::Text(text) => dest.push_detached_text(text),
        ClonePlan::Element {
            tag,
            attrs,
            children,
        } => {
            let id = dest.push_detached_element(tag.clone(), dest.root);
            for (name, value) in attrs {
                dest.set_attr(id, name, value);
            }
            for child in children {
                let child = materialize(dest, child);
                dest.attach(id, child);
            }
            id
        }
        ClonePlan::Fragment { children } => {
            let id = dest.push_detached_fragment();
            for child in children {
                let child = materialize(dest, child);
                dest.attach(id, child);
            }
            id
        }
    }
}

/// Every element under `root` (excluding `root` itself) that `keep` accepts,
/// in document order. Walks child lists only, so it never crosses into a
/// shadow tree — the same boundary `querySelectorAll` respects.
fn collect_elements(
    doc: &Document,
    root: NodeId,
    out: &mut Vec<NodeId>,
    keep: &mut dyn FnMut(&Document, NodeId) -> bool,
) {
    let mut stack: Vec<NodeId> = doc.children(root).iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        if doc.tag(node).is_some() && keep(doc, node) {
            out.push(node);
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
}

/// `[href, origin, protocol, host, hostname, port, pathname, search, hash]`
/// for `input` resolved against `base`.
///
/// Parsing lives in Rust (the `url` crate) so the JS `URL` class inherits the
/// same resolution rules the loader uses for relative links.
fn url_parts(base: &str, input: &str) -> Vec<String> {
    let trimmed = input.trim();
    let parsed = match url::Url::parse(trimmed) {
        Ok(url) => url,
        Err(_) => match url::Url::parse(base).and_then(|b| b.join(trimmed)) {
            Ok(url) => url,
            Err(_) => return vec![String::new(); 9],
        },
    };
    vec![
        parsed.as_str().to_string(),
        parsed.origin().ascii_serialization(),
        format!("{}:", parsed.scheme()),
        parsed.host_str().map(|h| match parsed.port() {
            Some(port) => format!("{h}:{port}"),
            None => h.to_string(),
        })
        .unwrap_or_default(),
        parsed.host_str().unwrap_or("").to_string(),
        parsed.port().map(|p| p.to_string()).unwrap_or_default(),
        parsed.path().to_string(),
        match parsed.query() {
            Some(q) => format!("?{q}"),
            None => String::new(),
        },
        match parsed.fragment() {
            Some(f) => format!("#{f}"),
            None => String::new(),
        },
    ]
}

fn text_of(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    // Pushed in reverse so `pop` yields the children in document order.
    let mut stack: Vec<NodeId> = doc.children(id).iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        match &doc.node(node).data {
            NodeData::Text(t) => out.push_str(t),
            _ => {
                for &child in doc.children(node).iter().rev() {
                    stack.push(child);
                }
            }
        }
    }
    out
}

/// Copy the subtree rooted at `source` into `dest`, attaching it to `parent`.
/// Returns the id of the copied root.
fn graft(dest: &mut Document, source: &Document, from: NodeId, parent: NodeId) -> NodeId {
    let tag = source.tag(from).unwrap_or("span").to_string();
    let new_id = dest.push_detached_element(tag, parent);
    if let NodeData::Element { attrs, .. } = &source.node(from).data {
        let attrs = attrs.clone();
        for (name, value) in attrs {
            dest.set_attr(new_id, &name, &value);
        }
    }
    for &child in source.children(from) {
        match &source.node(child).data {
            NodeData::Text(text) => {
                let text = text.clone();
                dest.push_text_node(new_id, &text);
            }
            NodeData::Element { .. } => {
                let child_id = graft(dest, source, child, new_id);
                dest.attach(new_id, child_id);
            }
            // Parsed fragments never contain shadow roots: the parser has no
            // syntax for them, and `innerHTML` does not clone an existing one.
            NodeData::Document | NodeData::ShadowRoot { .. } | NodeData::Fragment => {}
        }
    }
    new_id
}

/// Serialize the children of `id` back to HTML (used by `innerHTML`).
fn serialize_children(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    for &child in doc.children(id) {
        serialize_node(doc, child, &mut out);
    }
    out
}

fn serialize_node(doc: &Document, id: NodeId, out: &mut String) {
    match &doc.node(id).data {
        NodeData::Text(text) => out.push_str(text),
        // `innerHTML` serializes the light tree; a shadow root is not part of
        // it, so there is nothing to emit. A fragment is not a child either.
        NodeData::ShadowRoot { .. } | NodeData::Document | NodeData::Fragment => return,
        NodeData::Element { .. } => {
            let tag = doc.tag(id).unwrap_or("span");
            out.push('<');
            out.push_str(tag);
            if let Some(attrs) = doc.attrs(id) {
                for (name, value) in attrs {
                    out.push(' ');
                    out.push_str(name);
                    out.push('=');
                    out.push('"');
                    out.push_str(value);
                    out.push('"');
                }
            }
            out.push('>');
            for &child in doc.children(id) {
                serialize_node(doc, child, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Invalidation;

    #[test]
    fn paint_only_never_implies_layout() {
        let mut invalidation = Invalidation::default();
        invalidation.mark_paint();
        assert!(invalidation.paint);
        assert!(!invalidation.style, "a value change is not a style change");
        assert!(!invalidation.layout, "a value change must not rebuild boxes");
        assert!(!invalidation.is_empty());
    }

    #[test]
    fn style_implies_paint_but_not_layout() {
        let mut invalidation = Invalidation::default();
        invalidation.mark_style();
        assert!(invalidation.style);
        assert!(invalidation.paint);
        assert!(!invalidation.layout, "layout is decided by the style diff");
    }

    #[test]
    fn layout_implies_paint() {
        let mut invalidation = Invalidation::default();
        invalidation.mark_layout();
        assert!(invalidation.layout);
        assert!(invalidation.paint, "new boxes have to be drawn");
        assert!(!invalidation.style);
    }

    #[test]
    fn merge_unions_every_flag() {
        let mut a = Invalidation::default();
        a.mark_style();
        let mut b = Invalidation::default();
        b.mark_layout();
        b.mark_paint();
        a.merge(b);
        assert!(a.style && a.layout && a.paint);

        let mut c = Invalidation::default();
        c.merge(Invalidation::default());
        assert!(c.is_empty(), "merging nothing changes nothing");
        c.clear();
        assert!(c.is_empty());
    }

    // --- Shadow DOM and Custom Elements ----------------------------------

    fn run_script(
        html: &str,
        source: &str,
    ) -> (naro_html::Document, Vec<String>, Vec<String>) {
        let doc = naro_html::Document::parse(html);
        let scripts = vec![("teste".to_string(), source.to_string())];
        let (session, outcome) = crate::JsSession::start(doc, &scripts);
        let snapshot = session.as_ref().map(|s| s.snapshot()).expect("session");
        (snapshot, outcome.logs, outcome.errors)
    }

    fn has_line(lines: &[String], want: &str) -> bool {
        lines.iter().any(|line| line == want)
    }

    #[test]
    fn attach_shadow_exposes_the_root_and_its_content() {
        let (_doc, logs, errors) = run_script(
            "<body><div id=\"host\"></div></body>",
            r##"
            const host = document.getElementById("host");
            const root = host.attachShadow({ mode: "open" });
            root.innerHTML = "<p id='inside'>sombra</p>";
            console.log("same=" + (host.shadowRoot === root));
            console.log("text=" + root.querySelector("#inside").textContent);
            console.log("hidden=" + (document.querySelector("#inside") === null));
            console.log("node=" + root.nodeType + "," + (root.host === host) + "," + root.mode);
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "same=true"), "logs: {logs:?}");
        assert!(has_line(&logs, "text=sombra"), "logs: {logs:?}");
        // `document.querySelector` must not cross the shadow boundary.
        assert!(has_line(&logs, "hidden=true"), "logs: {logs:?}");
        assert!(has_line(&logs, "node=11,true,open"), "logs: {logs:?}");
    }

    #[test]
    fn closed_shadow_root_is_not_reachable_from_the_host() {
        let (doc, logs, errors) = run_script(
            "<body><div id=\"host\"></div></body>",
            r##"
            const host = document.getElementById("host");
            const root = host.attachShadow({ mode: "closed" });
            root.innerHTML = "<span>trancado</span>";
            console.log("host=" + (host.shadowRoot === null));
            console.log("root=" + root.mode + "," + root.textContent);
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "host=true"), "logs: {logs:?}");
        assert!(has_line(&logs, "root=closed,trancado"), "logs: {logs:?}");
        // The tree exists in the DOM even though the host hides it.
        assert!(!doc.shadow_roots.is_empty());
    }

    #[test]
    fn custom_element_connected_callback_runs_on_define() {
        let (doc, logs, errors) = run_script(
            "<body><x-counter id=\"c\"></x-counter></body>",
            r##"
            class XCounter extends HTMLElement {
              connectedCallback() { this.textContent = "conectado"; }
            }
            customElements.define("x-counter", XCounter);
            const el = document.getElementById("c");
            console.log("text=" + el.textContent);
            console.log("inst=" + (el instanceof XCounter) + "," + (el instanceof HTMLElement));
            console.log("get=" + (customElements.get("x-counter") === XCounter));

            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "text=conectado"), "logs: {logs:?}");
        assert!(has_line(&logs, "inst=true,true"), "logs: {logs:?}");
        assert!(has_line(&logs, "get=true"), "logs: {logs:?}");
        let _ = doc;
    }

    #[test]
    fn connected_callback_waits_until_the_element_is_on_the_page() {
        let (_doc, logs, errors) = run_script(
            "<body></body>",
            r##"
            class XLate extends HTMLElement {
              connectedCallback() { console.log("conectado"); }
            }
            customElements.define("x-late", XLate);
            const el = document.createElement("x-late");
            console.log("antes=" + (el instanceof XLate));
            document.body.appendChild(el);
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        // `instanceof` already holds after `createElement`, but the callback
        // must wait for insertion: exactly one "conectado".
        assert!(has_line(&logs, "antes=true"), "logs: {logs:?}");
        assert_eq!(
            logs.iter().filter(|l| l.as_str() == "conectado").count(),
            1,
            "logs: {logs:?}"
        );
    }

    #[test]
    fn custom_element_created_later_is_upgraded() {
        let (_doc, logs, errors) = run_script(
            "<body></body>",
            r##"
            class XItem extends HTMLElement {
              connectedCallback() { this.setAttribute("data-ready", "1"); }
            }
            customElements.define("x-item", XItem);
            const el = document.createElement("x-item");
            document.body.appendChild(el);
            console.log("ready=" + el.getAttribute("data-ready"));
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "ready=1"), "logs: {logs:?}");
    }

    #[test]
    fn custom_element_disconnected_callback_runs_on_remove() {
        let (_doc, logs, errors) = run_script(
            "<body><x-gone id=\"g\"></x-gone></body>",
            r##"
            class XGone extends HTMLElement {
              disconnectedCallback() { console.log("bye"); }
            }
            customElements.define("x-gone", XGone);
            document.getElementById("g").remove();
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "bye"), "logs: {logs:?}");
    }

    #[test]
    fn custom_element_inside_a_shadow_tree_is_upgraded() {
        let (_doc, logs, errors) = run_script(
            "<body><div id=\"host\"></div></body>",
            r##"
            class XInner extends HTMLElement {
              connectedCallback() { console.log("inner"); }
            }
            const root = document.getElementById("host").attachShadow({ mode: "open" });
            root.innerHTML = "<x-inner></x-inner>";
            customElements.define("x-inner", XInner);
            "##,
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(has_line(&logs, "inner"), "logs: {logs:?}");
    }

    #[test]
    fn invalid_custom_element_names_throw() {
        let (_doc, logs, errors) = run_script(
            "<body></body>",
            r##"
            try {
              customElements.define("semhifen", class extends HTMLElement {});
            } catch (e) {
              console.log("erro");
            }
            "##,
        );
        assert!(has_line(&logs, "erro"), "logs: {logs:?}");
        assert!(errors.is_empty(), "errors: {errors:?}");
    }
}
