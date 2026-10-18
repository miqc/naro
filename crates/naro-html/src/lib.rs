//! NARO's own HTML tokenizer and tree builder.
//!
//! This is intentionally a small, forgiving subset of HTML: it never fails,
//! it lowercases tag and attribute names, and it understands the handful of
//! implied-end-tag rules that real pages rely on (`p`, `li`, `td`, `tr`).

pub type NodeId = usize;

/// Visibility of a shadow tree to page script.
///
/// `attachShadow({mode:"closed"})` still creates a tree; it is simply not
/// reachable through `element.shadowRoot`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowMode {
    Open,
    Closed,
}

#[derive(Clone, Debug)]
pub enum NodeData {
    Document,
    Element { tag: String, attrs: Vec<(String, String)> },
    Text(String),
    /// A shadow root. It is **not** a child of its host: `host.children` stays
    /// the light DOM, and composition is what puts this tree on screen. Its
    /// `parent` is the host, so inherited properties flow in and
    /// `getRootNode()` works.
    ShadowRoot { host: NodeId, mode: ShadowMode },
}

#[derive(Clone, Debug)]
pub struct Node {
    pub data: NodeData,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub title: String,
    /// Host → shadow root, in attachment order. A side table rather than a
    /// field on `Node` because the shadow root is not part of the host's
    /// child list and every tree walk must keep ignoring it.
    pub shadow_roots: Vec<(NodeId, NodeId)>,
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

const RAW_TEXT: &[&str] = &["script", "style", "title", "textarea"];

const CLOSES_P: &[&str] = &[
    "div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "section", "article",
    "header", "footer", "nav", "main", "aside", "blockquote", "pre", "table", "form", "fieldset",
    "figure", "hr", "address",
];

impl Document {
    pub fn parse(src: &str) -> Document {
        let mut doc = Document {
            nodes: Vec::new(),
            root: 0,
            title: String::new(),
            shadow_roots: Vec::new(),
        };
        doc.nodes.push(Node {
            data: NodeData::Document,
            parent: None,
            children: Vec::new(),
        });
        let mut stack: Vec<NodeId> = vec![0];
        let mut title_seen = false;
        let bytes = src.as_bytes();
        let mut i = 0usize;

        while i < bytes.len() {
            if bytes[i] != b'<' {
                let start = i;
                while i < bytes.len() && bytes[i] != b'<' {
                    i += 1;
                }
                let raw = &src[start..i];
                let text = decode_entities(raw);
                if !text.is_empty() {
                    let parent = *stack.last().unwrap();
                    doc.push_text(parent, text);
                }
                continue;
            }

            if src[i..].starts_with("<!--") {
                i = match src[i + 4..].find("-->") {
                    Some(pos) => i + 4 + pos + 3,
                    None => bytes.len(),
                };
                continue;
            }

            if src[i..].starts_with("<!") {
                i = match src[i..].find('>') {
                    Some(pos) => i + pos + 1,
                    None => bytes.len(),
                };
                continue;
            }

            if src[i..].starts_with("</") {
                let (name, next) = read_tag_name(src, i + 2);
                // Skip the rest of the closing tag (attributes/whitespace) and its `>`.
                i = match src[next..].find('>') {
                    Some(pos) => next + pos + 1,
                    None => bytes.len(),
                };
                if name.is_empty() {
                    continue;
                }
                if let Some(pos) = stack.iter().rposition(|&id| {
                    matches!(&doc.nodes[id].data, NodeData::Element { tag, .. } if tag == &name)
                }) {
                    stack.truncate(pos);
                }
                continue;
            }

            let (name, after_name) = read_tag_name(src, i + 1);
            if name.is_empty() {
                let parent = *stack.last().unwrap();
                doc.push_text(parent, "<".to_string());
                i += 1;
                continue;
            }
            let (attrs, self_closing, next) = read_attrs(src, after_name);
            i = next;

            if name == "p" {
                while let Some(&top) = stack.last() {
                    if doc.tag(top) == Some("p") {
                        stack.pop();
                    } else {
                        break;
                    }
                }
            }
            if CLOSES_P.contains(&name.as_str()) {
                if let Some(&top) = stack.last() {
                    if doc.tag(top) == Some("p") {
                        stack.pop();
                    }
                }
            }
            let closes_self = match name.as_str() {
                "li" => Some("li"),
                "td" | "th" => Some("td"),
                "tr" => Some("tr"),
                "option" => Some("option"),
                _ => None,
            };
            if let Some(target) = closes_self {
                if let Some(&top) = stack.last() {
                    let top_tag = doc.tag(top);
                    if top_tag == Some(target) || (target == "td" && top_tag == Some("th")) {
                        stack.pop();
                    }
                }
            }

            let id = doc.push_element(*stack.last().unwrap(), name.clone(), attrs);
            let is_void = VOID.contains(&name.as_str()) || self_closing;
            if is_void {
                continue;
            }

            if RAW_TEXT.contains(&name.as_str()) {
                let close = find_ci_close(src, i, &name).unwrap_or(bytes.len());
                let raw = &src[i..close];
                let text = decode_entities(raw);
                if !text.is_empty() {
                    doc.push_text(id, text.clone());
                    if name == "title" && !title_seen {
                        doc.title = text.trim().to_string();
                        title_seen = true;
                    }
                }
                i = match src[close..].find('>') {
                    Some(pos) => close + pos + 1,
                    None => bytes.len(),
                };
                continue;
            }

            stack.push(id);
        }

        doc
    }

    fn push_element(&mut self, parent: NodeId, tag: String, attrs: Vec<(String, String)>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::Element { tag, attrs },
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    fn push_text(&mut self, parent: NodeId, text: String) {
        let id = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::Text(text),
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            NodeData::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    /// Text of every `<style>` element, in document order.
    ///
    /// Computed on demand rather than cached at parse time, so a `<style>`
    /// inserted, removed or rewritten by JavaScript reaches the cascade
    /// without any extra bookkeeping.
    pub fn collect_style_text(&self) -> String {
        self.collect_style_text_from(self.root)
    }

    /// Text of every `<style>` inside `root`'s subtree, in document order.
    ///
    /// Used for a shadow root's own stylesheet: `collect_style_text` walks the
    /// document tree, which never reaches a shadow tree.
    pub fn collect_style_text_from(&self, root: NodeId) -> String {
        let mut out = String::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if self.tag(id) == Some("style") {
                self.collect_text(id, &mut out);
                out.push('\n');
            }
            for &child in self.nodes[id].children.iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    fn collect_text(&self, id: NodeId, out: &mut String) {
        for &child in &self.nodes[id].children {
            match &self.nodes[child].data {
                NodeData::Text(text) => out.push_str(text),
                NodeData::Element { .. } => self.collect_text(child, out),
                // A shadow root is never in a child list, so this arm is
                // unreachable; it exists to keep the match exhaustive.
                NodeData::Document | NodeData::ShadowRoot { .. } => {}
            }
        }
    }

    pub fn attrs(&self, id: NodeId) -> Option<&[(String, String)]> {
        match &self.nodes[id].data {
            NodeData::Element { attrs, .. } => Some(attrs),
            _ => None,
        }
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        self.attrs(id)?
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Create an element that is not yet attached to the tree.
    pub fn push_detached_element(&mut self, tag: String, parent: NodeId) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::Element { tag, attrs: Vec::new() },
            parent: Some(parent),
            children: Vec::new(),
        });
        id
    }

    /// Remove a node from its parent's child list and clear its parent.
    ///
    /// The node keeps its own children, so a detached subtree stays intact;
    /// `attach`/`attach_before` set the parent again. Clearing it is what makes
    /// `parentNode` and `isConnected` honest after `remove()`.
    pub fn detach(&mut self, id: NodeId) {
        if let Some(parent) = self.nodes[id].parent {
            self.nodes[parent].children.retain(|&c| c != id);
            self.nodes[id].parent = None;
        }
    }

    /// Insert `child` before `reference` in `parent`'s children.
    pub fn attach_before(&mut self, parent: NodeId, child: NodeId, reference: NodeId) {
        self.nodes[child].parent = Some(parent);
        let pos = self.nodes[parent]
            .children
            .iter()
            .position(|&c| c == reference)
            .unwrap_or(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(pos, child);
    }

    pub fn attach(&mut self, parent: NodeId, child: NodeId) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    /// Create a text node that is not attached to any parent.
    pub fn push_detached_text(&mut self, text: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::Text(text.to_string()),
            parent: None,
            children: Vec::new(),
        });
        id
    }

    /// Insert `child` immediately after `reference`, under `parent`.
    pub fn attach_after(&mut self, parent: NodeId, child: NodeId, reference: NodeId) {
        self.nodes[child].parent = Some(parent);
        let pos = self.nodes[parent]
            .children
            .iter()
            .position(|&c| c == reference)
            .map(|i| i + 1)
            .unwrap_or(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(pos, child);
    }

    /// Append a text node to `parent`.
    pub fn push_text_node(&mut self, parent: NodeId, text: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::Text(text.to_string()),
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Replace an element's children with a single text node.
    pub fn set_text_content(&mut self, id: NodeId, value: &str) {
        let children = std::mem::take(&mut self.nodes[id].children);
        for child in children {
            self.nodes[child].parent = None;
        }
        if !value.is_empty() {
            let text = self.nodes.len();
            self.nodes.push(Node {
                data: NodeData::Text(value.to_string()),
                parent: Some(id),
                children: Vec::new(),
            });
            self.nodes[id].children.push(text);
        }
    }

    /// Rewrite an attribute in place; used to absolutize resource URLs.
    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        if let NodeData::Element { attrs, .. } = &mut self.nodes[id].data {
            if let Some(slot) = attrs.iter_mut().find(|(n, _)| n == name) {
                slot.1 = value.to_string();
            } else {
                attrs.push((name.to_string(), value.to_string()));
            }
        }
    }

    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            NodeData::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .filter(move |&c| matches!(self.nodes[c].data, NodeData::Element { .. }))
    }

    /// 1-based position among the element siblings of `id`. Text and comment
    /// nodes are not counted, matching `:nth-child` semantics.
    pub fn element_index(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes[id].parent?;
        let mut index = 0usize;
        for &child in &self.nodes[parent].children {
            if matches!(self.nodes[child].data, NodeData::Element { .. }) {
                index += 1;
                if child == id {
                    return Some(index);
                }
            }
        }
        None
    }

    /// Immediately preceding sibling that is an element.
    pub fn previous_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let mut previous = None;
        for &child in &self.nodes[parent].children {
            if child == id {
                return previous;
            }
            if matches!(self.nodes[child].data, NodeData::Element { .. }) {
                previous = Some(child);
            }
        }
        None
    }

    /// Immediately following sibling that is an element.
    pub fn next_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let mut seen = false;
        for &child in &self.nodes[parent].children {
            if child == id {
                seen = true;
                continue;
            }
            if seen && matches!(self.nodes[child].data, NodeData::Element { .. }) {
                return Some(child);
            }
        }
        None
    }

    /// Depth-first search for the first element with the given tag.
    /// Attach an empty shadow root to `host`, returning its id.
    ///
    /// Attaching twice returns the existing root: the DOM throws there, but a
    /// second call is a no-op for the tree, and the JS wrapper is the layer
    /// that reports the error.
    pub fn attach_shadow(&mut self, host: NodeId, mode: ShadowMode) -> NodeId {
        if let Some(root) = self.shadow_root(host) {
            return root;
        }
        let root = self.nodes.len();
        self.nodes.push(Node {
            data: NodeData::ShadowRoot { host, mode },
            parent: Some(host),
            children: Vec::new(),
        });
        self.shadow_roots.push((host, root));
        root
    }

    /// The shadow root attached to `host`, if any — closed ones included.
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.shadow_roots
            .iter()
            .find(|(h, _)| *h == host)
            .map(|(_, root)| *root)
    }

    /// The host of a shadow root node.
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        match &self.nodes.get(root)?.data {
            NodeData::ShadowRoot { host, .. } => Some(*host),
            _ => None,
        }
    }

    /// The mode a shadow root was attached with.
    pub fn shadow_mode(&self, root: NodeId) -> Option<ShadowMode> {
        match &self.nodes.get(root)?.data {
            NodeData::ShadowRoot { mode, .. } => Some(*mode),
            _ => None,
        }
    }

    /// Nearest enclosing shadow root of `id`, if it is inside a shadow tree.
    ///
    /// The shadow root is not part of the document tree, so this is also the
    /// test for "is this node shadow content": a node with a scope belongs to
    /// exactly that tree.
    pub fn shadow_scope(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = id;
        loop {
            if matches!(self.nodes.get(cur)?.data, NodeData::ShadowRoot { .. }) {
                return Some(cur);
            }
            cur = self.nodes.get(cur)?.parent?;
        }
    }

    /// Every element with this tag, document order, shadow trees included.
    ///
    /// Custom-element upgrades need this: `querySelectorAll` deliberately does
    /// not cross a shadow boundary, but an upgrade walks every node the
    /// document owns.
    pub fn elements_by_tag(&self, tag: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if let NodeData::Element { tag: name, .. } = &node.data {
                if name == tag {
                    out.push(id);
                }
            }
        }
        out.sort_unstable();
        out
    }

    pub fn find_tag(&self, tag: &str) -> Option<NodeId> {
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if self.tag(id) == Some(tag) {
                return Some(id);
            }
            for &child in self.nodes[id].children.iter().rev() {
                stack.push(child);
            }
        }
        None
    }
}

fn read_tag_name(src: &str, start: usize) -> (String, usize) {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() || c == b'/' || c == b'>' {
            break;
        }
        i += 1;
    }
    let name = src[start..i].to_ascii_lowercase();
    (name, i)
}

fn read_attrs(src: &str, start: usize) -> (Vec<(String, String)>, bool, usize) {
    let bytes = src.as_bytes();
    let mut attrs = Vec::new();
    let mut i = start;
    let mut self_closing = false;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'>' {
            i += 1;
            break;
        }
        if bytes[i] == b'/' {
            self_closing = true;
            i += 1;
            if i < bytes.len() && bytes[i] == b'>' {
                i += 1;
                break;
            }
            continue;
        }
        let name_start = i;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_whitespace() || c == b'=' || c == b'>' || c == b'/' {
                break;
            }
            i += 1;
        }
        let name = src[name_start..i].to_ascii_lowercase();
        if name.is_empty() {
            i += 1;
            continue;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let vstart = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = src[vstart..i].to_string();
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                let vstart = i;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c.is_ascii_whitespace() || c == b'>' {
                        break;
                    }
                    i += 1;
                }
                value = src[vstart..i].to_string();
            }
        }
        attrs.push((name, value));
    }
    (attrs, self_closing, i)
}

/// Find `</tag` (case-insensitive) starting at `from`, returning its offset.
fn find_ci_close(src: &str, from: usize, tag: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let needle = format!("</{}", tag);
    let nb = needle.as_bytes();
    let mut i = from;
    while i + nb.len() <= bytes.len() {
        if bytes[i] == b'<' && bytes[i..i + nb.len()].eq_ignore_ascii_case(nb) {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn decode_entities(src: &str) -> String {
    if !src.contains('&') {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'&' {
                i += 1;
            }
            out.push_str(&src[start..i]);
            continue;
        }
        let semi = match src[i..].find(';') {
            Some(pos) if pos <= 10 => i + pos,
            _ => {
                out.push('&');
                i += 1;
                continue;
            }
        };
        let body = &src[i + 1..semi];
        let decoded = if let Some(num) = body.strip_prefix('#') {
            let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                num.parse::<u32>().ok()
            };
            code.and_then(char::from_u32).map(|c| c.to_string())
        } else {
            match body {
                "amp" => Some("&".to_string()),
                "lt" => Some("<".to_string()),
                "gt" => Some(">".to_string()),
                "quot" => Some("\"".to_string()),
                "apos" => Some("'".to_string()),
                "nbsp" => Some("\u{00a0}".to_string()),
                "copy" => Some("\u{00a9}".to_string()),
                "mdash" => Some("\u{2014}".to_string()),
                "ndash" => Some("\u{2013}".to_string()),
                "hellip" => Some("\u{2026}".to_string()),
                "rsquo" => Some("\u{2019}".to_string()),
                "lsquo" => Some("\u{2018}".to_string()),
                "ldquo" => Some("\u{201c}".to_string()),
                "rdquo" => Some("\u{201d}".to_string()),
                _ => None,
            }
        };
        match decoded {
            Some(text) => {
                out.push_str(&text);
                i = semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_id(doc: &Document, id: &str) -> NodeId {
        (0..doc.nodes.len())
            .find(|&n| doc.attr(n, "id") == Some(id))
            .unwrap_or_else(|| panic!("no element with id {id:?}"))
    }

    #[test]
    fn attach_shadow_keeps_the_light_tree_separate() {
        let mut doc = Document::parse(
            "<body><div id=\"host\"><p id=\"light\">luz</p></div></body>",
        );
        let host = by_id(&doc, "host");
        let root = doc.attach_shadow(host, ShadowMode::Open);
        doc.push_text_node(root, "sombra");

        // The shadow root is not a child of the host: the light tree is
        // untouched, which is what keeps `host.children` meaningful.
        assert!(!doc.children(host).contains(&root));
        assert_eq!(doc.shadow_root(host), Some(root));
        assert_eq!(doc.shadow_host(root), Some(host));
        assert_eq!(doc.shadow_mode(root), Some(ShadowMode::Open));
        // Its parent is the host, so inherited properties flow in and
        // `getRootNode()` can walk out.
        assert_eq!(doc.node(root).parent, Some(host));
        // A shadow root is not an element.
        assert_eq!(doc.tag(root), None);
    }

    #[test]
    fn shadow_scope_names_the_enclosing_tree() {
        let mut doc = Document::parse("<body><div id=\"host\"></div></body>");
        let host = by_id(&doc, "host");
        let root = doc.attach_shadow(host, ShadowMode::Closed);
        let inner = doc.push_detached_element("span".into(), root);
        doc.attach(root, inner);

        assert_eq!(doc.shadow_scope(inner), Some(root));
        assert_eq!(doc.shadow_scope(root), Some(root));
        // The host itself is in the document, not in the tree it hosts.
        assert_eq!(doc.shadow_scope(host), None);
    }

    #[test]
    fn collect_style_text_keeps_shadow_styles_out_of_the_document() {
        let mut doc = Document::parse(
            "<html><head><style>.page{color:red}</style></head><body><div id=\"host\"></div></body></html>",
        );
        let host = by_id(&doc, "host");
        let root = doc.attach_shadow(host, ShadowMode::Open);
        let style = doc.push_detached_element("style".into(), root);
        doc.attach(root, style);
        doc.push_text_node(style, ".inner{color:blue}");

        let page = doc.collect_style_text();
        assert!(page.contains(".page"), "document sheet kept: {page:?}");
        assert!(!page.contains(".inner"), "shadow sheet leaked: {page:?}");

        let tree = doc.collect_style_text_from(root);
        assert!(tree.contains(".inner"));
        assert!(!tree.contains(".page"));
    }

    #[test]
    fn elements_by_tag_sees_shadow_content() {
        // Custom-element upgrade walks every node the document owns, which
        // includes shadow trees — `querySelectorAll` deliberately does not.
        let mut doc = Document::parse("<body><div id=\"host\"></div></body>");
        let host = by_id(&doc, "host");
        let root = doc.attach_shadow(host, ShadowMode::Open);
        let inner = doc.push_detached_element("x-item".into(), root);
        doc.attach(root, inner);

        let found = doc.elements_by_tag("x-item");
        assert_eq!(found, vec![inner]);
        // The document's own query never crosses the boundary.
        assert!(doc.find_tag("x-item").is_none());
    }

    #[test]
    fn parses_entities_and_attributes() {
        let doc =
            Document::parse("<DIV ID=\"x\" class=hero data-v='1'>a &amp; b &lt;c&gt; &#65;&#x42;</DIV>");
        let div = doc.find_tag("div").unwrap();
        assert_eq!(doc.attr(div, "id"), Some("x"));
        assert_eq!(doc.attr(div, "class"), Some("hero"));
        assert_eq!(doc.attr(div, "data-v"), Some("1"));
        let text = doc
            .children(div)
            .iter()
            .filter_map(|&c| doc.text(c))
            .collect::<String>();
        assert_eq!(text, "a & b <c> AB");
    }

    #[test]
    fn auto_closes_paragraphs_and_list_items() {
        let doc = Document::parse("<p>one<p>two<div>three</div><ul><li>a<li>b</ul>");
        let ps: Vec<NodeId> = (0..doc.nodes.len())
            .filter(|&i| doc.tag(i) == Some("p"))
            .collect();
        assert_eq!(ps.len(), 2);
        assert_eq!(doc.nodes[ps[0]].parent, Some(0));
        assert_eq!(doc.nodes[ps[1]].parent, Some(0));
        let lis: Vec<NodeId> = (0..doc.nodes.len())
            .filter(|&i| doc.tag(i) == Some("li"))
            .collect();
        assert_eq!(lis.len(), 2);
        let ul = doc.find_tag("ul").unwrap();
        assert_eq!(doc.nodes[lis[0]].parent, Some(ul));
        assert_eq!(doc.nodes[lis[1]].parent, Some(ul));
    }

    #[test]
    fn captures_title_and_style() {
        let doc = Document::parse(
            "<html><head><title> Hi &amp; bye </title><style>p { color: red }</style></head><body><p>x</p></body></html>",
        );
        assert_eq!(doc.title, "Hi & bye");
        assert!(doc.collect_style_text().contains("color: red"));
        assert_eq!(doc.tag(doc.find_tag("body").unwrap()), Some("body"));
    }

    #[test]
    fn void_elements_do_not_nest() {
        let doc = Document::parse("<p>a<br>b<img src=x><hr></p>");
        let p = doc.find_tag("p").unwrap();
        let tags: Vec<Option<&str>> = doc.children(p).iter().map(|&c| doc.tag(c)).collect();
        assert!(tags.contains(&Some("br")));
        assert!(tags.contains(&Some("img")));
        // `hr` closes the paragraph, so it is a sibling rather than a child.
        let hr = doc.find_tag("hr").unwrap();
        assert_eq!(doc.nodes[hr].parent, doc.nodes[p].parent);
    }

    #[test]
    fn element_index_skips_text_nodes() {
        let doc = Document::parse("<div>a<b></b>c<i></i></div>");
        let b = doc.find_tag("b").unwrap();
        let i = doc.find_tag("i").unwrap();
        assert_eq!(doc.element_index(b), Some(1));
        assert_eq!(doc.element_index(i), Some(2));
    }

    #[test]
    fn previous_element_sibling_skips_text() {
        let doc = Document::parse("<div>a<b></b>c<i></i></div>");
        let b = doc.find_tag("b").unwrap();
        let i = doc.find_tag("i").unwrap();
        assert_eq!(doc.previous_element_sibling(i), Some(b));
        assert_eq!(doc.previous_element_sibling(b), None);
        assert_eq!(doc.next_element_sibling(b), Some(i));
        assert_eq!(doc.next_element_sibling(i), None);
    }

    #[test]
    fn unclosed_tags_still_yield_usable_tree() {
        let doc = Document::parse("<div><span>text");
        let div = doc.find_tag("div").unwrap();
        let span = doc.find_tag("span").unwrap();
        assert_eq!(doc.nodes[span].parent, Some(div));
        assert_eq!(doc.children(span).len(), 1);
    }
}
