//! The browser pipeline: navigation state, document loading, layout and paint.

use naro_css::{
    compute_styles_scoped, compute_styles_with, geometry_changed, parse_stylesheet_for, Color,
    ComputedStyle, ParseReport, StyleContext,
};
use naro_html::{escape_html, Document, NodeData, NodeId};
use naro_layout::{layout_with_viewport as layout_document, BlockBox, Layout, Rect};

/// Parse the document sheet plus one sheet per shadow root, compute styles,
/// and absolutize background images against `url`.
///
/// Shadow trees get their own sheet because their `<style>` elements are not
/// reachable from the document tree, and because their rules must not leak out
/// of the tree (only `:host` reaches the host).
fn compute_page_styles(
    doc: &Document,
    url: &str,
    viewport_width: f32,
    context: StyleContext,
    cache: &naro_css::SheetCache,
) -> (Vec<ComputedStyle>, ParseReport) {
    let author = cache.parse(&doc.collect_style_text(), viewport_width);
    let mut report = author.report.clone();
    let mut shadow = std::collections::HashMap::new();
    for (_, root) in &doc.shadow_roots {
        let sheet = cache.parse(&doc.collect_style_text_from(*root), viewport_width);
        report.rules += sheet.report.rules;
        report.interactive += sheet.report.interactive;
        report.unsupported_pseudo += sheet.report.unsupported_pseudo;
        report.skipped.extend(sheet.report.skipped.iter().cloned());
        shadow.insert(*root, (*sheet).clone());
    }
    let mut styles = compute_styles_scoped(doc, author.as_ref(), &shadow, context);
    for style in styles.iter_mut() {
        if let Some(bg) = style.background_image.clone() {
            if let Some(abs) = resolve(url, &bg) {
                style.background_image = Some(abs);
            }
        }
    }
    (styles, report)
}


use naro_net::{normalize_input, resolve, FetchedPage};
use naro_image::{decode, SharedImage};
use naro_js::{ElementBox, Invalidation, ScriptOutcome};
use naro_paint::Canvas;
use std::collections::HashMap;
use naro_text::{FontSet, TextMeasure};

pub const START_URL: &str = "naro://start";
/// Images kept decoded per session; bounded so long browsing cannot grow it.
const MAX_IMAGES: usize = 48;

pub const START_HTML: &str = r#"<!doctype html>
<html><head><title>NARO — Início</title>
<style>
body { margin: 0; background-color: #f7f8fa; color: #14161a; font-size: 16px; }
.hero { background-color: #101418; color: #ffffff; padding: 48px 56px; }
.hero h1 { font-size: 44px; margin: 0 0 8px 0; font-weight: 700; }
.hero p { margin: 0; color: #c7ccd4; font-size: 18px; }
.content { padding: 32px 56px; }
h2 { font-size: 20px; margin: 24px 0 8px 0; }
p { margin: 10px 0; line-height: 1.5; }
a { color: #1a56db; }
.badge { color: #0a7d3f; font-weight: 700; }
hr { border: 1px solid #d9dde4; margin: 24px 0; }
</style></head>
<body>
<div class="hero"><h1>NARO</h1><p>Navegador experimental escrito em Rust. Motor próprio, sem WebView.</p></div>
<div class="content">
<p class="badge">Primeiro fluxo funcional</p>
<h2>O que já funciona</h2>
<p>HTML básico, CSS essencial, layout de blocos e texto, pintura em software, navegação pela barra de endereços, histórico e recarregamento.</p>
<hr>
<h2>Experimente</h2>
<p>Digite um endereço na barra acima ou abra <a href="https://example.com">example.com</a>.</p>
<p>Atalhos: Ctrl+L foca a barra, Enter navega, Alt+←/→ histórico, F5 recarrega.</p>
</div>
</body></html>
"#;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Loading(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Navigation {
    pub generation: u64,
    pub fetch: Option<String>,
}

/// A navigation a form asked for.
///
/// `GET` carries the query in `url`; `POST` carries an
/// `application/x-www-form-urlencoded` body instead, so the two cannot be
/// confused by a caller that only looks at the URL.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingNavigation {
    pub url: String,
    pub method: FormMethod,
    pub body: Option<String>,
}

/// A caret position inside the laid-out text: which run, and which byte of
/// that run.
///
/// The run index, not the node, is what identifies a position: one node's text
/// is split into several runs when it wraps, so a `(node, byte)` pair could
/// name two different places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPos {
    pub run: usize,
    pub byte: usize,
    /// DOM node the run belongs to.
    pub node: NodeId,
}

/// A text selection, anchor to focus.
///
/// Kept as two positions rather than a range so a drag in either direction
/// works, and so `getSelection()` can report `anchorNode`/`focusNode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextPos,
    pub focus: TextPos,
}

impl Selection {
    /// The two ends in document order.
    fn ordered(&self) -> (TextPos, TextPos) {
        if (self.anchor.run, self.anchor.byte) <= (self.focus.run, self.focus.byte) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMethod {
    Get,
    Post,
}

impl PendingNavigation {
    pub fn is_post(&self) -> bool {
        self.method == FormMethod::Post
    }
}

pub struct Engine {
    pub fonts: FontSet,
    pub scale: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub doc: Option<Document>,
    pub styles: Vec<ComputedStyle>,
    pub layout: Option<Layout>,
    /// Decoded images keyed by absolute URL; bounded by `MAX_IMAGES`.
    pub images: HashMap<String, SharedImage>,
    pub display_url: String,
    pub page_title: String,
    /// Messages produced by JavaScript during the last document load.
    pub js: ScriptOutcome,
    /// Live JavaScript realm for the current document; dropped on navigation.
    pub js_session: Option<naro_js::JsSession>,
    /// DOM node of the currently focused form control, if any.
    pub focus: Option<usize>,
    /// Caret position inside the focused control's value, in bytes.
    pub caret: usize,
    pub status: Status,
    pub scroll_y: f32,
    pub generation: u64,
    pub history: Vec<String>,
    pub history_index: usize,
    /// What the last parsed author stylesheet accepted, dropped and neutralised.
    pub css_report: naro_css::ParseReport,
    /// Work the last DOM mutation left pending. Consumed by [`Engine::flush`].
    pub dirty: Invalidation,
    /// How many times the cascade was recomputed (diagnostics and tests).
    pub restyle_count: u32,
    /// How many times boxes were rebuilt.
    pub relayout_count: u32,
    /// How many times only the paint attributes were refreshed.
    pub repaint_count: u32,
    /// Navigation a form submission asked for, consumed by the caller that
    /// owns navigation (the window loop or the headless driver).
    pending_nav: Option<PendingNavigation>,
    /// `<select>` whose popup is open, if any. The popup itself is chrome.
    select_open: Option<usize>,
    /// Text selection, if any.
    pub selection: Option<Selection>,
    /// Last parsed `<style>` text, so a load or a restyle that did not change
    /// it does not parse the whole sheet again.
    sheet_cache: naro_css::SheetCache,
}

impl Engine {
    pub fn new(scale: f32) -> Result<Engine, String> {
        let fonts = FontSet::load_system()?;
        let mut engine = Engine {
            fonts,
            scale: if scale > 0.0 { scale } else { 1.0 },
            viewport_width: 1000.0,
            viewport_height: 700.0,
            doc: None,
            styles: Vec::new(),
            layout: None,
            images: HashMap::new(),
            display_url: START_URL.to_string(),
            page_title: String::new(),
            js: ScriptOutcome::default(),
            js_session: None,
            focus: None,
            caret: 0,
            status: Status::Idle,
            scroll_y: 0.0,
            generation: 0,
            history: Vec::new(),
            history_index: 0,
            css_report: naro_css::ParseReport::default(),
            dirty: Invalidation::default(),
            restyle_count: 0,
            relayout_count: 0,
            repaint_count: 0,
            pending_nav: None,
            select_open: None,
            selection: None,
            sheet_cache: naro_css::SheetCache::default(),
        };
        // Callers decide what to load; nothing is parsed or laid out here.
        engine.generation += 1;
        Ok(engine)
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        let width = width.max(1.0);
        let height = height.max(1.0);
        // Layout depends only on the available width: a pure height change
        // (window dragged vertically) needs no re-layout at all.
        let width_changed = (width - self.viewport_width).abs() > 0.5;
        self.viewport_width = width;
        self.viewport_height = height;
        if width_changed {
            self.relayout();
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        if scale > 0.0 {
            self.scale = scale;
        }
    }

    fn relayout(&mut self) {
        if let Some(doc) = &self.doc {
            self.layout = Some(layout_document(
                doc,
                &self.styles,
                &self.fonts,
                self.viewport_width,
                self.viewport_height,
                &self.images,
            ));
            self.scroll_y = self.scroll_y.min(self.max_scroll());
            self.push_geometry();
        }
    }

    /// Hand the measured border boxes to the JavaScript realm, so
    /// `getBoundingClientRect` and the `offset*`/`client*` properties read the
    /// layout that was actually computed instead of re-deriving it from CSS.
    fn push_geometry(&self) {
        let Some(session) = &self.js_session else { return };
        let Some(layout) = &self.layout else { return };
        let node_count = self.doc.as_ref().map_or(0, |d| d.nodes.len());
        session.set_geometry(measure_geometry_nodes(layout, node_count));
        // The same pass that publishes boxes publishes the cascade and the
        // viewport, so `getComputedStyle` and `matchMedia` see exactly what
        // layout and paint used.
        session.set_computed(self.styles.clone());
        session.set_viewport(self.viewport_width, self.viewport_height);
    }

    pub fn navigate(&mut self, url: &str, push_history: bool) -> Navigation {
        self.generation += 1;
        // Timers, listeners, focus and node handles belong to the old document.
        self.js_session = None;
        self.focus = None;
        self.caret = 0;
        let generation = self.generation;

        if push_history {
            self.history.truncate(self.history_index + 1);
            self.history.push(url.to_string());
            self.history_index = self.history.len() - 1;
        }

        let lower = url.to_ascii_lowercase();
        if lower.starts_with("naro://") {
            if url == START_URL {
                self.set_html(START_URL, START_HTML);
            } else {
                self.show_error(url, "página interna desconhecida");
            }
            return Navigation { generation, fetch: None };
        }

        if lower.starts_with("file://") {
            let path = url::Url::parse(url)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let path = match path {
                Some(p) => p,
                None => {
                    self.show_error(url, "caminho de arquivo inválido");
                    return Navigation { generation, fetch: None };
                }
            };
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let html = String::from_utf8_lossy(&bytes).to_string();
                    let resources = local_images(url, &html);
                    self.set_html_with_images(url, &html, &resources);
                }
                Err(err) => self.show_error(url, &format!("não foi possível ler o arquivo: {err}")),
            }
            return Navigation { generation, fetch: None };
        }

        if lower.starts_with("http://") || lower.starts_with("https://") {
            self.status = Status::Loading(url.to_string());
            self.display_url = url.to_string();
            return Navigation { generation, fetch: Some(url.to_string()) };
        }

        self.show_error(url, "esquema de URL não suportado");
        Navigation { generation, fetch: None }
    }

    pub fn apply_fetch(&mut self, generation: u64, result: Result<FetchedPage, String>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(page) => {
                self.set_html_with_images(&page.url, &page.html, &page.images);
                if self.history_index < self.history.len() {
                    self.history[self.history_index] = page.url;
                }
                // A subresource the origin policy refused is a page-level
                // error, not a silent absence: report it where the other load
                // diagnostics land.
                for url in &page.blocked {
                    self.js
                        .errors
                        .push(format!("recurso bloqueado pela política de origem: {url}"));
                }
            }
            Err(message) => {
                let url = self.display_url.clone();
                self.show_error(&url, &message);
            }
        }
    }

    /// Viewport and root font size handed to the cascade, so `vw`/`vh`/`rem`
    /// use the real window instead of the parser defaults.
    fn style_context(&self) -> StyleContext {
        StyleContext {
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            root_font_size: 16.0,
        }
    }

    pub fn set_html(&mut self, url: &str, html: &str) {
        self.set_html_with_images(url, html, &[]);
    }

    /// Parse and lay out a document, registering any pre-fetched image bytes.
    /// `resources` maps absolute URLs to undecoded payloads.
    pub fn set_html_with_images(
        &mut self,
        url: &str,
        html: &str,
        resources: &[(String, Vec<u8>)],
    ) {
        let mut doc = Document::parse(html);
        // Absolutize resource URLs so the pre-fetched payloads (keyed by
        // absolute URL) can be matched back to their DOM nodes.
        for id in 0..doc.nodes.len() {
            let tag = doc.tag(id).unwrap_or("");
            let attr = match tag {
                "img" => "src",
                "script" => "src",
                "link" => "href",
                _ => continue,
            };
            if let Some(value) = doc.attr(id, attr).map(|s| s.to_string()) {
                if let Some(abs) = resolve(url, &value) {
                    doc.set_attr(id, attr, &abs);
                }
            }
        }
        for (abs, bytes) in resources {
            if self.images.len() >= MAX_IMAGES {
                break;
            }
            if self.images.contains_key(abs) {
                continue;
            }
            if let Ok(image) = decode(bytes) {
                self.images.insert(abs.clone(), std::sync::Arc::new(image));
            }
        }

        // A `<meta http-equiv="Content-Security-Policy">` applies to a page
        // that arrived without a header, `file://` included. Read before the
        // document moves into the session.
        let csp = meta_csp(&doc);
        // Run document scripts against the live DOM before styling, so their
        // mutations are visible to the cascade and the layout.
        let scripts = collect_scripts(&doc, resources);
        // Scripts that measure the page need a layout to measure. When the
        // document has scripts, build one from the parsed tree first; it is
        // thrown away once the scripts have run and the real cascade happens.
        let provisional = if scripts.is_empty() {
            None
        } else {
            let (styles, _) = compute_page_styles(
                &doc,
                url,
                self.viewport_width,
                self.style_context(),
                &self.sheet_cache,
            );
            let geometry = measure_geometry(
                &doc,
                &layout_document(
                    &doc,
                    &styles,
                    &self.fonts,
                    self.viewport_width,
                    self.viewport_height,
                    &self.images,
                ),
            );
            // The cascade travels with the boxes: a script that reads
            // `getComputedStyle` during load must see the same values the
            // renderer will use, not the initial ones.
            Some((geometry, styles))
        };
        let width = self.viewport_width;
        let context = self.style_context();
        let fonts = &self.fonts;
        let images = &self.images;
        // Lets the session re-measure after the load handlers ran, so a timer
        // that reads geometry is not looking at the pre-handler tree.
        let mut measure = |doc: &Document| -> (Vec<Option<ElementBox>>, Vec<ComputedStyle>) {
            let (styles, _) = compute_page_styles(doc, url, width, context, &self.sheet_cache);
            let layout = layout_document(doc, &styles, fonts, width, context.viewport_height, images);
            (measure_geometry(doc, &layout), styles)
        };
        let (session, js) = match provisional {
            Some((geometry, styles)) => naro_js::JsSession::start_measured(
                doc,
                &scripts,
                geometry,
                styles,
                Some(&mut measure),
                url,
            ),
            None => naro_js::JsSession::start(doc, &scripts),
        };
        self.js = js;
        self.js_session = session;
        if let Some(session) = &self.js_session {
            session.set_url(&self.display_url);
            session.set_history_index(self.history_index as i64);
            session.set_viewport(self.viewport_width, self.viewport_height);
            session.set_csp(csp);
        }
        // Load-time script may already have asked for a toggle, a submit or a
        // history change; those have to reach the tree before it is styled,
        // because a checkbox that script clicked is styled as checked.
        self.drain_script_ops();
        let doc = match &self.js_session {
            Some(session) => session.snapshot(),
            None => return,
        };

        let (styles, report) = compute_page_styles(
            &doc,
            url,
            self.viewport_width,
            self.style_context(),
            &self.sheet_cache,
        );
        self.css_report = report;
        let layout = layout_document(
            &doc,
            &styles,
            &self.fonts,
            self.viewport_width,
            self.viewport_height,
            &self.images,
        );

        self.page_title = if doc.title.is_empty() {
            url.to_string()
        } else {
            doc.title.clone()
        };
        self.display_url = url.to_string();
        self.scroll_y = 0.0;
        self.status = Status::Idle;
        self.styles = styles;
        self.layout = Some(layout);
        self.doc = Some(doc);
        self.push_geometry();
        // A fresh document starts fully built; nothing is pending.
        self.dirty.clear();
    }

    pub fn show_error(&mut self, url: &str, message: &str) {
        let html = format!(
            r#"<!doctype html><html><head><title>Erro — NARO</title><style>
body {{ margin: 0; background-color: #ffffff; color: #1a1a1a; font-size: 16px; }}
.box {{ margin: 40px; padding: 24px; background-color: #fdecec; border: 1px solid #f3b7b7; color: #8a1f1f; }}
h1 {{ font-size: 24px; margin: 0 0 12px 0; }}
p {{ margin: 8px 0; line-height: 1.5; }}
</style></head><body><div class="box">
<h1>Não foi possível carregar a página</h1>
<p>{}</p>
<p>{}</p>
</div></body></html>"#,
            escape_html(message),
            escape_html(url)
        );
        self.set_html(url, &html);
        self.status = Status::Error(message.to_string());
    }

    pub fn can_back(&self) -> bool {
        self.history_index > 0
    }

    pub fn can_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    pub fn back(&mut self) -> Navigation {
        if self.can_back() {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();
            self.navigate(&url, false)
        } else {
            Navigation { generation: self.generation, fetch: None }
        }
    }

    pub fn forward(&mut self) -> Navigation {
        if self.can_forward() {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();
            self.navigate(&url, false)
        } else {
            Navigation { generation: self.generation, fetch: None }
        }
    }

    pub fn reload(&mut self) -> Navigation {
        let url = self.display_url.clone();
        self.navigate(&url, false)
    }

    pub fn max_scroll(&self) -> f32 {
        match &self.layout {
            Some(layout) => (layout.content_height - self.viewport_height).max(0.0),
            None => 0.0,
        }
    }

    pub fn scroll_by(&mut self, dy: f32) {
        let max = self.max_scroll();
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max);
    }

    /// `page_y` is in document coordinates (already includes the scroll offset).
    pub fn hit_link(&self, page_x: f32, page_y: f32) -> Option<String> {
        let layout = self.layout.as_ref()?;
        for link in &layout.links {
            if link.rect.contains(page_x, page_y) {
                return resolve(&self.display_url, &link.href);
            }
        }
        None
    }

    pub fn paint_page(&mut self, canvas: &mut Canvas<'_>, content: Rect) {
        canvas.set_clip(content);
        let background = match &self.layout {
            Some(layout) => layout.background,
            None => Color::WHITE,
        };
        let background = if background.a == 0 { Color::WHITE } else { background };
        canvas.fill_rect(content.x, content.y, content.w, content.h, background);

        let layout = match &self.layout {
            Some(layout) => layout,
            None => {
                canvas.reset_clip();
                return;
            }
        };

        let scroll = self.scroll_y;
        // Blocks are pushed post-order (children before parents); painting in
        // reverse draws ancestors first so descendants stay on top.
        for block in layout.blocks.iter().rev() {
            let y = block.rect.y - scroll + content.y;
            let x = block.rect.x + content.x;
            // `overflow: hidden/auto/scroll` clips the box to its ancestor clip.
            match block.clip {
                Some(clip) => canvas.set_clip(Rect {
                    x: clip.x + content.x,
                    y: clip.y - scroll + content.y,
                    w: clip.w,
                    h: clip.h,
                }),
                None => canvas.set_clip(content),
            }
            if let Some((dx, dy, blur, color)) = block.box_shadow {
                // Approximate the blur with a few stacked translucent rects.
                let steps = 3;
                for i in 0..steps {
                    let grow = blur * (i as f32 + 1.0) / steps as f32;
                    let a = color.a / (steps + 1) as u8;
                    canvas.fill_round_rect(
                        x + dx - grow,
                        y + dy - grow,
                        block.rect.w + grow * 2.0,
                        block.rect.h + grow * 2.0,
                        block.border_radius + grow,
                        Color { a, ..color },
                    );
                }
            }
            if !block.paint {
                continue;
            }
            if block.background.a > 0 {
                canvas.fill_round_rect(
                    x,
                    y,
                    block.rect.w,
                    block.rect.h,
                    block.border_radius,
                    block.background,
                );
            }
            if let Some(image) = &block.background_image {
                canvas.draw_image(x, y, block.rect.w, block.rect.h, image);
            }
            let b = block.border;
            if b.top > 0.0 {
                canvas.fill_rect(x, y, block.rect.w, b.top, block.border_color);
            }
            if b.bottom > 0.0 {
                canvas.fill_rect(
                    x,
                    y + block.rect.h - b.bottom,
                    block.rect.w,
                    b.bottom,
                    block.border_color,
                );
            }
            if b.left > 0.0 {
                canvas.fill_rect(x, y + b.top, b.left, block.rect.h - b.top - b.bottom, block.border_color);
            }
            if b.right > 0.0 {
                canvas.fill_rect(
                    x + block.rect.w - b.right,
                    y + b.top,
                    b.right,
                    block.rect.h - b.top - b.bottom,
                    block.border_color,
                );
            }
        }
        canvas.set_clip(content);

        // A checked checkbox and a selected radio draw their mark on top of
        // the box the block pass already painted.
        if let Some(doc) = &self.doc {
            for block in &layout.blocks {
                if block.tag != "input" || !block.paint {
                    continue;
                }
                let kind = doc
                    .attr(block.node, "type")
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if kind != "checkbox" && kind != "radio" {
                    continue;
                }
                if doc.attr(block.node, "checked").is_none() {
                    continue;
                }
                let x = block.rect.x + content.x;
                let y = block.rect.y - scroll + content.y;
                let w = block.rect.w;
                let h = block.rect.h;
                if y + h < content.y || y > content.y + content.h {
                    continue;
                }
                if kind == "radio" {
                    // A filled dot, centred, at the same weight as the mark.
                    let radius = (w.min(h) / 2.0 - 3.0).max(1.5);
                    canvas.fill_round_rect(
                        x + w / 2.0 - radius,
                        y + h / 2.0 - radius,
                        radius * 2.0,
                        radius * 2.0,
                        radius,
                        Color::rgb(0x1a, 0x1a, 0x1a),
                    );
                } else {
                    // A tick: two strokes, scaled to the box so a CSS-sized
                    // checkbox still gets a mark that fits.
                    let pad = (w.min(h) * 0.22).max(1.5);
                    let points = [
                        (x + pad, y + h * 0.52),
                        (x + w * 0.42, y + h - pad),
                        (x + w - pad, y + pad),
                    ];
                    canvas.stroke_path(&points, (w.min(h) * 0.14).max(1.2), Color::rgb(0x1a, 0x1a, 0x1a));
                }
            }
        }

        for image in &layout.images {
            let y = image.rect.y - scroll + content.y;
            if y > content.y + content.h || y + image.rect.h < content.y {
                continue;
            }
            canvas.draw_image(
                image.rect.x + content.x,
                y,
                image.rect.w,
                image.rect.h,
                &image.image,
            );
        }

        // Inline SVG sits on top of its box, in page coordinates.
        for shape in &layout.svg_shapes {
            if !shape.paint {
                continue;
            }
            let mut shape = shape.clone();
            shape.origin.0 += content.x;
            shape.origin.1 += content.y - scroll;
            canvas.fill_svg_shape(&shape);
        }

        // The selection highlight goes under the glyphs, so the text stays
        // readable, and it is clipped to the content box like everything else.
        for rect in self.selection_rects() {
            let y = rect.y - scroll + content.y;
            if y + rect.h < content.y || y > content.y + content.h {
                continue;
            }
            canvas.fill_rect(
                rect.x + content.x,
                y,
                rect.w,
                rect.h,
                Color::rgb(0xb3, 0xd4, 0xfc),
            );
        }

        for run in &layout.text_runs {
            if !run.paint {
                continue;
            }
            let baseline = run.baseline - scroll + content.y;
            if baseline < content.y - run.size * 2.0 || baseline > content.y + content.h + run.size * 2.0 {
                continue;
            }
            let text = run.text(layout).to_string();
            canvas.draw_text(
                run.x + content.x,
                baseline,
                &text,
                run.size,
                run.color,
                run.bold,
                &mut self.fonts,
            );
            if run.underline {
                let width = naro_text::TextMeasure::measure(&self.fonts, &text, run.size, run.bold);
                canvas.fill_rect(
                    run.x + content.x,
                    baseline + 1.5,
                    width,
                    1.0,
                    run.color,
                );
            }
        }
        canvas.reset_clip();
    }

    /// The text position nearest `(x, y)` in page coordinates.
    ///
    /// Lines are found by baseline band, then the byte is found by measuring
    /// prefixes with the same font the paint pass used, so the caret lands
    /// between the two characters it was drawn between.
    pub fn hit_text(&self, x: f32, y: f32) -> Option<TextPos> {
        let layout = self.layout.as_ref()?;
        let mut best: Option<(f32, usize)> = None;
        for (index, run) in layout.text_runs.iter().enumerate() {
            if !run.paint {
                continue;
            }
            // A line's band: from the ascender to just below the baseline.
            let top = run.baseline - run.size;
            let bottom = run.baseline + run.size * 0.35;
            if y < top || y > bottom {
                continue;
            }
            let text = run.text(layout);
            let width = self.fonts.measure(text, run.size, run.bold);
            // Distance to the run's horizontal extent; a click to the right of
            // a line still lands at its end.
            let distance = if x < run.x {
                run.x - x
            } else if x > run.x + width {
                x - (run.x + width)
            } else {
                0.0
            };
            let vertical = (y - (run.baseline - run.size * 0.35)).abs();
            let score = distance * 4.0 + vertical;
            if best.map(|(b, _)| score < b).unwrap_or(true) {
                best = Some((score, index));
            }
        }
        let (_, index) = best?;
        let run = &layout.text_runs[index];
        let text = run.text(layout);
        let mut byte = 0usize;
        let mut best_delta = f32::INFINITY;
        for (offset, _) in text.char_indices().chain(std::iter::once((text.len(), '\0'))) {
            let prefix = &text[..offset];
            let width = self.fonts.measure(prefix, run.size, run.bold);
            let delta = (run.x + width - x).abs();
            if delta < best_delta {
                best_delta = delta;
                byte = offset;
            }
        }
        Some(TextPos {
            run: index,
            byte,
            node: run.node,
        })
    }

    /// Start a selection at `(x, y)`, if there is text there.
    pub fn begin_selection(&mut self, x: f32, y: f32) -> bool {
        match self.hit_text(x, y) {
            Some(pos) => {
                self.selection = Some(Selection {
                    anchor: pos,
                    focus: pos,
                });
                self.sync_selection();
                true
            }
            None => {
                self.selection = None;
                self.sync_selection();
                false
            }
        }
    }

    /// Move the focus end of the selection, keeping the anchor.
    pub fn extend_selection(&mut self, x: f32, y: f32) {
        let Some(anchor) = self.selection.map(|s| s.anchor) else {
            return;
        };
        if let Some(pos) = self.hit_text(x, y) {
            self.selection = Some(Selection { anchor, focus: pos });
            self.sync_selection();
        }
    }

    pub fn set_selection(&mut self, anchor: (f32, f32), focus: (f32, f32)) -> bool {
        let Some(start) = self.hit_text(anchor.0, anchor.1) else {
            self.selection = None;
            return false;
        };
        let end = self.hit_text(focus.0, focus.1).unwrap_or(start);
        self.selection = Some(Selection {
            anchor: start,
            focus: end,
        });
        self.sync_selection();
        true
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.sync_selection();
    }

    /// Publish the selected text to script, so `getSelection()` agrees with
    /// what is painted.
    fn sync_selection(&self) {
        if let Some(session) = &self.js_session {
            session.set_selection_text(&self.selection_text());
        }
    }

    /// The selected text, runs joined in document order.
    pub fn selection_text(&self) -> String {
        let Some(selection) = self.selection else {
            return String::new();
        };
        let Some(layout) = self.layout.as_ref() else {
            return String::new();
        };
        let (start, end) = selection.ordered();
        let mut out = String::new();
        for index in start.run..=end.run.min(layout.text_runs.len().saturating_sub(1)) {
            let run = &layout.text_runs[index];
            let text = run.text(layout);
            let from = if index == start.run { start.byte } else { 0 };
            let to = if index == end.run { end.byte } else { text.len() };
            let from = from.min(text.len());
            let to = to.min(text.len()).max(from);
            if !out.is_empty() && from == 0 && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(&text[from..to]);
        }
        out
    }

    /// One rectangle per selected run segment, in page coordinates.
    pub fn selection_rects(&self) -> Vec<Rect> {
        let Some(selection) = self.selection else {
            return Vec::new();
        };
        let Some(layout) = self.layout.as_ref() else {
            return Vec::new();
        };
        let (start, end) = selection.ordered();
        let mut out = Vec::new();
        for index in start.run..=end.run.min(layout.text_runs.len().saturating_sub(1)) {
            let run = &layout.text_runs[index];
            let text = run.text(layout);
            let from = if index == start.run { start.byte } else { 0 };
            let to = if index == end.run { end.byte } else { text.len() };
            let from = from.min(text.len());
            let to = to.min(text.len()).max(from);
            if from == to {
                continue;
            }
            let x0 = run.x + self.fonts.measure(&text[..from], run.size, run.bold);
            let x1 = run.x + self.fonts.measure(&text[..to], run.size, run.bold);
            out.push(Rect {
                x: x0,
                y: run.baseline - run.size * 0.95,
                w: (x1 - x0).max(1.0),
                h: run.size * 1.2,
            });
        }
        out
    }

    /// Deepest laid-out element under `(x, y)` in page coordinates.
    pub fn hit_node(&self, page_x: f32, page_y: f32) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let mut best: Option<&BlockBox> = None;
        for block in &layout.blocks {
            let area = block.rect.w * block.rect.h;
            if block.rect.contains(page_x, page_y)
                && best.map(|b| area < b.rect.w * b.rect.h).unwrap_or(true)
            {
                best = Some(block);
            }
        }
        best.map(|b| b.node)
    }

    /// Dispatch a click at page coordinates. Returns the JS outcome and
    /// whether any handler ran (in which case the click is consumed).
    pub fn click_at(&mut self, page_x: f32, page_y: f32) -> (ScriptOutcome, bool) {
        let node = match self.hit_node(page_x, page_y) {
            Some(node) => node,
            None => {
                // A click outside any box leaves the control, as in a browser.
                self.blur();
                return (ScriptOutcome::default(), false);
            }
        };
        // Form controls take focus before the click is dispatched.
        let is_control = match self.doc.as_ref().and_then(|d| d.tag(node)) {
            Some("input") | Some("textarea") | Some("button") | Some("select") => {
                self.focus_node(node);
                true
            }
            _ => false,
        };
        if !is_control && self.focus.is_some() {
            self.blur();
        }
        let mut out = ScriptOutcome::default();
        let Some(session) = &self.js_session else {
            return (out, is_control);
        };
        let (mut dispatched, called) = session.dispatch(node, "click");
        out.merge(std::mem::take(&mut dispatched));
        out.merge(session.drain_microtasks());
        out.merge(session.run_timers(20));
        self.flush();
        // Default actions run after the handlers, and only when no handler
        // took the click.
        if called == 0 {
            self.default_click(node);
        }
        (out, called > 0 || is_control)
    }

    /// What a click does when no handler took it: flip a checkbox, check a
    /// radio, submit a form, activate a label, or open a select.
    ///
    /// Shared by the real click path and by `element.click()` from script, so
    /// the two cannot diverge.
    pub fn default_click(&mut self, node: usize) {
        if self.is_toggle_control(node) {
            self.toggle_control(node);
            return;
        }
        if self.is_submit_control(node) {
            self.submit_from(node);
            return;
        }
        if let Some(target) = self.label_target(node) {
            // Clicking a `<label>` activates the control it names, which is
            // the only way a bare `<label for>` works.
            self.focus_node(target);
            if self.is_toggle_control(target) {
                self.toggle_control(target);
            }
            return;
        }
        if self.doc.as_ref().and_then(|d| d.tag(node)) == Some("select") {
            self.open_select(node);
        }
    }

    /// Run pending JavaScript work and apply the resulting invalidation.
    ///
    /// Returns `true` when something ran or the page had to be rebuilt, so the
    /// caller knows a redraw is needed.
    pub fn pump_js(&mut self, budget_ms: i64) -> bool {
        let has_timers = self
            .js_session
            .as_ref()
            .map(|s| s.has_pending_timers())
            .unwrap_or(false);
        // Mutations from an earlier task can leave work pending even when no
        // timer is queued: flush them instead of waiting for the next event.
        if !has_timers && self.pending_invalidation().is_empty() {
            return false;
        }
        let mut out = ScriptOutcome::default();
        if has_timers {
            if let Some(session) = &self.js_session {
                out.merge(session.run_timers(budget_ms));
                out.merge(session.drain_microtasks());
            }
        }
        let rebuilt = self.flush();
        self.js.logs.extend(out.logs);
        self.js.errors.extend(out.errors);
        rebuilt || has_timers
    }

    /// Invalidation reported by the live JavaScript realm.
    fn pending_invalidation(&self) -> Invalidation {
        let mut invalidation = self.dirty;
        if let Some(session) = &self.js_session {
            invalidation.merge(session.invalidation());
        }
        invalidation
    }

    /// Apply everything the last task invalidated, in dependency order:
    /// style → layout → paint.
    ///
    /// Returns `true` when the document was rebuilt. Called once per task, so
    /// many mutations in the same script cost one rebuild; calling it again
    /// with nothing pending is a no-op, which is what keeps a MutationObserver
    /// callback from looping.
    /// Apply everything script asked the engine to do.
    ///
    /// `history.pushState`, `form.submit()`, `form.reset()` and
    /// `element.click()` are queued because script cannot reach the engine.
    /// The queue is emptied first, so an action that itself dispatches an
    /// event cannot re-enter this loop over the same batch.
    fn drain_script_ops(&mut self) {
        // The live tree is the session's; `self.doc` is the snapshot layout
        // reads. An op that mutates the document must act on a snapshot that
        // exists, and every change is mirrored back into the session so the
        // next script statement sees it.
        if let Some(session) = &self.js_session {
            self.doc = Some(session.snapshot());
        }
        let history = match &self.js_session {
            Some(session) => session.take_history_ops(),
            None => Vec::new(),
        };
        for op in history {
            self.apply_history_op(op);
        }
        let forms = match &self.js_session {
            Some(session) => session.take_form_ops(),
            None => Vec::new(),
        };
        for (node, op) in forms {
            match op {
                naro_js::FormOp::Submit => self.submit_form(node),
                naro_js::FormOp::Reset => self.reset_form(node),
                naro_js::FormOp::OpenSelect => {
                    self.open_select(node);
                }
                naro_js::FormOp::DefaultClick => self.default_click(node),
            }
        }
        // Whatever the ops changed has to reach the live tree, or the next
        // script statement would read the pre-op document.
        self.sync_session();
    }

    /// Hand the engine's document back to the live session.
    ///
    /// The session owns the tree script sees; the engine owns the snapshot it
    /// styles. Every host-side mutation goes through here so the two cannot
    /// drift apart.
    fn sync_session(&self) {
        if let (Some(session), Some(doc)) = (&self.js_session, &self.doc) {
            session.set_document(doc.clone());
        }
    }

    pub fn flush(&mut self) -> bool {
        self.drain_script_ops();
        let mut invalidation = std::mem::take(&mut self.dirty);
        if let Some(session) = &self.js_session {
            invalidation.merge(session.take_invalidation());
        }
        if invalidation.is_empty() {
            return false;
        }

        if invalidation.style {
            let snapshot = match &self.js_session {
                Some(session) => session.snapshot(),
                None => return false,
            };
            let (styles, report) = compute_page_styles(
                &snapshot,
                &self.display_url,
                self.viewport_width,
                self.style_context(),
                &self.sheet_cache,
            );
            self.css_report = report;
            self.restyle_count += 1;
            // A restyle only forces a relayout when the geometry can actually
            // change; `color: red` repaints without touching the boxes.
            if geometry_changed(&self.styles, &styles) {
                invalidation.layout = true;
            }
            self.styles = styles;
            self.doc = Some(snapshot);
        }

        if invalidation.layout {
            self.relayout_count += 1;
            self.relayout();
        } else if invalidation.style {
            // Boxes and runs stay where they are; only their colours and
            // decorations are copied from the new computed styles.
            if let Some(layout) = &mut self.layout {
                layout.refresh_paint(&self.styles, &self.images);
                self.repaint_count += 1;
            }
        }

        if invalidation.layout {
            // Measurements must not lag behind the DOM that produced them.
            self.push_geometry();
        }

        // `paint` needs no work here: the caller repaints the whole frame.
        // Keeping the flag explicit is what lets a future partial repaint be
        // added without touching the invalidation rules.
        true
    }

    /// Apply one queued `history` request: move the index, rewrite the URL.
    ///
    /// No document is fetched and nothing is re-rendered — that is the point
    /// of the History API, and it is what lets a single-page app change the
    /// address bar without losing its state.
    fn apply_history_op(&mut self, op: naro_js::HistoryOp) {
        match op {
            naro_js::HistoryOp::Push { url, replace } => {
                let target = if url.is_empty() {
                    self.display_url.clone()
                } else {
                    url
                };
                if replace {
                    if self.history.is_empty() {
                        self.history.push(target.clone());
                        self.history_index = 0;
                    } else {
                        self.history[self.history_index] = target.clone();
                    }
                } else {
                    self.history.truncate(self.history_index + 1);
                    self.history.push(target.clone());
                    self.history_index = self.history.len() - 1;
                }
                self.display_url = target;
            }
            naro_js::HistoryOp::Back => {
                if self.history_index > 0 {
                    self.history_index -= 1;
                }
            }
            naro_js::HistoryOp::Forward => {
                if self.history_index + 1 < self.history.len() {
                    self.history_index += 1;
                }
            }
            naro_js::HistoryOp::Go(delta) => {
                let last = self.history.len().saturating_sub(1);
                let target = (self.history_index as i64 + delta).max(0) as usize;
                self.history_index = target.min(last);
            }
        }
        if let Some(url) = self.history.get(self.history_index) {
            self.display_url = url.clone();
        }
        if let Some(session) = &self.js_session {
            session.set_history_index(self.history_index as i64);
            session.set_url(&self.display_url);
        }
    }

    fn control_value(&self, node: usize) -> String {
        match &self.doc {
            Some(doc) => {
                if let Some(v) = doc.attr(node, "value") {
                    return v.to_string();
                }
                // Fresh controls start empty; `textarea` uses its child text.
                let mut text = String::new();
                for &child in doc.children(node) {
                    if let Some(t) = doc.text(child) {
                        text.push_str(t);
                    }
                }
                text
            }
            None => String::new(),
        }
    }

    fn set_control_value(&mut self, node: usize, value: &str) {
        if let Some(doc) = &mut self.doc {
            doc.set_attr(node, "value", value);
        }
        if let Some(session) = &self.js_session {
            session.set_attribute(node, "value", value);
        }
        // The typed text is painted inside a fixed-size control: no restyle,
        // no relayout, only a new frame.
        self.dirty.mark_paint();
    }

    fn emit(&mut self, node: usize, event: &str) {
        let mut out = ScriptOutcome::default();
        if let Some(session) = &self.js_session {
            let (mut dispatched, _) = session.dispatch(node, event);
            out.merge(std::mem::take(&mut dispatched));
            out.merge(session.drain_microtasks());
            out.merge(session.run_timers(10));
        }
        self.js.logs.extend(out.logs);
        self.js.errors.extend(out.errors);
        self.flush();
    }

    /// Focus a control, firing `blur` on the previous one and `focus` here.
    pub fn focus_node(&mut self, node: usize) {
        if self.focus == Some(node) {
            return;
        }
        let previous = self.focus;
        // Commit the old control's value before leaving it.
        if let Some(old) = previous {
            if let Some(doc) = &self.doc {
                if matches!(doc.tag(old), Some("input") | Some("textarea")) {
                    self.emit(old, "change");
                }
            }
        }
        if let Some(old) = previous {
            self.emit(old, "blur");
        }
        self.focus = Some(node);
        self.caret = self.control_value(node).len();
        self.emit(node, "focus");
    }

    pub fn blur(&mut self) {
        if let Some(node) = self.focus.take() {
            if let Some(doc) = &self.doc {
                if matches!(doc.tag(node), Some("input") | Some("textarea")) {
                    self.emit(node, "change");
                }
            }
            self.emit(node, "blur");
            self.caret = 0;
        }
    }

    /// Insert text at the caret of the focused control.
    pub fn insert_text(&mut self, text: &str) {
        let node = match self.focus {
            Some(n) => n,
            None => return,
        };
        let mut value = self.control_value(node);
        let at = self.caret.min(value.len());
        if !value.is_char_boundary(at) {
            return;
        }
        value.insert_str(at, text);
        self.caret = at + text.len();
        self.set_control_value(node, &value);
        self.emit(node, "input");
    }

    /// Delete the character before the caret.
    pub fn backspace(&mut self) {
        let node = match self.focus {
            Some(n) => n,
            None => return,
        };
        let value = self.control_value(node);
        if self.caret == 0 || self.caret > value.len() {
            return;
        }
        let start = value[..self.caret].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
        let mut new_value = value.clone();
        new_value.replace_range(start..self.caret, "");
        self.caret = start;
        self.set_control_value(node, &new_value);
        self.emit(node, "input");
    }

    /// Focus the next (or previous) focusable control in document order.
    pub fn focus_step(&mut self, forward: bool) {
        let doc = match &self.doc {
            Some(doc) => doc,
            None => return,
        };
        let mut controls: Vec<usize> = Vec::new();
        for id in 0..doc.nodes.len() {
            let focusable = match doc.tag(id) {
                Some("input") => doc
                    .attr(id, "type")
                    .map(|t| !matches!(t.to_ascii_lowercase().as_str(), "hidden" | "button" | "submit" | "checkbox" | "radio"))
                    .unwrap_or(true),
                Some("textarea") => true,
                Some("button") => true,
                _ => false,
            };
            if focusable && doc.attr(id, "disabled").is_none() {
                controls.push(id);
            }
        }
        if controls.is_empty() {
            return;
        }
        let current = self.focus.and_then(|f| controls.iter().position(|&c| c == f));
        let next = match (current, forward) {
            (Some(i), true) => (i + 1) % controls.len(),
            (Some(i), false) => (i + controls.len() - 1) % controls.len(),
            (None, true) => 0,
            (None, false) => controls.len() - 1,
        };
        self.focus_node(controls[next]);
    }

    /// Navigation the page asked for through a form submission.
    ///
    /// `Engine` never navigates on its own; the window loop and the headless
    /// driver own that, so the request is handed over instead of applied.
    pub fn take_pending_nav(&mut self) -> Option<PendingNavigation> {
        self.pending_nav.take()
    }

    /// Enter on a focused control: submit the enclosing form, as a browser
    /// does for a text field, and click `<button>`.
    pub fn press_enter(&mut self) {
        let Some(node) = self.focus else { return };
        let tag = self.doc.as_ref().and_then(|d| d.tag(node)).map(|t| t.to_string());
        if tag.as_deref() == Some("button") {
            self.emit(node, "click");
            return;
        }
        self.emit(node, "change");
        // A text control inside a form submits it; Enter is the shortcut.
        self.submit_from(node);
    }

    /// Submit the form that contains `node`, if any.
    ///
    /// Builds the successful-control set the way a browser does: only named
    /// controls count, `disabled` ones are skipped, an unchecked checkbox or
    /// radio contributes nothing, a checked one contributes its `value`
    /// (`on` when absent), and a `<select>` contributes its chosen option.
    /// The submit button itself is never included. `method=post` becomes an
    /// `application/x-www-form-urlencoded` body instead of a query.
    fn submit_from(&mut self, node: usize) {
        let Some(doc) = &self.doc else { return };
        let mut current = Some(node);
        let mut form = None;
        while let Some(id) = current {
            if doc.tag(id) == Some("form") {
                form = Some(id);
                break;
            }
            current = doc.node(id).parent;
        }
        let Some(form) = form else { return };
        let method = match doc
            .attr(form, "method")
            .unwrap_or("get")
            .to_ascii_lowercase()
            .as_str()
        {
            "post" => FormMethod::Post,
            _ => FormMethod::Get,
        };
        let action = doc.attr(form, "action").unwrap_or("").trim().to_string();
        let target = if action.is_empty() {
            self.display_url.clone()
        } else {
            match resolve(&self.display_url, &action) {
                Some(url) => url,
                None => self.display_url.clone(),
            }
        };
        let mut query = String::new();
        let mut stack = vec![form];
        while let Some(id) = stack.pop() {
            if doc.attr(id, "disabled").is_some() {
                continue;
            }
            match doc.tag(id) {
                Some("input") => {
                    let kind = doc
                        .attr(id, "type")
                        .unwrap_or("text")
                        .to_ascii_lowercase();
                    // Submit/reset/button/image controls never contribute a
                    // value of their own.
                    let usable = !matches!(
                        kind.as_str(),
                        "submit" | "reset" | "button" | "image" | "file"
                    );
                    if usable {
                        let toggle = matches!(kind.as_str(), "checkbox" | "radio");
                        let checked = doc.attr(id, "checked").is_some();
                        if toggle && !checked {
                            continue;
                        }
                        if let Some(name) = doc.attr(id, "name") {
                            if !name.is_empty() {
                                let value = if toggle {
                                    doc.attr(id, "value").unwrap_or("on").to_string()
                                } else {
                                    doc.attr(id, "value").unwrap_or("").to_string()
                                };
                                push_query(&mut query, name, &value);
                            }
                        }
                    }
                }
                Some("select") => {
                    if let Some(name) = doc.attr(id, "name") {
                        if !name.is_empty() && doc.attr(id, "disabled").is_none() {
                            let value = self.select_value(id);
                            push_query(&mut query, name, &value);
                        }
                    }
                }
                Some("textarea") => {
                    if let Some(name) = doc.attr(id, "name") {
                        if !name.is_empty() {
                            let value = self.control_value(id);
                            push_query(&mut query, name, &value);
                        }
                    }
                }
                _ => {}
            }
            for &child in doc.children(id).iter().rev() {
                stack.push(child);
            }
        }
        self.pending_nav = Some(match method {
            // A GET replaces the query string; a POST keeps the action's own
            // query and sends the controls in the body.
            FormMethod::Get => {
                let separator = if target.contains('?') { '&' } else { '?' };
                let url = if query.is_empty() {
                    target
                } else {
                    format!("{target}{separator}{query}")
                };
                PendingNavigation {
                    url,
                    method: FormMethod::Get,
                    body: None,
                }
            }
            FormMethod::Post => PendingNavigation {
                url: target,
                method: FormMethod::Post,
                body: Some(query),
            },
        });
    }

    /// True for a control whose click flips its own state.
    fn is_toggle_control(&self, node: usize) -> bool {
        let Some(doc) = &self.doc else { return false };
        if doc.tag(node) != Some("input") {
            return false;
        }
        matches!(
            doc.attr(node, "type").unwrap_or("").to_ascii_lowercase().as_str(),
            "checkbox" | "radio"
        )
    }

    /// Flip a checkbox, or check a radio and uncheck its siblings.
    ///
    /// The state is the `checked` attribute, so the cascade (`:checked`), the
    /// paint pass and script all read the same thing. A radio cannot be
    /// unchecked by clicking it, as in a browser.
    pub fn toggle_control(&mut self, node: usize) {
        // One implementation of the state rule, in `naro-html`: the engine
        // applies it to the live tree when there is a session, and to its own
        // snapshot when there is none.
        let changed = match &self.js_session {
            Some(session) => {
                let changed = session.toggle_checked(node);
                // The session owns the live tree; refresh the snapshot so
                // layout and paint see the same `checked` attribute.
                self.doc = Some(session.snapshot());
                changed
            }
            None => self
                .doc
                .as_mut()
                .map(|doc| doc.toggle_checked(node))
                .unwrap_or(false),
        };
        if !changed {
            return;
        }
        self.dirty.mark_style();
        // Script sees the new state through `input` and `change`, in that
        // order, exactly as a browser does.
        self.emit(node, "input");
        self.emit(node, "change");
    }

    /// Open a `<select>`: returns its option list for the caller to draw.
    ///
    /// The engine owns the geometry and the state; the window loop owns the
    /// popup, because it is chrome and not part of the page.
    pub fn open_select(&mut self, node: usize) -> Vec<(NodeId, String, bool)> {
        let Some(doc) = &self.doc else { return Vec::new() };
        if doc.tag(node) != Some("select") {
            return Vec::new();
        }
        self.focus_node(node);
        self.select_open = Some(node);
        self.select_options(node)
    }

    pub fn close_select(&mut self) {
        self.select_open = None;
    }

    pub fn is_select_open(&self) -> Option<usize> {
        self.select_open
    }

    /// Every option of `node`, with its label and whether it is selected.
    pub fn select_options(&self, node: usize) -> Vec<(NodeId, String, bool)> {
        let Some(doc) = &self.doc else { return Vec::new() };
        let selected = selected_option(doc, node);
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = doc.children(node).iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if doc.tag(id) == Some("option") {
                let label = doc
                    .attr(id, "label")
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| doc.text_content(id).trim().to_string());
                out.push((id, label, Some(id) == selected));
                continue;
            }
            for &child in doc.children(id).iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    /// The box of a `<select>`, so the popup can hang from it.
    pub fn select_rect(&self, node: usize) -> Option<Rect> {
        self.layout
            .as_ref()?
            .blocks
            .iter()
            .find(|b| b.node == node)
            .map(|b| b.rect)
    }

    /// Choose an option: mark it `selected`, drop the others, close, notify.
    pub fn choose_option(&mut self, select: usize, option: NodeId) {
        let Some(doc) = &mut self.doc else { return };
        if doc.tag(select) != Some("select") || doc.tag(option) != Some("option") {
            return;
        }
        let mut options = Vec::new();
        let mut stack: Vec<NodeId> = doc.children(select).iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if doc.tag(id) == Some("option") {
                options.push(id);
                continue;
            }
            for &child in doc.children(id).iter().rev() {
                stack.push(child);
            }
        }
        for id in options {
            if id == option {
                doc.set_attr(id, "selected", "selected");
            } else {
                remove_attr(doc, id, "selected");
            }
        }
        self.select_open = None;
        // A closed select paints the chosen label, which is a layout text
        // run, so the choice needs a relayout and not just a repaint.
        self.dirty.mark_layout();
        // Push the new tree into the session before notifying: `flush` (which
        // `emit` runs) re-reads the session, and would otherwise undo this.
        self.sync_session();
        self.emit(select, "input");
        self.emit(select, "change");
    }

    /// The value a `<select>` would submit.
    pub fn select_value(&self, node: usize) -> String {
        let Some(doc) = &self.doc else { return String::new() };
        match selected_option(doc, node) {
            Some(option) => doc
                .attr(option, "value")
                .map(|v| v.to_string())
                .unwrap_or_else(|| doc.text_content(option).trim().to_string()),
            None => String::new(),
        }
    }

    /// Submit a form from script, bypassing the click path.
    pub fn submit_form(&mut self, form: usize) {
        self.submit_from(form);
    }

    /// Reset a form: every control returns to the value it was parsed with.
    pub fn reset_form(&mut self, form: usize) {
        let Some(doc) = &mut self.doc else { return };
        let mut stack = vec![form];
        while let Some(id) = stack.pop() {
            match doc.tag(id) {
                Some("input") => {
                    let kind = doc.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
                    if matches!(kind.as_str(), "checkbox" | "radio") {
                        if doc.attr(id, "checked-default").is_some() {
                            doc.set_attr(id, "checked", "checked");
                        } else {
                            remove_attr(doc, id, "checked");
                        }
                    } else {
                        let initial = doc
                            .attr(id, "value-default")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        doc.set_attr(id, "value", &initial);
                    }
                }
                Some("textarea") => {
                    let initial = doc
                        .attr(id, "value-default")
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    doc.set_attr(id, "value", &initial);
                }
                Some("select") => {
                    let mut options = Vec::new();
                    let mut inner: Vec<NodeId> = doc.children(id).iter().rev().copied().collect();
                    while let Some(option) = inner.pop() {
                        if doc.tag(option) == Some("option") {
                            options.push(option);
                            continue;
                        }
                        for &child in doc.children(option).iter().rev() {
                            inner.push(child);
                        }
                    }
                    let mut restored = false;
                    for option in options {
                        let initial = doc.attr(option, "selected-default").is_some();
                        if initial && !restored {
                            doc.set_attr(option, "selected", "selected");
                            restored = true;
                        } else {
                            remove_attr(doc, option, "selected");
                        }
                    }
                }
                _ => {}
            }
            for &child in doc.children(id).iter().rev() {
                stack.push(child);
            }
        }
        self.dirty.mark_style();
        self.sync_session();
        self.emit(form, "reset");
    }

    /// Focus the control a `<label for>` points at, and forward the click.
    pub fn label_target(&self, label: usize) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        if doc.tag(label) != Some("label") {
            return None;
        }
        if let Some(target) = doc.attr(label, "for") {
            let wanted = target.to_string();
            for id in 0..doc.nodes.len() {
                if doc.attr(id, "id") == Some(wanted.as_str()) {
                    return Some(id);
                }
            }
            return None;
        }
        // Without `for`, the first control inside the label is the target.
        let mut stack: Vec<NodeId> = doc.children(label).to_vec();
        while let Some(id) = stack.pop() {
            if matches!(
                doc.tag(id),
                Some("input") | Some("select") | Some("textarea") | Some("button")
            ) {
                return Some(id);
            }
            for &child in doc.children(id) {
                stack.push(child);
            }
        }
        None
    }

    /// True when the element is a submit control of a form.
    fn is_submit_control(&self, node: usize) -> bool {
        let Some(doc) = &self.doc else { return false };
        match doc.tag(node) {
            Some("input") => {
                let kind = doc
                    .attr(node, "type")
                    .unwrap_or("text")
                    .to_ascii_lowercase();
                matches!(kind.as_str(), "submit" | "image")
            }
            Some("button") => {
                // A button without `type` inside a form submits it.
                match doc.attr(node, "type") {
                    Some(t) => t.eq_ignore_ascii_case("submit"),
                    None => self.has_form_ancestor(node),
                }
            }
            _ => false,
        }
    }

    fn has_form_ancestor(&self, node: usize) -> bool {
        let Some(doc) = &self.doc else { return false };
        let mut current = doc.node(node).parent;
        while let Some(id) = current {
            if doc.tag(id) == Some("form") {
                return true;
            }
            current = doc.node(id).parent;
        }
        false
    }

    fn dispatch_only(&mut self, node: usize, event: &str) -> (ScriptOutcome, bool) {
        let mut out = ScriptOutcome::default();
        let called = match &self.js_session {
            Some(session) => {
                let (dispatched, called) = session.dispatch(node, event);
                out.merge(dispatched);
                out.merge(session.drain_microtasks());
                called
            }
            None => 0,
        };
        (out, called > 0)
    }

    pub fn control_value_of(&self, node: usize) -> String {
        self.control_value(node)
    }

    /// True when any laid-out text run contains `needle`.
    pub fn has_text(&self, needle: &str) -> bool {
        match &self.layout {
            Some(layout) => layout.text_runs.iter().any(|r| r.text(layout).contains(needle)),
            None => false,
        }
    }

    pub fn status_line(&self) -> String {
        match &self.status {
            Status::Loading(url) => format!("Carregando {url}…"),
            Status::Error(message) => format!("Erro: {message}"),
            Status::Idle => self.display_url.clone(),
        }
    }

    pub fn dump_tree(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("TITLE: {}\n", self.page_title));
        out.push_str(&format!("URL: {}\n", self.display_url));
        out.push_str(&format!("STATUS: {:?}\n", self.status));
        out.push_str(&format!(
            "SYNC: restyle={} relayout={} repaint={}\n",
            self.restyle_count, self.relayout_count, self.repaint_count
        ));
        out.push_str(&format!(
            "CSS: rules={} invalid={} pseudo-element={} unsupported-not={} at-rule={} interactive={} unsupported-pseudo={}\n",
            self.css_report.rules,
            self.css_report.count(naro_css::SkipReason::InvalidSyntax),
            self.css_report.count(naro_css::SkipReason::PseudoElement),
            self.css_report.count(naro_css::SkipReason::UnsupportedNot),
            self.css_report.count(naro_css::SkipReason::UnsupportedAtRule),
            self.css_report.interactive,
            self.css_report.unsupported_pseudo,
        ));
        for line in &self.js.logs {
            out.push_str(&format!("JS LOG: {line}\n"));
        }
        for line in &self.js.errors {
            out.push_str(&format!("JS ERROR: {line}\n"));
        }
        let content_height = self.layout.as_ref().map(|l| l.content_height).unwrap_or(0.0);
        out.push_str(&format!("CONTENT_HEIGHT: {content_height:.2}\n"));

        out.push_str("DOM:\n");
        if let Some(doc) = &self.doc {
            self.dump_node(doc, doc.root, 0, &mut out);
        }

        out.push_str("BLOCKS:\n");
        if let Some(layout) = &self.layout {
            for block in &layout.blocks {
                out.push_str(&format!(
                    "  {}#{} rect=({:.1},{:.1},{:.1},{:.1}) bg={} border={}\n",
                    block.tag,
                    self.doc.as_ref().and_then(|d| d.attr(block.node, "class")).unwrap_or(""),
                    block.rect.x,
                    block.rect.y,
                    block.rect.w,
                    block.rect.h,
                    block.background.to_hex(),
                    block.border.top
                ));
                if let Some(clip) = block.clip {
                    out.push_str(&format!(
                        "      clip=({:.0},{:.0},{:.0},{:.0})\n",
                        clip.x, clip.y, clip.w, clip.h
                    ));
                }
            }
        }

        out.push_str("TEXT:\n");
        if let Some(layout) = &self.layout {
            for image in &layout.images {
                out.push_str(&format!(
                    "  IMG rect=({:.1},{:.1},{:.1},{:.1}) src={}x{}\n",
                    image.rect.x, image.rect.y, image.rect.w, image.rect.h,
                    image.image.width, image.image.height
                ));
            }
            for run in &layout.text_runs {
                out.push_str(&format!(
                    "  ({:.1},{:.1}) \"{}\" size={:.1} color={} link={}\n",
                    run.x,
                    run.baseline,
                    run.text(layout),
                    run.size,
                    run.color.to_hex(),
                    run.link.as_deref().unwrap_or("-")
                ));
            }
        }
        out
    }

    fn dump_node(&self, doc: &Document, id: NodeId, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        match &doc.node(id).data {
            NodeData::Document => out.push_str(&format!("{indent}<document>\n")),
            NodeData::Element { tag, attrs } => {
                let id_attr = doc.attr(id, "id").map(|v| format!("#{v}")).unwrap_or_default();
                let classes = doc
                    .attr(id, "class")
                    .map(|v| {
                        v.split_whitespace()
                            .map(|c| format!(".{c}"))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let _ = attrs;
                out.push_str(&format!("{indent}{tag}{id_attr}{classes}\n"));
            }
            // A shadow root is printed under its host, never as a child; a
            // fragment is not part of the tree at all.
            NodeData::ShadowRoot { .. } | NodeData::Fragment => {}
            NodeData::Text(text) => {
                let trimmed: String = text.chars().take(60).collect();
                out.push_str(&format!("{indent}\"{trimmed}\"\n"));
                return;
            }
        }
        for &child in doc.children(id) {
            self.dump_node(doc, child, depth + 1, out);
        }
        // The shadow tree is not in the host's child list; printing it here
        // keeps the dump faithful to what is actually composed on screen.
        if let Some(root) = doc.shadow_root(id) {
            let mode = match doc.shadow_mode(root) {
                Some(naro_html::ShadowMode::Closed) => "closed",
                _ => "open",
            };
            out.push_str(&format!("{indent}#shadow-root ({mode})\n"));
            for &child in doc.children(root) {
                self.dump_node(doc, child, depth + 2, out);
            }
        }
    }
}

/// Read `<img>` payloads referenced by a local document, bounded like the
/// network path so a huge page cannot stall navigation.
/// `<script>` sources in document order: inline bodies, or external files
/// that were pre-fetched (a missing external script is reported, not ignored).
fn collect_scripts(doc: &Document, resources: &[(String, Vec<u8>)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for id in 0..doc.nodes.len() {
        if doc.tag(id) != Some("script") {
            continue;
        }
        if let Some(kind) = doc.attr(id, "type") {
            let kind = kind.to_ascii_lowercase();
            if !kind.is_empty()
                && !kind.contains("javascript")
                && !kind.contains("ecmascript")
                && !kind.contains("module")
            {
                continue;
            }
        }
        let src = doc.attr(id, "src").unwrap_or("").trim().to_string();
        if src.is_empty() {
            let body: String = doc
                .children(id)
                .iter()
                .filter_map(|&c| doc.text(c))
                .collect();
            if !body.trim().is_empty() {
                out.push((format!("<script> #{}", out.len() + 1), body));
            }
        } else if let Some((url, bytes)) = resources.iter().find(|(u, _)| u == &src) {
            if let Ok(code) = String::from_utf8(bytes.clone()) {
                out.push((url.clone(), code));
            }
        } else {
            out.push((src.clone(), String::new()));
        }
    }
    out
}

fn local_images(base: &str, html: &str) -> Vec<(String, Vec<u8>)> {
    const MAX_IMAGES: usize = 12;
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let doc = Document::parse(html);
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    // `background-image: url(...)` inside inline <style> blocks.
    let mut i = 0usize;
    while let Some(pos) = html[i..].find("url(") {
        let start = i + pos + 4;
        let end = match html[start..].find(')') {
            Some(end) => end,
            None => break,
        };
        let raw = html[start..start + end]
            .trim()
            .trim_matches(|c| c == '"' || c == '\'');
        i = start + end + 1;
        if raw.is_empty() || raw.starts_with("data:") || raw.starts_with('#') {
            continue;
        }
        if out.len() >= MAX_IMAGES {
            break;
        }
        let abs = match resolve(base, raw) {
            Some(abs) => abs,
            None => continue,
        };
        if seen.iter().any(|u| u == &abs) {
            continue;
        }
        seen.push(abs.clone());
        let path = match url::Url::parse(&abs).ok().and_then(|u| u.to_file_path().ok()) {
            Some(p) => p,
            None => continue,
        };
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_BYTES {
                continue;
            }
        }
        if let Ok(bytes) = std::fs::read(&path) {
            out.push((abs, bytes));
        }
    }
    // External scripts must run for `file://` documents too.
    for id in 0..doc.nodes.len() {
        if doc.tag(id) != Some("script") {
            continue;
        }
        let src = match doc.attr(id, "src") {
            Some(src) if !src.trim().is_empty() => src,
            _ => continue,
        };
        let abs = match resolve(base, src) {
            Some(abs) => abs,
            None => continue,
        };
        if seen.iter().any(|u| u == &abs) {
            continue;
        }
        seen.push(abs.clone());
        let path = match url::Url::parse(&abs).ok().and_then(|u| u.to_file_path().ok()) {
            Some(p) => p,
            None => continue,
        };
        if let Ok(bytes) = std::fs::read(&path) {
            out.push((abs, bytes));
        }
    }
    for id in 0..doc.nodes.len() {
        if out.len() >= MAX_IMAGES {
            break;
        }
        if doc.tag(id) != Some("img") {
            continue;
        }
        let src = match doc.attr(id, "src") {
            Some(src) if !src.trim().is_empty() => src,
            _ => continue,
        };
        let abs = match resolve(base, src) {
            Some(abs) => abs,
            None => continue,
        };
        if seen.iter().any(|u| u == &abs) {
            continue;
        }
        seen.push(abs.clone());
        let path = match url::Url::parse(&abs).ok().and_then(|u| u.to_file_path().ok()) {
            Some(p) => p,
            None => continue,
        };
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_BYTES {
                continue;
            }
        }
        if let Ok(bytes) = std::fs::read(&path) {
            out.push((abs, bytes));
        }
    }
    out
}

/// Per-phase timings for one document, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseTimings {
    pub parse_ms: f64,
    /// Parsing the author stylesheet(s). Scales with the number of rules.
    pub css_ms: f64,
    /// The cascade. Scales with rules × nodes.
    pub style_ms: f64,
    pub layout_ms: f64,
}

/// Time the pipeline phases separately so a regression can be attributed.
/// Runs `iters` times and returns the median of each phase.
pub fn bench_phases(fonts: &FontSet, html: &str, width: f32, iters: usize) -> PhaseTimings {
    let iters = iters.max(1);
    let mut parse = Vec::with_capacity(iters);
    let mut css = Vec::with_capacity(iters);
    let mut style = Vec::with_capacity(iters);
    let mut lay = Vec::with_capacity(iters);

    for _ in 0..iters {
        let t = std::time::Instant::now();
        let doc = Document::parse(html);
        parse.push(t.elapsed().as_secs_f64() * 1000.0);

        // Split the CSS parse from the cascade: they scale with different
        // inputs (rules vs. nodes) and are optimized separately.
        let t = std::time::Instant::now();
        let author = parse_stylesheet_for(&doc.collect_style_text(), width);
        css.push(t.elapsed().as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let styles = compute_styles_with(
            &doc,
            &author,
            StyleContext { viewport_width: width, viewport_height: 700.0, root_font_size: 16.0 },
        );
        style.push(t.elapsed().as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let empty = std::collections::HashMap::new();
        let _ = layout_document(&doc, &styles, fonts, width, 700.0, &empty);
        lay.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v[v.len() / 2]
    };
    PhaseTimings {
        parse_ms: median(&mut parse),
        css_ms: median(&mut css),
        style_ms: median(&mut style),
        layout_ms: median(&mut lay),
    }
}

/// Normalize a user-typed address; re-exported so the UI does not depend on
/// `naro-net` directly for this one rule.
/// The CSP a document declares through `<meta http-equiv>`.
fn meta_csp(doc: &Document) -> Option<naro_net::Csp> {
    for id in 0..doc.nodes.len() {
        if doc.tag(id) != Some("meta") {
            continue;
        }
        let equiv = doc
            .attr(id, "http-equiv")
            .unwrap_or("")
            .to_ascii_lowercase();
        if equiv != "content-security-policy" {
            continue;
        }
        if let Some(csp) = doc.attr(id, "content").and_then(naro_net::Csp::parse) {
            return Some(csp);
        }
    }
    None
}

/// Nearest enclosing `<form>` of `node`, if any.
fn enclosing_form(doc: &Document, node: NodeId) -> Option<NodeId> {
    let mut current = doc.node(node).parent;
    while let Some(id) = current {
        if doc.tag(id) == Some("form") {
            return Some(id);
        }
        current = doc.node(id).parent;
    }
    None
}

/// Every radio under `scope` that belongs to the same group as `name`.
fn collect_radios(doc: &Document, scope: NodeId, name: &str, out: &mut Vec<NodeId>) {
    let mut stack = vec![scope];
    while let Some(id) = stack.pop() {
        if doc.tag(id) == Some("input")
            && doc
                .attr(id, "type")
                .map(|t| t.eq_ignore_ascii_case("radio"))
                .unwrap_or(false)
            && doc.attr(id, "name") == Some(name)
        {
            out.push(id);
        }
        for &child in doc.children(id).iter().rev() {
            stack.push(child);
        }
    }
}

/// Drop an attribute from an element, if present.
fn remove_attr(doc: &mut Document, id: NodeId, name: &str) {
    if let NodeData::Element { attrs, .. } = &mut doc.nodes[id].data {
        attrs.retain(|(n, _)| n != name);
    }
}

/// The option a `<select>` currently shows: the last `selected` one, or the
/// first, as a browser does.
fn selected_option(doc: &Document, select: NodeId) -> Option<NodeId> {
    let mut first = None;
    let mut chosen = None;
    let mut stack: Vec<NodeId> = doc.children(select).iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if doc.tag(id) == Some("option") {
            if first.is_none() {
                first = Some(id);
            }
            if doc.attr(id, "selected").is_some() {
                chosen = Some(id);
            }
            continue;
        }
        for &child in doc.children(id).iter().rev() {
            stack.push(child);
        }
    }
    chosen.or(first)
}

/// Append `name=value` to a query string, percent-encoding both sides.
fn push_query(query: &mut String, name: &str, value: &str) {
    if !query.is_empty() {
        query.push('&');
    }
    for byte in name.bytes() {
        push_encoded(query, byte);
    }
    query.push('=');
    for byte in value.bytes() {
        push_encoded(query, byte);
    }
}

fn push_encoded(out: &mut String, byte: u8) {
    let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
    if unreserved {
        out.push(byte as char);
    } else if byte == b' ' {
        // `application/x-www-form-urlencoded` writes a space as `+`.
        out.push('+');
    } else {
        out.push('%');
        out.push_str(&format!("{byte:02X}"));
    }
}

/// Border boxes of every element in `layout`, indexed by `NodeId`.
fn measure_geometry_nodes(layout: &Layout, node_count: usize) -> Vec<Option<ElementBox>> {
    let mut boxes: Vec<Option<ElementBox>> = vec![None; node_count];
    for block in &layout.blocks {
        if block.node >= boxes.len() {
            continue;
        }
        boxes[block.node] = Some(ElementBox {
            x: block.rect.x,
            y: block.rect.y,
            w: block.rect.w,
            h: block.rect.h,
            border_top: block.border.top,
            border_right: block.border.right,
            border_bottom: block.border.bottom,
            border_left: block.border.left,
        });
    }
    boxes
}

fn measure_geometry(doc: &Document, layout: &Layout) -> Vec<Option<ElementBox>> {
    measure_geometry_nodes(layout, doc.nodes.len())
}

pub fn normalize(url: &str) -> String {
    normalize_input(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Option<Engine> {
        let mut engine = Engine::new(1.0).ok()?;
        engine.set_viewport(900.0, 600.0);
        engine.navigate(START_URL, true);
        Some(engine)
    }

    #[test]
    fn start_page_renders_blocks_and_text() {
        let engine = match engine() {
            Some(e) => e,
            None => return,
        };
        assert_eq!(engine.page_title, "NARO — Início");
        let layout = engine.layout.as_ref().unwrap();
        assert!(layout.blocks.iter().any(|b| b.background == Color::rgb(0x10, 0x14, 0x18)));
        assert!(layout.text_runs.iter().any(|r| r.text(layout) == "NARO"));
        assert!(layout
            .text_runs
            .iter()
            .any(|r| r.text(layout).contains("funcional")));
        assert!(layout.content_height > 300.0);
    }

    #[test]
    fn link_hit_test_resolves_against_the_page_url() {
        let engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let layout = engine.layout.as_ref().unwrap();
        let link = layout.links.iter().find(|l| l.href.contains("example.com")).unwrap();
        let x = link.rect.x + link.rect.w / 2.0;
        let y = link.rect.y + link.rect.h / 2.0;
        assert_eq!(engine.hit_link(x, y).as_deref(), Some("https://example.com/"));
        assert_eq!(engine.hit_link(0.0, 0.0), None);
    }

    #[test]
    fn history_moves_back_and_forward() {
        let mut engine = match engine() {
            Some(e) => e,
            None => return,
        };
        assert!(!engine.can_back());
        engine.set_html("https://a.test/", "<html><body><p>a</p></body></html>");
        engine.history.push("https://a.test/".to_string());
        engine.history_index = engine.history.len() - 1;
        assert!(engine.can_back());

        let nav = engine.back();
        assert!(nav.fetch.is_none());
        assert_eq!(engine.display_url, START_URL);
        assert!(engine.can_forward());

        engine.forward();
        assert_eq!(engine.display_url, "https://a.test/");
    }

    #[test]
    fn navigation_drops_the_previous_js_session() {
        let mut engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let html = "<html><body><p id=\"p\">x</p><script>setTimeout(function(){document.title='velho';}, 5);</script></body></html>";
        engine.set_html("https://a.test/", html);
        if engine.js_session.is_none() {
            return; // runtime unavailable in this environment
        }
        assert!(engine.js_session.is_some());
        engine.navigate("https://b.test/", true);
        assert!(engine.js_session.is_none(), "session must not survive navigation");
    }

    #[test]
    fn click_reaches_a_registered_handler() {
        let mut engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let html = "<html><body><div id=\"b\">antes</div><script>document.getElementById('b').addEventListener('click', function(){ document.getElementById('b').textContent = 'depois'; });</script></body></html>";
        engine.set_html("https://a.test/", html);
        if engine.js_session.is_none() {
            return;
        }
        let rect = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.tag == "div")
            .expect("div box")
            .rect;
        let (x, y) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        let node = engine.hit_node(x, y).unwrap_or(0);
        let (_, consumed) = engine.click_at(x, y);
        assert!(consumed, "handler not reached; node {node}");
        let doc = engine.doc.as_ref().unwrap();
        let text = (0..doc.nodes.len())
            .filter(|&i| doc.tag(i) == Some("div"))
            .filter_map(|i| doc.text(doc.children(i)[0]))
            .collect::<String>();
        assert_eq!(text, "depois", "node {node}");
    }

    fn run_html(html: &str) -> Option<Engine> {
        let mut engine = Engine::new(1.0).ok()?;
        engine.set_viewport(800.0, 600.0);
        engine.set_html("https://t.test/", html);
        if engine.js_session.is_none() {
            return None;
        }
        Some(engine)
    }

    fn collect_text(engine: &Engine, tag: &str) -> String {
        let doc = engine.doc.as_ref().unwrap();
        (0..doc.nodes.len())
            .filter(|&i| doc.tag(i) == Some(tag))
            .map(|i| {
                doc.children(i)
                    .iter()
                    .filter_map(|&c| doc.text(c))
                    .collect::<String>()
            })
            .collect()
    }


    // --- Phase 3: dynamic styles and invalidation ------------------------

    /// Colour of the first text run whose text is exactly `text`.
    fn run_color(engine: &Engine, text: &str) -> Option<String> {
        let layout = engine.layout.as_ref()?;
        layout
            .text_runs
            .iter()
            .find(|r| r.text(layout) == text)
            .map(|r| r.color.to_hex())
    }

    /// Click the centre of the first element matching `selector`.
    fn click_selector(engine: &mut Engine, selector: &str) -> bool {
        let node = match naro_css::query_selector(engine.doc.as_ref().unwrap(), selector) {
            Some(node) => node,
            None => return false,
        };
        let rect = match engine
            .layout
            .as_ref()
            .and_then(|l| l.blocks.iter().find(|b| b.node == node))
        {
            Some(block) => block.rect,
            None => return false,
        };
        // Mirror the window loop: the outcome of a click is part of the page's
        // log and error streams.
        let (outcome, consumed) =
            engine.click_at(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        engine.js.logs.extend(outcome.logs);
        engine.js.errors.extend(outcome.errors);
        consumed
    }

    #[test]
    fn class_change_restyles_without_relayout() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} #t{color:#1a1a1a} #t.on{color:#0a7d3f}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').classList.add('on');});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#0a7d3f"));
        assert_eq!(engine.restyle_count, 1, "one restyle for the click");
        assert_eq!(engine.relayout_count, 0, "a colour change keeps the boxes");
        assert_eq!(engine.repaint_count, 1, "paint data was refreshed");
    }

    #[test]
    fn class_removal_restores_the_previous_style() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} #t{color:#1a1a1a} #t.on{color:#0a7d3f}</style></head>\
             <body><p id=\"t\" class=\"on\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').classList.remove('on');});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#0a7d3f"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
    }

    #[test]
    fn attribute_change_reaches_the_cascade() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} [data-state=on]{color:#681da8}</style></head>\
             <body><p id=\"t\" data-state=\"off\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').setAttribute('data-state','on');});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#681da8"));
    }

    #[test]
    fn inline_style_change_applies() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').style.color = '#8a1f1f';});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#8a1f1f"));
        assert_eq!(engine.relayout_count, 0);
    }

    #[test]
    fn camel_case_inline_style_is_applied() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').style.backgroundColor = '#0a7d3f';});</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#b"));
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#t").unwrap();
        assert_eq!(
            engine.doc.as_ref().unwrap().attr(node, "style"),
            Some("background-color:#0a7d3f")
        );
        assert_eq!(
            engine.styles[node].background,
            naro_css::Color::rgb(0x0a, 0x7d, 0x3f)
        );
    }

    #[test]
    fn inserted_style_element_applies() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             var s = document.createElement('style');\
             s.textContent = '#t { color: #b45309 }';\
             document.body.appendChild(s);});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#b45309"));
    }

    #[test]
    fn rewritten_style_element_applies() {
        let Some(mut engine) = run_html(
            "<html><head><style id=\"s\">#t { color: #334155 }</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('s').textContent = '#t { color: #0f766e }';});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#334155"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#0f766e"));
    }

    #[test]
    fn removed_style_element_reverts_the_cascade() {
        let Some(mut engine) = run_html(
            "<html><head><style id=\"s\">#t { color: #0e7490 }</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             var s = document.getElementById('s');\
             s.parentNode.removeChild(s);});</script></body></html>",
        ) else {
            return;
        };
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#0e7490"));
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(run_color(&engine, "alvo").as_deref(), Some("#1a1a1a"));
        // The <style> node is gone, so the stylesheet text no longer has it.
        let text = engine.doc.as_ref().unwrap().collect_style_text();
        assert!(!text.contains("#0e7490"), "stylesheet text: {text:?}");
    }

    #[test]
    fn inserting_and_removing_nodes_relayouts() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0;height:10px}</style></head>\
             <body><p id=\"a\">um</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             var p = document.createElement('p'); p.textContent = 'dois';\
             document.body.appendChild(p);\
             document.body.removeChild(document.getElementById('a'));});</script></body></html>",
        ) else {
            return;
        };
        let before = engine.layout.as_ref().unwrap().blocks.len();
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(engine.relayout_count, 1, "node count changed");
        assert_eq!(engine.layout.as_ref().unwrap().blocks.len(), before);
        assert!(engine.has_text("dois"));
        assert!(!engine.has_text("um"));
    }

    #[test]
    fn mutations_in_one_task_group_into_one_restyle() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} .on{color:#0a7d3f} [data-x]{color:#681da8}</style></head>\
             <body><p id=\"a\">um</p><p id=\"b\">dois</p><p id=\"c\">tres</p>\
             <button id=\"go\">ir</button>\
             <script>document.getElementById('go').addEventListener('click',function(){\
             document.getElementById('a').classList.add('on');\
             document.getElementById('b').style.color = '#8a1f1f';\
             document.getElementById('c').setAttribute('data-x','1');\
             document.getElementById('c').classList.add('on');\
             });</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#go"));
        assert_eq!(engine.restyle_count, 1, "four mutations, one restyle");
        assert_eq!(engine.relayout_count, 0);
        assert_eq!(run_color(&engine, "um").as_deref(), Some("#0a7d3f"));
        assert_eq!(run_color(&engine, "dois").as_deref(), Some("#8a1f1f"));
        assert_eq!(run_color(&engine, "tres").as_deref(), Some("#681da8"));
    }

    #[test]
    fn mutation_observer_delivers_the_changes() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>var seen = [];\
             var o = new MutationObserver(function(rs){ for (var i=0;i<rs.length;i++) seen.push(rs[i].type + ':' + rs[i].attributeName); });\
             o.observe(document.getElementById('t'), { attributes: true });\
             document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').setAttribute('data-x','1');\
             setTimeout(function(){ console.log('seen=' + seen.join(',')); }, 0);\
             });</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#b"));
        assert!(
            engine.js.logs.iter().any(|l| l.contains("seen=attributes:data-x")),
            "logs: {:?}",
            engine.js.logs
        );
    }

    #[test]
    fn observer_callback_mutations_do_not_loop() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} .a{color:#0a7d3f}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>var n = 0;\
             var o = new MutationObserver(function(){ n++; if (n < 3) document.getElementById('t').classList.add('a'); });\
             o.observe(document.getElementById('t'), { attributes: true });\
             document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').setAttribute('data-x','1');\
             setTimeout(function(){ console.log('n=' + n); }, 0);\
             });</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#b"));
        // Three observer rounds, then no more: the chain is bounded.
        assert!(
            engine.js.logs.iter().any(|l| l.contains("n=3")),
            "logs: {:?}",
            engine.js.logs
        );
        assert!(engine.restyle_count <= 2, "restyles: {}", engine.restyle_count);
    }

    #[test]
    fn a_flushed_mutation_does_not_retrigger_itself() {
        let Some(mut engine) = run_html(
            "<html><head><style>p{margin:0} .on{color:#0a7d3f}</style></head>\
             <body><p id=\"t\">alvo</p><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('t').classList.add('on');});</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#b"));
        let after_click = engine.restyle_count;
        // The frame loop calls pump_js every frame: nothing is pending, so it
        // must report "no work" and never restyle again.
        for _ in 0..5 {
            assert!(!engine.pump_js(8), "a settled page is not work");
        }
        assert_eq!(engine.restyle_count, after_click, "no self-retriggering");
        assert!(!engine.flush());
    }

    #[test]
    fn typing_only_repaints() {
        let Some(mut engine) = run_html("<html><body><input id=\"i\"><p>x</p></body></html>")
        else {
            return;
        };
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#i").unwrap();
        engine.focus_node(node);
        let before = (engine.restyle_count, engine.relayout_count, engine.repaint_count);
        engine.insert_text("abc");
        assert_eq!(engine.control_value_of(node), "abc");
        assert_eq!(
            (engine.restyle_count, engine.relayout_count, engine.repaint_count),
            before,
            "typing must not restyle or relayout"
        );
    }

    #[test]
    fn flush_without_work_is_a_noop() {
        let Some(mut engine) = run_html("<html><body><p>x</p></body></html>") else {
            return;
        };
        assert!(!engine.flush(), "nothing is pending after a load");
        assert_eq!(engine.restyle_count, 0);
        assert_eq!(engine.relayout_count, 0);
        assert!(!engine.pump_js(8), "no timers and no invalidation");
    }

    #[test]
    fn pump_js_flushes_pending_mutations_without_timers() {
        let Some(mut engine) = run_html("<html><body><p id=\"t\">x</p></body></html>") else {
            return;
        };
        // Mutate the live DOM the way an event handler would, then let the
        // frame loop apply it without any timer being queued.
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#t").unwrap();
        if let Some(session) = &engine.js_session {
            session.set_attribute(node, "style", "color:#0a7d3f");
            engine.dirty.mark_style();
        }
        assert!(engine.pump_js(8), "a pending mutation is work");
        assert_eq!(engine.restyle_count, 1);
    }


    // --- Stage 4: identity, measurement and DOM APIs ---------------------

    /// Run a script that prints `key=value` pairs, and return its last log.
    fn log_of(engine: &Engine) -> String {
        engine.js.logs.last().cloned().unwrap_or_default()
    }

    #[test]
    fn repeated_queries_return_the_same_object() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"b\"><p id=\"p\">x</p></div>\
             <script>var a = document.querySelector('#b');\
             var b = document.querySelector('#b');\
             var c = document.getElementById('b');\
             var d = document.getElementById('p').parentNode;\
             console.log('same=' + (a === b) + ',' + (a === c) + ',' + (a === d));</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(log_of(&engine).contains("same=true,true,true"), "{}", log_of(&engine));
    }

    #[test]
    fn identity_survives_a_mutation() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"b\">x</div><button id=\"go\">g</button>\
             <script>var a = document.querySelector('#b');\
             document.getElementById('go').addEventListener('click', function(){\
               a.classList.add('on'); a.setAttribute('data-x','1');\
               console.log('kept=' + (a === document.querySelector('#b')));\
             });</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#go"));
        assert!(log_of(&engine).contains("kept=true"), "{}", log_of(&engine));
    }

    #[test]
    fn a_removed_node_keeps_its_identity_and_is_detached() {
        let Some(mut engine) = run_html(
            "<html><body><p id=\"a\">um</p><p id=\"b\">dois</p>\
             <script>var a = document.getElementById('a');\
             a.remove();\
             console.log('gone=' + (document.getElementById('a') === null) +\
               ' tag=' + a.tagName + ' parent=' + (a.parentNode === null) +\
               ' connected=' + a.isConnected);</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(
            log_of(&engine).contains("gone=true tag=P parent=true connected=false"),
            "{}",
            log_of(&engine)
        );
        assert!(!engine.has_text("um"));
        assert!(engine.has_text("dois"));
    }

    #[test]
    fn bounding_rect_matches_the_layout() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}#w{padding:20px}\
             #b{width:200px;height:40px;padding:10px;border:2px solid #000}</style></head>\
             <body><div id=\"w\"><div id=\"b\"></div></div>\
             <script>var r = document.getElementById('b').getBoundingClientRect();\
             var e = document.getElementById('b');\
             console.log('r=' + [r.x,r.y,r.width,r.height,r.right,r.bottom].join(',') +\
               ' off=' + [e.offsetWidth,e.offsetHeight,e.offsetLeft,e.offsetTop].join(',') +\
               ' client=' + [e.clientWidth,e.clientHeight].join(','));</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        // The border box starts at the wrapper's padding edge (20,20); its
        // size includes the 2px borders and 10px padding on each side, so the
        // content is 200x40 while `clientWidth` (padding box) is 220.
        assert!(
            log_of(&engine).contains("r=20,20,224,64,244,84 off=224,64,20,20 client=220,60"),
            "GOT: {}",
            log_of(&engine)
        );
    }

    #[test]
    fn measurements_follow_a_geometry_change() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}#b{width:100px;height:8px}</style></head>\
             <body><div id=\"b\"></div><button id=\"go\">g</button>\
             <script>var b = document.getElementById('b');\
             document.getElementById('go').addEventListener('click', function(){\
               console.log('before=' + b.offsetWidth);\
               b.style.width = '250px';\
             });</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#go"));
        assert_eq!(engine.relayout_count, 1, "a width change relayouts");
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#b").unwrap();
        let block = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.node == node)
            .expect("box");
        assert!((block.rect.w - 250.0).abs() < 0.01, "width {}", block.rect.w);
    }

    #[test]
    fn measurements_follow_node_insertion_and_removal() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}p{height:20px;margin:0}</style></head>\
             <body><p id=\"a\">um</p><p id=\"b\">dois</p><button id=\"go\">g</button>\
             <script>document.getElementById('go').addEventListener('click', function(){\
               var p = document.createElement('p');\
               document.body.insertBefore(p, document.getElementById('a'));\
               document.getElementById('b').remove();\
             });</script></body></html>",
        ) else {
            return;
        };
        let before = engine.layout.as_ref().unwrap().blocks.len();
        assert!(click_selector(&mut engine, "#go"));
        assert_eq!(engine.relayout_count, 1);
        assert_eq!(engine.layout.as_ref().unwrap().blocks.len(), before);
        let a = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#a").unwrap();
        let block = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.node == a)
            .expect("box");
        // The inserted paragraph is empty, so it still takes a line box; `#a`
        // moved down by that height.
        assert!(block.rect.y > 0.0, "y {}", block.rect.y);
        assert!(!engine.has_text("dois"));
    }

    #[test]
    fn dataset_reads_writes_and_deletes() {
        let Some(mut engine) = run_html(
            "<html><body><p id=\"p\" data-user-id=\"7\" data-x=\"a\">x</p>\
             <script>var p = document.getElementById('p');\
             var out = [p.dataset.userId, p.dataset.x];\
             p.dataset.state = 'on';\
             out.push(p.getAttribute('data-state'));\
             p.setAttribute('data-user-id', '9');\
             out.push(p.dataset.userId);\
             delete p.dataset.x;\
             out.push(p.getAttribute('data-x') === null);\
             console.log('ds=' + out.join(','));</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(log_of(&engine).contains("ds=7,a,on,9,true"), "{}", log_of(&engine));
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#p").unwrap();
        assert_eq!(
            engine.doc.as_ref().unwrap().attr(node, "data-user-id"),
            Some("9")
        );
        assert_eq!(engine.doc.as_ref().unwrap().attr(node, "data-x"), None);
    }

    #[test]
    fn create_text_node_and_relationships() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"d\">x</div>\
             <script>var d = document.getElementById('d');\
             var t = document.createTextNode('abc');\
             d.appendChild(t);\
             console.log('t=' + t.nodeType + ',' + (d.lastChild === t) +\
               ',' + (t.parentElement.id === 'd') +\
               ' kids=' + d.childNodes.length + ':' + d.children.length);</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(
            log_of(&engine).contains("t=3,true,true kids=2:0"),
            "{}",
            log_of(&engine)
        );
        assert!(engine.has_text("abc"));
    }

    #[test]
    fn insert_adjacent_html_covers_all_four_positions() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"d\"><p id=\"in\">x</p></div>\
             <script>var d = document.getElementById('d');\
             d.insertAdjacentHTML('beforebegin', '<i id=\"bb\">b</i>');\
             d.insertAdjacentHTML('afterbegin', '<b id=\"ab\">a</b>');\
             d.insertAdjacentHTML('beforeend', '<u id=\"be\">e</u>');\
             d.insertAdjacentHTML('afterend', '<s id=\"ae\">f</s>');\
             var ids = ['bb','ab','be','ae'].map(function (id) {\
               return document.getElementById(id) ? id : 'missing';\
             });\
             console.log('adj=' + ids.join(',') + ' order=' + d.firstChild.id +\
               ',' + d.lastChild.id + ' sib=' + d.previousSibling.id + ',' + d.nextSibling.id);</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(
            log_of(&engine).contains("adj=bb,ab,be,ae order=ab,be sib=bb,ae"),
            "{}",
            log_of(&engine)
        );
        assert!(engine.has_text("a"));
        assert!(engine.has_text("e"));
    }

    #[test]
    fn insert_adjacent_html_ignores_an_unknown_position() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"d\">x</div>\
             <script>var d = document.getElementById('d');\
             d.insertAdjacentHTML('sideways', '<p id=\"bad\">nope</p>');\
             console.log('bad=' + (document.getElementById('bad') === null));</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(log_of(&engine).contains("bad=true"), "{}", log_of(&engine));
        assert!(!engine.has_text("nope"));
    }

    #[test]
    fn matches_and_closest_use_the_css_engine() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"outer\"><p id=\"p\" class=\"a\"><span id=\"s\">x</span></p></div>\
             <script>var s = document.getElementById('s');\
             var p = document.getElementById('p');\
             console.log('m=' + p.matches('div > .a') + ',' + p.matches('div > span') +\
               ',' + p.matches('[class~=a]') +\
               ' c=' + s.closest('p').id + ',' + s.closest('.a').id + ',' + s.closest('#outer').id +\
               ',' + (s.closest('section') === null) + ',' + p.closest('p').id);</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(
            log_of(&engine).contains("m=true,false,true c=p,p,outer,true,p"),
            "{}",
            log_of(&engine)
        );
    }

    #[test]
    fn element_scoped_queries_stay_inside_the_subtree() {
        let Some(mut engine) = run_html(
            "<html><body><div id=\"a\"><p id=\"inner\">x</p></div><p id=\"outer\">y</p>\
             <script>var a = document.getElementById('a');\
             console.log('q=' + a.querySelector('p').id + ',' + a.querySelectorAll('p').length +\
               ',' + document.querySelectorAll('p').length +\
               ',' + (a.querySelector('body') === null));</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        assert!(
            log_of(&engine).contains("q=inner,1,2,true"),
            "{}",
            log_of(&engine)
        );
    }

    #[test]
    fn a_paint_only_change_keeps_the_measurements() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}#b{width:100px;height:10px}</style></head>\
             <body><div id=\"b\"></div><button id=\"go\">g</button>\
             <script>document.getElementById('go').addEventListener('click', function(){\
               document.getElementById('b').style.background = '#0a7d3f';});</script></body></html>",
        ) else {
            return;
        };
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#b").unwrap();
        let width_before = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.node == node)
            .unwrap()
            .rect
            .w;
        assert!(click_selector(&mut engine, "#go"));
        assert_eq!(engine.relayout_count, 0, "a background change keeps geometry");
        let width_after = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.node == node)
            .unwrap()
            .rect
            .w;
        assert_eq!(width_before, width_after);
    }

    // --- Stage 5: layout in the engine (style changes and geometry) ------

    #[test]
    fn a_width_change_relayouts_and_repaints() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}#a{width:100px;height:20px}</style></head>\
             <body><div id=\"a\"></div><button id=\"b\">b</button>\
             <script>document.getElementById('b').addEventListener('click',function(){\
             document.getElementById('a').style.width='250px';});</script></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#b"));
        assert_eq!(engine.relayout_count, 1, "geometry changed");
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#a").unwrap();
        let block = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.node == node)
            .expect("box");
        assert!((block.rect.w - 250.0).abs() < 0.01, "width {}", block.rect.w);
    }

    #[test]
    fn measurements_track_flex_layout() {
        let Some(mut engine) = run_html(
            "<html><head><style>body{margin:0}.row{display:flex}#a{flex:1}#b{flex:1}</style></head>\
             <body><div class=\"row\"><div id=\"a\">a</div><div id=\"b\">b</div></div>\
             <script>var r = document.getElementById('a').getBoundingClientRect();\
             console.log('w=' + r.width + ' x=' + r.x);</script></body></html>",
        ) else {
            return;
        };
        engine.pump_js(8);
        // `run_html` uses an 800px viewport; two equal flex items split it.
        assert!(log_of(&engine).contains("w=400 x=0"), "{}", log_of(&engine));
    }

    #[test]
    fn enter_submits_the_enclosing_form() {
        let Some(mut engine) = run_html(
            "<html><body><form action=\"/search\">\
             <input name=\"hl\" value=\"pt-BR\" type=\"hidden\">\
             <input id=\"q\" name=\"q\" value=\"\">\
             <input type=\"submit\" value=\"ir\">\
             </form></body></html>",
        ) else {
            return;
        };
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "#q").unwrap();
        engine.focus_node(node);
        engine.insert_text("naro web");
        engine.press_enter();
        let nav = engine.take_pending_nav().expect("form submitted");
        assert!(nav.url.starts_with("https://t.test/search?"), "{nav:?}");
        assert!(nav.url.contains("q=naro+web"), "space encoded as +: {nav:?}");
        assert!(nav.url.contains("hl=pt-BR"), "hidden input included: {nav:?}");
        assert!(!nav.url.contains("ir="), "submit button excluded: {nav:?}");
        assert!(engine.take_pending_nav().is_none(), "consumed once");
    }

    #[test]
    fn clicking_a_submit_control_submits_the_form() {
        let Some(mut engine) = run_html(
            "<html><body><form action=\"/search\"><input id=\"q\" name=\"q\" value=\"x\">\
             <input id=\"go\" type=\"submit\" value=\"ir\"></form></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#go"));
        let nav = engine.take_pending_nav().expect("submitted");
        assert!(nav.url.contains("q=x"), "{nav:?}");
    }

    #[test]
    fn a_js_handler_can_cancel_the_submission() {
        let Some(mut engine) = run_html(
            "<html><body><form action=\"/search\"><input name=\"q\" value=\"x\">\
             <input id=\"go\" type=\"submit\" value=\"ir\">\
             <script>document.getElementById('go').addEventListener('click', function (e) {\
               console.log('handled'); });</script></form></body></html>",
        ) else {
            return;
        };
        assert!(click_selector(&mut engine, "#go"));
        assert!(
            engine.take_pending_nav().is_none(),
            "a handler owns the click, so the default action does not run"
        );
    }

    #[test]
    fn google_search_field_accepts_text() {
        let html = match std::fs::read_to_string("../../.tmp/g.html") {
            Ok(h) => h,
            Err(_) => return,
        };
        let mut engine = match Engine::new(1.0) { Ok(e) => e, Err(_) => return };
        engine.set_viewport(1000.0, 700.0);
        engine.set_html("https://www.google.com/", &html);
        if engine.js_session.is_none() { return; }
        let node = naro_css::query_selector(engine.doc.as_ref().unwrap(), "textarea.gLFyf")
            .or_else(|| naro_css::query_selector(engine.doc.as_ref().unwrap(), "textarea"))
            .expect("search field");
        engine.focus_node(node);
        assert_eq!(engine.focus, Some(node), "textarea is focusable");
        engine.insert_text("naro");
        assert_eq!(engine.control_value_of(node), "naro", "value updated");
        // Enter submits the search form of the real page.
        engine.press_enter();
        let nav = engine.take_pending_nav().expect("search submitted");
        assert!(nav.url.starts_with("https://www.google.com/search?"), "{nav:?}");
        assert!(nav.url.contains("q=naro"), "{nav:?}");
    }

    #[test]
    fn infinite_script_is_interrupted() {
        let engine = match run_html(
            "<html><body><p id=\"a\">ok</p><script>while(true){}</script><script>document.getElementById('a').textContent='vivo';</script></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        assert!(engine.js.errors.iter().any(|e| e.contains("interrompido")));
        assert_eq!(collect_text(&engine, "p"), "vivo");
    }

    #[test]
    fn events_bubble_to_ancestors() {
        let mut engine = match run_html(
            "<html><body><div id=\"pai\"><div id=\"filho\">x</div></div><script>document.getElementById('pai').addEventListener('click', function(){ document.getElementById('filho').textContent = 'pai-visto'; });</script></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        let rect = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.tag == "div" && b.rect.h > 0.0)
            .unwrap()
            .rect;
        let (_, consumed) = engine.click_at(rect.x + 2.0, rect.y + 2.0);
        assert!(consumed);
        assert_eq!(collect_text(&engine, "div"), "pai-visto");
    }

    #[test]
    fn stop_propagation_blocks_the_parent() {
        let mut engine = match run_html(
            "<html><body><div id=\"pai\"><div id=\"filho\">x</div></div><script>document.getElementById('pai').addEventListener('click', function(){ document.getElementById('pai').setAttribute('data-hit','1'); }); document.getElementById('filho').addEventListener('click', function(e){ e.stopPropagation(); });</script></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        let rect = engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.tag == "div" && b.rect.h > 0.0)
            .unwrap()
            .rect;
        engine.click_at(rect.x + 2.0, rect.y + 2.0);
        let doc = engine.doc.as_ref().unwrap();
        let pai = (0..doc.nodes.len()).find(|&i| doc.attr(i, "id") == Some("pai")).unwrap();
        assert_eq!(doc.attr(pai, "data-hit"), None);
    }

    #[test]
    fn inner_html_creates_real_elements() {
        let engine = match run_html(
            "<html><body><div id=\"h\"></div><script>document.getElementById('h').innerHTML = '<b>forte</b>';</script></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        assert_eq!(collect_text(&engine, "b"), "forte");
        let layout = engine.layout.as_ref().unwrap();
        assert!(layout.text_runs.iter().any(|r| r.text(layout).contains("forte")));
    }

    fn form_engine(html: &str) -> Option<Engine> {
        let mut engine = Engine::new(1.0).ok()?;
        engine.set_viewport(800.0, 600.0);
        engine.set_html("https://t.test/", html);
        Some(engine)
    }

    fn control_rect(engine: &Engine, tag: &str) -> Rect {
        engine
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.tag == tag && b.rect.h > 0.0)
            .unwrap()
            .rect
    }

    #[test]
    fn typing_updates_the_control_and_fires_input() {
        let mut engine = match form_engine(
            "<html><body><input id=\"i\"><p id=\"e\">x</p><script>document.getElementById('i').addEventListener('input', function(){ document.getElementById('e').textContent = 'v:' + document.getElementById('i').value; });</script></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        let rect = control_rect(&engine, "input");
        engine.click_at(rect.x + 4.0, rect.y + 4.0);
        assert!(engine.focus.is_some(), "click must focus the input");
        engine.insert_text("abc");
        let node = engine.focus.unwrap();
        assert_eq!(engine.control_value_of(node), "abc");
        assert_eq!(collect_text(&engine, "p"), "v:abc");
    }

    #[test]
    fn backspace_and_tab() {
        let mut engine = match form_engine(
            "<html><body><input id=\"a\"><input id=\"b\"></body></html>",
        ) {
            Some(e) => e,
            None => return,
        };
        engine.focus_step(true);
        let first = engine.focus.unwrap();
        engine.insert_text("abc");
        engine.backspace();
        assert_eq!(engine.control_value_of(first), "ab");
        engine.focus_step(true);
        assert_ne!(engine.focus, Some(first));
        engine.backspace();
        assert_eq!(engine.control_value_of(first), "ab");
    }

    #[test]
    fn navigation_clears_focus() {
        let mut engine = match form_engine("<html><body><input id=\"a\"></body></html>") {
            Some(e) => e,
            None => return,
        };
        engine.focus_step(true);
        assert!(engine.focus.is_some());
        engine.navigate("https://other.test/", true);
        assert_eq!(engine.focus, None);
    }

    #[test]
    fn scrolling_is_clamped() {
        let mut engine = match engine() {
            Some(e) => e,
            None => return,
        };
        engine.scroll_by(-10_000.0);
        assert_eq!(engine.scroll_y, 0.0);
        engine.scroll_by(10_000.0);
        assert_eq!(engine.scroll_y, engine.max_scroll());
    }

    #[test]
    fn painting_changes_pixels() {
        let mut engine = match engine() {
            Some(e) => e,
            None => return,
        };
        let mut pixels = vec![0u32; 300 * 200];
        let mut canvas = Canvas::new(&mut pixels, 300, 200, 1.0);
        canvas.clear(Color::WHITE);
        engine.paint_page(&mut canvas, Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 });
        assert!(canvas.pixels.iter().any(|p| *p != 0x00FF_FFFF));
    }
}

