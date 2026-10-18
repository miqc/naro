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

const BOOTSTRAP: &str = r#"
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
        __naro_dispatch(id, String(ev && ev.type ? ev.type : ev), path);
        return true;
      },
      click() {
        const path = [];
        let cur = id;
        while (cur >= 0) { path.push(cur); cur = __naro_parent(cur); }
        __naro_dispatch(id, "click", path);
      },
      get tagName() { return __naro_tag(id); },
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
        // 11 = shadow root, 1 = element, 3 = text, 9 = document.
        if (__naro_is_shadow_root(id)) return 11;
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
    get body() { return wrap(__naro_body()); },
    get head() { return wrap(__naro_head()); },
    get documentElement() { return wrap(__naro_root()); },
    get title() { return __naro_doc_title(); },
    set title(v) { __naro_set_doc_title(String(v)); }
  };
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
  // `location` is derived from the document URL the engine loaded. Scripts
  // read `location.search`/`hash` for query-string handling; without it any
  // such script dies on `undefined`.
  globalThis.location = (function () {
    const href = __naro_url();
    const split = (marker) => {
      const i = href.indexOf(marker);
      return i < 0 ? "" : href.slice(i);
    };
    const beforeQuery = href.split("?")[0].split('#')[0];
    const scheme = beforeQuery.indexOf("://");
    const rest = scheme < 0 ? beforeQuery : beforeQuery.slice(scheme + 3);
    const slash = rest.indexOf("/");
    const host = slash < 0 ? rest : rest.slice(0, slash);
    return {
      href: href,
      search: split("?").split('#')[0],
      hash: split('#'),
      protocol: scheme < 0 ? "" : href.slice(0, scheme + 1),
      host: host,
      hostname: host.split(":")[0],
      port: host.indexOf(":") < 0 ? "" : host.slice(host.indexOf(":") + 1),
      pathname: slash < 0 ? "/" : rest.slice(slash).split("?")[0].split('#')[0],
      origin: scheme < 0 ? "" : href.slice(0, scheme + 3) + host,
      reload: function () {},
      assign: function () {},
      replace: function () {},
      toString: function () { return href; }
    };
  })();
  globalThis.CustomEvent = function (type) { this.type = String(type); };

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
  globalThis.__naro_dispatch = function (id, type, path) {
    // `path` is [target, parent, ..., root]: capture, target, then bubbling.
    const ev = {
      type: type,
      target: wrap(id),
      currentTarget: null,
      eventPhase: 0,
      defaultPrevented: false,
      __stop: false,
      __stopImmediate: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this.__stop = true; },
      stopImmediatePropagation() { this.__stop = true; this.__stopImmediate = true; }
    };
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
      // Zero-delay chains must not spin forever inside one host call.
      if (t.due >= now) break;
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
"#;

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
        JsSession::start_measured(doc, scripts, geometry, None, "")
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
        measure: Option<&mut dyn FnMut(&Document) -> Vec<Option<ElementBox>>>,
        url: &str,
    ) -> (Option<JsSession>, ScriptOutcome) {
        let dom = Rc::new(RefCell::new(Dom {
            doc,
            logs: Vec::new(),
            pending_errors: Vec::new(),
            value_dirty: Vec::new(),
            invalidation: Invalidation::default(),
            geometry,
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
        out.merge(session.call_js("DOMContentLoaded", |ctx| {
            let f = ctx.globals().get::<_, Function>("__naro_dispatch")?;
            f.call::<_, ()>((root as i64, "DOMContentLoaded", vec![root as i64]))
        }));
        out.merge(session.drain_microtasks());
        if let Some(measure) = measure {
            // Re-measure once the load handlers have run: a timer that reads
            // geometry right after `DOMContentLoaded` must not see the tree as
            // it was before those handlers touched it.
            if session.has_pending_timers() {
                let boxes = measure(&session.dom.borrow().doc);
                session.set_geometry(boxes);
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
        self.call_js("microtasks", |ctx| {
            let f = ctx.globals().get::<_, Function>("__naro_run_microtasks")?;
            f.call::<_, ()>(())
        })
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
            dom.doc.detach(child as usize);
            if reference >= 0 {
                dom.doc.attach_before(parent as usize, child as usize, reference as usize);
            } else {
                dom.doc.attach(parent as usize, child as usize);
            }
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
            dom.doc.detach(child as usize);
            dom.doc.attach(parent as usize, child as usize);
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
                    NodeData::Document | NodeData::ShadowRoot { .. } => continue,
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

    Ok(())
}

fn text_of(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    let mut stack: Vec<NodeId> = doc.children(id).to_vec();
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
            NodeData::Document | NodeData::ShadowRoot { .. } => {}
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
        // it, so there is nothing to emit.
        NodeData::ShadowRoot { .. } | NodeData::Document => return,
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
