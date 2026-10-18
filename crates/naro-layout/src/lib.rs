//! Block and inline layout over NARO's own box tree.
//!
//! Supported: block flow, inline text with greedy line breaking, margins,
//! padding, borders, text alignment. Not supported: floats, tables, width /
//! max-width, flex/grid, images.

pub mod svg;

use naro_css::{
    AlignContent, AlignItems, Color, ComputedStyle, Display, Edges, FlexDirection, FlexWrap,
    JustifyContent, Overflow, Position, TextAlign, Track,
};
use naro_image::SharedImage;
use std::collections::HashMap;
use naro_html::{Document, NodeId};
use naro_text::TextMeasure;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl TextRun {
    pub fn text<'a>(&self, layout: &'a Layout) -> &'a str {
        let start = self.start as usize;
        let end = start + self.len as usize;
        layout.text.get(start..end).unwrap_or("")
    }
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Debug)]
pub struct BlockBox {
    pub rect: Rect,
    pub background: Color,
    pub background_image: Option<SharedImage>,
    pub border: Edges,
    pub border_color: Color,
    pub tag: String,
    /// Source DOM node, so clicks can be routed to JavaScript listeners.
    pub node: usize,
    /// Effective clip inherited from ancestors with `overflow != visible`.
    pub clip: Option<Rect>,
    pub border_radius: f32,
    pub box_shadow: Option<(f32, f32, f32, Color)>,
    /// `opacity: 0` and `visibility: hidden` keep the box but skip the paint.
    pub paint: bool,
    /// `opacity`, applied to every colour this box paints.
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub struct TextRun {
    pub x: f32,
    pub baseline: f32,
    /// Byte range into `Layout::text`, the per-document text arena.
    pub start: u32,
    pub len: u32,
    pub size: f32,
    pub bold: bool,
    pub color: Color,
    pub link: Option<String>,
    pub underline: bool,
    /// DOM node whose computed style produced this run, so a paint-only
    /// restyle can refresh `color`/`underline` without rebuilding the line.
    pub node: usize,
    /// `visibility`/`opacity` of the run's element.
    pub paint: bool,
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub struct LinkBox {
    pub rect: Rect,
    pub href: String,
}

#[derive(Clone, Debug)]
pub struct ImageBox {
    pub rect: Rect,
    pub image: SharedImage,
}

/// One filled SVG shape, in device coordinates.
///
/// Inline `<svg>` is common for wordmarks and icons; the engine draws these
/// after the boxes so they sit where the layout put them. Only filled paths
/// are supported — no strokes, gradients, transforms or `clip-path`.
#[derive(Clone, Debug)]
pub struct SvgShape {
    pub node: usize,
    /// Subpaths in user space, filled together with the nonzero rule.
    pub subpaths: Vec<svg::SubPath>,
    /// Top-left corner of the viewport in page coordinates, before scaling.
    pub origin: (f32, f32),
    /// User-space to device-space scale.
    pub scale: (f32, f32),
    pub color: Color,
    pub paint: bool,
    pub opacity: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// One arena for every string painted from this document. Runs hold byte
    /// ranges instead of their own `String`, so a page costs one allocation
    /// per layout instead of one per text run.
    pub text: String,
    pub blocks: Vec<BlockBox>,
    pub text_runs: Vec<TextRun>,
    pub links: Vec<LinkBox>,
    pub images: Vec<ImageBox>,
    pub content_height: f32,
    /// Page background, taken from `<body>`.
    pub background: Color,
    /// Node `background` came from, so a paint-only refresh can update it.
    pub background_node: Option<usize>,
    /// Filled SVG shapes, painted above the boxes.
    pub svg_shapes: Vec<SvgShape>,
}

impl Layout {
    /// Copy the purely visual attributes out of `styles` into the boxes and
    /// runs, leaving every position and size untouched.
    ///
    /// The layout is the only thing that knows where boxes go, so a restyle
    /// that changes only colours has no geometry work to do — but the colours
    /// live in the boxes because that is what the painter reads. Refreshing
    /// them here keeps the result identical to a full rebuild at a fraction of
    /// the cost, and makes "paint-only" a real category instead of a comment.
    pub fn refresh_paint(
        &mut self,
        styles: &[ComputedStyle],
        images: &HashMap<String, SharedImage>,
    ) {
        for block in &mut self.blocks {
            let Some(style) = styles.get(block.node) else { continue };
            block.paint = style.visible && style.opacity > 0.0;
            block.opacity = style.opacity;
            block.background = style.background;
            block.border_color = style.border_color;
            block.border_radius = style.border_radius;
            block.box_shadow = style.box_shadow;
            block.background_image = style
                .background_image
                .as_ref()
                .and_then(|url| images.get(url))
                .cloned();
        }
        for run in &mut self.text_runs {
            let Some(style) = styles.get(run.node) else { continue };
            run.paint = style.visible && style.opacity > 0.0;
            run.opacity = style.opacity;
            run.color = style.color;
            run.underline = style.underline;
        }
        if let Some(style) = self.background_node.and_then(|node| styles.get(node)) {
            self.background = style.background;
        }
    }
}

enum InlineItem {
    Text { text: String, style: ComputedStyle, link: Option<String>, node: NodeId },
    /// Whitespace between inline elements. It separates words but does not
    /// start a line, and it must not affect block margin collapsing.
    Space,
    Break,
}

enum FlowItem {
    Block(BoxNode),
    /// `input`/`button`: a real box that participates in the current line
    /// (the browser's `inline-block`), so it keeps hit-testing and painting.
    InlineBox(BoxNode),
    Inline(InlineItem),
    Image(ImageNode),
}

struct ImageNode {
    src: String,
    width_attr: Option<f32>,
    height_attr: Option<f32>,
    /// Absolutely positioned images are taken out of the inline flow and
    /// placed against the nearest positioned ancestor.
    absolute: bool,
    top: Option<f32>,
    right: Option<f32>,
    bottom: Option<f32>,
    left: Option<f32>,
}

struct BoxNode {
    node: NodeId,
    style: ComputedStyle,
    tag: String,
    items: Vec<FlowItem>,
    /// `colspan`/`rowspan` on a table cell; 1 when absent or invalid.
    colspan: usize,
    rowspan: usize,
}

fn image_node(doc: &Document, id: NodeId) -> Option<ImageNode> {
    let src = doc.attr(id, "src")?.trim().to_string();
    if src.is_empty() {
        return None;
    }
    let dim = |name: &str| {
        doc.attr(id, name)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
            .filter(|v| *v > 0.0)
    };
    // Inline `style="position:absolute;right:5px;top:4px"` is how Google pins
    // the keyboard icon inside the search field.
    let style = doc.attr(id, "style").unwrap_or("").to_ascii_lowercase();
    let absolute = style.contains("position:absolute") || style.contains("position: absolute");
    let px = |name: &str| -> Option<f32> {
        let idx = style.find(&format!("{name}:"))?;
        let rest = &style[idx + name.len() + 1..];
        let end = rest.find(';').unwrap_or(rest.len());
        rest[..end].trim().trim_end_matches("px").trim().parse::<f32>().ok()
    };
    Some(ImageNode {
        src,
        width_attr: dim("width"),
        height_attr: dim("height"),
        absolute,
        top: px("top"),
        right: px("right"),
        bottom: px("bottom"),
        left: px("left"),
    })
}

/// A declared height that does not depend on a containing-block height.
///
/// `height: 50%` needs the containing block's height, which content-driven
/// block layout does not have (the parent's height comes *from* its children).
/// CSS resolves such a percentage to `auto` in that case, so the box keeps its
/// content height instead of borrowing the width as the reference.
fn definite_height(style: &ComputedStyle) -> Option<f32> {
    style.height.as_ref().and_then(|h| h.absolute())
}

/// Best-effort height for an inline-level box before it is laid out.
fn predicted_height(b: &BoxNode) -> f32 {
    let s = &b.style;
    let content = definite_height(s).unwrap_or(s.font_size * s.line_height);
    content + s.padding.vertical_at(1000.0) + s.border.vertical()
}

/// What a text control paints: its `value`, or its placeholder.
fn control_text(doc: &Document, id: NodeId) -> Option<String> {
    if doc.tag(id) == Some("select") {
        return Some(select_text(doc, id));
    }
    if let Some(value) = doc.attr(id, "value") {
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let placeholder = doc.attr(id, "placeholder").unwrap_or("");
    if placeholder.is_empty() {
        None
    } else {
        Some(placeholder.to_string())
    }
}

/// The label a closed `<select>` paints: its selected option, or the first.
pub fn select_text(doc: &Document, id: NodeId) -> String {
    let mut first: Option<String> = None;
    let mut stack: Vec<NodeId> = doc.children(id).iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        if doc.tag(node) == Some("option") {
            let text = doc
                .attr(node, "label")
                .map(|l| l.to_string())
                .unwrap_or_else(|| doc.text_content(node).trim().to_string());
            if doc.attr(node, "selected").is_some() {
                return text;
            }
            if first.is_none() {
                first = Some(text);
            }
            continue;
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
    first.unwrap_or_default()
}

/// Box-level display values: these take part in the block flow.
fn is_block_level(display: Display) -> bool {
    matches!(
        display,
        Display::Block
            | Display::Flex
            | Display::Grid
            | Display::Table
            | Display::TableRow
            | Display::TableCell
    )
}

/// `absolute` and `fixed` are removed from the normal flow.
fn is_out_of_flow(position: Position) -> bool {
    matches!(position, Position::Absolute | Position::Fixed)
}

fn is_hidden_tag(tag: &str) -> bool {
    // `option` and `optgroup` are painted by the `<select>` they belong to,
    // never in the flow: a closed select shows one line of text.
    matches!(
        tag,
        "head" | "script" | "style" | "title" | "meta" | "link" | "base" | "option" | "optgroup"
    )
}

/// Elements that must not generate a box: `hidden`, and non-visual inputs.
fn is_hidden_node(doc: &Document, id: NodeId) -> bool {
    if doc.attr(id, "hidden").is_some() {
        return true;
    }
    if doc.tag(id) == Some("input") {
        if let Some(kind) = doc.attr(id, "type") {
            if kind.eq_ignore_ascii_case("hidden") {
                return true;
            }
        }
    }
    false
}

/// Collapse ASCII whitespace runs into a single space, preserving NBSP.
fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c == '\u{00a0}' {
            out.push(c);
            in_space = false;
        } else if c.is_ascii_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// The children actually laid out for `id`.
///
/// A host with a shadow root shows the shadow tree: flat-tree composition puts
/// the shadow content where the light children would be. `<slot>` distribution
/// is not implemented, so the light children are not laid out at all — a
/// documented limitation, not a silent mix of the two trees.
fn composed_children(doc: &Document, id: NodeId) -> &[NodeId] {
    doc.children(doc.shadow_root(id).unwrap_or(id))
}

fn build_box(doc: &Document, styles: &[ComputedStyle], id: NodeId, link: Option<String>) -> BoxNode {
    let style = styles[id].clone();
    let tag = doc.tag(id).unwrap_or("").to_string();
    let items = build_children(doc, styles, id, link);
    BoxNode {
        node: id,
        style,
        tag,
        items,
        colspan: attr_span(doc, id, "colspan"),
        rowspan: attr_span(doc, id, "rowspan"),
    }
}

/// `colspan`/`rowspan`, clamped to something a table can lay out.
fn attr_span(doc: &Document, id: NodeId, name: &str) -> usize {
    doc.attr(id, name)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 100)
}

fn build_children(
    doc: &Document,
    styles: &[ComputedStyle],
    parent: NodeId,
    link: Option<String>,
) -> Vec<FlowItem> {
    let mut items = Vec::new();
    let container = styles[parent].display;
    build_children_into(doc, styles, parent, link, container, &mut items);
    items
}

/// Same as [`build_children`], but `container` is the display of the box whose
/// flow these items join. `display: contents` elements are transparent, so
/// their children inherit the grandparent's context instead of their own.
fn build_children_into(
    doc: &Document,
    styles: &[ComputedStyle],
    parent: NodeId,
    link: Option<String>,
    container: Display,
    items: &mut Vec<FlowItem>,
) {
    for &child in composed_children(doc, parent) {
        match &doc.node(child).data {
            naro_html::NodeData::Text(raw) => {
                let text = collapse_ws(raw);
                if text.is_empty() {
                    continue;
                }
                // Whitespace between inline elements separates words in HTML;
                // dropping it glued anchors and buttons together.
                if text.trim().is_empty() {
                    items.push(FlowItem::Inline(InlineItem::Space));
                    continue;
                }
                items.push(FlowItem::Inline(InlineItem::Text {
                    text,
                    style: styles[child].clone(),
                    link: link.clone(),
                    node: child,
                }));
            }
            // A shadow root is never in a child list; `composed_children`
            // returns its children instead.
            naro_html::NodeData::ShadowRoot { .. }
            | naro_html::NodeData::Document
            | naro_html::NodeData::Fragment => {}
            naro_html::NodeData::Element { .. } => {
                let tag = doc.tag(child).unwrap_or("");
                if is_hidden_tag(tag) || is_hidden_node(doc, child) {
                    continue;
                }
                let style = &styles[child];
                if style.display == Display::None {
                    continue;
                }
                let child_link = if tag == "a" {
                    doc.attr(child, "href").map(|h| h.to_string()).or(link.clone())
                } else {
                    link.clone()
                };
                if tag == "br" {
                    items.push(FlowItem::Inline(InlineItem::Break));
                    continue;
                }
                if tag == "img" {
                    if let Some(img) = image_node(doc, child) {
                        items.push(FlowItem::Image(img));
                    }
                    continue;
                }

                // Form controls are inline-level boxes: they sit in the line
                // but still need a box for painting and hit-testing.
                if matches!(tag, "input" | "button" | "select")
                    && styles[parent].display != Display::Flex
                {
                    let mut node = build_box(doc, styles, child, child_link);
                    // The control paints its value/placeholder inside itself.
                    if let Some(text) = control_text(doc, child) {
                        node.items.insert(
                            0,
                            FlowItem::Inline(InlineItem::Text {
                                text,
                                style: styles[child].clone(),
                                link: None,
                                node: child,
                            }),
                        );
                    }
                    items.push(FlowItem::InlineBox(node));
                    continue;
                }
                // `display: contents` removes the box: the children join this
                // flow directly, with the same container context.
                if style.display == Display::Contents {
                    build_children_into(doc, styles, child, child_link, container, items);
                    continue;
                }
                // `<svg>` is an inline-level replaced element: it draws its
                // own content and never lays out its children as boxes.
                // `<svg>` is inline-level, but a flex container blockifies it
                // like any other child, so the flex algorithm can size it.
                if tag == "svg" && container != Display::Flex {
                    items.push(FlowItem::InlineBox(build_box(doc, styles, child, child_link)));
                    continue;
                }
                // `inline-block` is an inline-level atom that shrink-wraps and
                // lays out its own block contents. `display: inline` stays a
                // plain inline box: CSS ignores `width`/`height` there, and
                // Chrome does too (a 60x20 inline span paints 8x17 of text).
                if style.display == Display::InlineBlock && container != Display::Flex {
                    items.push(FlowItem::InlineBox(build_box(doc, styles, child, child_link)));
                    continue;
                }
                // Children of a flex container are blockified, as in CSS.
                if !matches!(style.display, Display::Inline) || container == Display::Flex {
                    let mut node = build_box(doc, styles, child, child_link);
                    // Text controls paint their value (or placeholder) inside
                    // their own box, so they stay focusable and hit-testable.
                    if tag == "textarea" {
                        if let Some(text) = control_text(doc, child) {
                            node.items.insert(
                                0,
                                FlowItem::Inline(InlineItem::Text {
                                    text,
                                    style: styles[child].clone(),
                                    link: None,
                                    node: child,
                                }),
                            );
                        }
                    }
                    // List markers belong to the item, not to the list: they
                    // must share the first line with the item's own text.
                    if tag == "li" && styles[child].list_marker {
                        node.items.insert(
                            0,
                            FlowItem::Inline(InlineItem::Text {
                                text: "\u{2022} ".to_string(),
                                style: styles[child].clone(),
                                link: None,
                                node: child,
                            }),
                        );
                    }
                    items.push(FlowItem::Block(node));
                } else {
                    flatten_inline(doc, styles, child, &child_link, items);
                }
            }
        }
    }
}

/// Inline elements do not create boxes; their children join the parent flow.
fn flatten_inline(
    doc: &Document,
    styles: &[ComputedStyle],
    id: NodeId,
    link: &Option<String>,
    out: &mut Vec<FlowItem>,
) {
    for &child in composed_children(doc, id) {
        match &doc.node(child).data {
            naro_html::NodeData::Text(raw) => {
                let text = collapse_ws(raw);
                if text.is_empty() {
                    continue;
                }
                if text.trim().is_empty() {
                    out.push(FlowItem::Inline(InlineItem::Space));
                    continue;
                }
                out.push(FlowItem::Inline(InlineItem::Text {
                    text,
                    style: styles[child].clone(),
                    link: link.clone(),
                    node: child,
                }));
            }
            naro_html::NodeData::ShadowRoot { .. }
            | naro_html::NodeData::Document
            | naro_html::NodeData::Fragment => {}
            naro_html::NodeData::Element { .. } => {
                let tag = doc.tag(child).unwrap_or("");
                if is_hidden_tag(tag) || is_hidden_node(doc, child) {
                    continue;
                }
                let style = &styles[child];
                if style.display == Display::None {
                    continue;
                }
                let child_link = if tag == "a" {
                    doc.attr(child, "href").map(|h| h.to_string()).or_else(|| link.clone())
                } else {
                    link.clone()
                };
                if tag == "br" {
                    out.push(FlowItem::Inline(InlineItem::Break));
                } else if tag == "img" {
                    if let Some(img) = image_node(doc, child) {
                        out.push(FlowItem::Image(img));
                    }
                } else if tag == "svg" {
                    out.push(FlowItem::InlineBox(build_box(doc, styles, child, child_link)));
                } else if style.display == Display::Contents {
                    // No box: the children behave as if they were siblings.
                    flatten_inline(doc, styles, child, &child_link, out);
                } else if style.display == Display::InlineBlock {
                    out.push(FlowItem::InlineBox(build_box(doc, styles, child, child_link)));
                } else if style.display != Display::Inline {
                    out.push(FlowItem::Block(build_box(doc, styles, child, child_link)));
                } else {
                    flatten_inline(doc, styles, child, &child_link, out);
                }
            }
        }
    }
}

/// Tolerance used when deciding whether a word fits a line.
///
/// Shrink-to-fit boxes get a width that came out of the same measurement, but
/// as a border-box minus padding it round-trips through float subtraction. A
/// sub-pixel tolerance keeps a word from wrapping onto its own line because of
/// that last bit — the error is invisible, the wrap is not.
const FIT_EPSILON: f32 = 0.05;

struct Word {
    /// Byte range into `LineBuilder::buf`, so words never allocate.
    start: u32,
    len: u32,
    /// Horizontal offset from the line start, including any atoms placed
    /// before this word on the same line.
    x: f32,
    size: f32,
    bold: bool,
    color: Color,
    link: Option<String>,
    underline: bool,
    node: NodeId,
    paint: bool,
    opacity: f32,
    width: f32,
    /// Width of the space that separates this word from the previous one.
    space_before: f32,
    line_height: f32,
}

/// Where an inline atom's layout output lives, so the line's alignment shift
/// can move the box, its descendants, its text runs and its images together.
#[derive(Clone, Copy, Debug)]
struct AtomMark {
    block: usize,
    runs: (usize, usize),
    images: (usize, usize),
}

struct LineBuilder<'a> {
    measure: &'a dyn TextMeasure,
    x: f32,
    width: f32,
    align: TextAlign,
    y: f32,
    default_size: f32,
    default_line_height: f32,
    words: Vec<Word>,
    /// Tallest inline atom (image, input, button) placed on this line.
    atom_height: f32,
    /// Backing store for the words of the current line; cleared on flush.
    buf: String,
    /// Memoized metrics per (size, bold) for the current line.
    metrics_cache: Vec<(u32, bool, f32, f32)>,
    line_width: f32,
    /// Whitespace was seen since the last emitted word, so the next one needs a space.
    pending_space: bool,
    runs: Vec<TextRun>,
    links: Vec<LinkBox>,
    /// Arena for this builder; merged into `Flow::arena` when the flow ends.
    arena: String,
    /// Inline atoms on the line being built, with the ranges of text runs and
    /// images their own layout produced. Alignment applies to the whole line at
    /// flush time, so an atom must not centre itself.
    pending_atoms: Vec<AtomMark>,
    /// `(mark, dx)` for atoms whose line has been flushed.
    atom_shifts: Vec<(AtomMark, f32)>,
}

impl<'a> LineBuilder<'a> {
    fn new(
        measure: &'a dyn TextMeasure,
        x: f32,
        y: f32,
        width: f32,
        align: TextAlign,
        default_size: f32,
        default_line_height: f32,
        arena: String,
    ) -> Self {
        LineBuilder {
            measure,
            x,
            width,
            align,
            y,
            default_size,
            default_line_height,
            words: Vec::new(),
            atom_height: 0.0,
            buf: String::new(),
            metrics_cache: Vec::new(),
            line_width: 0.0,
            pending_space: false,
            runs: Vec::new(),
            links: Vec::new(),
            arena,
            pending_atoms: Vec::new(),
            atom_shifts: Vec::new(),
        }
    }

    /// Horizontal cursor inside the line, for inline-level boxes.
    fn x_offset(&self) -> f32 {
        self.line_width
    }

    fn y_top(&self) -> f32 {
        self.y
    }

    /// True when nothing has been placed on the current line yet.
    fn at_line_start(&self) -> bool {
        self.words.is_empty() && self.atom_height <= 0.0 && self.line_width <= 0.0
    }

    /// Width of the separating space that would precede an atom here.
    fn atom_space(&self, size: f32, bold: bool) -> f32 {
        if self.pending_space && !self.at_line_start() {
            self.measure.measure(" ", size, bold)
        } else {
            0.0
        }
    }

    /// Does an atom of `width` (plus `lead` space) still fit on this line?
    fn fits(&self, width: f32, lead: f32) -> bool {
        if self.width <= 0.0 {
            return true;
        }
        if self.at_line_start() {
            return true;
        }
        self.line_width + lead + width <= self.width + FIT_EPSILON
    }

    /// Remember where an inline atom's box went, so `flush` can shift the whole
    /// line by the alignment offset instead of centring each atom separately.
    fn record_atom(&mut self, mark: AtomMark) {
        self.pending_atoms.push(mark);
    }

    /// Record an inline atom (image or form control) on the current line.
    fn place_atom(&mut self, width: f32, height: f32) {
        self.line_width += width;
        self.atom_height = self.atom_height.max(height);
        self.pending_space = false;
    }

    fn advance(&mut self, width: f32) {
        self.line_width += width;
        self.pending_space = true;
    }

    fn reset_x(&mut self) {
        self.line_width = 0.0;
    }

    fn hard_break(&mut self) {
        if self.words.is_empty() && self.atom_height <= 0.0 {
            self.y += self.default_size * self.default_line_height;
        } else {
            self.flush();
        }
    }

    fn push_text(&mut self, text: &str, style: &ComputedStyle, link: &Option<String>, node: NodeId) {
        let mut start: Option<usize> = None;
        for (i, c) in text.char_indices() {
            if c == ' ' {
                if let Some(s) = start.take() {
                    self.push_word(&text[s..i], style, link, node);
                }
                self.pending_space = true;
            } else if start.is_none() {
                start = Some(i);
            }
        }
        if let Some(s) = start {
            self.push_word(&text[s..], style, link, node);
        }
    }

    fn push_word(&mut self, text: &str, style: &ComputedStyle, link: &Option<String>, node: NodeId) {
        let size = style.font_size;
        let bold = style.font_weight >= 600;
        let word_w = self.measure.measure(text, size, bold);
        let mut space_w = if self.pending_space && !self.words.is_empty() {
            self.space_width(size, bold)
        } else {
            0.0
        };
        if !self.words.is_empty()
            && self.line_width + space_w + word_w > self.width + FIT_EPSILON
            && self.width > 0.0
        {
            self.flush();
            space_w = 0.0;
        }
        if space_w + word_w <= self.width + FIT_EPSILON || self.width <= 0.0 {
            self.line_width += space_w;
            self.add_word(text, style, link, node, word_w, space_w);
        } else {
            self.split_long_word(text, style, link, node, space_w);
        }
        self.pending_space = false;
    }

    fn add_word(
        &mut self,
        text: &str,
        style: &ComputedStyle,
        link: &Option<String>,
        node: NodeId,
        width: f32,
        space_before: f32,
    ) {
        let start_x = self.line_width;
        self.line_width += width;
        let start = self.buf.len() as u32;
        self.buf.push_str(text);
        self.words.push(Word {
            start,
            len: text.len() as u32,
            x: start_x,
            size: style.font_size,
            bold: style.font_weight >= 600,
            color: style.color,
            link: link.clone(),
            underline: style.underline,
            node,
            paint: style.visible && style.opacity > 0.0,
            opacity: style.opacity,
            width,
            space_before,
            line_height: style.font_size * style.line_height,
        });
    }

/// Greedy per-character breaking for words wider than the line.
    fn split_long_word(
        &mut self,
        text: &str,
        style: &ComputedStyle,
        link: &Option<String>,
        node: NodeId,
        leading_space: f32,
    ) {
        let size = style.font_size;
        let bold = style.font_weight >= 600;
        let mut chunk_start = 0usize;
        let mut chunk_w = 0.0;
        let mut space = leading_space;
        for (i, ch) in text.char_indices() {
            let cw = self.char_width(ch, size, bold);
            if i > chunk_start && self.line_width + chunk_w + cw > self.width + FIT_EPSILON {
                let (done, rest) = text.split_at(i);
                let _ = rest;
                self.add_word(&done[chunk_start..], style, link, node, chunk_w, space);
                self.flush();
                chunk_start = i;
                chunk_w = 0.0;
                space = 0.0;
            }
            chunk_w += cw;
        }
        if chunk_start < text.len() {
            self.add_word(&text[chunk_start..], style, link, node, chunk_w, space);
        }
    }

    /// Width of a single space at this size/weight, memoized per line.
    fn space_width(&mut self, size: f32, bold: bool) -> f32 {
        let key = (size.to_bits(), bold);
        if let Some((_, _, w, _)) = self.metrics_cache.iter().find(|(s, b, _, _)| *s == key.0 && *b == key.1) {
            return *w;
        }
        let w = self.measure.measure(" ", size, bold);
        let ascent = self.measure.line_metrics(size, bold).0;
        self.metrics_cache.push((key.0, bold, w, ascent));
        w
    }

    fn char_width(&mut self, ch: char, size: f32, bold: bool) -> f32 {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.measure.measure(s, size, bold)
    }

    fn ascent_of(&mut self, size: f32, bold: bool) -> f32 {
        let key = (size.to_bits(), bold);
        if let Some((_, _, _, a)) = self.metrics_cache.iter().find(|(s, b, _, _)| *s == key.0 && *b == key.1) {
            return *a;
        }
        let w = self.measure.measure(" ", size, bold);
        let ascent = self.measure.line_metrics(size, bold).0;
        self.metrics_cache.push((key.0, bold, w, ascent));
        ascent
    }

    fn flush(&mut self) {
        if self.words.is_empty() && self.atom_height <= 0.0 {
            return;
        }
        let mut line_height = self.atom_height;
        let mut ascent = 0.0f32;
        let sizes: Vec<(f32, bool, f32)> =
            self.words.iter().map(|w| (w.size, w.bold, w.line_height)).collect();
        for (size, bold, lh) in sizes {
            line_height = line_height.max(lh);
            ascent = ascent.max(self.ascent_of(size, bold));
        }
        if line_height <= 0.0 {
            line_height = self.default_size * self.default_line_height;
        }
        let total: f32 = self
            .words
            .iter()
            .map(|w| w.x + w.width)
            .fold(0.0f32, f32::max)
            .max(self.line_width);
        let align_offset = match self.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => ((self.width - total) / 2.0).max(0.0),
            TextAlign::Right => (self.width - total).max(0.0),
        };
        let baseline = self.y + ascent;
        for mark in std::mem::take(&mut self.pending_atoms) {
            self.atom_shifts.push((mark, align_offset));
        }

        let mut link_spans: Vec<(String, f32, f32)> = Vec::new();
        let mut words = std::mem::take(&mut self.words);
        let mut buf = std::mem::take(&mut self.buf);
        // Coalesce adjacent words that share a style and link into one run,
        // so the run list is proportional to style changes, not to words.
        let mut pending: Option<TextRun> = None;
        let mut last_end: Option<f32> = None;
        for w in words.drain(..) {
            let text = &buf[w.start as usize..(w.start + w.len) as usize];
            let run_x = self.x + align_offset + w.x;
            if let Some(href) = &w.link {
                let end = run_x + w.width;
                match link_spans.iter_mut().find(|(h, _, _)| h == href) {
                    Some((_, start, last_end)) => {
                        *start = start.min(run_x);
                        *last_end = last_end.max(end);
                    }
                    // Clone only when a new span starts, not per word.
                    None => link_spans.push((href.clone(), run_x, end)),
                }
            }

            // Merge only words that are actually adjacent on the line: an
            // inline atom between them must not be swallowed by the run.
            let adjacent = last_end
                .map(|end| (w.x - end - w.space_before).abs() < 0.5)
                .unwrap_or(false);
            let mergeable = adjacent
                && pending.as_ref().is_some_and(|p| {
                    p.size == w.size
                        && p.bold == w.bold
                        && p.color == w.color
                        && p.link == w.link
                        && w.space_before > 0.0
                });
            if mergeable {
                let p = pending.as_mut().unwrap();
                self.arena.push(' ');
                self.arena.push_str(text);
                p.len = self.arena.len() as u32 - p.start;
                last_end = Some(w.x + w.width);
            } else {
                if let Some(run) = pending.take() {
                    self.runs.push(run);
                }
                let start = self.arena.len() as u32;
                self.arena.push_str(text);
                last_end = Some(w.x + w.width);
                pending = Some(TextRun {
                    x: run_x,
                    baseline,
                    start,
                    len: self.arena.len() as u32 - start,
                    size: w.size,
                    bold: w.bold,
                    color: w.color,
                    link: w.link,
                    underline: w.underline,
                    node: w.node,
                    paint: w.paint,
                    opacity: w.opacity,
                });
            }
        }
        if let Some(run) = pending {
            self.runs.push(run);
        }

        for (href, start, end) in link_spans {
            self.links.push(LinkBox {
                rect: Rect { x: start, y: self.y, w: end - start, h: line_height },
                href,
            });
        }

        words.clear();
        buf.clear();
        self.words = words;
        self.buf = buf;
        self.y += line_height;
        self.line_width = 0.0;
        self.atom_height = 0.0;
        self.pending_space = false;
    }
}

struct Flow<'a> {
    measure: &'a dyn TextMeasure,
    images: &'a HashMap<String, SharedImage>,
    blocks: Vec<BlockBox>,
    runs: Vec<TextRun>,
    links: Vec<LinkBox>,
    image_boxes: Vec<ImageBox>,
    arena: String,
    /// Reusable buffer for the per-flow text arena.
    scratch: String,
    /// Clip inherited from ancestors with `overflow` other than `visible`.
    clip: Option<Rect>,
    /// Content box of the nearest positioned ancestor (absolute containing block).
    containing: Rect,
    /// The viewport, used as the containing block for `position: fixed`.
    viewport: Rect,
    /// Filled SVG shapes collected while walking the tree.
    svg_shapes: Vec<SvgShape>,
    /// Where each out-of-flow box *would* have gone in the normal flow.
    ///
    /// CSS 2.1 §10.3.7/§10.6.4: an absolutely positioned box with `auto`
    /// offsets is placed at its static position, not at the containing block's
    /// origin. Without this, two icons that are absolute siblings with no
    /// `left`/`top` stack on top of each other.
    static_positions: std::collections::HashMap<usize, (f32, f32)>,
    /// The document being laid out, so attributes (`viewBox`, `d`, `fill`) can
    /// be read without threading them through every box.
    doc: Option<&'a Document>,
    /// Width decided by the flex algorithm for the item being laid out.
    ///
    /// A flex item's used width is not its declared `width` — it may have been
    /// grown or shrunk — so `block()` must not clamp it again.
    flex_width: Option<f32>,
}

impl<'a> Flow<'a> {
    /// Lay out a sequence of flow items; returns the bottom of the flow.
    fn flow(&mut self, items: &[FlowItem], x: f32, y: f32, width: f32, style: &ComputedStyle) -> f32 {
        let measure = self.measure;
        let mut line = LineBuilder::new(
            measure,
            x,
            y,
            width,
            style.text_align,
            style.font_size,
            style.line_height,
            std::mem::take(&mut self.scratch),
        );
        let mut cursor_y = y;
        let mut prev_bottom_margin = 0.0f32;
        let mut emitted = false;

        for item in items {
            match item {
                FlowItem::Inline(InlineItem::Text { text, style, link, node }) => {
                    line.push_text(text, style, link, *node);
                    emitted = true;
                }
                FlowItem::Inline(InlineItem::Space) => {
                    line.pending_space = true;
                }
                FlowItem::Inline(InlineItem::Break) => {
                    line.hard_break();
                    emitted = true;
                }
                FlowItem::InlineBox(child) if is_out_of_flow(child.style.position) => {
                    // Static position of an inline-level absolute box: where the
                    // line currently is.
                    self.static_positions
                        .entry(child.node)
                        .or_insert((x + line.x_offset(), line.y_top()));
                }
                FlowItem::InlineBox(child) => {
                    // Shrink-to-fit width: explicit `width`, else max-content.
                    // The margins belong to the atom too — `block()` subtracts
                    // them from the width it receives, so they are added here.
                    let box_w = self
                        .inline_atom_width(child, width)
                        .clamp(1.0, width.max(1.0));
                    let font_size = child.style.font_size;
                    let bold = child.style.font_weight >= 600;
                    let mut lead = line.atom_space(font_size, bold);
                    if !line.fits(box_w, lead) {
                        line.flush();
                        lead = 0.0;
                    }
                    // Atoms are placed left to right from the line start; the
                    // line's `text-align` is applied once, at flush, to every
                    // atom and word together. Centring an atom here made two
                    // centred buttons overlap and appear out of order.
                    let local = line.x_offset() + lead;
                    let x_pos = x + local;
                    let runs_before = self.runs.len();
                    let images_before = self.image_boxes.len();
                    // Lay the box out first: an `inline-block` reserves its
                    // real height on the line, not a predicted one.
                    let bottom = self.block(child, x_pos, line.y_top(), box_w);
                    let box_h = (bottom - line.y_top()).max(0.0);
                    line.record_atom(AtomMark {
                        block: self.blocks.len().saturating_sub(1),
                        runs: (runs_before, self.runs.len()),
                        images: (images_before, self.image_boxes.len()),
                    });
                    line.place_atom(lead + box_w, box_h);
                    emitted = true;
                }
                FlowItem::Image(img) if img.absolute => {
                    self.place_absolute_image(img);
                }
                FlowItem::Image(img) => {
                    let (w, h) = self.image_size(img);
                    let w = w.min(width.max(1.0));
                    let h = if w < self.image_size(img).0 {
                        // Clamp wide images to the line, keeping the ratio.
                        let ratio = self.image_size(img).1 / self.image_size(img).0.max(1.0);
                        w * ratio
                    } else {
                        h
                    };
                    let size = style.font_size;
                    let bold = style.font_weight >= 600;
                    let mut lead = line.atom_space(size, bold);
                    if !line.fits(w, lead) {
                        line.flush();
                        lead = 0.0;
                    }
                    // Images follow the same rule as atoms: placed from the
                    // line start, aligned with the rest of the line at flush.
                    let x_pos = x + line.x_offset() + lead;
                    if let Some(shared) = self.images.get(&img.src) {
                        self.image_boxes.push(ImageBox {
                            rect: Rect { x: x_pos, y: line.y_top(), w, h },
                            image: shared.clone(),
                        });
                    }
                    line.place_atom(lead + w, h);
                    emitted = true;
                }
                FlowItem::Block(child) if is_out_of_flow(child.style.position) => {
                    // Static position of a block-level absolute box: the flow
                    // cursor, with its top margin collapsed like a normal block.
                    line.flush();
                    let margin_top = child.style.margin.top_at(width);
                    let gap = if emitted {
                        prev_bottom_margin.max(margin_top)
                    } else {
                        margin_top
                    };
                    let y_here = cursor_y.max(line.y) + gap;
                    self.static_positions
                        .entry(child.node)
                        .or_insert((x + child.style.margin.left_at(width), y_here));
                    emitted = true;
                    prev_bottom_margin = child.style.margin.bottom_at(width);
                }
                FlowItem::Block(child) => {
                    line.flush();
                    cursor_y = cursor_y.max(line.y);
                    let margin_top = child.style.margin.top_at(width);
                    let gap = if emitted {
                        prev_bottom_margin.max(margin_top)
                    } else {
                        margin_top
                    };
                    let bottom = self.block(child, x, cursor_y + gap, width);
                    cursor_y = bottom + child.style.margin.bottom_at(width);
                    prev_bottom_margin = child.style.margin.bottom_at(width);
                    emitted = true;
                }
            }
        }
        line.flush();
        // The line's `text-align` shift applies to its atoms too, not only to
        // the text runs (which `flush` already aligned).
        let shifts = std::mem::take(&mut line.atom_shifts);
        for (index, (mark, dx)) in shifts.iter().enumerate() {
            if *dx == 0.0 || mark.block >= self.blocks.len() {
                continue;
            }
            // Each atom owns the blocks up to the next one; shifting with the
            // `shift_subtree` heuristic would also move a following sibling
            // that happens to start where this one ends.
            let end = shifts
                .get(index + 1)
                .map(|(next, _)| next.block)
                .unwrap_or(self.blocks.len())
                .min(self.blocks.len());
            for entry in self.blocks[mark.block..end].iter_mut() {
                entry.rect.x += dx;
            }
            let runs_end = mark.runs.1.min(self.runs.len());
            for run in self.runs[mark.runs.0..runs_end].iter_mut() {
                run.x += dx;
            }
            let images_end = mark.images.1.min(self.image_boxes.len());
            for image in self.image_boxes[mark.images.0..images_end].iter_mut() {
                image.rect.x += dx;
            }
        }
        let line_bottom = line.y;
        let base = self.arena.len() as u32;
        self.arena.push_str(&line.arena);
        line.arena.clear();
        self.scratch = line.arena;
        for mut run in line.runs {
            run.start += base;
            self.runs.push(run);
        }
        self.links.extend(line.links);
        line_bottom.max(cursor_y)
    }

    /// Rendered size for an `<img>`: explicit attributes win, one attribute
    /// keeps the intrinsic aspect ratio, otherwise the intrinsic size is used.
    fn image_size(&self, img: &ImageNode) -> (f32, f32) {
        let intrinsic = self
            .images
            .get(&img.src)
            .map(|i| (i.width as f32, i.height as f32))
            .unwrap_or((150.0, 150.0));
        match (img.width_attr, img.height_attr) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => {
                let h = if intrinsic.0 > 0.0 {
                    w * intrinsic.1 / intrinsic.0
                } else {
                    w
                };
                (w, h)
            }
            (None, Some(h)) => {
                let w = if intrinsic.1 > 0.0 {
                    h * intrinsic.0 / intrinsic.1
                } else {
                    h
                };
                (w, h)
            }
            (None, None) => intrinsic,
        }
    }

    /// Widest line a box can produce without wrapping; used to size flex items.
    /// The content width fixed by an absolute `width`, if there is one.
    ///
    /// A box with a definite width contributes exactly that width to its
    /// container's intrinsic size. Measuring its children instead made a
    /// `24px` icon span report the `viewBox` extent of the `<svg>` inside it
    /// (960), and a `width: 100%` child resolve against the probe width.
    fn definite_content_width(&self, b: &BoxNode) -> Option<f32> {
        let declared = b.style.width.as_ref()?.absolute()?;
        Some(if b.style.border_box {
            (declared - b.style.padding.horizontal_at(declared) - b.style.border.horizontal())
                .max(0.0)
        } else {
            declared
        })
    }

    fn max_content_width(&self, b: &BoxNode) -> f32 {
        if let Some(w) = self.definite_content_width(b) {
            return w;
        }
        // An `<svg>` is a replaced element: its contribution is its `viewBox`
        // extent, not the (empty) content of its `<path>` children.
        if b.tag == "svg" {
            return self.svg_intrinsic(b).0;
        }
        // A form control's own label is its `value` (or placeholder): the
        // browser sizes a button from the text it paints, and without this a
        // "Pesquisa Google" button was measured as its widest word and the text
        // wrapped into a narrow column.
        if matches!(b.tag.as_str(), "input" | "button" | "select") {
            if let Some(doc) = self.doc {
                if let Some(label) = control_text(doc, b.node) {
                    let mut widest = 0.0f32;
                    for word in label.split(' ') {
                        let w = self.measure.measure(
                            word,
                            b.style.font_size,
                            b.style.font_weight >= 600,
                        );
                        widest = widest.max(w);
                    }
                    // Buttons do not wrap their label, so the whole string counts.
                    let whole = self.measure.measure(
                        &label,
                        b.style.font_size,
                        b.style.font_weight >= 600,
                    );
                    return whole.max(widest);
                }
            }
        }
        // A row flex container is as wide as its items side by side, not as
        // wide as the widest one. Without this an auto-width container whose
        // children are `flex: 1 1 0%` measures 0 and the whole row collapses.
        if b.style.display == Display::Flex && b.style.flex_direction == FlexDirection::Row {
            let items: Vec<&BoxNode> = b
                .items
                .iter()
                .filter_map(|item| match item {
                    FlowItem::Block(child) if !is_out_of_flow(child.style.position) => Some(child),
                    _ => None,
                })
                .collect();
            if !items.is_empty() {
                let total: f32 = items
                    .iter()
                    .map(|child| {
                        // A zero basis (`flex: 1 1 0%`) does not mean the item
                        // contributes nothing: its max-content contribution is
                        // still its content size.
                        let own = match child.style.flex_basis {
                            Some(px) if px > 0.0 => px,
                            _ => match &child.style.width {
                                Some(w) => w.resolve(1000.0),
                                None => self.max_content_width(child),
                            },
                        };
                        own + child.style.margin.horizontal_at(1000.0)
                            + child.style.padding.horizontal_at(1000.0)
                            + child.style.border.horizontal()
                    })
                    .sum();
                let gaps = b.style.gap * (items.len() as f32 - 1.0).max(0.0);
                return total + gaps;
            }
        }
        let mut widest = 0.0f32;
        let mut current = 0.0f32;
        for item in &b.items {
            match item {
                FlowItem::Inline(InlineItem::Text { text, style, .. }) => {
                    for word in text.split(' ') {
                        let w = self
                            .measure
                            .measure(word, style.font_size, style.font_weight >= 600);
                        let space = self.measure.measure(" ", style.font_size, style.font_weight >= 600);
                        // The separator only counts between words; the
                        // parenthesised form keeps `+ w` outside the `if`.
                        current += (if current > 0.0 { space } else { 0.0 }) + w;
                    }
                }
                FlowItem::Inline(InlineItem::Space) => {
                    current += self.measure.measure(" ", 16.0, false);
                }
                FlowItem::InlineBox(child) => {
                    // Inline atoms share a line, so their widths add up. Taking
                    // the max made a `<center>` holding two buttons as wide as
                    // one button, and the buttons wrapped into a narrow column.
                    current += self.intrinsic_atom_width(child);
                }
                FlowItem::Inline(InlineItem::Break) => {
                    widest = widest.max(current);
                    current = 0.0;
                }
                FlowItem::Block(child) => {
                    widest = widest.max(current);
                    current = 0.0;
                    widest = widest.max(
                        self.max_content_width(child)
                            + child.style.margin.horizontal_at(1000.0)
                            + child.style.padding.horizontal_at(1000.0)
                            + child.style.border.horizontal(),
                    );
                }
                FlowItem::Image(img) => {
                    widest = widest.max(current);
                    current = 0.0;
                    let (w, _) = self.image_size(img);
                    widest = widest.max(w);
                }
            }
        }
        widest.max(current)
    }

    /// Smallest width the box can take without overflowing its content: the
    /// widest unbreakable word, or the widest child for a container.
    ///
    /// This is the CSS "content-based minimum size" a flex item falls back to
    /// through `min-width: auto`; without it an item can be squeezed to zero
    /// and every word wraps to one character per line.
    fn min_content_width(&self, b: &BoxNode) -> f32 {
        if let Some(w) = self.definite_content_width(b) {
            return w;
        }
        // A replaced element and a form control cannot wrap: their minimum is
        // their whole width. Returning 0 for a button let a flex container
        // squeeze the row that holds them down to a few pixels.
        if b.tag == "svg" {
            if matches!(&b.style.width, Some(naro_css::Dim::Linear { percent, .. }) if *percent > 0.0)
            {
                return 0.0;
            }
            return self.svg_intrinsic(b).0;
        }
        if matches!(b.tag.as_str(), "input" | "button" | "select") {
            if let Some(doc) = self.doc {
                if let Some(label) = control_text(doc, b.node) {
                    return self.measure.measure(
                        &label,
                        b.style.font_size,
                        b.style.font_weight >= 600,
                    );
                }
            }
        }
        let mut widest = 0.0f32;
        for item in &b.items {
            match item {
                FlowItem::Inline(InlineItem::Text { text, style, .. }) => {
                    for word in text.split(' ') {
                        let w = self
                            .measure
                            .measure(word, style.font_size, style.font_weight >= 600);
                        widest = widest.max(w);
                    }
                }
                FlowItem::InlineBox(child) => {
                    // Adjacent atoms stay on one line unless the line breaks,
                    // and a break there is exactly "min-content": the widest
                    // single atom.
                    widest = widest.max(self.intrinsic_atom_width(child));
                }
                FlowItem::Block(child) => {
                    widest = widest.max(
                        self.min_content_width(child)
                            + child.style.margin.horizontal_at(1000.0)
                            + child.style.padding.horizontal_at(1000.0)
                            + child.style.border.horizontal(),
                    );
                }
                FlowItem::Image(img) => {
                    widest = widest.max(self.image_size(img).0);
                }
                FlowItem::Inline(InlineItem::Space) | FlowItem::Inline(InlineItem::Break) => {}
            }
        }
        widest
    }

    /// Border-box width of an inline atom (`input`, `button`, `inline-block`,
    /// `svg`) at `available` width.
    ///
    /// `max-content` and the line builder must agree on this number: when they
    /// disagreed (one honoured the 160px default for a text input, the other the
    /// label width), a container was measured smaller than the atoms it had to
    /// hold and every atom wrapped to its own line.
    /// The width an atom contributes when its container is being measured.
    ///
    /// A replaced element sized in percentages has no definite size against an
    /// indefinite container: CSS says the percentage behaves as `auto` and the
    /// intrinsic size is used. Returning 0 here keeps a `width: 100%` icon from
    /// forcing its wrapper (and the whole row) to the icon's own extent.
    fn intrinsic_atom_width(&self, child: &BoxNode) -> f32 {
        if child.tag == "svg" {
            if matches!(&child.style.width, Some(naro_css::Dim::Linear { percent, .. }) if *percent > 0.0)
            {
                return child.style.padding.horizontal_at(1000.0)
                    + child.style.border.horizontal()
                    + child.style.margin.horizontal_at(1000.0);
            }
        }
        self.inline_atom_width(child, 1000.0)
    }

    fn inline_atom_width(&self, child: &BoxNode, available: f32) -> f32 {
        let outer = child.style.padding.horizontal_at(available)
            + child.style.border.horizontal()
            + child.style.margin.horizontal_at(available);
        let content = if child.tag == "svg" {
            // `width: 100%` resolves against the container this atom sits in,
            // which is what makes an icon fill its 24px wrapper.
            self.svg_size(child, available).0
        } else if let Some(w) = &child.style.width {
            w.resolve(available)
        } else {
            let natural = self.max_content_width(child);
            // A text field has a default width; a submit button is as wide as
            // its label, like any other inline-block.
            let is_text_field = child.tag == "input"
                && self.doc.and_then(|d| d.attr(child.node, "type")).map_or(true, |t| {
                    !matches!(
                        t.to_ascii_lowercase().as_str(),
                        "submit" | "reset" | "button" | "image" | "checkbox" | "radio"
                    )
                })
                || child.tag == "select";
            if is_text_field {
                natural.max(160.0)
            } else {
                natural.max(8.0)
            }
        };
        content + outer
    }

    /// Intrinsic size of an `<svg>`: its `viewBox` extent, or the CSS default
    /// 300×150 when it has none.
    ///
    /// This is the contribution a percentage-sized SVG makes to a container
    /// that is being measured (`max-content`): `width: 100%` against an
    /// indefinite width behaves as `auto`, so the intrinsic size is used. It is
    /// what lets `align-items: center` shrink-wrap a logo instead of giving it
    /// the whole row.
    fn svg_intrinsic(&self, b: &BoxNode) -> (f32, f32) {
        let doc = self.doc;
        doc.and_then(|d| d.attr(b.node, "viewbox").or_else(|| d.attr(b.node, "viewBox")))
            .and_then(|v| {
                let parts: Vec<f32> = v
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter_map(|p| p.parse::<f32>().ok())
                    .collect();
                if parts.len() == 4 && parts[2] > 0.0 && parts[3] > 0.0 {
                    Some((parts[2], parts[3]))
                } else {
                    None
                }
            })
            .unwrap_or((300.0, 150.0))
    }

    /// Size an `<svg>` from its attributes, `viewBox` and CSS.
    ///
    /// No declared size means `width: 100%` of the space offered and a height
    /// derived from the `viewBox` ratio, as CSS specifies for a replaced
    /// element with an intrinsic ratio.
    fn svg_size(&self, b: &BoxNode, available: f32) -> (f32, f32) {
        // An `<svg>` is a replaced element sized like an image: `width: 100%`
        // resolves against the containing block's content box, and with no
        // `width` at all SVG2's initial `auto` behaves as `100%` too.
        let doc = self.doc;
        let attr = |name: &str| -> Option<f32> {
            doc.and_then(|d| d.attr(b.node, name))
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
        };
        let view_box = doc
            .and_then(|d| d.attr(b.node, "viewbox").or_else(|| d.attr(b.node, "viewBox")))
            .and_then(|v| {
                let parts: Vec<f32> = v
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter_map(|p| p.parse::<f32>().ok())
                    .collect();
                if parts.len() == 4 {
                    Some((parts[0], parts[1], parts[2], parts[3]))
                } else {
                    None
                }
            });
        let (vb_w, vb_h) = view_box.map(|(_, _, w, h)| (w, h)).unwrap_or((300.0, 150.0));
        let s = &b.style;
        // SVG2 gives the root `<svg>` an initial `width`/`height` of `auto`,
        // which computes to `100%`: an icon fills the wrapper it sits in.
        // Chrome measures such an icon at exactly the wrapper's box, so the
        // wrapper has to be a box with a definite width (`display: inline`
        // with a `width` is not one — CSS ignores the width there).
        let width = match (&s.width, attr("width")) {
            (Some(w), _) => w.resolve(available).max(1.0),
            (None, Some(w)) => w,
            (None, None) => available.max(1.0),
        };
        let height = match (&s.height, attr("height")) {
            (Some(h), _) => h.resolve(available),
            (None, Some(h)) => h,
            (None, None) => {
                if vb_w > 0.0 {
                    // The height follows the `viewBox` ratio of that width.
                    width * vb_h / vb_w
                } else {
                    width * 0.5
                }
            }
        };
        (width, height)
    }

    /// Lay out an `<svg>` element: a replaced box sized from its attributes
    /// (or `viewBox`), with its filled paths recorded for the painter.
    fn svg(&mut self, b: &BoxNode, x: f32, y: f32, available_width: f32) -> f32 {
        let s = &b.style;
        let doc = self.doc;
        let view_box = doc
            .and_then(|d| d.attr(b.node, "viewbox").or_else(|| d.attr(b.node, "viewBox")))
            .and_then(|v| {
                let parts: Vec<f32> = v
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter_map(|p| p.parse::<f32>().ok())
                    .collect();
                if parts.len() == 4 {
                    Some((parts[0], parts[1], parts[2], parts[3]))
                } else {
                    None
                }
            });
        let (width, height) = self.svg_size(b, available_width);
        let border_x = x + s.margin.left_at(available_width);
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_y = y + s.border.top + s.padding.top_at(available_width);
        let rect = Rect {
            x: border_x,
            y,
            w: width + s.padding.horizontal_at(available_width) + s.border.horizontal(),
            h: height + s.padding.vertical_at(available_width) + s.border.vertical(),
        };
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: None,
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: self.clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });
        if let Some(doc) = doc {
            let (vb_x, vb_y, vb_w, vb_h) = view_box.unwrap_or((0.0, 0.0, width, height));
            let scale = (
                if vb_w > 0.0 { width / vb_w } else { 1.0 },
                if vb_h > 0.0 { height / vb_h } else { 1.0 },
            );
            // Every `<path>` under the `<svg>` is filled with its own `fill`.
            let mut stack = vec![b.node];
            while let Some(node) = stack.pop() {
                for &child in doc.children(node) {
                    if doc.tag(child) == Some("path") {
                        let d = doc.attr(child, "d").unwrap_or("");
                        let subpaths = svg::parse_path(d);
                        if subpaths.is_empty() {
                            continue;
                        }
                        // `fill` inherits from the `<svg>`; `currentColor`
                        // means the element's own computed `color`, and the
                        // default is black, as CSS specifies.
                        // `color` is inherited, so the `<svg>`'s own computed
                        // colour is what `currentColor` resolves to.
                        let resolve_fill = |value: Option<&str>| -> Option<Option<Color>> {
                            match value {
                                Some(v) if v.trim().eq_ignore_ascii_case("currentcolor") => {
                                    Some(Some(s.color))
                                }
                                Some(v) if v.trim().eq_ignore_ascii_case("none") => Some(None),
                                Some(v) => Some(Color::parse(v)),
                                None => None,
                            }
                        };
                        let fill = match resolve_fill(doc.attr(child, "fill"))
                            .or_else(|| resolve_fill(doc.attr(b.node, "fill")))
                        {
                            Some(Some(color)) => color,
                            // `fill: none` draws nothing.
                            Some(None) => continue,
                            None => s.color,
                        };
                        self.svg_shapes.push(SvgShape {
                            node: child,
                            subpaths,
                            origin: (content_x - vb_x * scale.0, content_y - vb_y * scale.1),
                            scale,
                            color: fill,
                            paint: s.visible && s.opacity > 0.0,
                            opacity: s.opacity,
                        });
                    } else {
                        stack.push(child);
                    }
                }
            }
        }
        y + rect.h
    }

    /// Lay out a block whose border box starts at `y`; returns its bottom edge.
    /// Content width of a container box: the space offered, narrowed by the
    /// box's own `width`/`max-width`/`min-width`. `block` did this inline; the
    /// flex and grid paths used the offered width verbatim, so a `width: 36px`
    /// flex button filled its whole parent and dragged its icon with it.
    fn box_content_width(s: &naro_css::ComputedStyle, available_width: f32) -> f32 {
        let mut w = (available_width
            - s.margin.horizontal_at(available_width)
            - s.padding.horizontal_at(available_width)
            - s.border.horizontal())
        .max(0.0);
        if let Some(declared) = &s.width {
            let declared = declared.resolve(available_width);
            let inner = if s.border_box {
                (declared - s.padding.horizontal_at(available_width) - s.border.horizontal())
                    .max(0.0)
            } else {
                declared
            };
            w = w.min(inner);
        }
        if let Some(cap) = &s.max_width {
            let cap = cap.resolve(available_width);
            let cap = if s.border_box {
                (cap - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                cap
            };
            w = w.min(cap);
        }
        if let Some(floor) = &s.min_width {
            let floor = floor.resolve(available_width);
            let floor = if s.border_box {
                (floor - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                floor
            };
            w = w.max(floor.min(available_width));
        }
        w
    }

    fn block(&mut self, b: &BoxNode, x: f32, y: f32, available_width: f32) -> f32 {
        // Consume the flex width first: the early returns below (svg, flex,
        // grid, table) never reach the normal path, and a leftover value would
        // be applied to whatever box is laid out next.
        let forced = self.flex_width.take();
        if b.tag == "svg" {
            return self.svg(b, x, y, available_width);
        }
        if b.style.display == Display::Flex {
            return self.flex(b, x, y, available_width);
        }
        if b.style.display == Display::Table {
            // `display: table` is also used as a layout trick by real pages.
            // Only take the table path when the box actually has rows.
            if !collect_rows(&b.items).is_empty() {
                return self.table(b, x, y, available_width);
            }
        }
        if b.style.display == Display::TableRow {
            // A row outside a table has no shared grid: its own cells define
            // the columns.
            let columns = self.table_columns(&[b], available_width, false, true);
            let mut occupied = vec![0usize; columns.len()];
            let mut spans = Vec::new();
            return self.table_row(
                b,
                x,
                y,
                available_width,
                &columns,
                &mut occupied,
                &mut spans,
                0,
            );
        }
        if b.style.display == Display::TableCell {
            return self.table_cell(b, x, y, available_width);
        }
        if b.style.display == Display::Grid {
            return self.grid(b, x, y, available_width);
        }
        let s = &b.style;
        let mut content_w = (available_width
            - s.margin.horizontal_at(available_width)
            - s.padding.horizontal_at(available_width)
            - s.border.horizontal())
        .max(0.0);
        // A flex item's width was decided by the flex algorithm; `width` and
        // the min/max clamps already fed into it, so they are not re-applied.
        if let Some(forced) = forced {
            content_w = (forced
                - s.padding.horizontal_at(available_width)
                - s.border.horizontal())
            .max(0.0);
        }
        // `width` fixes the content box; `max-width` caps it. Auto margins
        // then center the resulting box inside the available space.
        if forced.is_none() {
        if let Some(w) = &s.width {
            let declared = w.resolve(available_width);
            let content = if s.border_box {
                // border-box: the declared width includes padding and border.
                (declared - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                declared
            };
            content_w = content_w.min(content);
        }
        if let Some(m) = &s.max_width {
            let cap = m.resolve(available_width);
            let cap = if s.border_box {
                (cap - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                cap
            };
            content_w = content_w.min(cap);
        }
        if let Some(min) = &s.min_width {
            let floor = min.resolve(available_width);
            let floor = if s.border_box {
                (floor - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                floor
            };
            content_w = content_w.max(floor.min(available_width));
        }
        }
        let outer = content_w + s.padding.horizontal_at(available_width) + s.border.horizontal();
        let mut offset = 0.0;
        if forced.is_none() && (s.margin_auto_left || s.margin_auto_right) {
            let free = (available_width - outer - s.margin.horizontal_at(available_width)).max(0.0);
            if s.margin_auto_left && s.margin_auto_right {
                offset = free / 2.0;
            } else if s.margin_auto_left {
                offset = free;
            }
        }
        let border_x = x + s.margin.left_at(available_width) + offset;
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_top = y + s.border.top + s.padding.top_at(available_width);

        let saved_clip = self.clip;
        let saved_containing = self.containing;
        // `overflow` clips descendants to this box's padding box.
        // The height is not known before the children are laid out, so the
        // clip rect is recorded with a sentinel and patched afterwards.
        let own_rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: -1.0,
        };
        let clip_start = self.blocks.len();
        if s.overflow != Overflow::Visible {
            self.clip = Some(match saved_clip {
                Some(outer) => intersect(outer, own_rect),
                None => own_rect,
            });
        }
        if s.position != Position::Static {
            self.containing = Rect {
                x: content_x,
                y: content_top,
                w: content_w,
                h: 0.0,
            };
        }

        let child_containing = self.containing;
        let content_bottom = self.flow(&b.items, content_x, content_top, content_w, s);
        self.clip = saved_clip;
        self.containing = saved_containing;

        let mut content_height = content_bottom - content_top;
        // An empty text control still occupies one line box, as in a browser.
        if matches!(b.tag.as_str(), "input" | "textarea") {
            content_height = content_height.max(s.font_size * s.line_height);
        }
        let mut height = s.border.vertical() + s.padding.vertical_at(available_width) + content_height;
        if let Some(h) = definite_height(s) {
            // A declared `height` is definite: the box keeps it and the
            // content overflows (clipped only when `overflow` says so). This
            // is what makes `height: 20px` a 20px box, as in a browser.
            height = h + s.padding.vertical_at(available_width) + s.border.vertical();
        }
        height = self.clamp_height(s, height, available_width);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: height,
        };
        // Patch descendant clips that referenced this box's unknown height.
        if s.overflow != Overflow::Visible {
            let full = Rect { h: height, ..rect };
            for block in self.blocks[clip_start..].iter_mut() {
                if let Some(clip) = block.clip {
                    if clip.h < 0.0 {
                        block.clip = Some(match saved_clip {
                            Some(outer) => intersect(outer, full),
                            None => full,
                        });
                    }
                }
            }
        }

        let index = self.blocks.len();
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: saved_clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });

        // `position: relative` shifts the box and everything inside it.
        if s.position == Position::Relative {
            let dx = s.left.as_ref().map(|d| d.resolve(available_width)).unwrap_or_else(|| {
                s.right.as_ref().map(|d| -d.resolve(available_width)).unwrap_or(0.0)
            });
            // `top`/`bottom` percentages need the containing block's height,
            // which is unknown here; they resolve to zero instead of borrowing
            // the width.
            let dy = s.top.as_ref().and_then(|d| d.absolute()).unwrap_or_else(|| {
                s.bottom.as_ref().and_then(|d| d.absolute()).map(|v| -v).unwrap_or(0.0)
            });
            if dx != 0.0 || dy != 0.0 {
                shift_subtree(&mut self.blocks[index..], dx, dy);
            }
        }

        // Absolutely positioned descendants are laid out against the nearest
        // positioned ancestor and removed from the normal flow.
        let mut absolutes: Vec<&BoxNode> = Vec::new();
        collect_absolutes(&b.items, &mut absolutes);
        if !absolutes.is_empty() {
            // Absolute children resolve against this box when it is positioned,
            // otherwise against the containing block it inherited.
            self.containing = child_containing;
            for child in absolutes {
                self.place_absolute(child);
            }
            self.containing = saved_containing;
        }

        y + height
    }

    /// Position an absolutely positioned image against the containing block.
    fn place_absolute_image(&mut self, img: &ImageNode) {
        let (w, h) = self.image_size(img);
        let area = self.containing;
        let x = if let Some(left) = img.left {
            area.x + left
        } else if let Some(right) = img.right {
            area.x + area.w - w - right
        } else {
            area.x
        };
        let y = if let Some(top) = img.top {
            area.y + top
        } else if let Some(bottom) = img.bottom {
            area.y + area.h - h - bottom
        } else {
            area.y
        };
        if let Some(shared) = self.images.get(&img.src) {
            self.image_boxes.push(ImageBox {
                rect: Rect { x, y, w, h },
                image: shared.clone(),
            });
        }
    }

    /// Position an absolutely positioned box against `Flow::containing`.
    fn place_absolute(&mut self, child: &BoxNode) {
        let s = &child.style;
        // `fixed` ignores positioned ancestors and uses the viewport.
        let area = if s.position == Position::Fixed { self.viewport } else { self.containing };
        // CSS 2.1 §10.3.7: with `left`/`right` both set the box fills the gap;
        // with neither (or only one) its width is shrink-to-fit, never the
        // whole containing block. A fixed icon overlay would otherwise be as
        // wide as the viewport and every `width:100%` inside it would follow.
        let width = match (&s.width, &s.left, &s.right) {
            (Some(w), _, _) => w.resolve(area.w),
            (None, Some(left), Some(right)) => {
                (area.w - left.resolve(area.w) - right.resolve(area.w)).max(0.0)
            }
            _ => {
                let preferred = self.max_content_width(child);
                let minimum = self.min_content_width(child);
                let outer = s.padding.horizontal_at(area.w) + s.border.horizontal();
                preferred.max(minimum).min(area.w) + outer
            }
        };
        let outer_w = width + s.padding.horizontal_at(area.w) + s.border.horizontal();
        // `auto` offsets fall back to the box's static position, which is where
        // it would have been laid out in the normal flow.
        let (static_x, static_y) = self
            .static_positions
            .get(&child.node)
            .copied()
            .unwrap_or((area.x, area.y));
        let x = if let Some(left) = &s.left {
            area.x + left.resolve(area.w)
        } else if let Some(right) = &s.right {
            area.x + area.w - outer_w - right.resolve(area.w)
        } else {
            static_x
        };
        let y = if let Some(top) = &s.top {
            area.y + top.resolve(area.h.max(0.0))
        } else if let Some(bottom) = &s.bottom {
            area.y + area.h - bottom.resolve(area.h.max(0.0))
        } else {
            static_y
        };
        self.block(child, x, y, width);
    }

    /// Flexbox: row/column, wrap, grow/shrink/basis, justify-content,
    /// align-items/self, align-content, gap and auto margins.
    ///
    /// Items are sized from their flex basis (or their own `width`, or
    /// max-content), then grown or shrunk against the container's content box,
    /// with `min-width`/`max-width` clamping every result.
    fn flex(&mut self, b: &BoxNode, x: f32, y: f32, available_width: f32) -> f32 {
        let s = &b.style;
        let content_w = Self::box_content_width(s, available_width);
        let border_x = x + s.margin.left_at(available_width);
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_top = y + s.border.top + s.padding.top_at(available_width);

        let children: Vec<&BoxNode> = b
            .items
            .iter()
            .filter_map(|item| match item {
                FlowItem::Block(child) if !is_out_of_flow(child.style.position) => Some(child),
                _ => None,
            })
            .collect();

        let content_bottom = if children.is_empty() {
            content_top
        } else if s.flex_direction == FlexDirection::Column {
            // The cross axis is horizontal here, so `align-items` decides each
            // item's width and horizontal offset — a column flex container
            // with `align-items: center` is how pages centre a logo.
            let mut cursor = content_top;
            for (i, child) in children.iter().enumerate() {
                if i > 0 {
                    cursor += s.gap;
                }
                let align = child.style.align_self.unwrap_or(s.align_items);
                let outer = child.style.margin.horizontal_at(content_w)
                    + child.style.padding.horizontal_at(content_w)
                    + child.style.border.horizontal();
                let natural = match child.style.flex_basis {
                    Some(px) => px,
                    None => match &child.style.width {
                        Some(w) => w.resolve(content_w),
                        None => self.max_content_width(child),
                    },
                } + outer;
                // A percentage width has no definite basis in a column flex
                // container's cross axis; fall back to the content size so
                // `align-items: center` can shrink-wrap the item.
                let natural = if matches!(child.style.width, Some(naro_css::Dim::Linear { percent, .. }) if percent > 0.0)
                {
                    self.max_content_width(child) + outer
                } else {
                    natural
                };
                let (width, offset) = match align {
                    AlignItems::Stretch => (content_w, 0.0),
                    AlignItems::Center => {
                        let w = natural.min(content_w);
                        (w, (content_w - w) / 2.0)
                    }
                    AlignItems::End => {
                        let w = natural.min(content_w);
                        (w, content_w - w)
                    }
                    AlignItems::Start => (natural.min(content_w), 0.0),
                };
                let top = cursor + child.style.margin.top_at(content_w);
                self.flex_width = Some(width);
                let bottom = self.block(child, content_x + offset, top, width);
                cursor = bottom + child.style.margin.bottom_at(content_w);
            }
            cursor
        } else {
            self.flex_row(&children, content_x, content_top, content_w, s)
        };

        // An empty text control still occupies one line box, as in a browser.
        // `block()` has the same rule; a control that is `display: flex` (which
        // is common) must not collapse to just its padding.
        let mut content_height = content_bottom - content_top;
        if matches!(b.tag.as_str(), "input" | "textarea") {
            content_height = content_height.max(s.font_size * s.line_height);
        }
        let mut height =
            s.border.vertical() + s.padding.vertical_at(available_width) + content_height;
        height = self.clamp_height(s, height, available_width);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: height,
        };
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: self.clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });
        y + height
    }

    /// Apply `min-height`/`max-height` to a resolved border-box height.
    fn clamp_height(&self, style: &ComputedStyle, height: f32, base: f32) -> f32 {
        let mut out = height;
        if let Some(max) = style.max_height.as_ref().and_then(|d| d.absolute()) {
            out = out.min(max.max(0.0));
        }
        if let Some(min) = style.min_height.as_ref().and_then(|d| d.absolute()) {
            out = out.max(min);
        }
        let _ = base;
        out
    }

    /// Lay out one row-direction flex container: resolve item widths, wrap into
    /// lines, place each line and distribute the container's leftover height.
    fn flex_row(
        &mut self,
        children: &[&BoxNode],
        content_x: f32,
        content_top: f32,
        content_w: f32,
        s: &ComputedStyle,
    ) -> f32 {
        let gap = s.gap;
        let n = children.len() as f32;

        // Base size: flex-basis, else the item's own width, else max-content.
        let mut widths: Vec<f32> = children
            .iter()
            .map(|c| {
                let outer_margins = c.style.margin.horizontal_at(content_w)
                    + c.style.padding.horizontal_at(content_w)
                    + c.style.border.horizontal();
                let base = match c.style.flex_basis {
                    Some(px) => px,
                    None => match &c.style.width {
                        Some(w) => w.resolve(content_w),
                        None => self.max_content_width(c),
                    },
                };
                base + outer_margins
            })
            .collect();

        let total: f32 = widths.iter().sum::<f32>() + gap * (n - 1.0).max(0.0);
        let wrap = s.flex_wrap == FlexWrap::Wrap;

        // Split into lines before growing: growth never wraps.
        let mut lines: Vec<(usize, usize)> = Vec::new();
        if wrap {
            let mut line_start = 0usize;
            let mut used = 0.0f32;
            for (i, w) in widths.iter().enumerate() {
                let extra = if i == line_start { *w } else { gap + *w };
                if i > line_start && used + extra > content_w {
                    lines.push((line_start, i));
                    line_start = i;
                    used = *w;
                } else {
                    used += extra;
                }
            }
            lines.push((line_start, children.len()));
        } else {
            lines.push((0, children.len()));
        }

        // Grow or shrink each line against the content box.
        for (line_index, &(from, to)) in lines.iter().enumerate() {
            let count = (to - from) as f32;
            let used: f32 = widths[from..to].iter().sum::<f32>() + gap * (count - 1.0).max(0.0);
            let free = content_w - used;
            if free > 0.0 {
                let grow_total: f32 = children[from..to].iter().map(|c| c.style.flex_grow).sum();
                if grow_total > 0.0 {
                    for (offset, child) in children[from..to].iter().enumerate() {
                        if child.style.flex_grow > 0.0 {
                            widths[from + offset] += free * child.style.flex_grow / grow_total;
                        }
                    }
                }
            } else if free < 0.0 {
                // CSS shrinks by `flex-shrink × base size`, not by base alone.
                let weights: Vec<f32> = children[from..to]
                    .iter()
                    .enumerate()
                    .map(|(offset, c)| c.style.flex_shrink * widths[from + offset])
                    .collect();
                let weight_total: f32 = weights.iter().sum();
                if weight_total > 0.0 {
                    let deficit = -free;
                    for (offset, weight) in weights.iter().enumerate() {
                        widths[from + offset] -= deficit * weight / weight_total;
                    }
                }
            }
            // `min-width`/`max-width` bound every result, including a basis
            // that is larger than the space available.
            for (offset, child) in children[from..to].iter().enumerate() {
                let index = from + offset;
                let outer = child.style.margin.horizontal_at(content_w)
                    + child.style.padding.horizontal_at(content_w)
                    + child.style.border.horizontal();
                let mut w = widths[index].max(0.0);
                if let Some(max) = child.style.max_width.as_ref().and_then(|d| d.absolute()) {
                    w = w.min(max + outer);
                }
                // `min-width: auto` on a flex item is the content-based
                // minimum, not zero: an item never shrinks below its longest
                // word (or its widest child).
                let floor = match child.style.min_width.as_ref().and_then(|d| d.absolute()) {
                    Some(min) => min,
                    None => self.min_content_width(child),
                };
                w = w.max((floor + outer).min(content_w));
                widths[index] = w;
            }
            let _ = line_index;
        }

        // Place every line, remembering where each one landed so the vertical
        // distribution can be applied afterwards.
        struct LineInfo {
            first_block: usize,
            top: f32,
            height: f32,
        }
        let mut infos: Vec<LineInfo> = Vec::with_capacity(lines.len());
        let mut cursor_y = content_top;
        for &(from, to) in lines.iter() {
            let first_block = self.blocks.len();
            let tallest = self.flex_line(
                &children[from..to],
                &widths[from..to],
                content_x,
                cursor_y,
                content_w,
                gap,
                s,
            );
            infos.push(LineInfo { first_block, top: cursor_y, height: tallest });
            cursor_y += tallest + gap;
        }
        let content_bottom = cursor_y - gap;

        // `align-content` only has free space when the container's height is
        // definite; otherwise the lines define it.
        let content_h = match definite_height(s) {
            Some(h) => {
                (h - s.padding.vertical_at(content_w) - s.border.vertical()).max(0.0)
            }
            None => 0.0,
        };
        if infos.len() > 1 && content_h > content_bottom - content_top {
            let leftover = content_h - (content_bottom - content_top);
            let lines_n = infos.len() as f32;
            let (start, between) = match s.align_content {
                AlignContent::Start | AlignContent::Stretch => (0.0, 0.0),
                AlignContent::Center => (leftover / 2.0, 0.0),
                AlignContent::End => (leftover, 0.0),
                AlignContent::SpaceBetween => {
                    (0.0, leftover / (lines_n - 1.0).max(1.0))
                }
                AlignContent::SpaceAround => {
                    let extra = leftover / lines_n;
                    (extra / 2.0, extra)
                }
            };
            let mut shift = start;
            for (index, info) in infos.iter().enumerate() {
                if shift > 0.0 {
                    let from = info.first_block;
                    let to = infos
                        .get(index + 1)
                        .map(|next| next.first_block)
                        .unwrap_or(self.blocks.len());
                    shift_subtree(&mut self.blocks[from..to], 0.0, shift);
                }
                shift += between;
            }
        }
        content_bottom.max(content_top)
    }

    /// Minimal table: rows are laid out top to bottom; each row's cells share
    /// the row and are sized by `max-content`, then shrunk proportionally when
    /// they overflow the table width. No rowspan/colspan, no `table-layout`.
    fn table(&mut self, b: &BoxNode, x: f32, y: f32, available_width: f32) -> f32 {
        let s = &b.style;
        let mut content_w = (available_width
            - s.margin.horizontal_at(available_width)
            - s.padding.horizontal_at(available_width)
            - s.border.horizontal())
        .max(0.0);
        let mut declared_width = false;
        // `width` on the table caps the space its columns may use.
        if let Some(w) = &s.width {
            let declared = w.resolve(available_width);
            declared_width = true;
            content_w = content_w.min(if s.border_box {
                (declared - s.padding.horizontal_at(available_width) - s.border.horizontal()).max(0.0)
            } else {
                declared
            });
        }
        let border_x = x + s.margin.left_at(available_width);
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_top = y + s.border.top + s.padding.top_at(available_width);

        let rows: Vec<&BoxNode> = collect_rows(&b.items);
        // One pass decides every column, so cells line up between rows.
        let columns = self.table_columns(&rows, content_w, s.table_fixed, declared_width);

        let saved_containing = self.containing;
        let mut cursor_y = content_top;
        let mut row_bottoms: Vec<f32> = Vec::with_capacity(rows.len());
        // (block index, first column, column span, row span) for cells that
        // claim height from the rows below them.
        let mut spans: Vec<(usize, usize, usize, usize)> = Vec::new();
        // `occupied[c]` is the first row index at which column `c` is free.
        let mut occupied: Vec<usize> = vec![0; columns.len()];
        for (row_index, row) in rows.iter().enumerate() {
            let bottom = self.table_row(
                row,
                content_x,
                cursor_y,
                content_w,
                &columns,
                &mut occupied,
                &mut spans,
                row_index,
            );
            row_bottoms.push(bottom);
            cursor_y = bottom;
        }
        self.containing = saved_containing;

        // A `rowspan` cell reaches down to the bottom of the last row it
        // covers, so its background and borders span the whole block.
        for (block_index, row_start, _, rowspan) in spans {
            if rowspan < 2 {
                continue;
            }
            let last = (row_start + rowspan - 1).min(row_bottoms.len().saturating_sub(1));
            if let (Some(bottom), Some(block)) = (row_bottoms.get(last), self.blocks.get_mut(block_index)) {
                block.rect.h = block.rect.h.max(bottom - block.rect.y);
            }
        }
        let content_bottom = if rows.is_empty() { content_top } else { cursor_y };

        let mut height = s.border.vertical() + s.padding.vertical_at(available_width) + (content_bottom - content_top);
        height = self.clamp_height(s, height, available_width);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: height,
        };
        let index = self.blocks.len();
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: self.clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });
        if s.position == Position::Relative {
            let dx = s.left.as_ref().map(|d| d.resolve(available_width)).unwrap_or(0.0);
            let dy = s.top.as_ref().and_then(|d| d.absolute()).unwrap_or(0.0);
            if dx != 0.0 || dy != 0.0 {
                shift_subtree(&mut self.blocks[index..], dx, dy);
            }
        }
        y + height
    }

    /// Column widths for a table, shared by every row.
    ///
    /// `table-layout: fixed` takes the widths from the first row (or splits the
    /// table evenly), as CSS specifies; `auto` uses each column's max-content
    /// and then fits the table.
    fn table_columns(
        &self,
        rows: &[&BoxNode],
        content_w: f32,
        fixed: bool,
        declared_width: bool,
    ) -> Vec<f32> {
        let column_count = rows
            .iter()
            .map(|row| row_cells(row).iter().map(|(_, span)| *span).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);

        if fixed {
            // Only the first row defines the grid. Columns with a declared
            // width keep it; the rest share what is left, equally.
            let mut widths = vec![0.0f32; column_count];
            let mut declared = vec![false; column_count];
            let first = rows.first().map(|row| row_cells(row)).unwrap_or_default();
            let mut column = 0usize;
            for (cell, span) in first {
                let span = span.max(1);
                if let Some(w) = &cell.style.width {
                    let share = w.resolve(content_w) / span as f32;
                    for offset in 0..span {
                        if column + offset < widths.len() {
                            widths[column + offset] = share;
                            declared[column + offset] = true;
                        }
                    }
                }
                column += span;
            }
            let used: f32 = widths.iter().sum();
            let free_count = declared.iter().filter(|d| !**d).count();
            let share = if free_count > 0 {
                (content_w - used).max(0.0) / free_count as f32
            } else {
                0.0
            };
            for (index, w) in widths.iter_mut().enumerate() {
                if !declared[index] {
                    *w = share;
                }
            }
            return widths;
        }

        let mut widths = vec![0.0f32; column_count];
        for row in rows {
            let mut column = 0usize;
            for (cell, span) in row_cells(row) {
                let span = span.max(1);
                let intrinsic = match &cell.style.width {
                    Some(w) => w.resolve(content_w),
                    None => self.max_content_width(cell),
                } + cell.style.padding.horizontal_at(content_w)
                    + cell.style.border.horizontal();
                let share = (intrinsic / span as f32).max(8.0);
                for offset in 0..span {
                    if column + offset < widths.len() {
                        widths[column + offset] = widths[column + offset].max(share);
                    }
                }
                column += span;
            }
        }
        let total: f32 = widths.iter().sum();
        if total > content_w && total > 0.0 {
            let scale = content_w / total;
            for w in widths.iter_mut() {
                *w *= scale;
            }
        } else if declared_width && total < content_w && !widths.is_empty() {
            // A table with an explicit width spreads the extra space over the
            // columns instead of leaving a gap on the right.
            let extra = (content_w - total) / widths.len() as f32;
            for w in widths.iter_mut() {
                *w += extra;
            }
        }
        widths
    }

    /// A table row: its cells are placed left to right on one line, using the
    /// column widths the table computed.
    #[allow(clippy::too_many_arguments)]
    fn table_row(
        &mut self,
        b: &BoxNode,
        x: f32,
        y: f32,
        available_width: f32,
        columns: &[f32],
        occupied: &mut [usize],
        spans: &mut Vec<(usize, usize, usize, usize)>,
        row_index: usize,
    ) -> f32 {
        let s = &b.style;
        let content_w = (available_width
            - s.margin.horizontal_at(available_width)
            - s.padding.horizontal_at(available_width)
            - s.border.horizontal())
        .max(0.0);
        let border_x = x + s.margin.left_at(available_width);
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_top = y + s.border.top + s.padding.top_at(available_width);

        let cells: Vec<&BoxNode> = b
            .items
            .iter()
            .filter_map(|item| match item {
                FlowItem::Block(child) => Some(child),
                _ => None,
            })
            .collect();

        let mut cursor_x = content_x;
        let mut column = 0usize;
        let mut row_h = 0.0f32;
        for cell in cells {
            // Skip columns still held by a `rowspan` from an earlier row.
            while column < occupied.len() && occupied[column] > row_index {
                cursor_x += columns.get(column).copied().unwrap_or(0.0);
                column += 1;
            }
            let colspan = cell.colspan;
            let rowspan = cell.rowspan;
            let mut width = 0.0f32;
            for offset in 0..colspan {
                width += columns.get(column + offset).copied().unwrap_or(0.0);
            }
            if width <= 0.0 {
                width = content_w;
            }
            let own = self.blocks.len();
            let bottom = self.table_cell(cell, cursor_x, content_top, width);
            let h = bottom - content_top;
            row_h = row_h.max(h);
            if rowspan > 1 {
                        spans.push((own, row_index, colspan, rowspan));
                for offset in 0..colspan {
                    if column + offset < occupied.len() {
                        occupied[column + offset] = row_index + rowspan;
                    }
                }
            }
            cursor_x += width;
            column += colspan;
        }

        let mut height = s.border.vertical() + s.padding.vertical_at(available_width) + row_h;
        height = self.clamp_height(s, height, available_width);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: height,
        };
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: self.clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });
        y + height
    }

    /// A table cell is a block container whose width is decided by the row.
    fn table_cell(&mut self, b: &BoxNode, x: f32, y: f32, width: f32) -> f32 {
        let s = &b.style;
        let content_w = (width
            - s.margin.horizontal_at(width)
            - s.padding.horizontal_at(width)
            - s.border.horizontal())
        .max(0.0);
        let border_x = x + s.margin.left_at(width);
        let content_x = border_x + s.border.left + s.padding.left_at(width);
        let content_top = y + s.border.top + s.padding.top_at(width);

        let saved_clip = self.clip;
        let saved_containing = self.containing;
        let child_containing = self.containing;
        let own_rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(width) + content_w,
            h: -1.0,
        };
        let clip_start = self.blocks.len();
        if s.overflow != Overflow::Visible {
            self.clip = Some(match saved_clip {
                Some(outer) => intersect(outer, own_rect),
                None => own_rect,
            });
        }
        if s.position != Position::Static {
            self.containing = Rect { x: content_x, y: content_top, w: content_w, h: 0.0 };
        }

        let content_bottom = self.flow(&b.items, content_x, content_top, content_w, s);
        self.clip = saved_clip;
        self.containing = saved_containing;

        let height = s.border.vertical() + s.padding.vertical_at(width) + (content_bottom - content_top);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(width) + content_w,
            h: height,
        };
        if s.overflow != Overflow::Visible {
            let full = Rect { h: height, ..rect };
            for block in self.blocks[clip_start..].iter_mut() {
                if let Some(clip) = block.clip {
                    if clip.h < 0.0 {
                        block.clip = Some(match saved_clip {
                            Some(outer) => intersect(outer, full),
                            None => full,
                        });
                    }
                }
            }
        }
        let index = self.blocks.len();
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: saved_clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });

        let mut absolutes: Vec<&BoxNode> = Vec::new();
        collect_absolutes(&b.items, &mut absolutes);
        if !absolutes.is_empty() {
            self.containing = child_containing;
            for child in absolutes {
                self.place_absolute(child);
            }
            self.containing = saved_containing;
        }
        if s.position == Position::Relative {
            let dx = s.left.as_ref().map(|d| d.resolve(width)).unwrap_or(0.0);
            let dy = s.top.as_ref().and_then(|d| d.absolute()).unwrap_or(0.0);
            if dx != 0.0 || dy != 0.0 {
                shift_subtree(&mut self.blocks[index..], dx, dy);
            }
        }
        y + height
    }

    /// Minimal CSS Grid: explicit `grid-template-columns/rows`, `gap` and
    /// row-major auto placement. No named lines, areas or explicit placement.
    fn grid(&mut self, b: &BoxNode, x: f32, y: f32, available_width: f32) -> f32 {
        let s = &b.style;
        let content_w = Self::box_content_width(s, available_width);
        let border_x = x + s.margin.left_at(available_width);
        let content_x = border_x + s.border.left + s.padding.left_at(available_width);
        let content_top = y + s.border.top + s.padding.top_at(available_width);

        let children: Vec<&BoxNode> = b
            .items
            .iter()
            .filter_map(|item| match item {
                FlowItem::Block(child) if !is_out_of_flow(child.style.position) => Some(child),
                _ => None,
            })
            .collect();

        let gap = if s.grid_gap > 0.0 { s.grid_gap } else { s.gap };
        let columns = resolve_grid_columns(s, content_w, gap);
        let col_count = columns.len().max(1);

        // Row-major auto placement, honouring `grid-column`/`grid-row` spans.
        let mut occupied: Vec<(usize, usize)> = Vec::new();
        let mut placements: Vec<(usize, usize, usize, usize)> = Vec::new(); // col,row,cspan,rspan
        for child in &children {
            let cspan = child.style.grid_column.span.max(1) as usize;
            let rspan = child.style.grid_row.span.max(1) as usize;
            let want_col = child
                .style
                .grid_column
                .start
                .map(|n| (n.saturating_sub(1)) as usize);
            let want_row = child
                .style
                .grid_row
                .start
                .map(|n| (n.saturating_sub(1)) as usize);
            let start_row = want_row.unwrap_or(0);
            let start_col = want_col;
            let mut placed = None;
            'search: for row in start_row..(start_row + children.len() + 4) {
                let cols: Vec<usize> = match start_col {
                    Some(c) => vec![c],
                    None => (0..col_count.saturating_sub(cspan - 1).max(1)).collect(),
                };
                for col in cols {
                    if col + cspan > col_count {
                        continue;
                    }
                    let free = (row..row + rspan).all(|r| {
                        (col..col + cspan).all(|c| !occupied.contains(&(c, r)))
                    });
                    if free {
                        for r in row..row + rspan {
                            for c in col..col + cspan {
                                occupied.push((c, r));
                            }
                        }
                        placements.push((col, row, cspan, rspan));
                        placed = Some(());
                        break 'search;
                    }
                }
            }
            if placed.is_none() {
                // No room found (explicit line beyond the grid): clamp.
                let col = start_col.unwrap_or(0).min(col_count.saturating_sub(1));
                let row = start_row;
                placements.push((col, row, cspan.min(col_count), rspan));
            }
        }

        let row_count = placements
            .iter()
            .map(|(_, r, _, rs)| r + rs)
            .max()
            .unwrap_or(0);
        let mut row_heights: Vec<f32> = (0..row_count)
            .map(|r| s.grid_rows.get(r).map(|t| t.px).unwrap_or(0.0))
            .collect();
        let row_top = |heights: &[f32], row: usize, gap: f32| -> f32 {
            let mut y = 0.0;
            for h in heights.iter().take(row) {
                y += h + gap;
            }
            y
        };

        // Row-spanning items are laid out first: their height must be known
        // before the rows they cover can position later items.
        let mut order: Vec<usize> = (0..children.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(placements[i].3));
        let first_block_of_grid = self.blocks.len();
        let mut block_start: Vec<usize> = vec![0; children.len()];
        let mut own_blocks: Vec<usize> = vec![0; children.len()];
        for &index in &order {
            let child = children[index];
            let (col, row, cspan, rspan) = placements[index];
            let width: f32 = columns[col..(col + cspan).min(col_count)]
                .iter()
                .sum::<f32>()
                + gap * (cspan as f32 - 1.0).max(0.0);
            let left: f32 = columns.iter().take(col).sum::<f32>() + gap * col as f32;
            let top = content_top + row_top(&row_heights, row, gap);
            let start_index = self.blocks.len();
            block_start[index] = start_index;
            // `justify-items` places an item narrower than its area. The
            // default (`stretch`) fills the area, which is what the layout
            // below assumes; `center`/`end` shrink-wrap the item first.
            let (item_w, item_dx) = match s.justify_items {
                AlignItems::Stretch => (width, 0.0),
                other => {
                    let outer = child.style.margin.horizontal_at(width)
                        + child.style.padding.horizontal_at(width)
                        + child.style.border.horizontal();
                    let natural = match &child.style.width {
                        Some(w) => w.resolve(width),
                        None => self.max_content_width(child),
                    } + outer;
                    let w = natural.min(width).max(1.0);
                    let dx = match other {
                        AlignItems::Center => (width - w) / 2.0,
                        AlignItems::End => width - w,
                        _ => 0.0,
                    };
                    (w, dx)
                }
            };
            let bottom = self.block(
                child,
                content_x + left + item_dx,
                top + child.style.margin.top_at(width),
                item_w,
            );
            let height = bottom - top + child.style.margin.bottom_at(width);
            // Remember the item's own box so `align-items` can place it in the
            // row once every row height is known.
            own_blocks[index] = self.blocks.len().saturating_sub(1);
            if rspan == 1 {
                row_heights[row] = row_heights[row].max(height);
            } else {
                let span_end = (row + rspan).min(row_heights.len().max(row + rspan));
                while row_heights.len() < span_end {
                    row_heights.push(0.0);
                }
                let have: f32 = row_heights[row..span_end].iter().sum::<f32>()
                    + gap * (rspan as f32 - 1.0);
                let deficit = (height - have).max(0.0);
                if deficit > 0.0 {
                    row_heights[span_end - 1] += deficit;
                }
            }
        }
        let _ = &block_start;

        // A grid item narrower than its area is positioned by `justify-items`
        // (not modelled); the default `stretch` already fills it, and an item
        // with a fixed width stays at the start. Centring the whole grid
        // content is `justify-content`, handled below.
        if s.justify_content != JustifyContent::Start && !columns.is_empty() {
            let used: f32 = columns.iter().sum::<f32>() + gap * (columns.len() as f32 - 1.0);
            let free = (content_w - used).max(0.0);
            let offset = match s.justify_content {
                JustifyContent::Center => free / 2.0,
                JustifyContent::End => free,
                _ => 0.0,
            };
            if offset > 0.0 {
                let start = first_block_of_grid;
                for block in self.blocks[start..].iter_mut() {
                    block.rect.x += offset;
                }
            }
        }

        // `align-items`/`align-self` inside the cell, for single-row items.
        for (index, child) in children.iter().enumerate() {
            let (_, row, _, rspan) = placements[index];
            if rspan != 1 {
                continue;
            }
            let align = child.style.align_self.unwrap_or(s.align_items);
            let cell_h = row_heights.get(row).copied().unwrap_or(0.0);
            let Some(block) = self.blocks.get(own_blocks[index]) else { continue };
            let item_h = block.rect.h;
            let offset = match align {
                AlignItems::Center => ((cell_h - item_h) / 2.0).max(0.0),
                AlignItems::End => (cell_h - item_h).max(0.0),
                _ => 0.0,
            };
            let from = own_blocks[index];
            let end = own_blocks.get(index + 1).copied().unwrap_or(self.blocks.len());
            if offset > 0.0 {
                for block in self.blocks[from..end].iter_mut() {
                    block.rect.y += offset;
                }
            }
            if align == AlignItems::Stretch && child.style.height.is_none() && item_h < cell_h {
                if let Some(block) = self.blocks.get_mut(from) {
                    block.rect.h = cell_h;
                }
            }
        }

        let content_bottom = if row_heights.is_empty() {
            content_top
        } else {
            content_top + row_top(&row_heights, row_heights.len(), gap) - gap
        };

        let height = s.border.vertical() + s.padding.vertical_at(available_width) + (content_bottom - content_top);
        let rect = Rect {
            x: border_x,
            y,
            w: s.border.horizontal() + s.padding.horizontal_at(available_width) + content_w,
            h: height,
        };
        let saved_clip = self.clip;
        self.blocks.push(BlockBox {
            rect,
            background: s.background,
            background_image: s
                .background_image
                .as_ref()
                .and_then(|url| self.images.get(url).cloned()),
            border: s.border,
            border_color: s.border_color,
            tag: b.tag.clone(),
            node: b.node,
            clip: saved_clip,
            border_radius: s.border_radius,
            box_shadow: s.box_shadow,
            paint: s.visible && s.opacity > 0.0,
            opacity: s.opacity,
        });
        y + height
    }

    #[allow(clippy::too_many_arguments)]
    fn flex_line(
        &mut self,
        children: &[&BoxNode],
        widths: &[f32],
        content_x: f32,
        top: f32,
        content_w: f32,
        gap: f32,
        style: &ComputedStyle,
    ) -> f32 {
        if children.is_empty() {
            return 0.0;
        }
        // Growth and shrink already happened in `flex_row`; here the widths
        // are final and only distribution is left.
        let used: f32 = widths.iter().sum::<f32>() + gap * (children.len() as f32 - 1.0);
        let mut widths: Vec<f32> = widths.to_vec();
        let leftover = (content_w - used).max(0.0);
        // Auto margins absorb the free space before `justify-content` runs, so
        // `margin-left: auto` pushes an item to the end of the line.
        let auto_margins: usize = children
            .iter()
            .map(|c| {
                c.style.margin_auto_left as usize + c.style.margin_auto_right as usize
            })
            .sum();
        let (start, between, mut auto_gap) = if auto_margins > 0 && leftover > 0.0 {
            (0.0, 0.0, Some(leftover / auto_margins as f32))
        } else {
            let (start, between) = match style.justify_content {
            JustifyContent::Start => (0.0, 0.0),
            JustifyContent::Center => (leftover / 2.0, 0.0),
            JustifyContent::End => (leftover, 0.0),
            JustifyContent::SpaceBetween => {
                let extra = if children.len() > 1 {
                    leftover / (children.len() as f32 - 1.0)
                } else {
                    0.0
                };
                (0.0, extra)
            }
                JustifyContent::SpaceAround => {
                    let extra = leftover / children.len() as f32;
                    (extra / 2.0, extra)
                }
            };
            (start, between, None)
        };

        let mut cursor = content_x + start;
        let mut tallest = 0.0f32;
        let mut placed: Vec<(f32, f32)> = Vec::new();
        let mut own_blocks: Vec<usize> = Vec::with_capacity(children.len());
        for (child, width) in children.iter().zip(widths.iter()) {
            if let Some(step) = auto_gap {
                if child.style.margin_auto_left {
                    cursor += step;
                }
            }
            self.flex_width = Some(*width);
            let bottom = self.block(child, cursor, top + child.style.margin.top_at(*width), *width);
            let height = bottom - top - child.style.margin.top_at(*width) + child.style.margin.bottom_at(*width);
            placed.push((cursor, height));
            own_blocks.push(self.blocks.len().saturating_sub(1));
            tallest = tallest.max(height);
            cursor += width + gap + between;
            if let Some(step) = auto_gap {
                if child.style.margin_auto_right {
                    cursor += step;
                }
            }
        }
        auto_gap = None;
        let _ = auto_gap;

        // align-items / align-self: vertical offset, or stretch, per item.
        for (index, (child, (_, height))) in children.iter().zip(placed.iter()).enumerate() {
            let align = child.style.align_self.unwrap_or(style.align_items);
            let offset = match align {
                AlignItems::Center => (tallest - height) / 2.0,
                AlignItems::End => tallest - height,
                _ => 0.0,
            };
            // Only this item's own subtree moves: a sibling on the same line
            // must not be dragged along.
            let from = own_blocks[index];
            let end = own_blocks.get(index + 1).copied().unwrap_or(self.blocks.len());
            if offset > 0.0 {
                for block in self.blocks[from..end].iter_mut() {
                    block.rect.y += offset;
                }
            }
            // `stretch` grows the item box to the line, when it has no height
            // of its own. Its content stays at the top, as in a browser.
            if align == AlignItems::Stretch && child.style.height.is_none() && *height < tallest {
                if let Some(block) = self.blocks.get_mut(from) {
                    block.rect.h = tallest;
                }
            }
        }
        tallest
    }
}

/// Collect absolutely positioned boxes anywhere inside `items`.
/// Cells of a row with their column spans.
fn row_cells(row: &BoxNode) -> Vec<(&BoxNode, usize)> {
    row.items
        .iter()
        .filter_map(|item| match item {
            FlowItem::Block(child) => Some((child, child.colspan)),
            _ => None,
        })
        .collect()
}

/// Rows of a table, looking through `thead`/`tbody`/`tfoot` wrappers.
fn collect_rows<'a>(items: &'a [FlowItem]) -> Vec<&'a BoxNode> {
    let mut out = Vec::new();
    for item in items {
        if let FlowItem::Block(child) = item {
            if child.style.display == Display::TableRow {
                out.push(child);
            } else if matches!(child.tag.as_str(), "thead" | "tbody" | "tfoot") {
                out.extend(collect_rows(&child.items));
            }
        }
    }
    out
}

fn collect_absolutes<'a>(items: &'a [FlowItem], out: &mut Vec<&'a BoxNode>) {
    for item in items {
        if let FlowItem::Block(child) = item {
            if is_out_of_flow(child.style.position) {
                out.push(child);
            } else {
                collect_absolutes(&child.items, out);
            }
        }
    }
}

/// Resolve grid columns, expanding `repeat(auto-fit|auto-fill, …)` against the
/// container width and honouring `minmax()` bounds.
fn resolve_grid_columns(style: &ComputedStyle, available: f32, gap: f32) -> Vec<f32> {
    let tracks: Vec<Track> = match &style.grid_auto_repeat {
        Some(repeat) => {
            let minimum = if repeat.track.min_px > 0.0 {
                repeat.track.min_px
            } else if repeat.track.px > 0.0 {
                repeat.track.px
            } else if repeat.track.percent > 0.0 {
                // A percentage track cannot drive the repeat count without
                // dividing by itself; fall back to a single track.
                available.max(1.0)
            } else {
                // A pure `fr` track cannot drive auto-repeat; fall back to one.
                available.max(1.0)
            };
            let count = (((available + gap) / (minimum + gap)).floor() as usize).clamp(1, 12);
            vec![repeat.track; count]
        }
        None => style.grid_columns.clone(),
    };
    resolve_tracks(&tracks, available, gap)
}

/// Resolve grid tracks into concrete pixel widths.
///
/// Fixed tracks (`px`, `%`) keep their size; `fr` tracks share what is left
/// after them, and every track is floored by its `minmax()` minimum and capped
/// by its maximum. A `fr` share is not added on top of the minimum — it is
/// clamped to it, which is what `minmax(200px, 1fr)` means.
fn resolve_tracks(tracks: &[Track], available: f32, gap: f32) -> Vec<f32> {
    if tracks.is_empty() {
        return vec![available.max(0.0)];
    }
    let gaps = gap * (tracks.len() as f32 - 1.0);
    let space = (available - gaps).max(0.0);
    let mut sizes: Vec<f32> = tracks
        .iter()
        .map(|t| {
            let fixed = t.px + t.percent * available / 100.0;
            fixed.max(t.min_px).max(0.0)
        })
        .collect();
    let fr_total: f32 = tracks.iter().map(|t| t.fr).sum();
    if fr_total > 0.0 {
        let fixed_sum: f32 = tracks
            .iter()
            .zip(sizes.iter())
            .filter(|(t, _)| t.fr == 0.0)
            .map(|(_, size)| *size)
            .sum();
        let free = (space - fixed_sum).max(0.0);
        for (track, size) in tracks.iter().zip(sizes.iter_mut()) {
            if track.fr > 0.0 {
                *size = (free * track.fr / fr_total).max(track.min_px);
            }
        }
    }
    for (track, size) in tracks.iter().zip(sizes.iter_mut()) {
        if let Some(max) = track.max_px {
            *size = size.min(max.max(track.min_px));
        }
    }
    sizes
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
}

/// Shift a block and every block nested inside it (they follow in the vec).
fn shift_subtree(blocks: &mut [BlockBox], dx: f32, dy: f32) {
    if let Some(first) = blocks.first() {
        let right = first.rect.right() + dx;
        let bottom = first.rect.bottom() + dy;
        for block in blocks.iter_mut() {
            if block.rect.x < right + 0.01 && block.rect.y <= bottom + 0.01 {
                block.rect.x += dx;
                block.rect.y += dy;
            }
        }
    }
}

pub fn layout(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn TextMeasure,
    viewport_width: f32,
    images: &HashMap<String, SharedImage>,
) -> Layout {
    layout_with_viewport(doc, styles, measure, viewport_width, 700.0, images)
}

/// Same as [`layout`], with an explicit viewport height so `position: fixed`
/// resolves against the real window instead of a placeholder.
pub fn layout_with_viewport(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn TextMeasure,
    viewport_width: f32,
    viewport_height: f32,
    images: &HashMap<String, SharedImage>,
) -> Layout {
    let items = build_children(doc, styles, doc.root, None);
    let root_style = ComputedStyle::default();
    let mut flow = Flow {
        measure,
        images,
        blocks: Vec::new(),
        runs: Vec::new(),
        links: Vec::new(),
        image_boxes: Vec::new(),
        arena: String::new(),
        scratch: String::new(),
        clip: None,
        containing: Rect { x: 0.0, y: 0.0, w: viewport_width.max(1.0), h: 10_000.0 },
        viewport: Rect { x: 0.0, y: 0.0, w: viewport_width.max(1.0), h: viewport_height.max(1.0) },
        svg_shapes: Vec::new(),
        static_positions: std::collections::HashMap::new(),
        doc: Some(doc),
        flex_width: None,
    };
    let content_height = flow.flow(&items, 0.0, 0.0, viewport_width.max(1.0), &root_style);

    let background_node = doc.find_tag("body");
    let background = background_node
        .map(|id| styles[id].background)
        .unwrap_or(Color::TRANSPARENT);

    Layout {
        text: flow.arena,
        blocks: flow.blocks,
        text_runs: flow.runs,
        links: flow.links,
        images: flow.image_boxes,
        content_height,
        background,
        background_node,
        svg_shapes: flow.svg_shapes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naro_css::{compute_styles, compute_styles_with, parse_stylesheet, StyleContext};

    struct MockMeasure;

    impl TextMeasure for MockMeasure {
        fn measure(&self, text: &str, size_px: f32, _bold: bool) -> f32 {
            text.chars().count() as f32 * size_px * 0.5
        }
        fn line_metrics(&self, size_px: f32, _bold: bool) -> (f32, f32) {
            (size_px * 0.8, size_px * 0.2)
        }
    }

    fn build(html: &str, css: &str, width: f32) -> Layout {
        let doc = Document::parse(html);
        let styles = compute_styles(&doc, &parse_stylesheet(css));
        layout(&doc, &styles, &MockMeasure, width, &HashMap::new())
    }

    /// Layout with an explicit viewport, for `vw`/`vh`/`vmin`/`vmax`.
    fn build_viewport(html: &str, css: &str, width: f32, viewport: (f32, f32)) -> Layout {
        let doc = Document::parse(html);
        let styles = compute_styles_with(
            &doc,
            &parse_stylesheet(css),
            StyleContext {
                viewport_width: viewport.0,
                viewport_height: viewport.1,
                root_font_size: 16.0,
            },
        );
        layout(&doc, &styles, &MockMeasure, width, &HashMap::new())
    }



    /// Layout plus the document it came from, so a test can name a node.
    fn build_doc(html: &str, css: &str, width: f32) -> (Document, Layout) {
        let doc = Document::parse(html);
        let styles = compute_styles(&doc, &parse_stylesheet(css));
        let layout = layout(&doc, &styles, &MockMeasure, width, &HashMap::new());
        (doc, layout)
    }

    fn node_with_id(doc: &Document, id: &str) -> NodeId {
        (0..doc.nodes.len())
            .find(|&n| doc.attr(n, "id") == Some(id))
            .unwrap_or_else(|| panic!("no element with id {id:?}"))
    }

    fn block_for<'a>(layout: &'a Layout, doc: &Document, id: &str) -> &'a BlockBox {
        let node = node_with_id(doc, id);
        layout
            .blocks
            .iter()
            .find(|b| b.node == node)
            .unwrap_or_else(|| panic!("no box for #{id}"))
    }

    #[test]
    fn flex_item_width_does_not_leak_into_the_next_item() {
        // The column container hands each item a width through `flex_width`.
        // An item that leaves `block()` through an early return (here
        // `display: flex`) must not leave that value behind for the next one.
        let (doc, layout) = build_doc(
            "<body><div class=\"col\"><div id=\"a\">x</div><svg viewBox=\"0 0 10 10\"><path d=\"M0 0h1v1H0z\"/></svg></div><div id=\"b\">abcd</div></body>",
            "body{margin:0} .col{display:flex;flex-direction:column;align-items:flex-start;width:400px} #a{display:flex;width:100px}",
            400.0,
        );
        let b = block_for(&layout, &doc, "b");
        // #b is an ordinary block: it fills the 400px viewport. The 10px of
        // the trailing `<svg>` must not survive as its width.
        assert!((b.rect.w - 400.0).abs() < 0.5, "width {}", b.rect.w);
    }

    #[test]
    fn unsized_svg_fills_its_wrapper_and_nothing_else() {
        // SVG2 gives the root `<svg>` an initial `width: auto`, which computes
        // to `100%`: an icon with only a `viewBox` fills the 24px wrapper.
        // Chrome measures exactly this box as 24x24. The old fallbacks — the
        // `viewBox` extent in user units (960) or the CSS default object size
        // (300x150) — painted a page-sized square over everything.
        let (doc, layout) = build_doc(
            "<body><div class=\"row\"><div class=\"icon\"><span id=\"box\"><svg viewBox=\"0 0 960 960\"><path d=\"M0 0h1v1H0z\"/></svg></span></div><div id=\"fill\"></div></div></body>",
            "body{margin:0} .row{display:flex;width:600px;height:40px} .icon{display:inline-block} \
             #box{display:inline-block;width:24px;height:24px} #fill{flex:1}",
            600.0,
        );
        let svg = layout
            .blocks
            .iter()
            .find(|b| b.tag == "svg")
            .expect("svg box");
        assert!((svg.rect.w - 24.0).abs() < 0.5, "svg width {}", svg.rect.w);
        assert!((svg.rect.h - 24.0).abs() < 0.5, "svg height {}", svg.rect.h);
        let fill = block_for(&layout, &doc, "fill");
        assert!((fill.rect.w - 576.0).abs() < 0.5, "fill width {}", fill.rect.w);
    }

    #[test]
    fn width_on_a_non_replaced_inline_box_is_ignored() {
        // CSS applies `width`/`height` only to block containers and replaced
        // elements; Chrome renders this span 8px wide, the width of its glyph.
        let (doc, layout) = build_doc(
            "<body><p id=\"p\"><span id=\"pill\" class=\"pill\">x</span></p></body>",
            "body{margin:0} .pill{display:inline;width:60px;height:20px;background:#1558d6}",
            400.0,
        );
        let pill = node_with_id(&doc, "pill");
        assert!(
            !layout.blocks.iter().any(|b| b.node == pill),
            "an inline non-replaced element must not get its own box"
        );
    }

    #[test]
    fn flex_and_grid_containers_honour_their_own_width() {
        // The offered width is the parent's content width; the box's own
        // `width` narrows it. Ignoring it made a 36px icon button fill its row.
        let (doc, layout) = build_doc(
            "<body><div id=\"f\"><div id=\"fa\"></div></div><div id=\"g\"><div id=\"ga\"></div></div></body>",
            "body{margin:0} #f{display:flex;width:300px;height:20px} #fa{flex:1} \
             #g{display:grid;width:200px;height:20px;grid-template-columns:1fr 1fr}",
            1000.0,
        );
        let f = block_for(&layout, &doc, "f");
        assert!((f.rect.w - 300.0).abs() < 0.5, "flex width {}", f.rect.w);
        let g = block_for(&layout, &doc, "g");
        assert!((g.rect.w - 200.0).abs() < 0.5, "grid width {}", g.rect.w);
    }

    #[test]
    fn auto_width_flex_row_shrink_wraps_around_a_sized_child() {
        // The Google search bar shape: an auto-width flex row holding a 36px
        // icon button whose `<svg>` is `width: 100%`. The row must shrink-wrap
        // to 36px; measuring the SVG's `viewBox` (960) as a percentage of the
        // probe width made the row 678px and put it over the text field.
        let (doc, layout) = build_doc(
            "<body><div class=\"row\"><div id=\"icons\" class=\"icons\"><div class=\"icon\">\
             <button class=\"btn\"><span class=\"ic\"><svg viewBox=\"0 0 960 960\"><path d=\"M0 0h1v1H0z\"/></svg></span></button>\
             </div></div><div id=\"field\"></div></div></body>",
            "body{margin:0} .row{display:flex;width:688px;height:50px} .icons{display:flex;align-self:center} \
             .icon{position:relative;display:inline-block} .btn{display:flex;align-items:center;justify-content:center;width:36px;height:36px;padding:0;border:0} \
             .ic{display:inline-block;width:24px;height:24px} .ic svg{display:block;width:100%;height:100%} #field{flex:1}",
            1000.0,
        );
        let icons = block_for(&layout, &doc, "icons");
        assert!((icons.rect.w - 36.0).abs() < 0.5, "icons width {}", icons.rect.w);
        let field = block_for(&layout, &doc, "field");
        assert!((field.rect.w - 652.0).abs() < 0.5, "field width {}", field.rect.w);
        let svg = layout.blocks.iter().find(|b| b.tag == "svg").expect("svg box");
        assert!((svg.rect.w - 24.0).abs() < 0.5, "svg width {}", svg.rect.w);
    }

    #[test]
    fn percentage_padding_and_margin_use_the_container_width() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{padding:10% 5%; margin:5%}",
            400.0,
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        // The container is 400 wide: 5% horizontal padding/margin is 20px,
        // 10% vertical padding is 40px — vertical sides also use the width.
        assert!((div.rect.w - 360.0).abs() < 0.01, "width {}", div.rect.w);
        assert!((div.rect.x - 20.0).abs() < 0.01, "x {}", div.rect.x);
        assert!((div.rect.h - (40.0 + 40.0 + 21.6)).abs() < 0.5, "height {}", div.rect.h);
    }

    #[test]
    fn percentage_height_against_auto_height_is_auto() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{height:50%}",
            400.0,
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        // Content height, not half of the 400px width.
        assert!((div.rect.h - 21.6).abs() < 0.5, "height {}", div.rect.h);
    }

    #[test]
    fn vh_inside_calc_reaches_layout() {
        let layout = build_viewport(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{height:calc(20vh - 40px)}",
            400.0,
            (1000.0, 700.0),
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.h - 100.0).abs() < 0.5, "height {}", div.rect.h);
    }

    #[test]
    fn percentage_widths_resolve_against_the_container() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{width:50%}",
            400.0,
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.w - 200.0).abs() < 0.01, "width {}", div.rect.w);
    }

    #[test]
    fn calc_percentage_minus_pixels_reaches_layout() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{width:calc(100% - 32px)}",
            400.0,
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.w - 368.0).abs() < 0.01, "width {}", div.rect.w);
    }

    #[test]
    fn min_max_clamp_produce_real_widths() {
        let layout = build(
            "<body><div class=\"a\">a</div><div class=\"b\">b</div></body>",
            "body{margin:0} .a{width:min(800px, 90%)} .b{width:max(320px, 50%)}",
            400.0,
        );
        let a = layout.blocks.iter().find(|b| b.tag == "div" && b.rect.y < 100.0).unwrap();
        assert!((a.rect.w - 360.0).abs() < 0.01, "min width {}", a.rect.w);
        let b = layout.blocks.iter().rev().find(|b| b.tag == "div").unwrap();
        assert!((b.rect.w - 320.0).abs() < 0.01, "max width {}", b.rect.w);
    }

    #[test]
    fn clamp_uses_the_viewport_for_vw() {
        // 80vw of 1000 is 800, clamped up to 320 and down to 1200.
        let layout = build_viewport(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{width:clamp(320px, 80vw, 1200px)}",
            1000.0,
            (1000.0, 700.0),
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.w - 800.0).abs() < 0.01, "width {}", div.rect.w);
    }

    #[test]
    fn rem_padding_uses_the_root_font_size() {
        let html = "<html><body><div class=\"a\">x</div></body></html>";
        // 1rem with a 20px root must behave exactly like 20px.
        let rem = build(html, "html{font-size:20px} body{margin:0} .a{padding:1rem}", 400.0);
        let px = build(html, "html{font-size:20px} body{margin:0} .a{padding:20px}", 400.0);
        let rem_div = rem.blocks.iter().find(|b| b.tag == "div").unwrap();
        let px_div = px.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((rem_div.rect.h - px_div.rect.h).abs() < 0.01);
        assert!((rem_div.rect.w - px_div.rect.w).abs() < 0.01);
        // And it is not silently the 16px default.
        let sixteen = build(html, "html{font-size:16px} body{margin:0} .a{padding:1rem}", 400.0);
        let sixteen_div = sixteen.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((sixteen_div.rect.h - px_div.rect.h).abs() > 5.0);
    }

    #[test]
    fn grid_percentage_columns_use_the_container_width() {
        let layout = build(
            "<body><div class=\"g\"><div>a</div><div>b</div></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:25% 75%}",
            400.0,
        );
        let cells: Vec<&BlockBox> = layout
            .blocks
            .iter()
            .filter(|b| b.tag == "div")
            // Skip the grid container itself (it spans the full width).
            .filter(|b| b.rect.w < 399.0)
            .collect();
        assert_eq!(cells.len(), 2);
        assert!((cells[0].rect.w - 100.0).abs() < 0.5, "first {}", cells[0].rect.w);
        assert!((cells[1].rect.w - 300.0).abs() < 0.5, "second {}", cells[1].rect.w);
    }

    #[test]
    fn vh_sets_a_real_height() {
        let layout = build_viewport(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{height:10vh}",
            400.0,
            (1000.0, 700.0),
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.h - 70.0).abs() < 0.5, "height {}", div.rect.h);
    }

    #[test]
    fn paint_refresh_updates_colours_but_not_geometry() {
        let layout = build(
            "<body><p class=\"a\">x</p></body>",
            "body{margin:0;background-color:#ffffff} .a{color:#000000;width:100px}",
            200.0,
        );
        let doc = Document::parse("<body><p class=\"a\">x</p></body>");
        let mut styles = naro_css::compute_styles(
            &doc,
            &naro_css::parse_stylesheet("body{margin:0;background-color:#000000} .a{color:#ff0000;width:100px}"),
        );
        let mut refreshed = layout;
        let blocks_before: Vec<_> = refreshed.blocks.iter().map(|b| b.rect).collect();
        refreshed.refresh_paint(&styles, &HashMap::new());
        let blocks_after: Vec<_> = refreshed.blocks.iter().map(|b| b.rect).collect();
        assert_eq!(blocks_before.len(), blocks_after.len());
        for (a, b) in blocks_before.iter().zip(blocks_after.iter()) {
            assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h), "geometry moved");
        }
        assert_eq!(refreshed.background, Color::rgb(0, 0, 0));
        assert_eq!(refreshed.text_runs[0].color, Color::rgb(255, 0, 0));
        styles.clear();
    }

    #[test]
    fn long_text_wraps_into_multiple_lines() {
        let layout = build(
            "<body><p>aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd</p></body>",
            "body{margin:0} p{margin:0}",
            100.0,
        );
        let baselines: Vec<f32> = layout.text_runs.iter().map(|r| r.baseline).collect();
        assert!(baselines.len() >= 4);
        assert!(layout
            .text_runs
            .iter()
            .any(|r| r.text(&layout) == "dddddddddd"));
        assert!(layout.content_height > 40.0);
    }

    #[test]
    fn vertical_margin_separates_blocks() {
        let layout = build(
            "<body><div class=\"a\">a</div><div class=\"b\">b</div></body>",
            "body{margin:0} div{margin:10px 0}",
            200.0,
        );
        assert_eq!(layout.blocks.len(), 3); // body + 2 divs
        let divs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "div").collect();
        assert_eq!(divs.len(), 2);
        assert_eq!(divs[0].rect.y, 10.0);
        assert!((divs[1].rect.y - (divs[0].rect.bottom() + 20.0)).abs() < 0.01);
    }

    #[test]
    fn links_produce_link_boxes() {
        let layout = build(
            "<body><p>veja <a href=\"https://example.com\">example.com</a></p></body>",
            "body{margin:0}",
            300.0,
        );
        assert_eq!(layout.links.len(), 1);
        assert_eq!(layout.links[0].href, "https://example.com");
        assert!(layout.text_runs.iter().any(|r| r.link.is_some()));
    }

    #[test]
    fn display_none_creates_no_box() {
        let layout = build(
            "<body><div class=\"gone\">x</div><div>y</div></body>",
            "body{margin:0} .gone{display:none}",
            300.0,
        );
        let divs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "div").collect();
        assert_eq!(divs.len(), 1);
    }

    #[test]
    fn flex_row_places_children_side_by_side() {
        let layout = build(
            "<body><div class=\"nav\"><span>a</span><span>b</span><span>c</span></div></body>",
            "body{margin:0} .nav{display:flex; gap:10px; padding:10px}",
            300.0,
        );
        let spans: Vec<&BlockBox> = layout
            .blocks
            .iter()
            .filter(|b| b.tag == "span" && b.rect.y > 0.0)
            .collect();
        assert_eq!(spans.len(), 3);
        assert!(spans[0].rect.x < spans[1].rect.x);
        assert!(spans[1].rect.x < spans[2].rect.x);
        let gap = spans[1].rect.x - (spans[0].rect.x + spans[0].rect.w);
        assert!((gap - 10.0).abs() < 0.01, "gap was {gap}");
    }

    #[test]
    fn flex_column_stacks_children() {
        let layout = build(
            "<body><div class=\"col\"><div>a</div><div>b</div></div></body>",
            "body{margin:0} .col{display:flex; flex-direction:column; gap:6px; padding:10px}",
            300.0,
        );
        let divs: Vec<&BlockBox> = layout
            .blocks
            .iter()
            .filter(|b| b.tag == "div" && b.rect.y > 0.0)
            .collect();
        assert_eq!(divs.len(), 2);
        assert!(divs[0].rect.y < divs[1].rect.y);
        let gap = divs[1].rect.y - divs[0].rect.bottom();
        assert!((gap - 6.0).abs() < 0.01, "gap was {gap}");
    }

    #[test]
    fn flex_justify_center_offsets_the_line() {
        let layout = build(
            "<body><div class=\"row\"><span>a</span></div></body>",
            "body{margin:0} .row{display:flex; justify-content:center}",
            300.0,
        );
        let span = layout.blocks.iter().find(|b| b.tag == "span").unwrap();
        assert!(span.rect.x > 100.0, "centered at {}", span.rect.x);
    }

    #[test]
    fn max_width_centers_with_auto_margins() {
        let layout = build(
            "<body><div class=\"wrap\">x</div></body>",
            "body{margin:0} .wrap{max-width:200px; margin:0 auto}",
            600.0,
        );
        let div = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((div.rect.w - 200.0).abs() < 0.01, "width {}", div.rect.w);
        assert!((div.rect.x - 200.0).abs() < 0.01, "x {}", div.rect.x);
    }

    #[test]
    fn flex_grow_shares_free_space() {
        let layout = build(
            "<body><div class=\"bar\"><span>a</span><span class=\"g\"></span><span>b</span></div></body>",
            "body{margin:0} .bar{display:flex} .g{flex-grow:1}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert_eq!(spans.len(), 3);
        assert!(spans[1].rect.w > 300.0, "grow width {}", spans[1].rect.w);
        assert!(spans[2].rect.right() <= 400.5);
    }

    #[test]
    fn box_sizing_changes_content_width() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{width:200px; padding:20px; border:5px solid #000}",
            600.0,
        );
        let a = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        // content-box: 200 content + 40 padding + 10 border = 250 border box
        assert!((a.rect.w - 250.0).abs() < 0.01, "content-box {}", a.rect.w);

        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{box-sizing:border-box; width:200px; padding:20px; border:5px solid #000}",
            600.0,
        );
        let a = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((a.rect.w - 200.0).abs() < 0.01, "border-box {}", a.rect.w);
    }

    #[test]
    fn min_width_overrides_the_available_width() {
        let layout = build(
            "<body><div class=\"a\">x</div></body>",
            "body{margin:0} .a{width:50px; min-width:180px}",
            600.0,
        );
        let a = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert!((a.rect.w - 180.0).abs() < 0.01, "min-width {}", a.rect.w);
    }

    #[test]
    fn overflow_hidden_records_a_clip() {
        let layout = build(
            "<body><div class=\"c\"><div class=\"inner\">x</div></div></body>",
            "body{margin:0} .c{height:20px; overflow:hidden} .inner{height:80px}",
            400.0,
        );
        let divs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "div").collect();
        assert_eq!(divs.len(), 2);
        assert!(divs[0].clip.is_some(), "inner box inherits the clip");
        assert!(divs[1].clip.is_none(), "the clipping box itself is not clipped");
        assert!((divs[1].rect.h - 20.0).abs() < 0.01, "clipped box keeps 20px");
    }

    #[test]
    fn absolute_uses_the_nearest_positioned_ancestor() {
        let layout = build(
            "<body><div class=\"rel\"><div class=\"abs\">x</div></div></body>",
            "body{margin:0} .rel{position:relative; margin-left:50px; padding:10px} .abs{position:absolute; top:20px; left:30px; width:40px; height:15px}",
            400.0,
        );
        let abs = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div" && (b.rect.w - 40.0).abs() < 0.01)
            .expect("absolute box");
        // containing block is the relative box's content box: x=50+10, y=0+10
        assert!((abs.rect.x - 90.0).abs() < 0.01, "x {}", abs.rect.x);
        assert!((abs.rect.y - 30.0).abs() < 0.01, "y {}", abs.rect.y);
    }

    #[test]
    fn grid_places_items_in_columns() {
        let layout = build(
            "<body><div class=\"g\"><p>a</p><p>b</p><p>c</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:1fr 1fr; gap:10px; padding:10px}",
            410.0,
        );
        let inner = grid_cells(&layout);
        assert_eq!(inner.len(), 3, "three grid cells");
        assert!(inner[0].0 < inner[1].0, "two columns");
        assert!(inner[2].1 > inner[0].1, "third item wraps to a new row");
    }

    /// Grid item boxes: `<p>` cells inside the `<div>` container.
    fn grid_cells(layout: &Layout) -> Vec<(f32, f32, f32, f32)> {
        layout
            .blocks
            .iter()
            .filter(|b| b.tag == "p" && b.rect.h > 0.0)
            .map(|b| (b.rect.x, b.rect.y, b.rect.w, b.rect.h))
            .collect()
    }

    #[test]
    fn minmax_and_auto_fit_produce_adaptive_columns() {
        let layout = build(
            "<body><div class=\"g\"><p>a</p><p>b</p><p>c</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:repeat(auto-fit, minmax(120px, 1fr)); gap:10px}",
            410.0,
        );
        let cells = grid_cells(&layout);
        assert_eq!(cells.len(), 3);
        // (410 + 10) / (120 + 10) = 3.23 -> 3 columns of 130
        assert!((cells[0].2 - 130.0).abs() < 0.5, "width {}", cells[0].2);
        assert!(cells[0].1 == cells[1].1 && cells[1].1 == cells[2].1);
    }

    #[test]
    fn auto_fit_shrinks_the_column_count() {
        let layout = build(
            "<body><div class=\"g\"><p>a</p><p>b</p><p>c</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:repeat(auto-fit, minmax(200px, 1fr)); gap:10px}",
            410.0,
        );
        let cells = grid_cells(&layout);
        // (410 + 10) / (200 + 10) = 2 columns
        assert!(cells[0].2 > 195.0, "width {}", cells[0].2);
        assert!(cells[2].1 > cells[0].1, "third item wraps");
    }

    #[test]
    fn grid_column_span_covers_two_columns() {
        let layout = build(
            "<body><div class=\"g\"><p class=\"wide\">a</p><p>b</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:1fr 1fr; gap:10px} .wide{grid-column:span 2}",
            410.0,
        );
        let cells = grid_cells(&layout);
        let wide = cells.iter().find(|c| c.2 > 300.0).expect("spanning cell");
        assert!((wide.2 - 410.0).abs() < 1.0, "span width {}", wide.2);
        let narrow = cells.iter().find(|c| c.2 < 300.0).expect("single cell");
        assert!(narrow.1 > wide.1, "next item moves to the following row");
    }

    #[test]
    fn grid_row_span_increases_the_row_height() {
        let layout = build(
            "<body><div class=\"g\"><p class=\"tall\">a</p><p>b</p><p>c</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:1fr 1fr; gap:10px} .tall{grid-row:span 2; height:120px}",
            410.0,
        );
        let cells = grid_cells(&layout);
        let tall = cells.iter().find(|c| c.3 > 100.0).expect("tall cell");
        let container = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div")
            .expect("grid container");
        // The spanned rows must be tall enough to contain the 120px item.
        assert!(
            container.rect.bottom() >= tall.1 + 120.0,
            "container {} vs item bottom {}",
            container.rect.bottom(),
            tall.1 + 120.0
        );
        // The second row starts below the first row's tallest single-row item.
        let third = cells
            .iter()
            .find(|c| c.1 > tall.1 + 20.0)
            .expect("item in the second row");
        assert!(third.1 >= tall.1 + 21.0);
    }

    #[test]
    fn inputs_and_button_share_one_line() {
        let layout = build(
            "<body><div class=\"row\"><input value=\"a\"><input value=\"b\"><button>ok</button></div></body>",
            "body{margin:0} input{width:100px} button{width:60px}",
            600.0,
        );
        let boxes: Vec<&BlockBox> = layout
            .blocks
            .iter()
            .filter(|b| matches!(b.tag.as_str(), "input" | "button"))
            .collect();
        assert_eq!(boxes.len(), 3);
        assert!(boxes[0].rect.x < boxes[1].rect.x, "inputs side by side");
        assert!(boxes[1].rect.x < boxes[2].rect.x, "button after inputs");
        assert!((boxes[0].rect.y - boxes[1].rect.y).abs() < 0.01, "same line");
    }

    #[test]
    fn inline_controls_wrap_in_a_narrow_container() {
        let layout = build(
            "<body><div class=\"row\"><input><input><input></div></body>",
            "body{margin:0} input{width:200px}",
            450.0,
        );
        let boxes: Vec<&BlockBox> = layout
            .blocks
            .iter()
            .filter(|b| b.tag == "input")
            .collect();
        assert_eq!(boxes.len(), 3);
        assert!((boxes[0].rect.y - boxes[1].rect.y).abs() < 0.01, "two fit");
        assert!(boxes[2].rect.y > boxes[0].rect.y + 5.0, "third wraps");
        assert!(boxes[2].rect.x < boxes[0].rect.x + 1.0, "wrapped item restarts at the left");
    }

    #[test]
    fn text_and_image_share_a_line() {
        let layout = build(
            "<body><p>antes <img src=\"x.png\"> depois</p></body>",
            "body{margin:0} img{width:40px; height:20px}",
            600.0,
        );
        // The image has no decoded bytes in this test, but it still occupies
        // its intrinsic 40px slot on the line.
        let antes = layout
            .text_runs
            .iter()
            .find(|r| r.text(&layout).contains("antes"))
            .expect("text before image");
        let depois = layout
            .text_runs
            .iter()
            .find(|r| r.text(&layout).contains("depois"))
            .expect("text after image");
        assert!(depois.x > antes.x + 40.0, "image reserves horizontal space");
        assert!(
            (depois.baseline - antes.baseline).abs() < 0.01,
            "same line"
        );
    }

    #[test]
    fn inline_spaces_separate_links() {
        let layout = build(
            "<body><p><a href=\"#a\">um</a> <a href=\"#b\">dois</a></p></body>",
            "body{margin:0}",
            600.0,
        );
        let a = layout.links.iter().find(|l| l.href == "#a").unwrap();
        let b = layout.links.iter().find(|l| l.href == "#b").unwrap();
        assert!(b.rect.x > a.rect.right() + 1.0, "gap between links");
    }

    #[test]
    fn absolute_image_leaves_the_inline_flow() {
        let with_abs = build(
            "<body><p>antes <img src=\"k.png\" width=\"80\" height=\"40\" style=\"position:absolute;right:5px;top:4px\"> depois</p></body>",
            "body{margin:0}",
            600.0,
        );
        let plain = build(
            "<body><p>antes depois</p></body>",
            "body{margin:0}",
            600.0,
        );
        // An absolutely positioned image reserves no inline space, so the text
        // sits exactly where it would without the element.
        let a = with_abs.text_runs.iter().find(|r| r.text(&with_abs).contains("depois")).unwrap();
        let b = plain.text_runs.iter().find(|r| r.text(&plain).contains("depois")).unwrap();
        assert!((a.x - b.x).abs() < 0.5, "x {} vs {}", a.x, b.x);
        assert!(with_abs.images.is_empty(), "nothing decoded in this test");
    }

    #[test]
    fn clip_height_is_patched_after_layout() {
        let layout = build(
            "<body><div class=\"c\"><p>a</p></div></body>",
            "body{margin:0} .c{overflow:hidden; padding:10px}",
            300.0,
        );
        let p = layout.blocks.iter().find(|b| b.tag == "p").unwrap();
        let clip = p.clip.expect("inherited clip");
        assert!(clip.h > 20.0, "clip height {}", clip.h);
    }

    #[test]
    fn table_places_cells_in_rows_and_columns() {
        let layout = build(
            "<body><table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table></body>",
            "body{margin:0} table{width:400px} td{padding:4px}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        assert_eq!(cells.len(), 4);
        assert!((cells[0].rect.y - cells[1].rect.y).abs() < 0.01, "row 1 shares a line");
        assert!(cells[1].rect.x > cells[0].rect.x, "columns advance");
        assert!(cells[2].rect.y > cells[0].rect.y, "row 2 below row 1");
        assert!((cells[2].rect.x - cells[0].rect.x).abs() < 0.01, "columns align");
        let table = layout.blocks.iter().find(|b| b.tag == "table").unwrap();
        assert!((table.rect.w - 400.0).abs() < 0.01, "declared table width");
    }

    #[test]
    fn table_cells_shrink_to_fit_the_table() {
        let layout = build(
            "<body><table><tr><td>aaaaaaaaaa</td><td>bbbbbbbbbb</td></tr></table></body>",
            "body{margin:0} table{width:100px} td{padding:0}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        let total: f32 = cells.iter().map(|c| c.rect.w).sum();
        assert!(total <= 101.0, "cells fit the table: {total}");
    }

    #[test]
    fn relative_cell_contains_absolute_image() {
        let layout = build(
            "<body><table><tr><td style=\"position:relative\"><img src=\"i.png\" width=\"10\" height=\"10\" style=\"position:absolute;right:0;top:0\"></td></tr></table></body>",
            "body{margin:0} table{width:300px} td{padding:10px}",
            600.0,
        );
        let cell = layout.blocks.iter().find(|b| b.tag == "td").unwrap();
        // The image is absolute and has no decoded bytes, so no ImageBox is
        // produced; what matters is that the cell exists and is positioned.
        assert!(cell.rect.w > 0.0);
        assert!(cell.rect.h > 0.0);
    }

    // --- Stage 5: layout completeness ------------------------------------

    #[test]
    fn flex_basis_and_shorthand_control_the_base_size() {
        // `flex-basis` wins over `width`.
        let layout = build(
            "<body><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></body>",
            "body{margin:0} .row{display:flex} .a{width:40px; flex-basis:120px} .b{width:60px}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!((spans[0].rect.w - 120.0).abs() < 0.01, "basis {}", spans[0].rect.w);
        assert!((spans[1].rect.w - 60.0).abs() < 0.01, "width {}", spans[1].rect.w);

        // `flex: 1` means `1 1 0%`, so two items split the row evenly.
        let layout = build(
            "<body><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></body>",
            "body{margin:0} .row{display:flex} span{flex:1}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!((spans[0].rect.w - 200.0).abs() < 0.5, "first {}", spans[0].rect.w);
        assert!((spans[1].rect.w - 200.0).abs() < 0.5, "second {}", spans[1].rect.w);
    }

    #[test]
    fn flex_shrink_is_weighted_by_the_base_size() {
        // Two items of 300 and 100 in a 200px row: the deficit is shared by
        // `shrink × base`, so the wider item gives up three times as much.
        let layout = build(
            "<body><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></body>",
            "body{margin:0} .row{display:flex} .a{flex:0 1 300px} .b{flex:0 1 100px}",
            200.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        let total = spans[0].rect.w + spans[1].rect.w;
        assert!(total <= 200.5, "fits the row: {total}");
        assert!((spans[0].rect.w - 150.0).abs() < 1.0, "a {}", spans[0].rect.w);
        assert!((spans[1].rect.w - 50.0).abs() < 1.0, "b {}", spans[1].rect.w);
    }

    #[test]
    fn flex_shrink_zero_keeps_the_item_size() {
        let layout = build(
            "<body><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></body>",
            "body{margin:0} .row{display:flex} .a{flex:0 0 300px} .b{flex:0 0 300px}",
            200.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        // Neither may shrink: the line overflows instead.
        assert!((spans[0].rect.w - 300.0).abs() < 0.5, "a {}", spans[0].rect.w);
        assert!((spans[1].rect.w - 300.0).abs() < 0.5, "b {}", spans[1].rect.w);
    }

    #[test]
    fn flex_min_and_max_width_bound_the_result() {
        let layout = build(
            "<body><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></body>",
            "body{margin:0} .row{display:flex} .a{flex:1; max-width:80px} .b{flex:1; min-width:250px}",
            600.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!(spans[0].rect.w <= 80.5, "capped at max-width {}", spans[0].rect.w);
        assert!(spans[1].rect.w >= 250.0, "floored at min-width {}", spans[1].rect.w);
    }

    #[test]
    fn flex_auto_margins_push_items_apart() {
        let layout = build(
            "<body><div class=\"row\"><span>a</span><span class=\"end\">b</span></div></body>",
            "body{margin:0} .row{display:flex} .end{margin-left:auto}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!(
            (spans[1].rect.right() - 400.0).abs() < 1.0,
            "pushed to the end: right {}",
            spans[1].rect.right()
        );
    }

    #[test]
    fn align_content_distributes_wrapped_lines() {
        // Three 60px items with a 20px gap need two lines in a 200px row; a
        // 200px-tall container leaves room to spread them apart.
        let layout = build(
            "<body><div class=\"row\"><span>a</span><span>b</span><span>c</span></div></body>",
            "body{margin:0} .row{display:flex; flex-wrap:wrap; gap:20px; height:200px; align-content:space-between} span{width:60px}",
            200.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert_eq!(spans.len(), 3);
        let first_line_y = spans[0].rect.y;
        let second_line_y = spans[2].rect.y;
        assert!(second_line_y > first_line_y + 100.0, "lines spread: {second_line_y}");
    }

    #[test]
    fn align_items_stretch_fills_the_line() {
        let layout = build(
            "<body><div class=\"row\"><span class=\"short\">a</span><span class=\"tall\">b</span></div></body>",
            "body{margin:0} .row{display:flex; align-items:stretch} .short{width:40px} .tall{width:40px; height:100px}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!((spans[1].rect.h - 100.0).abs() < 0.5, "tall {}", spans[1].rect.h);
        assert!((spans[0].rect.h - 100.0).abs() < 0.5, "stretched {}", spans[0].rect.h);
    }

    #[test]
    fn align_items_center_offsets_the_item() {
        let layout = build(
            "<body><div class=\"row\"><span class=\"s\">a</span><span class=\"t\">b</span></div></body>",
            "body{margin:0} .row{display:flex; align-items:center} .s{width:40px} .t{width:40px; height:100px}",
            400.0,
        );
        let spans: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "span").collect();
        assert!(spans[0].rect.y > spans[1].rect.y + 20.0, "centered: {}", spans[0].rect.y);
    }

    #[test]
    fn table_columns_are_shared_between_rows() {
        let layout = build(
            "<body><table><tr><td>a</td><td>conteudo bem mais longo</td></tr>\
             <tr><td>conteudo bem mais longo</td><td>b</td></tr></table></body>",
            "body{margin:0} table{width:400px} td{padding:2px}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        assert_eq!(cells.len(), 4);
        // Column 0 keeps one width in both rows, and so does column 1.
        assert!((cells[0].rect.w - cells[2].rect.w).abs() < 0.01, "col 0");
        assert!((cells[1].rect.w - cells[3].rect.w).abs() < 0.01, "col 1");
        assert!((cells[0].rect.x - cells[2].rect.x).abs() < 0.01, "col 0 x");
    }

    #[test]
    fn table_colspan_spans_the_columns() {
        let layout = build(
            "<body><table><tr><td colspan=\"2\">wide</td></tr><tr><td>a</td><td>b</td></tr></table></body>",
            "body{margin:0} table{width:400px} td{padding:2px}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        assert_eq!(cells.len(), 3);
        let wide = &cells[0];
        let a = &cells[1];
        let b = &cells[2];
        assert!(
            (wide.rect.w - (a.rect.w + b.rect.w)).abs() < 0.5,
            "span {} vs {} + {}",
            wide.rect.w,
            a.rect.w,
            b.rect.w
        );
        assert!(b.rect.y > wide.rect.y, "second row below the span");
    }

    #[test]
    fn table_rowspan_extends_the_cell() {
        let layout = build(
            "<body><table><tr><td rowspan=\"2\">tall</td><td>a</td></tr>\
             <tr><td>b</td></tr></table></body>",
            "body{margin:0} table{width:300px} td{padding:10px; height:40px}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        assert_eq!(cells.len(), 3);
        let tall = &cells[0];
        let a = &cells[1];
        let b = &cells[2];
        // The second row's cell starts to the right of the spanning cell, not
        // under it, and the spanning cell covers both rows.
        assert!(b.rect.y > a.rect.y, "row 2 below row 1");
        assert!((b.rect.x - a.rect.x).abs() < 0.01, "second column aligns");
        assert!(
            tall.rect.bottom() >= b.rect.bottom() - 0.5,
            "span covers both rows: {} vs {}",
            tall.rect.bottom(),
            b.rect.bottom()
        );
    }

    #[test]
    fn table_layout_fixed_takes_widths_from_the_first_row() {
        let layout = build(
            "<body><table><tr><td class=\"w\">a</td><td>b</td></tr>\
             <tr><td>um texto bastante comprido aqui</td><td>c</td></tr></table></body>",
            "body{margin:0} table{width:400px; table-layout:fixed} td{padding:0} .w{width:300px}",
            600.0,
        );
        let cells: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "td").collect();
        assert_eq!(cells.len(), 4);
        // The first row fixes the grid: 300 and 100, whatever row 2 contains.
        assert!((cells[0].rect.w - 300.0).abs() < 0.5, "first {}", cells[0].rect.w);
        assert!((cells[1].rect.w - 100.0).abs() < 0.5, "second {}", cells[1].rect.w);
        assert!((cells[2].rect.w - 300.0).abs() < 0.5, "row 2 keeps the width");
    }

    #[test]
    fn display_contents_removes_the_box() {
        let layout = build(
            "<body><div class=\"wrap\"><div class=\"inner\">x</div></div></body>",
            "body{margin:0} .wrap{display:contents} .inner{height:20px}",
            300.0,
        );
        let divs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "div").collect();
        assert_eq!(divs.len(), 1, "only the inner box is laid out");
        assert_eq!(divs[0].rect.x, 0.0, "children join the parent flow");
    }

    #[test]
    fn inline_block_shrink_wraps_and_keeps_its_height() {
        let layout = build(
            "<body><p>antes <span class=\"ib\"><div>um</div><div>dois</div></span> depois</p></body>",
            "body{margin:0} .ib{display:inline-block; padding:4px} .ib div{height:20px}",
            600.0,
        );
        let span = layout
            .blocks
            .iter()
            .find(|b| b.tag == "span" && b.rect.h > 30.0)
            .expect("inline-block box");
        // Two 20px block children plus 4px padding on each side.
        assert!((span.rect.h - 48.0).abs() < 1.0, "height {}", span.rect.h);
        let depois = layout
            .text_runs
            .iter()
            .find(|r| r.text(&layout).contains("depois"))
            .expect("text after the box");
        assert!(depois.x > span.rect.x, "text continues after the box");
    }

    #[test]
    fn min_and_max_height_bound_the_box() {
        let layout = build(
            "<body><div class=\"a\">x</div><div class=\"b\">x</div></body>",
            "body{margin:0} .a{min-height:120px} .b{max-height:10px; overflow:hidden}",
            300.0,
        );
        let divs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "div").collect();
        assert!((divs[0].rect.h - 120.0).abs() < 0.5, "min-height {}", divs[0].rect.h);
        assert!(divs[1].rect.h <= 10.5, "max-height {}", divs[1].rect.h);
    }

    #[test]
    fn fixed_position_uses_the_viewport() {
        let layout = build_viewport(
            "<body><div class=\"rel\"><div class=\"fix\">x</div></div></body>",
            "body{margin:0} .rel{position:relative; margin-left:100px; padding:50px} .fix{position:fixed; top:0; right:0; width:30px; height:10px}",
            1000.0,
            (1000.0, 700.0),
        );
        let fix = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div" && (b.rect.w - 30.0).abs() < 0.01)
            .expect("fixed box");
        // Anchored to the viewport's right edge, not to the positioned parent.
        assert!((fix.rect.x - 970.0).abs() < 0.5, "x {}", fix.rect.x);
        assert!(fix.rect.y.abs() < 0.5, "y {}", fix.rect.y);
    }

    #[test]
    fn minmax_floors_a_flexible_track() {
        // minmax(300px, 1fr) in a 400px grid: the track may not go below 300,
        // and the fr share is clamped to it rather than added to it.
        let layout = build(
            "<body><div class=\"g\"><p>a</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:minmax(300px, 1fr) minmax(0, 1fr)}",
            400.0,
        );
        let cells = grid_cells(&layout);
        assert_eq!(cells.len(), 1);
        assert!((cells[0].2 - 300.0).abs() < 0.5, "width {}", cells[0].2);
    }

    #[test]
    fn fr_tracks_share_the_space_left_by_fixed_tracks() {
        let layout = build(
            "<body><div class=\"g\"><p>a</p><p>b</p><p>c</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:100px 1fr 1fr}",
            500.0,
        );
        let cells = grid_cells(&layout);
        assert_eq!(cells.len(), 3);
        assert!((cells[0].2 - 100.0).abs() < 0.5, "fixed {}", cells[0].2);
        assert!((cells[1].2 - 200.0).abs() < 0.5, "fr {}", cells[1].2);
        assert!((cells[2].2 - 200.0).abs() < 0.5, "fr {}", cells[2].2);
    }

    #[test]
    fn unresolvable_grid_tracks_do_not_create_columns() {
        // `var(--missing)` must not leave an invisible 0px column behind.
        let layout = build(
            "<body><div class=\"g\"><p>a</p><p>b</p></div></body>",
            "body{margin:0} .g{display:grid; grid-template-columns:var(--nope)}",
            400.0,
        );
        let cells = grid_cells(&layout);
        assert_eq!(cells.len(), 2);
        assert!((cells[0].2 - 400.0).abs() < 0.5, "single column {}", cells[0].2);
        assert!(cells[1].1 > cells[0].1, "second item on a new row");
    }

    #[test]
    fn grid_align_items_centers_the_item_in_its_row() {
        let layout = build(
            "<body><div class=\"g\"><p class=\"short\">a</p><p class=\"tall\">b</p></div></body>",
            "body{margin:0} p{margin:0} .g{display:grid; grid-template-columns:1fr 1fr; align-items:center} .tall{height:100px}",
            400.0,
        );
        let cells = grid_cells(&layout);
        assert_eq!(cells.len(), 2);
        assert!(cells[0].1 > cells[1].1 + 20.0, "centered: {}", cells[0].1);
    }

    #[test]
    fn grid_stretch_fills_the_row_height() {
        let layout = build(
            "<body><div class=\"g\"><p class=\"short\">a</p><p class=\"tall\">b</p></div></body>",
            "body{margin:0} p{margin:0} .g{display:grid; grid-template-columns:1fr 1fr; align-items:stretch} .tall{height:100px}",
            400.0,
        );
        let cells = grid_cells(&layout);
        assert!((cells[0].3 - 100.0).abs() < 0.5, "stretched height {}", cells[0].3);
    }

    #[test]
    fn inline_svg_is_laid_out_and_records_its_paths() {
        let layout = build(
            "<body><p><span class=\"icon\"><svg viewbox=\"0 0 24 24\"><path fill=\"#e00\" d=\"M2 2h20v20H2z\"/></svg></span> texto</p></body>",
            "body{margin:0} .icon{display:inline-block;width:20px;height:20px} svg{display:block;width:100%;height:100%}",
            400.0,
        );
        let svg = layout.blocks.iter().find(|b| b.tag == "svg").expect("svg box");
        // `width: 100%` of a 20px icon is 20px, not the viewBox extent.
        assert!((svg.rect.w - 20.0).abs() < 0.01, "width {}", svg.rect.w);
        assert!((svg.rect.h - 20.0).abs() < 0.01, "height {}", svg.rect.h);
        assert_eq!(layout.svg_shapes.len(), 1, "one filled path");
        // `#e00` expands to `#ee0000`.
        assert_eq!(layout.svg_shapes[0].color, Color::rgb(0xee, 0, 0));
        assert!((layout.svg_shapes[0].scale.0 - 20.0 / 24.0).abs() < 0.01);
    }

    #[test]
    fn svg_without_size_uses_the_viewbox_ratio() {
        let layout = build(
            "<body><svg viewbox=\"0 0 272 92\"><path d=\"M0 0h10v10H0z\"/></svg></body>",
            "body{margin:0}",
            544.0,
        );
        let svg = layout.blocks.iter().find(|b| b.tag == "svg").expect("svg box");
        // No width: it fills the available width; the height follows the ratio.
        assert!((svg.rect.w - 544.0).abs() < 0.5, "width {}", svg.rect.w);
        assert!((svg.rect.h - 544.0 * 92.0 / 272.0).abs() < 0.5, "height {}", svg.rect.h);
    }

    #[test]
    fn svg_current_color_uses_the_element_color() {
        let layout = build(
            "<body><svg width=\"10\" height=\"10\" style=\"color:#0a7d3f\"><path fill=\"currentColor\" d=\"M0 0h10v10H0z\"/></svg></body>",
            "body{margin:0}",
            100.0,
        );
        assert_eq!(layout.svg_shapes.len(), 1);
        assert_eq!(layout.svg_shapes[0].color, Color::rgb(0x0a, 0x7d, 0x3f));
    }

    #[test]
    fn flex_column_centers_items_on_the_cross_axis() {
        let layout = build(
            "<body><div class=\"c\"><div class=\"logo\"></div></div></body>",
            "body{margin:0} .c{display:flex; flex-direction:column; align-items:center} .logo{width:100px; height:20px}",
            400.0,
        );
        let logo = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div" && (b.rect.w - 100.0).abs() < 0.01)
            .expect("logo box");
        assert!((logo.rect.x - 150.0).abs() < 0.01, "centered at {}", logo.rect.x);
    }

    #[test]
    fn flex_row_max_content_adds_items_up() {
        // An auto-width row of two 50px items measures 100, not 50.
        let layout = build(
            "<body><div class=\"wrap\"><div class=\"row\"><span class=\"a\">a</span><span class=\"b\">b</span></div></div></body>",
            "body{margin:0} .row{display:flex} .a{flex:1 1 0%; width:50px} .b{flex:1 1 0%; width:50px} .wrap{display:inline-block}",
            600.0,
        );
        let row = layout.blocks.iter().find(|b| b.tag == "div" && b.rect.y >= 0.0).expect("row");
        assert!(row.rect.w >= 99.0, "row width {}", row.rect.w);
    }

    #[test]
    fn absolute_box_without_width_is_shrink_to_fit() {
        let layout = build(
            "<body><div class=\"rel\"><div class=\"icon\"></div></div></body>",
            "body{margin:0} .rel{position:relative; height:100px} .icon{position:absolute; top:0; left:0; width:24px; height:24px}",
            400.0,
        );
        let icon = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div" && (b.rect.h - 24.0).abs() < 0.01)
            .expect("icon box");
        assert!((icon.rect.w - 24.0).abs() < 0.01, "width {}", icon.rect.w);
    }

    #[test]
    fn visibility_hidden_keeps_the_box_and_skips_the_paint() {
        let layout = build(
            "<body><p>visivel</p><p class=\"gone\">escondido</p></body>",
            "body{margin:0} .gone{visibility:hidden}",
            400.0,
        );
        let runs: Vec<&TextRun> = layout.text_runs.iter().collect();
        let visible = runs.iter().find(|r| r.text(&layout) == "visivel").expect("visible run");
        let hidden = runs.iter().find(|r| r.text(&layout) == "escondido").expect("hidden run");
        assert!(visible.paint);
        assert!(!hidden.paint, "visibility:hidden is not painted");
        // The box still takes vertical space.
        let ps: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "p").collect();
        assert_eq!(ps.len(), 2);
        assert!(ps[1].rect.y > ps[0].rect.y);
    }

    #[test]
    fn opacity_zero_is_not_painted() {
        let layout = build(
            "<body><p class=\"zero\">nada</p></body>",
            "body{margin:0} .zero{opacity:0}",
            400.0,
        );
        assert!(!layout.text_runs[0].paint);
        assert_eq!(layout.text_runs[0].opacity, 0.0);
    }

    #[test]
    fn centered_line_aligns_its_atoms_together() {
        // Two centred inline controls must sit side by side, not on top of
        // each other: the line is centred as a whole.
        let layout = build(
            "<body><center><input class=\"a\"><input class=\"b\"></center></body>",
            "body{margin:0} center{display:block;text-align:center} input{width:150px;height:36px}",
            1000.0,
        );
        let inputs: Vec<&BlockBox> = layout.blocks.iter().filter(|b| b.tag == "input").collect();
        assert_eq!(inputs.len(), 2);
        // No whitespace between the controls, so they are adjacent.
        assert!(inputs[1].rect.x >= inputs[0].rect.right() - 0.01, "second after first");
        // The pair is centred: equal space on both sides.
        let left = inputs[0].rect.x;
        let right = 1000.0 - inputs[1].rect.right();
        assert!((left - right).abs() < 1.0, "left {left} right {right}");
    }

    #[test]
    fn justify_items_centers_a_grid_item() {
        let layout = build(
            "<body><div class=\"g\"><div class=\"logo\"></div></div></body>",
            "body{margin:0} .g{display:grid; justify-items:center; grid-template-columns:1fr} .logo{width:100px; height:20px}",
            400.0,
        );
        let logo = layout
            .blocks
            .iter()
            .find(|b| b.tag == "div" && (b.rect.w - 100.0).abs() < 0.01)
            .expect("logo box");
        assert!((logo.rect.x - 150.0).abs() < 0.01, "centered at {}", logo.rect.x);
    }

    #[test]
    fn a_flex_row_of_percentage_width_svg_keeps_its_intrinsic_size() {
        // A `width:100%` SVG inside an auto-width flex column contributes its
        // viewBox extent, so `align-items: center` can shrink-wrap it.
        let layout = build(
            "<body><div class=\"c\"><svg class=\"logo\" viewbox=\"0 0 272 92\" width=\"272\" height=\"92\"><path d=\"M0 0h10v10H0z\"/></svg></div></body>",
            "body{margin:0} .c{display:flex; flex-direction:column; align-items:center}",
            1000.0,
        );
        let svg = layout.blocks.iter().find(|b| b.tag == "svg").expect("svg box");
        assert!((svg.rect.w - 272.0).abs() < 0.5, "width {}", svg.rect.w);
        assert!((svg.rect.x - 364.0).abs() < 1.0, "centered at {}", svg.rect.x);
    }

    #[test]
    fn block_background_and_padding_are_applied() {
        let layout = build(
            "<body><div class=\"hero\">x</div></body>",
            "body{margin:0} .hero{background-color:#101418; padding:10px}",
            300.0,
        );
        let hero = layout.blocks.iter().find(|b| b.tag == "div").unwrap();
        assert_eq!(hero.background, Color::rgb(0x10, 0x14, 0x18));
        assert_eq!(hero.rect.x, 0.0);
        assert_eq!(hero.rect.w, 300.0);
        assert!(hero.rect.h >= 20.0);
    }

    #[test]
    fn shadow_content_is_laid_out_instead_of_the_light_children() {
        let mut doc = Document::parse(
            "<body><div id=\"host\"><p class=\"light\">luz</p></div></body>",
        );
        let host = (0..doc.nodes.len())
            .find(|&n| doc.attr(n, "id") == Some("host"))
            .expect("host");
        let root = doc.attach_shadow(host, naro_html::ShadowMode::Open);
        let inner = doc.push_detached_element("p".into(), root);
        doc.attach(root, inner);
        doc.set_attr(inner, "class", "shadow");
        doc.push_text_node(inner, "sombra");

        // The document sheet does not reach inside the tree, so the reset for
        // the shadow paragraph has to come from the tree's own sheet.
        let author = parse_stylesheet("body{margin:0}");
        let mut shadow = HashMap::new();
        shadow.insert(root, parse_stylesheet("p{margin:0}"));
        let styles = naro_css::compute_styles_scoped(
            &doc,
            &author,
            &shadow,
            naro_css::StyleContext::default(),
        );
        let layout = layout(&doc, &styles, &MockMeasure, 400.0, &HashMap::new());

        // Only the shadow paragraph is composed: the light one is not shown,
        // and the shadow root itself is not a box.
        let paragraphs: Vec<&BlockBox> =
            layout.blocks.iter().filter(|b| b.tag == "p").collect();
        assert_eq!(paragraphs.len(), 1, "one paragraph on screen");
        assert_eq!(paragraphs[0].node, inner);
        assert!(!layout.blocks.iter().any(|b| b.tag == "p" && b.node != inner));
        // The host takes the height of the composed content.
        let host_box = layout
            .blocks
            .iter()
            .find(|b| b.node == host)
            .expect("host box");
        assert!(
            (host_box.rect.h - paragraphs[0].rect.h).abs() < 0.5,
            "host {} vs paragraph {}",
            host_box.rect.h,
            paragraphs[0].rect.h
        );
        assert_eq!(layout.text_runs.len(), 1);
        assert_eq!(layout.text_runs[0].text(&layout), "sombra");
    }
}
