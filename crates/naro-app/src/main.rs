//! NARO window, chrome and event handling.

use std::num::NonZeroU32;
use std::sync::Arc;

use naro_css::Color;
use naro_engine::{Engine, Navigation, Status, START_URL};
use naro_layout::Rect;
use naro_net::{normalize_input, FetchedPage};
use naro_text::TextMeasure;
use naro_paint::Canvas;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

/// Height of the tab strip. This row *is* the window's title bar: the window
/// is created without OS decorations, so the tabs sit where the system title
/// used to be.
const TAB_ROW_H: f32 = 34.0;
const TOOLBAR_H: f32 = 40.0;
const STATUS_H: f32 = 22.0;
const BTN_W: f32 = 34.0;
const BTN_SIZE: f32 = 28.0;
/// Everything above the page content.
const CHROME_H: f32 = TAB_ROW_H + TOOLBAR_H;

const TAB_MIN_W: f32 = 56.0;
const TAB_MAX_W: f32 = 190.0;
const NEW_TAB_W: f32 = 30.0;
const WIN_BTN_W: f32 = 46.0;
/// Grab width of the invisible resize border. The window has no OS frame, so
/// this band is what makes the edges draggable.
const RESIZE_BORDER: f32 = 5.0;

const C_TOOLBAR: Color = Color::rgb(0xee, 0xf0, 0xf4);
const C_BORDER: Color = Color::rgb(0xd5, 0xd9, 0xe0);
const C_TEXT: Color = Color::rgb(0x14, 0x16, 0x1a);
const C_MUTED: Color = Color::rgb(0x8a, 0x90, 0x9a);
const C_DISABLED: Color = Color::rgb(0xa0, 0xa6, 0xb0);
const C_HOVER: Color = Color::rgb(0xe2, 0xe5, 0xea);
const C_ACCENT: Color = Color::rgb(0x1a, 0x56, 0xdb);
const C_FIELD_BORDER: Color = Color::rgb(0xc9, 0xce, 0xd8);
const C_STATUS_BG: Color = Color::rgb(0xf4, 0xf5, 0xf7);
const C_STATUS_TEXT: Color = Color::rgb(0x4a, 0x50, 0x5a);
/// Tab strip, which doubles as the window's title bar.
const C_TABROW: Color = Color::rgb(0xe3, 0xe6, 0xeb);
const C_TAB_HOVER: Color = Color::rgb(0xd7, 0xdb, 0xe2);
const C_WIN_CLOSE_HOVER: Color = Color::rgb(0xe8, 0x11, 0x23);
const C_HOVER_PRESSED: Color = Color::rgb(0xd2, 0xd6, 0xdd);
const C_TOOLTIP_BG: Color = Color::rgb(0x3c, 0x40, 0x48);

struct ChromeLayout {
    /// The tab strip; also the window's drag surface and title bar.
    tab_row: Rect,
    /// One rect per tab, in strip order.
    tabs: Vec<Rect>,
    /// Close button inside each tab, parallel to `tabs`.
    tab_close: Vec<Rect>,
    new_tab: Rect,
    win_min: Rect,
    win_max: Rect,
    win_close: Rect,
    toolbar: Rect,
    back: Rect,
    forward: Rect,
    reload: Rect,
    addr: Rect,
    content: Rect,
    status: Rect,
}

fn chrome_layout(w: f32, h: f32, tab_count: usize) -> ChromeLayout {
    let content_h = (h - CHROME_H - STATUS_H).max(0.0);
    let addr_w = (w - 128.0).max(0.0);

    // Window controls sit at the far right of the tab row, as in Chrome.
    let controls_x = w - 3.0 * WIN_BTN_W;
    let win_min = Rect { x: controls_x, y: 0.0, w: WIN_BTN_W, h: TAB_ROW_H };
    let win_max = Rect { x: controls_x + WIN_BTN_W, y: 0.0, w: WIN_BTN_W, h: TAB_ROW_H };
    let win_close = Rect { x: controls_x + 2.0 * WIN_BTN_W, y: 0.0, w: WIN_BTN_W, h: TAB_ROW_H };

    // Tabs start at the left and shrink as they multiply; the leftover space
    // to the right of the "+" is the drag surface.
    let strip_w = (controls_x - 8.0).max(0.0);
    let n = tab_count.max(1) as f32;
    let tab_w = ((strip_w - NEW_TAB_W - 6.0) / n).clamp(TAB_MIN_W, TAB_MAX_W);
    let mut tabs = Vec::with_capacity(tab_count);
    let mut tab_close = Vec::with_capacity(tab_count);
    for i in 0..tab_count {
        // Derive both edges from the same expression so tab `i`'s right edge
        // is exactly tab `i+1`'s left edge: computing `x + w` separately
        // produced a sub-pixel overlap that broke hit-testing at some widths.
        let x0 = 4.0 + i as f32 * tab_w;
        let x1 = 4.0 + (i + 1) as f32 * tab_w;
        tabs.push(Rect { x: x0, y: 4.0, w: x1 - x0, h: TAB_ROW_H - 4.0 });
        tab_close.push(Rect {
            x: x1 - 24.0,
            y: 4.0 + (TAB_ROW_H - 4.0 - 16.0) / 2.0,
            w: 16.0,
            h: 16.0,
        });
    }
    let new_tab = Rect {
        x: 4.0 + tab_count as f32 * tab_w + 4.0,
        y: 4.0 + (TAB_ROW_H - 4.0 - 24.0) / 2.0,
        w: NEW_TAB_W,
        h: 24.0,
    };

    ChromeLayout {
        tab_row: Rect { x: 0.0, y: 0.0, w, h: TAB_ROW_H },
        tabs,
        tab_close,
        new_tab,
        win_min,
        win_max,
        win_close,
        toolbar: Rect { x: 0.0, y: TAB_ROW_H, w, h: TOOLBAR_H },
        back: Rect { x: 8.0, y: TAB_ROW_H + 6.0, w: BTN_SIZE, h: BTN_SIZE },
        forward: Rect { x: 8.0 + BTN_W, y: TAB_ROW_H + 6.0, w: BTN_SIZE, h: BTN_SIZE },
        reload: Rect { x: 8.0 + 2.0 * BTN_W, y: TAB_ROW_H + 6.0, w: BTN_SIZE, h: BTN_SIZE },
        addr: Rect { x: 116.0, y: TAB_ROW_H + 6.0, w: addr_w, h: BTN_SIZE },
        content: Rect { x: 0.0, y: CHROME_H, w, h: content_h },
        status: Rect { x: 0.0, y: CHROME_H + content_h, w, h: STATUS_H },
    }
}

// --- icon set ----------------------------------------------------------
//
// Every glyph in the chrome is drawn here, from one stroke weight and one
// nominal box, so the toolbar, the tab strip and the window controls read as
// a single interface. Nothing depends on a glyph being present in the system
// font, which is what made the old buttons inconsistent (a typed "←" next to
// a geometric reload arc).

/// Stroke weight of every icon, in logical pixels.
const ICON_STROKE: f32 = 1.4;

/// One icon, drawn centred on a point at a nominal size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Icon {
    Back,
    Forward,
    Reload,
    Close,
    Plus,
    Minimize,
    Maximize,
    Restore,
}

/// A straight anti-aliased line.
fn draw_line(
    canvas: &mut Canvas<'_>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
    color: Color,
) {
    canvas.stroke_path(&[(x0, y0), (x1, y1)], width, color);
}

/// Draw `icon` centred at `(cx, cy)`. `size` is the icon box; every shape is
/// derived from it so one call site can scale the whole set.
///
/// Strokes go through the painter's anti-aliased polyline: the framebuffer
/// snaps `fill_rect` to whole pixels, so stamping squares produced a
/// staircase that read as "pixelated" at 1x.
fn draw_icon(
    canvas: &mut Canvas<'_>,
    icon: Icon,
    cx: f32,
    cy: f32,
    size: f32,
    color: Color,
) {
    let h = size / 2.0;
    match icon {
        // Chevrons span the same vertical extent as the reload circle: the
        // first version drew them at 0.72 of the box, which left them visibly
        // smaller and lighter than the other two icons in the row.
        Icon::Back => {
            canvas.stroke_path(
                &[
                    (cx + h * 0.48, cy - h * 0.80),
                    (cx - h * 0.48, cy),
                    (cx + h * 0.48, cy + h * 0.80),
                ],
                ICON_STROKE,
                color,
            );
        }
        Icon::Forward => {
            canvas.stroke_path(
                &[
                    (cx - h * 0.48, cy - h * 0.80),
                    (cx + h * 0.48, cy),
                    (cx - h * 0.48, cy + h * 0.80),
                ],
                ICON_STROKE,
                color,
            );
        }
        Icon::Reload => {
            // Open circle with the gap at the upper right, closed by an
            // arrowhead — the shape Chrome and Edge use. The arc is sampled
            // into a polyline so it gets the same anti-aliasing as the rest.
            let radius = h * 0.62;
            let mut points: Vec<(f32, f32)> = Vec::with_capacity(40);
            let steps = 40;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let angle = (-55.0 + 300.0 * t).to_radians();
                points.push((cx + radius * angle.cos(), cy + radius * angle.sin()));
            }
            canvas.stroke_path(&points, ICON_STROKE, color);
            // The arrowhead sits on the arc's start and points along its
            // tangent, so it reads as the stroke continuing, not as a blob
            // stuck to the circle.
            let angle = -55.0f32.to_radians();
            let (sin, cos) = angle.sin_cos();
            let tip = (cx + radius * cos, cy + radius * sin);
            let tangent = (-sin, cos);
            let normal = (cos, sin);
            let s = h * 0.46;
            canvas.fill_triangle(
                (
                    tip.0 + tangent.0 * s,
                    tip.1 + tangent.1 * s,
                ),
                (
                    tip.0 + normal.0 * s * 0.62,
                    tip.1 + normal.1 * s * 0.62,
                ),
                (
                    tip.0 - normal.0 * s * 0.62,
                    tip.1 - normal.1 * s * 0.62,
                ),
                color,
            );
        }
        Icon::Close => {
            let d = h * 0.60;
            canvas.stroke_path(
                &[(cx - d, cy - d), (cx + d, cy + d)],
                ICON_STROKE,
                color,
            );
            canvas.stroke_path(
                &[(cx - d, cy + d), (cx + d, cy - d)],
                ICON_STROKE,
                color,
            );
        }
        Icon::Plus => {
            let d = h * 0.70;
            canvas.stroke_path(&[(cx - d, cy), (cx + d, cy)], ICON_STROKE, color);
            canvas.stroke_path(&[(cx, cy - d), (cx, cy + d)], ICON_STROKE, color);
        }
        Icon::Minimize => {
            canvas.stroke_path(
                &[(cx - h * 0.55, cy), (cx + h * 0.55, cy)],
                ICON_STROKE,
                color,
            );
        }
        Icon::Maximize => {
            let d = h * 0.52;
            canvas.stroke_path(
                &[
                    (cx - d, cy - d),
                    (cx + d, cy - d),
                    (cx + d, cy + d),
                    (cx - d, cy + d),
                    (cx - d, cy - d),
                ],
                ICON_STROKE,
                color,
            );
        }
        Icon::Restore => {
            // Two offset squares: the back one peeking out at the top right.
            let d = h * 0.44;
            let front = [
                (cx - d, cy - d * 0.5),
                (cx + d * 0.5, cy - d * 0.5),
                (cx + d * 0.5, cy + d * 1.5),
                (cx - d, cy + d * 1.5),
                (cx - d, cy - d * 0.5),
            ];
            let back = [
                (cx - d * 0.5, cy - d * 1.5),
                (cx + d, cy - d * 1.5),
                (cx + d, cy + d * 0.5),
            ];
            canvas.stroke_path(&back, ICON_STROKE, color);
            canvas.stroke_path(&front, ICON_STROKE, color);
        }
    }
}

/// Draw a rounded hover/press background behind a button.
fn draw_button_bg(canvas: &mut Canvas<'_>, rect: &Rect, fill: Color, radius: f32) {
    canvas.fill_round_rect(rect.x, rect.y, rect.w, rect.h, radius, fill);
}

/// The three toolbar icons, drawn from the same stroke weight and box so the
/// row reads as one control group.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NavIcon {
    Back,
    Forward,
    Reload,
}

impl NavIcon {
    fn icon(self) -> Icon {
        match self {
            NavIcon::Back => Icon::Back,
            NavIcon::Forward => Icon::Forward,
            NavIcon::Reload => Icon::Reload,
        }
    }

    fn label(self) -> &'static str {
        match self {
            NavIcon::Back => "Voltar (Alt+←)",
            NavIcon::Forward => "Avançar (Alt+→)",
            NavIcon::Reload => "Recarregar (F5)",
        }
    }
}

struct Chrome {
    addr: String,
    cursor: usize,
    focused: bool,
    replace_on_type: bool,
}

impl Chrome {
    fn new() -> Chrome {
        let addr = START_URL.to_string();
        Chrome { cursor: addr.len(), addr, focused: false, replace_on_type: false }
    }
}

enum UserEvent {
    /// A fetch finished. `tab` says which tab asked for it: a background tab
    /// must not write into the visible one.
    Fetched { tab: u64, generation: u64, result: Result<FetchedPage, String> },
}

/// A tab that is not on screen.
///
/// The active tab's engine and chrome live directly on [`App`], so the whole
/// event and drawing path keeps working on plain fields; switching tabs swaps
/// the two values in and out instead of going through an index.
struct BackgroundTab {
    id: u64,
    engine: Engine,
    chrome: Chrome,
}

struct App {
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    /// Active tab.
    engine: Engine,
    chrome: Chrome,
    active_id: u64,
    /// Inactive tabs, in strip order.
    background: Vec<BackgroundTab>,
    /// Every tab id, active included, in strip order.
    order: Vec<u64>,
    next_tab_id: u64,
    modifiers: ModifiersState,
    cursor: (f32, f32),
    /// Last press in the drag surface, for double-click-to-maximize.
    last_drag_click: Option<std::time::Instant>,
    /// Set when the last tab was closed; the event loop closes the window.
    close_requested: bool,
    /// Toolbar button held down, for the pressed state.
    pressed_nav: Option<NavIcon>,
    /// New-tab button held down.
    pressed_new_tab: bool,
    /// Toolbar button with keyboard focus (F6 / Tab), for the focus ring.
    toolbar_focus: Option<NavIcon>,
    /// Page coordinates where a selection drag started, while it is running.
    selecting: Option<(f32, f32)>,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

/// Put `text` on the system clipboard.
///
/// A clipboard needs a running desktop session; in a headless or locked
/// environment `arboard` fails and the copy is skipped, which is why the
/// selection itself is verified through `--select` and not through a paste.
fn copy_to_clipboard(text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_string());
    }
}

impl App {
    fn new(
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
        initial_url: Option<String>,
    ) -> Result<App, String> {
        let mut engine = Engine::new(1.0)?;
        let mut chrome = Chrome::new();
        let active_id = 1;
        {
            let url = initial_url.unwrap_or_else(|| naro_engine::START_URL.to_string());
            let nav = engine.navigate(&url, true);
            if let Some(fetch_url) = nav.fetch {
                let proxy_for_fetch = proxy.clone();
                let generation = nav.generation;
                std::thread::spawn(move || {
                    let result = naro_net::with_session(|net| net.fetch(&fetch_url));
                    let _ = proxy_for_fetch.send_event(UserEvent::Fetched {
                        tab: active_id,
                        generation,
                        result,
                    });
                });
            }
            chrome.addr = engine.display_url.clone();
            chrome.cursor = chrome.addr.len();
        }
        Ok(App {
            window: None,
            context: None,
            surface: None,
            engine,
            chrome,
            active_id,
            background: Vec::new(),
            order: vec![active_id],
            next_tab_id: active_id + 1,
            modifiers: ModifiersState::empty(),
            cursor: (0.0, 0.0),
            last_drag_click: None,
            close_requested: false,
            pressed_nav: None,
            pressed_new_tab: false,
            toolbar_focus: None,
            selecting: None,
            proxy,
        })
    }

    /// Tab titles in strip order, with the active one marked.
    fn tab_titles(&self) -> Vec<(u64, String, bool)> {
        self.order
            .iter()
            .map(|&id| {
                let active = id == self.active_id;
                let title = if active {
                    self.engine.page_title.clone()
                } else {
                    self.background
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| t.engine.page_title.clone())
                        .unwrap_or_default()
                };
                let title = if title.is_empty() { "Nova aba".to_string() } else { title };
                (id, title, active)
            })
            .collect()
    }

    /// Make `id` the visible tab by swapping its state with the active one.
    fn activate_tab(&mut self, id: u64) {
        if id == self.active_id {
            return;
        }
        let Some(index) = self.background.iter().position(|t| t.id == id) else {
            return;
        };
        std::mem::swap(&mut self.engine, &mut self.background[index].engine);
        std::mem::swap(&mut self.chrome, &mut self.background[index].chrome);
        self.background[index].id = self.active_id;
        self.active_id = id;
        self.sync_title();
    }

    /// Open a tab next to the active one and switch to it.
    fn open_tab(&mut self, url: &str) {
        let engine = match Engine::new(self.engine.scale) {
            Ok(engine) => engine,
            Err(err) => {
                eprintln!("naro: {err}");
                return;
            }
        };
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut chrome = Chrome::new();
        chrome.addr = url.to_string();
        chrome.cursor = chrome.addr.len();
        self.background.push(BackgroundTab { id, engine, chrome });
        let at = self
            .order
            .iter()
            .position(|&t| t == self.active_id)
            .map(|i| i + 1)
            .unwrap_or(self.order.len());
        self.order.insert(at, id);
        self.activate_tab(id);
        // The new tab starts at its own start page; navigating it here would
        // need a fetch spawned for a tab that is already on screen.
        let nav = self.engine.navigate(url, true);
        self.start_navigation(nav);
        self.chrome.addr = self.engine.display_url.clone();
        self.chrome.cursor = self.chrome.addr.len();
    }

    /// Close `id`. Returns `false` when the window has no tabs left and the
    /// caller should close it, as a browser does.
    fn close_tab(&mut self, id: u64) -> bool {
        if self.order.len() <= 1 {
            return false;
        }
        let Some(position) = self.order.iter().position(|&t| t == id) else {
            return true;
        };
        self.order.remove(position);
        if id == self.active_id {
            // Switch to the neighbour first, so the tab being closed is the
            // one sitting in `background` and can simply be dropped.
            let next = self.order[position.min(self.order.len() - 1)];
            self.activate_tab(next);
        }
        if let Some(index) = self.background.iter().position(|t| t.id == id) {
            self.background.remove(index);
        }
        self.sync_title();
        true
    }

    fn start_navigation(&mut self, nav: Navigation) {
        if !self.chrome.focused {
            self.chrome.addr = self.engine.display_url.clone();
            self.chrome.cursor = self.chrome.addr.len();
        }
        if let Some(url) = nav.fetch {
            let proxy = self.proxy.clone();
            let generation = nav.generation;
            let tab = self.active_id;
            std::thread::spawn(move || {
                let result = naro_net::with_session(|net| net.fetch(&url));
                let _ = proxy.send_event(UserEvent::Fetched { tab, generation, result });
            });
        }
        self.sync_title();
    }

    fn sync_title(&mut self) {
        if let Some(window) = &self.window {
            let title = if self.engine.page_title.is_empty() {
                "NARO".to_string()
            } else {
                format!("{} — NARO", self.engine.page_title)
            };
            window.set_title(&title);
        }
    }

    fn chrome_rects(&self) -> ChromeLayout {
        let (w, h) = self.logical_size();
        chrome_layout(w, h, self.order.len())
    }

    /// Cheap rect test used on every mouse move; avoids laying out the chrome
    /// twice per event.
    fn over_chrome_button(&self, x: f32, y: f32) -> bool {
        let layout = self.chrome_rects();
        layout.back.contains(x, y)
            || layout.forward.contains(x, y)
            || layout.reload.contains(x, y)
            || layout.new_tab.contains(x, y)
            || layout.win_min.contains(x, y)
            || layout.win_max.contains(x, y)
            || layout.win_close.contains(x, y)
            || layout.tabs.iter().any(|r| r.contains(x, y))
    }

    /// Which toolbar button is at a point, if any.
    fn nav_button_at(&self, x: f32, y: f32) -> Option<NavIcon> {
        let layout = self.chrome_rects();
        if layout.back.contains(x, y) {
            Some(NavIcon::Back)
        } else if layout.forward.contains(x, y) {
            Some(NavIcon::Forward)
        } else if layout.reload.contains(x, y) {
            Some(NavIcon::Reload)
        } else {
            None
        }
    }

    /// Keyboard access to the toolbar. The chrome is painted by hand, so it
    /// has no focusable widgets of its own; F6 cycles focus through the three
    /// navigation buttons and Enter/Space activates the focused one.
    fn cycle_toolbar_focus(&mut self, backwards: bool) {
        let order = [NavIcon::Back, NavIcon::Forward, NavIcon::Reload];
        let next = match self.toolbar_focus {
            None => {
                if backwards {
                    order.len() - 1
                } else {
                    0
                }
            }
            Some(current) => {
                let i = order.iter().position(|&i| i == current).unwrap_or(0);
                if backwards {
                    (i + order.len() - 1) % order.len()
                } else {
                    (i + 1) % order.len()
                }
            }
        };
        self.toolbar_focus = Some(order[next]);
    }

    /// Run the action of a toolbar button.
    fn activate_nav(&mut self, icon: NavIcon) {
        let nav = match icon {
            NavIcon::Back => self.engine.back(),
            NavIcon::Forward => self.engine.forward(),
            NavIcon::Reload => self.engine.reload(),
        };
        self.start_navigation(nav);
    }

    /// Resize direction for a press near the window edge.
    ///
    /// The window has no OS frame, so this is the only thing that makes the
    /// borders draggable. A maximized window is not resizable, matching every
    /// other browser.
    fn resize_direction(&self, x: f32, y: f32) -> Option<winit::window::ResizeDirection> {
        use winit::window::ResizeDirection;
        let (w, h) = self.logical_size();
        let maximized = self.window.as_ref().map(|win| win.is_maximized()).unwrap_or(false);
        if maximized {
            return None;
        }
        let left = x <= RESIZE_BORDER;
        let right = x >= w - RESIZE_BORDER;
        let top = y <= RESIZE_BORDER;
        let bottom = y >= h - RESIZE_BORDER;
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(ResizeDirection::NorthWest),
            (_, true, true, _) => Some(ResizeDirection::NorthEast),
            (true, _, _, true) => Some(ResizeDirection::SouthWest),
            (_, true, _, true) => Some(ResizeDirection::SouthEast),
            (true, _, _, _) => Some(ResizeDirection::West),
            (_, true, _, _) => Some(ResizeDirection::East),
            (_, _, true, _) => Some(ResizeDirection::North),
            (_, _, _, true) => Some(ResizeDirection::South),
            _ => None,
        }
    }

    /// True when the point is empty tab-row space: the title-bar drag surface.
    fn over_drag_surface(&self, x: f32, y: f32) -> bool {
        let layout = self.chrome_rects();
        layout.tab_row.contains(x, y)
            && !layout.tabs.iter().any(|r| r.contains(x, y))
            && !layout.tab_close.iter().any(|r| r.contains(x, y))
            && !layout.new_tab.contains(x, y)
            && !layout.win_min.contains(x, y)
            && !layout.win_max.contains(x, y)
            && !layout.win_close.contains(x, y)
    }

    fn logical_size(&self) -> (f32, f32) {
        match &self.window {
            Some(window) => {
                let size = window.inner_size();
                let scale = window.scale_factor() as f32;
                (size.width as f32 / scale, size.height as f32 / scale)
            }
            None => (1200.0, 800.0),
        }
    }

    fn focus_address(&mut self, select_all: bool) {
        self.chrome.addr = self.engine.display_url.clone();
        self.chrome.cursor = self.chrome.addr.len();
        self.chrome.focused = true;
        self.chrome.replace_on_type = select_all;
    }

    fn navigate_to(&mut self, url: &str) {
        let nav = self.engine.navigate(url, true);
        self.start_navigation(nav);
        self.chrome.addr = self.engine.display_url.clone();
        self.chrome.cursor = self.chrome.addr.len();
    }

    /// Follow a navigation a form asked for.
    ///
    /// A `GET` is an ordinary navigation; a `POST` keeps the URL clean and
    /// sends the encoded controls as the request body, which is why the engine
    /// hands over a method instead of a URL with a query bolted on.
    fn navigate_pending(&mut self, pending: naro_engine::PendingNavigation) {
        if !pending.is_post() {
            self.navigate_to(&pending.url);
            return;
        }
        let nav = self.engine.navigate(&pending.url, true);
        let proxy = self.proxy.clone();
        let generation = nav.generation;
        let tab = self.active_id;
        let url = pending.url.clone();
        let body = pending.body.unwrap_or_default();
        std::thread::spawn(move || {
            let result = naro_net::with_session(|net| net.post_form(&url, &body));
            let _ = proxy.send_event(UserEvent::Fetched { tab, generation, result });
        });
        self.chrome.addr = self.engine.display_url.clone();
        self.chrome.cursor = self.chrome.addr.len();
        self.sync_title();
    }

    fn draw(&mut self) {
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return,
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale = window.scale_factor() as f32;
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let layout = chrome_layout(w, h, self.order.len());

        self.engine
            .set_viewport(layout.content.w.max(1.0), layout.content.h.max(1.0));
        if self.engine.pump_js(8) {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        let (nw, nh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(nw), Some(nh)) => (nw, nh),
            _ => return,
        };
        if let Some(surface) = &mut self.surface {
            let _ = surface.resize(nw, nh);
        }
        // Take the surface out so the buffer borrow does not conflict with
        // `&mut self` while drawing the chrome.
        let mut surface = match self.surface.take() {
            Some(surface) => surface,
            None => return,
        };
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(_) => {
                self.surface = Some(surface);
                return;
            }
        };
        {
            // Paint directly into the presentation buffer: no intermediate
            // framebuffer and no full-screen copy.
            let mut canvas = Canvas::new(&mut buffer, size.width, size.height, scale);
            canvas.clear(Color::WHITE);
            self.engine.paint_page(&mut canvas, layout.content);
            self.draw_select_popup(&mut canvas, &layout);
            self.draw_chrome(&mut canvas, &layout);
        }
        let _ = buffer.present();
        self.surface = Some(surface);
    }

    /// Geometry of the open `<select>` popup, in window coordinates.
    ///
    /// The popup is chrome, not page: it is drawn after the page and before
    /// the browser frame, and it is hit-tested before the page so a click
    /// inside it cannot also reach the element underneath.
    fn select_popup(&self, layout: &ChromeLayout) -> Option<(Rect, Vec<(usize, String, Rect)>)> {
        let select = self.engine.is_select_open()?;
        let rect = self.engine.select_rect(select)?;
        let options = self.engine.select_options(select);
        if options.is_empty() {
            return None;
        }
        let row_h = 20.0f32;
        let mut width = rect.w.max(60.0);
        {
            let fonts = &self.engine.fonts;
            for (_, label, _) in &options {
                let w = fonts.measure(label, 14.0, naro_text::FontFace::default()) + 16.0;
                width = width.max(w);
            }
        }
        let height = row_h * options.len() as f32 + 2.0;
        let x = layout.content.x + rect.x;
        let mut y = layout.content.y + rect.y - self.engine.scroll_y + rect.h;
        // Flip above the control when there is no room below it.
        if y + height > layout.content.y + layout.content.h {
            y = (layout.content.y + rect.y - self.engine.scroll_y - height).max(layout.content.y);
        }
        let popup = Rect { x, y, w: width, h: height };
        let rows = options
            .into_iter()
            .enumerate()
            .map(|(i, (node, label, selected))| {
                let _ = selected;
                (
                    node,
                    label,
                    Rect {
                        x: popup.x + 1.0,
                        y: popup.y + 1.0 + i as f32 * row_h,
                        w: popup.w - 2.0,
                        h: row_h,
                    },
                )
            })
            .collect();
        Some((popup, rows))
    }

    fn draw_select_popup(&mut self, canvas: &mut Canvas<'_>, layout: &ChromeLayout) {
        let Some((popup, rows)) = self.select_popup(layout) else {
            return;
        };
        let selected = self
            .engine
            .is_select_open()
            .map(|node| {
                self.engine
                    .select_options(node)
                    .into_iter()
                    .find(|(_, _, chosen)| *chosen)
                    .map(|(option, _, _)| option)
            })
            .flatten();
        canvas.fill_rect(popup.x, popup.y, popup.w, popup.h, Color::WHITE);
        for (node, label, row) in rows {
            let hovered = row.contains(self.cursor.0, self.cursor.1);
            if hovered {
                canvas.fill_rect(row.x, row.y, row.w, row.h, Color::rgb(0xd6, 0xe4, 0xf7));
            }
            if Some(node) == selected {
                canvas.fill_rect(row.x, row.y, row.w, row.h, Color::rgb(0xe8, 0xef, 0xfb));
            }
            let weight = if Some(node) == selected { 700 } else { 400 };
            canvas.draw_text(
                row.x + 6.0,
                row.y + row.h - 5.0,
                &label,
                14.0,
                Color::rgb(0x1a, 0x1a, 0x1a),
                naro_text::FontFace::new("", weight, false),
                &mut self.engine.fonts,
            );
        }
        // A 1px frame, drawn last so it sits on top of the hover fill.
        canvas.stroke_rect(popup.x, popup.y, popup.w, popup.h, 1.0, Color::rgb(0x76, 0x76, 0x76));
    }

    fn draw_chrome(&mut self, canvas: &mut Canvas<'_>, layout: &ChromeLayout) {
        let can_back = self.engine.can_back();
        let can_forward = self.engine.can_forward();
        let status_line = self.engine.status_line();
        let max_scroll = self.engine.max_scroll();
        let scroll_y = self.engine.scroll_y;
        let content_height = self.engine.layout.as_ref().map(|l| l.content_height).unwrap_or(0.0);
        let titles = self.tab_titles();
        let maximized = self.window.as_ref().map(|w| w.is_maximized()).unwrap_or(false);
        let fonts = &mut self.engine.fonts;

        let hover = self.cursor;
        let hovered = |r: &Rect| r.contains(hover.0, hover.1);

        // --- tab strip: this row is the window's title bar -----------------
        canvas.fill_rect(0.0, 0.0, layout.tab_row.w, TAB_ROW_H, C_TABROW);
        canvas.fill_rect(0.0, TAB_ROW_H, layout.toolbar.w, TOOLBAR_H, C_TOOLBAR);
        canvas.fill_rect(0.0, CHROME_H - 1.0, layout.toolbar.w, 1.0, C_BORDER);

        for (i, rect) in layout.tabs.iter().enumerate() {
            let Some((_, title, active)) = titles.get(i) else { continue };
            let over_tab = rect.contains(hover.0, hover.1);
            if *active {
                // Same colour as the toolbar, extended two pixels into it, so
                // the selected tab reads as one surface with the address bar.
                canvas.fill_round_rect(rect.x, rect.y, rect.w, rect.h + 2.0, 6.0, C_TOOLBAR);
            } else if over_tab {
                canvas.fill_round_rect(rect.x, rect.y, rect.w, rect.h, 6.0, C_TAB_HOVER);
            }
            let color = if *active { C_TEXT } else { C_STATUS_TEXT };
            let text_x = rect.x + 10.0;
            canvas.set_clip(Rect {
                x: text_x,
                y: rect.y,
                w: (rect.w - 38.0).max(0.0),
                h: rect.h,
            });
            canvas.draw_text(
                text_x,
                rect.y + 19.0,
                title,
                12.5,
                color,
                naro_text::FontFace::default(),
                fonts,
            );
            canvas.reset_clip();

            // Close button: always on the active tab, on hover otherwise.
            if let Some(close) = layout.tab_close.get(i) {
                if *active || over_tab {
                    let over_close = close.contains(hover.0, hover.1);
                    if over_close {
                        draw_button_bg(canvas, close, C_HOVER, 4.0);
                    }
                    draw_icon(
                        canvas,
                        Icon::Close,
                        close.x + close.w / 2.0,
                        close.y + close.h / 2.0,
                        10.0,
                        if over_close { C_TEXT } else { C_STATUS_TEXT },
                    );
                }
            }
        }

        // New tab: same rounded hover and pressed fill as every other button.
        let over_new_tab = hovered(&layout.new_tab);
        if over_new_tab || self.pressed_new_tab {
            let fill = if self.pressed_new_tab { C_HOVER_PRESSED } else { C_TAB_HOVER };
            draw_button_bg(canvas, &layout.new_tab, fill, 6.0);
        }
        draw_icon(
            canvas,
            Icon::Plus,
            layout.new_tab.x + layout.new_tab.w / 2.0,
            layout.new_tab.y + layout.new_tab.h / 2.0,
            15.0,
            C_STATUS_TEXT,
        );

        // --- window controls ----------------------------------------------
        for (rect, icon, is_close) in [
            (layout.win_min, Icon::Minimize, false),
            (
                layout.win_max,
                if maximized { Icon::Restore } else { Icon::Maximize },
                false,
            ),
            (layout.win_close, Icon::Close, true),
        ] {
            let over = rect.contains(hover.0, hover.1);
            if over {
                // Only the close button takes colour; the other two get the
                // same neutral hover as the rest of the chrome.
                let color = if is_close { C_WIN_CLOSE_HOVER } else { C_HOVER };
                canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, color);
            }
            let fg = if over && is_close { Color::WHITE } else { C_TEXT };
            draw_icon(
                canvas,
                icon,
                rect.x + rect.w / 2.0,
                rect.y + rect.h / 2.0,
                12.0,
                fg,
            );
        }

        // Back, forward and reload share one geometry: same box, same rounded
        // hover, same stroke weight, all three icons drawn rather than typed,
        // so the row is visually uniform.
        for (rect, icon, enabled) in [
            (layout.back, NavIcon::Back, can_back),
            (layout.forward, NavIcon::Forward, can_forward),
            (layout.reload, NavIcon::Reload, true),
        ] {
            let over = hovered(&rect);
            let pressed = self.pressed_nav == Some(icon);
            let focused = self.toolbar_focus == Some(icon);
            if over || pressed || focused {
                let fill = if pressed { C_HOVER_PRESSED } else { C_HOVER };
                draw_button_bg(canvas, &rect, fill, 6.0);
            }
            if focused {
                canvas.stroke_rect(
                    rect.x + 1.0,
                    rect.y + 1.0,
                    rect.w - 2.0,
                    rect.h - 2.0,
                    1.0,
                    C_ACCENT,
                );
            }
            let color = if enabled { C_TEXT } else { C_DISABLED };
            draw_icon(
                canvas,
                icon.icon(),
                rect.x + rect.w / 2.0,
                rect.y + rect.h / 2.0,
                15.0,
                color,
            );
        }

        // A label under the hovered or focused control: the chrome is painted
        // pixels with no accessibility tree, so the tooltip is what names the
        // action and its shortcut.
        let hint = [
            (NavIcon::Back, layout.back),
            (NavIcon::Forward, layout.forward),
            (NavIcon::Reload, layout.reload),
        ]
        .into_iter()
        .find(|(icon, rect)| match self.toolbar_focus {
            // Keyboard focus names the target; the tooltip follows it.
            Some(focused) => focused == *icon,
            None => hovered(rect),
        })
        .map(|(icon, rect)| (icon.label(), rect));
        if let Some((label, anchor)) = hint {
            let tw = canvas.text_width(label, 11.0, naro_text::FontFace::default(), fonts);
            let bx = (anchor.x + anchor.w / 2.0 - tw / 2.0 - 6.0).max(2.0);
            let by = anchor.y + anchor.h + 4.0;
            canvas.fill_round_rect(bx, by, tw + 12.0, 18.0, 4.0, C_TOOLTIP_BG);
            canvas.draw_text(
                bx + 6.0,
                by + 13.0,
                label,
                11.0,
                Color::WHITE,
                naro_text::FontFace::default(),
                fonts,
            );
        }

        let addr = &layout.addr;
        // Rounded like every other surface in the chrome; a square field next
        // to rounded buttons was the most visible inconsistency left.
        let field_r = addr.h / 2.0;
        canvas.fill_round_rect(addr.x, addr.y, addr.w, addr.h, field_r, C_FIELD_BORDER);
        canvas.fill_round_rect(
            addr.x + 1.0,
            addr.y + 1.0,
            addr.w - 2.0,
            addr.h - 2.0,
            field_r - 1.0,
            Color::WHITE,
        );

        canvas.set_clip(Rect {
            x: addr.x + 2.0,
            y: addr.y + 2.0,
            w: (addr.w - 4.0).max(0.0),
            h: addr.h - 4.0,
        });
        let text_x = addr.x + 10.0;
        let baseline = addr.y + 20.0;
        let (shown, color) = if self.chrome.addr.is_empty() && !self.chrome.focused {
            ("Digite um endereço ou naro://start".to_string(), C_MUTED)
        } else {
            (self.chrome.addr.clone(), C_TEXT)
        };
        canvas.draw_text(
            text_x,
            baseline,
            &shown,
            14.0,
            color,
            naro_text::FontFace::default(),
            fonts,
        );

        if self.chrome.focused {
            let prefix = &self.chrome.addr[..self.chrome.cursor.min(self.chrome.addr.len())];
            let caret_x =
                text_x + canvas.text_width(prefix, 14.0, naro_text::FontFace::default(), fonts);
            canvas.fill_rect(caret_x, addr.y + 7.0, 1.0, addr.h - 14.0, C_ACCENT);
        }
        canvas.reset_clip();

        let status = &layout.status;
        canvas.fill_rect(status.x, status.y, status.w, status.h, C_STATUS_BG);
        canvas.fill_rect(status.x, status.y, status.w, 1.0, C_BORDER);
        canvas.set_clip(*status);
        canvas.draw_text(
            status.x + 10.0,
            status.y + 15.0,
            &status_line,
            12.0,
            C_STATUS_TEXT,
            naro_text::FontFace::default(),
            fonts,
        );
        let brand = "NARO 0.1";
        let bw = canvas.text_width(brand, 12.0, naro_text::FontFace::default(), fonts);
        canvas.draw_text(
            status.x + status.w - bw - 10.0,
            status.y + 15.0,
            brand,
            12.0,
            C_MUTED,
            naro_text::FontFace::default(),
            fonts,
        );
        canvas.reset_clip();

        if max_scroll > 0.0 {
            let content = &layout.content;
            let track_x = content.x + content.w - 10.0;
            canvas.fill_rect(track_x, content.y, 10.0, content.h, Color::rgb(0xe9, 0xeb, 0xef));
            let ratio = if content_height > 0.0 {
                (content.h / content_height).clamp(0.05, 1.0)
            } else {
                1.0
            };
            let thumb_h = (content.h * ratio).max(24.0).min(content.h);
            let travel = (content.h - thumb_h).max(0.0);
            let thumb_y = content.y + travel * (scroll_y / max_scroll).clamp(0.0, 1.0);
            canvas.fill_rect(track_x, thumb_y, 10.0, thumb_h, Color::rgb(0xb7, 0xbd, 0xc8));
        }
    }

    fn is_link_at(&self, x: f32, y: f32) -> bool {
        let (w, h) = self.logical_size();
        let content = Rect { x: 0.0, y: CHROME_H, w, h: (h - CHROME_H - STATUS_H).max(0.0) };
        if !content.contains(x, y) {
            return false;
        }
        self.engine
            .hit_link(x - content.x, y - content.y + self.engine.scroll_y)
            .is_some()
    }

    fn handle_key(&mut self, event: &winit::event::KeyEvent) {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();

        // A focused form control consumes typing before any page shortcut.
        if self.engine.focus.is_some() && !self.chrome.focused {
            match &event.logical_key {
                Key::Named(NamedKey::Tab) => {
                    self.engine.focus_step(!self.modifiers.shift_key());
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                Key::Named(NamedKey::Escape) => {
                    // Escape closes the select popup before it leaves the
                    // control, as in a browser.
                    if self.engine.is_select_open().is_some() {
                        self.engine.close_select();
                    } else {
                        self.engine.blur();
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    self.engine.backspace();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                Key::Named(NamedKey::Enter) => {
                    self.engine.press_enter();
                    if let Some(pending) = self.engine.take_pending_nav() {
                        self.navigate_pending(pending);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.engine.caret = self.engine.caret.saturating_sub(1);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let len = self
                        .engine
                        .focus
                        .map(|n| self.engine.control_value_of(n).len())
                        .unwrap_or(0);
                    self.engine.caret = (self.engine.caret + 1).min(len);
                    return;
                }
                Key::Character(text) if !ctrl && !alt => {
                    self.engine.insert_text(text);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                _ => {}
            }
        }

        if let Key::Named(NamedKey::F6) = &event.logical_key {
            self.cycle_toolbar_focus(self.modifiers.shift_key());
            return;
        }
        if let Some(icon) = self.toolbar_focus {
            match &event.logical_key {
                Key::Named(NamedKey::Enter | NamedKey::Space) => {
                    self.activate_nav(icon);
                    return;
                }
                Key::Named(NamedKey::Escape) => {
                    self.toolbar_focus = None;
                    return;
                }
                _ => {}
            }
        }

        if ctrl && !alt {
            if let Key::Character(c) = &event.logical_key {
                if c.eq_ignore_ascii_case("l") {
                    self.focus_address(true);
                    return;
                }
                if c.eq_ignore_ascii_case("r") {
                    let nav = self.engine.reload();
                    self.start_navigation(nav);
                    return;
                }
                if c.eq_ignore_ascii_case("t") {
                    self.open_tab(naro_engine::START_URL);
                    return;
                }
                if c.eq_ignore_ascii_case("w") {
                    // Closing the last tab closes the window, as in a browser.
                    // The event loop is not reachable here, so the flag is set
                    // and `window_event` acts on it.
                    if !self.close_tab(self.active_id) {
                        self.close_requested = true;
                    }
                    return;
                }
                if c.eq_ignore_ascii_case("c") {
                    // A focused text control copies through the browser's own
                    // path; anywhere else it copies the page selection.
                    if self.engine.focus.is_some() && !self.chrome.focused {
                        let node = self.engine.focus.unwrap();
                        let value = self.engine.control_value_of(node);
                        copy_to_clipboard(&value);
                    } else {
                        copy_to_clipboard(&self.engine.selection_text());
                    }
                    return;
                }
            }
        }

        if alt {
            match &event.logical_key {
                Key::Named(NamedKey::ArrowLeft) => {
                    let nav = self.engine.back();
                    self.start_navigation(nav);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let nav = self.engine.forward();
                    self.start_navigation(nav);
                    return;
                }
                _ => {}
            }
        }

        match &event.logical_key {
            Key::Named(NamedKey::F5) => {
                let nav = self.engine.reload();
                self.start_navigation(nav);
                return;
            }
            Key::Named(NamedKey::Escape) => {
                self.chrome.focused = false;
                self.chrome.addr = self.engine.display_url.clone();
                self.chrome.cursor = self.chrome.addr.len();
                return;
            }
            _ => {}
        }

        if !self.chrome.focused {
            return;
        }

        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                let url = normalize_input(&self.chrome.addr);
                self.chrome.focused = false;
                self.navigate_to(&url);
            }
            Key::Named(NamedKey::Backspace) => {
                if self.chrome.replace_on_type {
                    self.chrome.addr.clear();
                    self.chrome.cursor = 0;
                    self.chrome.replace_on_type = false;
                } else if self.chrome.cursor > 0 {
                    let prev = self.chrome.addr[..self.chrome.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.chrome.addr.replace_range(prev..self.chrome.cursor, "");
                    self.chrome.cursor = prev;
                }
            }
            Key::Named(NamedKey::Delete) => {
                if self.chrome.cursor < self.chrome.addr.len() {
                    let next = self.chrome.addr[self.chrome.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.chrome.cursor + i)
                        .unwrap_or(self.chrome.addr.len());
                    self.chrome.addr.replace_range(self.chrome.cursor..next, "");
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.chrome.cursor > 0 {
                    self.chrome.cursor = self.chrome.addr[..self.chrome.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                self.chrome.replace_on_type = false;
            }
            Key::Named(NamedKey::ArrowRight) => {
                if self.chrome.cursor < self.chrome.addr.len() {
                    self.chrome.cursor = self.chrome.addr[self.chrome.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.chrome.cursor + i)
                        .unwrap_or(self.chrome.addr.len());
                }
                self.chrome.replace_on_type = false;
            }
            Key::Named(NamedKey::Home) => {
                self.chrome.cursor = 0;
                self.chrome.replace_on_type = false;
            }
            Key::Named(NamedKey::End) => {
                self.chrome.cursor = self.chrome.addr.len();
                self.chrome.replace_on_type = false;
            }
            Key::Character(text) if !ctrl && !alt => {
                if self.chrome.replace_on_type {
                    self.chrome.addr.clear();
                    self.chrome.cursor = 0;
                    self.chrome.replace_on_type = false;
                }
                let mut inserted = 0;
                for ch in text.chars().filter(|c| !c.is_control()) {
                    self.chrome.addr.insert(self.chrome.cursor, ch);
                    self.chrome.cursor += ch.len_utf8();
                    inserted += ch.len_utf8();
                }
                let _ = inserted;
            }
            _ => {}
        }
    }

    fn handle_click(&mut self, event_loop: &ActiveEventLoop, x: f32, y: f32) {
        let layout = self.chrome_rects();

        // --- window controls: real actions on the real window -------------
        if layout.win_min.contains(x, y) {
            if let Some(window) = &self.window {
                window.set_minimized(true);
            }
            return;
        }
        if layout.win_max.contains(x, y) {
            if let Some(window) = &self.window {
                window.set_maximized(!window.is_maximized());
            }
            return;
        }
        if layout.win_close.contains(x, y) {
            event_loop.exit();
            return;
        }

        // --- tab strip ----------------------------------------------------
        for (i, rect) in layout.tabs.iter().enumerate() {
            let Some(&id) = self.order.get(i) else { continue };
            if layout.tab_close.get(i).is_some_and(|c| c.contains(x, y)) {
                if !self.close_tab(id) {
                    event_loop.exit();
                }
                return;
            }
            if rect.contains(x, y) {
                self.activate_tab(id);
                return;
            }
        }
        if layout.new_tab.contains(x, y) {
            self.open_tab(naro_engine::START_URL);
            return;
        }
        // Empty strip space: the window drag surface. Double-click toggles
        // maximize, as the OS title bar does.
        if self.over_drag_surface(x, y) {
            let now = std::time::Instant::now();
            let double = self
                .last_drag_click
                .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(400));
            self.last_drag_click = Some(now);
            if let Some(window) = &self.window {
                if double {
                    window.set_maximized(!window.is_maximized());
                } else {
                    let _ = window.drag_window();
                }
            }
            return;
        }

        if let Some(icon) = self.nav_button_at(x, y) {
            // Clicking does not take keyboard focus: the ring is for F6
            // navigation only, as in a browser.
            self.toolbar_focus = None;
            self.activate_nav(icon);
            return;
        }
        if layout.addr.contains(x, y) {
            self.chrome.focused = true;
            self.chrome.cursor = self.chrome.addr.len();
            self.chrome.replace_on_type = false;
            return;
        }
        // The popup is above the page: a click on a row chooses, a click
        // anywhere else closes it and goes no further.
        if let Some((popup, rows)) = self.select_popup(&layout) {
            if popup.contains(x, y) {
                let hit = rows.iter().find(|(_, _, row)| row.contains(x, y));
                if let (Some(select), Some((option, _, _))) = (self.engine.is_select_open(), hit) {
                    self.engine.choose_option(select, *option);
                } else {
                    self.engine.close_select();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            self.engine.close_select();
        }

        if layout.content.contains(x, y) {
            self.chrome.focused = false;
            let page_x = x - layout.content.x;
            let page_y = y - layout.content.y + self.engine.scroll_y;

            // JavaScript listeners get the click first; links navigate only
            // when no handler consumed it.
            let (js, consumed) = self.engine.click_at(page_x, page_y);
            self.engine.js.errors.extend(js.errors);
            self.engine.js.logs.extend(js.logs);
            if !consumed {
                if let Some(pending) = self.engine.take_pending_nav() {
                    self.navigate_pending(pending);
                } else if let Some(target) = self.engine.hit_link(page_x, page_y) {
                    self.navigate_to(&target);
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // The tab strip is the title bar: the OS frame is removed and its
        // space reused. Dragging, resizing and the window buttons are
        // reimplemented against the real window below.
        let attrs = Window::default_attributes()
            .with_title("NARO")
            .with_decorations(false)
            .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 320.0))
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 800.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                eprintln!("naro: falha ao criar a janela: {err}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("naro: falha ao criar o contexto de pintura: {err}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("naro: falha ao criar a superfície: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        self.engine.set_viewport(
            size.width as f32 / scale,
            (size.height as f32 / scale - CHROME_H - STATUS_H).max(1.0),
        );
        self.window = Some(window.clone());
        self.context = Some(context);
        self.surface = Some(surface);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0) as f32;
                    self.engine.set_viewport(
                        size.width as f32 / scale,
                        (size.height as f32 / scale - CHROME_H - STATUS_H).max(1.0),
                    );
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0) as f32;
                let size = self.window.as_ref().map(|w| w.inner_size()).unwrap_or_default();
                self.engine.set_scale(scale);
                if size.width > 0 && size.height > 0 {
                    self.engine.set_viewport(
                        size.width as f32 / scale,
                        (size.height as f32 / scale - CHROME_H - STATUS_H).max(1.0),
                    );
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.handle_key(&event);
                    if self.close_requested {
                        event_loop.exit();
                        return;
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.selecting.is_some() {
                    let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0) as f32;
                    let x = position.x as f32 / scale;
                    let y = position.y as f32 / scale;
                    self.cursor = (x, y);
                    let layout = self.chrome_rects();
                    if layout.content.contains(x, y) {
                        self.engine.extend_selection(
                            x - layout.content.x,
                            y - layout.content.y + self.engine.scroll_y,
                        );
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0) as f32;
                self.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                let over_link = self.is_link_at(self.cursor.0, self.cursor.1);
                let over_button = self.over_chrome_button(self.cursor.0, self.cursor.1);
                let resize = self.resize_direction(self.cursor.0, self.cursor.1);
                let drag = self.over_drag_surface(self.cursor.0, self.cursor.1);
                if let Some(window) = &self.window {
                    use winit::window::ResizeDirection;
                    let icon = match resize {
                        Some(ResizeDirection::East | ResizeDirection::West) => {
                            CursorIcon::EwResize
                        }
                        Some(ResizeDirection::North | ResizeDirection::South) => {
                            CursorIcon::NsResize
                        }
                        Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => {
                            CursorIcon::NwseResize
                        }
                        Some(ResizeDirection::NorthEast | ResizeDirection::SouthWest) => {
                            CursorIcon::NeswResize
                        }
                        None if over_link || over_button => CursorIcon::Pointer,
                        None if drag => CursorIcon::Default,
                        None => CursorIcon::Default,
                    };
                    window.set_cursor(icon);
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                let had = self.pressed_nav.take().is_some();
                let had_tab = std::mem::take(&mut self.pressed_new_tab);
                if let Some(start) = self.selecting.take() {
                    let (x, y) = self.cursor;
                    let layout = self.chrome_rects();
                    if layout.content.contains(x, y) {
                        self.engine.extend_selection(
                            x - layout.content.x,
                            y - layout.content.y + self.engine.scroll_y,
                        );
                    }
                    // A drag that did not move is a click, so the page still
                    // gets it; a real drag is a selection and is not a click.
                    let moved = (start.0 - (x - layout.content.x)).abs() > 2.0
                        || (start.1 - (y - layout.content.y + self.engine.scroll_y)).abs() > 2.0;
                    if !moved {
                        self.engine.clear_selection();
                        self.handle_click(event_loop, x, y);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if had || had_tab {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (x, y) = self.cursor;
                // A press on the invisible border resizes; anywhere else it is
                // a normal click.
                if let Some(direction) = self.resize_direction(x, y) {
                    if let Some(window) = &self.window {
                        let _ = window.drag_resize_window(direction);
                    }
                    return;
                }
                self.toolbar_focus = None;
                self.pressed_nav = self.nav_button_at(x, y);
                self.pressed_new_tab = self.chrome_rects().new_tab.contains(x, y);
                // A press on page text starts a selection instead of a click:
                // the click is dispatched on release, when it is still a click.
                let layout = self.chrome_rects();
                let page = if layout.content.contains(x, y) {
                    Some((
                        x - layout.content.x,
                        y - layout.content.y + self.engine.scroll_y,
                    ))
                } else {
                    None
                };
                if let Some((px, py)) = page {
                    if self.engine.begin_selection(px, py) {
                        self.selecting = Some((px, py));
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                }
                self.handle_click(event_loop, x, y);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0) as f32;
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 40.0,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / scale,
                };
                self.engine.scroll_by(-dy);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Fetched { tab, generation, result } => {
                if tab == self.active_id {
                    self.engine.apply_fetch(generation, result);
                    self.sync_title();
                } else if let Some(background) = self.background.iter_mut().find(|t| t.id == tab) {
                    // A background tab loads on its own; only its title in the
                    // strip changes.
                    background.engine.apply_fetch(generation, result);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {}
}

fn load_for_headless(target: &str, width: f32) -> Engine {
    let mut engine = match Engine::new(1.0) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("naro: {err}");
            std::process::exit(1);
        }
    };
    engine.set_viewport(width, 700.0);
    let url = normalize_input(target);
    let nav = engine.navigate(&url, true);
    if let Some(fetch_url) = nav.fetch {
        let result = naro_net::with_session(|net| net.fetch(&fetch_url));
        engine.apply_fetch(nav.generation, result);
    }
    engine
}

/// Live/peak heap accounting, used only by `--bench` to attribute memory.
struct CountingAlloc;

static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static COUNTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn note_alloc(size: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    ALLOCS.fetch_add(1, Relaxed);
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}

unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let ptr = unsafe { std::alloc::System.alloc(layout) };
        if !ptr.is_null() && COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            note_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        if COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            LIVE.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        }
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let out = unsafe { std::alloc::System.realloc(ptr, layout, new_size) };
        if !out.is_null() && COUNTING.load(std::sync::atomic::Ordering::Relaxed) {
            use std::sync::atomic::Ordering::Relaxed;
            LIVE.fetch_sub(layout.size(), Relaxed);
            note_alloc(new_size);
        }
        out
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn reset_peak() -> usize {
    use std::sync::atomic::Ordering::Relaxed;
    ALLOCS.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

fn peak_bytes() -> usize {
    PEAK.load(std::sync::atomic::Ordering::Relaxed)
}

/// A tiny HTTP server over `tests/compat`, so `--serve` can exercise the
/// parts of the platform that need a real origin: `fetch`, cookies, CORS and
/// the HTTP cache.
///
/// Deliberately not a general-purpose server: `Connection: close`, one
/// request per connection, no keep-alive, no chunked bodies.
struct CompatServer {
    port: u16,
    /// Request counts per path, so a page can prove a response was cached.
    counts: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl CompatServer {
    fn start(base: &std::path::Path) -> Result<CompatServer, String> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("não foi possível abrir a porta: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("endereço local indisponível: {e}"))?
            .port();
        let counts = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::<String, usize>::new(),
        ));
        let root = base.to_path_buf();
        let shared = counts.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let root = root.clone();
                let counts = shared.clone();
                std::thread::spawn(move || {
                    let _ = serve_one(stream, &root, &counts);
                });
            }
        });
        Ok(CompatServer { port, counts })
    }

    fn url_for(&self, file: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, file)
    }

    /// How many times a path was requested since the server started.
    #[allow(dead_code)]
    fn count(&self, path: &str) -> usize {
        self.counts
            .lock()
            .map(|counts| counts.get(path).copied().unwrap_or(0))
            .unwrap_or(0)
    }
}

fn serve_one(
    mut stream: std::net::TcpStream,
    root: &std::path::Path,
    counts: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Write};

    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // Read the headers so the client's write completes; the values that
    // matter (`If-None-Match`) are captured here.
    let mut content_length = 0usize;
    let mut if_none_match = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            match name.as_str() {
                "content-length" => content_length = value.parse().unwrap_or(0),
                "if-none-match" => if_none_match = value,
                _ => {}
            }
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        use std::io::Read;
        reader.read_exact(&mut body)?;
    }

    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.clone(), String::new()),
    };
    let path = path.trim_start_matches('/').to_string();
    if let Ok(mut counts) = counts.lock() {
        *counts.entry(format!("/{path}")).or_insert(0) += 1;
    }

    let local_port = stream.local_addr().map(|a| a.port()).unwrap_or(0);
    let mut respond = |status: &str, headers: &[(&str, String)], body: &[u8]| -> std::io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(body)?;
        stream.flush()
    };

    // `/__port` — the port this server listens on, so a page can build a
    // second origin from the same host (`127.0.0.1` vs `localhost` is a
    // different origin, which is exactly what a CORS check needs).
    if path == "__port" {
        let port = local_port.to_string();
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            port.as_bytes(),
        );
    }

    // `/__count?p=/page.html` — how many times that path was requested.
    if path == "__count" {
        let wanted = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("p="))
            .map(|p| {
                let decoded = p.replace("%2F", "/").replace("%2f", "/");
                if decoded.starts_with('/') {
                    decoded
                } else {
                    format!("/{decoded}")
                }
            })
            .unwrap_or_else(|| "/".to_string());
        let seen = counts
            .lock()
            .map(|counts| counts.get(&wanted).copied().unwrap_or(0))
            .unwrap_or(0);
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            seen.to_string().as_bytes(),
        );
    }

    // `/__cookie` — a server-set cookie, so `document.cookie` has something
    // to read back that script did not write itself.
    if path == "__cookie" {
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain".to_string()),
                ("Set-Cookie", "srv=1; Path=/; HttpOnly".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            b"cookie-set",
        );
    }

    // `/__cache.txt` — cacheable for a minute, counted, so a page can prove
    // the second request never reached the server.
    if path == "__cache.txt" {
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain; charset=utf-8".to_string()),
                ("Cache-Control", "max-age=60".to_string()),
            ],
            "corpo-cache".as_bytes(),
        );
    }

    // `/__etag.txt` — revalidated on every request: `max-age=0` plus an ETag,
    // so the second request must come back as a 304 and still yield a body.
    if path == "__etag.txt" {
        if if_none_match.contains("\"v1\"") {
            return respond(
                "304 Not Modified",
                &[("ETag", "\"v1\"".to_string())],
                b"",
            );
        }
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain; charset=utf-8".to_string()),
                ("Cache-Control", "max-age=0".to_string()),
                ("ETag", "\"v1\"".to_string()),
            ],
            "corpo-etag".as_bytes(),
        );
    }

    // `/__latin1.txt` — a body that is only valid as windows-1252.
    if path == "__latin1.txt" {
        return respond(
            "200 OK",
            &[(
                "Content-Type",
                "text/plain; charset=windows-1252".to_string(),
            )],
            &[0xE7, b'a', b'f', 0xE9],
        );
    }

    // `/__cors` — a cross-origin response that allows everyone.
    if path == "__cors" {
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain; charset=utf-8".to_string()),
                ("Access-Control-Allow-Origin", "*".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            b"cors-ok",
        );
    }

    // `/__nocors` — a cross-origin response that allows nobody.
    if path == "__nocors" {
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/plain; charset=utf-8".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            b"segredo",
        );
    }

    // `/__csp.html` — a page whose policy allows only its own origin, so the
    // external script it references must be dropped.
    if path == "__csp.html" {
        let body = br#"<!doctype html><html><head><title>csp</title>
<meta http-equiv="Content-Security-Policy" content="script-src 'self'">
</head><body><p>pagina com csp</p>
<script src="__cors-script.js"></script>
</body></html>"#;
        return respond(
            "200 OK",
            &[
                ("Content-Type", "text/html; charset=utf-8".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            body,
        );
    }

    // `/__json` — a JSON body for `response.json()`.
    if path == "__json" {
        return respond(
            "200 OK",
            &[
                ("Content-Type", "application/json".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            br#"{"ok":true,"n":2}"#,
        );
    }

    // `/__post` — echoes the request body, so a POST can be verified.
    if path == "__post" {
        if method == "POST" {
            return respond(
                "200 OK",
                &[
                    ("Content-Type", "text/plain; charset=utf-8".to_string()),
                    ("Cache-Control", "no-store".to_string()),
                ],
                format!("recebido:{content_length}").as_bytes(),
            );
        }
        return respond("405 Method Not Allowed", &[], b"");
    }

    let file = root.join(&path);
    let Ok(bytes) = std::fs::read(&file) else {
        return respond(
            "404 Not Found",
            &[("Content-Type", "text/plain".to_string())],
            b"nao encontrado",
        );
    };
    let content_type = match file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => "text/plain; charset=utf-8",
    };
    // A fixture may need to name this server's own port (to build a second
    // origin), so `__PORT__` is substituted in HTML bodies. Every other type
    // is sent byte for byte: a PNG put through a string conversion is a
    // corrupt PNG.
    let body = if content_type.starts_with("text/html") {
        String::from_utf8_lossy(&bytes)
            .replace("__PORT__", &local_port.to_string())
            .into_bytes()
    } else {
        bytes
    };
    respond(
        "200 OK",
        &[
            ("Content-Type", content_type.to_string()),
            // Pages must not be cached: the corpus is re-read every run.
            ("Cache-Control", "no-store".to_string()),
        ],
        &body,
    )
}

/// Run the compatibility corpus. Each manifest line is
/// `<file> | key:value | ...`; see `tests/compat/manifest.txt`.
fn run_compat(dir: &str, serve: bool, verbose: bool) -> i32 {
    let base = std::path::Path::new(dir);
    let manifest = match std::fs::read_to_string(base.join("manifest.txt")) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("naro: manifesto não encontrado em {dir}: {err}");
            return 2;
        }
    };
    // `--serve` puts the corpus behind a real HTTP origin, which is what
    // `fetch`, `XMLHttpRequest`, cookies and CORS need to be exercised at all:
    // a `file://` page has no origin to compare against.
    let server = if serve {
        match CompatServer::start(base) {
            Ok(server) => Some(server),
            Err(err) => {
                eprintln!("naro: servidor de compatibilidade indisponível: {err}");
                return 2;
            }
        }
    } else {
        None
    };
    let mut pass = 0;
    let mut fail = 0;
    let mut unsupported = 0;
    let mut failures: Vec<String> = Vec::new();

    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('|').map(|p| p.trim());
        let file = match parts.next() {
            Some(f) => f,
            None => continue,
        };
        let url = match &server {
            Some(server) => server.url_for(file),
            None => {
                let path = std::fs::canonicalize(base.join(file))
                    .unwrap_or_else(|_| base.join(file));
                match url::Url::from_file_path(&path) {
                    Ok(u) => u.to_string(),
                    Err(_) => {
                        fail += 1;
                        failures.push(format!("{file}: caminho inválido"));
                        continue;
                    }
                }
            }
        };

        let mut engine = match Engine::new(1.0) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("naro: {err}");
                return 2;
            }
        };
        engine.set_viewport(1000.0, 700.0);
        let nav = engine.navigate(&url, true);
        if let Some(fetch_url) = nav.fetch {
            let result = naro_net::with_session(|net| net.fetch(&fetch_url));
            engine.apply_fetch(nav.generation, result);
        }

        let mut notes: Vec<String> = Vec::new();
        let mut is_unsupported = false;
        let mut ok = true;
        for check in parts {
            let (key, value) = match check.split_once(':') {
                Some(kv) => kv,
                None => (check, ""),
            };
            match key {
                "unsupported" => {
                    is_unsupported = true;
                    notes.push(format!("não suportado: {value}"));
                }
                "served" => {
                    // served[:motivo] — the case only means anything behind a
                    // real origin, so it is skipped (not failed) on `file://`.
                    if !serve {
                        is_unsupported = true;
                        let reason = if value.is_empty() {
                            "requer --serve"
                        } else {
                            value
                        };
                        notes.push(format!("não suportado: {reason}"));
                    }
                }
                "text" => {
                    if !engine.has_text(value) {
                        ok = false;
                        notes.push(format!("texto ausente: {value:?}"));
                    }
                }
                "notext" => {
                    if engine.has_text(value) {
                        ok = false;
                        notes.push(format!("texto inesperado: {value:?}"));
                    }
                }
                "imgs" => {
                    let want: usize = value.parse().unwrap_or(0);
                    let got = engine.layout.as_ref().map(|l| l.images.len()).unwrap_or(0);
                    if got != want {
                        ok = false;
                        notes.push(format!("imagens {got} != {want}"));
                    }
                }
                "blocks" => {
                    let want: usize = value.parse().unwrap_or(0);
                    let got = engine.layout.as_ref().map(|l| l.blocks.len()).unwrap_or(0);
                    if got < want {
                        ok = false;
                        notes.push(format!("blocos {got} < {want}"));
                    }
                }
                "runs" => {
                    let want: usize = value.parse().unwrap_or(0);
                    let got = engine.layout.as_ref().map(|l| l.text_runs.len()).unwrap_or(0);
                    if got < want {
                        ok = false;
                        notes.push(format!("runs {got} < {want}"));
                    }
                }
                "log" => {
                    if !engine.js.logs.iter().any(|l| l.contains(value)) {
                        ok = false;
                        notes.push(format!("log ausente: {value:?}"));
                    }
                }
                "underline" => {
                    let want: usize = value.parse().unwrap_or(1);
                    let got = engine
                        .layout
                        .as_ref()
                        .map(|l| l.text_runs.iter().filter(|r| r.underline).count())
                        .unwrap_or(0);
                    if got < want {
                        ok = false;
                        notes.push(format!("sublinhados {got} < {want}"));
                    }
                }
                "blockh" => {
                    // blockh:<tag>:<min height>
                    let mut it = value.split(':');
                    let tag = it.next().unwrap_or("");
                    let want: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let got = engine
                        .layout
                        .as_ref()
                        .and_then(|l| l.blocks.iter().find(|b| b.tag == tag))
                        .map(|b| b.rect.h)
                        .unwrap_or(0.0);
                    if got + 0.5 < want {
                        ok = false;
                        notes.push(format!("altura de {tag} {got:.1} < {want:.1}"));
                    }
                }
                "click" => {
                    // click:<css-selector> — dispatch a click on the centre of
                    // the first matching element, then keep evaluating checks.
                    let target = engine.doc.as_ref().and_then(|doc| {
                        naro_css::query_selector(doc, value).map(|node| (node, doc))
                    });
                    let rect = target.and_then(|(node, _)| {
                        engine
                            .layout
                            .as_ref()
                            .and_then(|l| l.blocks.iter().find(|b| b.node == node))
                            .map(|b| b.rect)
                    });
                    match rect {
                        Some(rect) => {
                            let (outcome, _) =
                                engine.click_at(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                            engine.js.logs.extend(outcome.logs);
                            engine.js.errors.extend(outcome.errors);
                        }
                        None => {
                            ok = false;
                            notes.push(format!("alvo de clique ausente: {value:?}"));
                        }
                    }
                }
                "choose" => {
                    // choose:<select-selector>=<option text> — open the select
                    // and pick that option, the way the popup does. The popup
                    // itself is chrome and has no place in a headless run, so
                    // the check drives the engine call behind it.
                    let Some((selector, wanted)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("choose malformado: {value:?}"));
                        continue;
                    };
                    let node = engine
                        .doc
                        .as_ref()
                        .and_then(|doc| naro_css::query_selector(doc, selector));
                    let Some(node) = node else {
                        ok = false;
                        notes.push(format!("select ausente: {selector:?}"));
                        continue;
                    };
                    let options = engine.open_select(node);
                    match options.iter().find(|(_, text, _)| text == wanted) {
                        Some((option, _, _)) => engine.choose_option(node, *option),
                        None => {
                            ok = false;
                            notes.push(format!(
                                "opção ausente: {wanted:?} em {selector:?} ({:?})",
                                options.iter().map(|(_, t, _)| t.clone()).collect::<Vec<_>>()
                            ));
                        }
                    }
                }
                "submit" => {
                    // submit:<selector>=<get|post>[:<trecho do corpo>] — click
                    // the control and report the navigation it asked for. The
                    // runner never follows it, so a form can be verified
                    // without a network round trip.
                    let Some((selector, want)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("submit malformado: {value:?}"));
                        continue;
                    };
                    let (want_method, want_body) = match want.split_once(':') {
                        Some((method, body)) => (method, Some(body)),
                        None => (want, None),
                    };
                    let node = engine
                        .doc
                        .as_ref()
                        .and_then(|doc| naro_css::query_selector(doc, selector));
                    let Some(node) = node else {
                        ok = false;
                        notes.push(format!("controle ausente: {selector:?}"));
                        continue;
                    };
                    engine.default_click(node);
                    let got = engine.take_pending_nav();
                    let (got_method, got_body) = match &got {
                        Some(nav) if nav.is_post() => ("post", nav.body.clone().unwrap_or_default()),
                        Some(nav) => ("get", nav.url.clone()),
                        None => ("none", String::new()),
                    };
                    let method_ok = got_method == want_method;
                    let body_ok = match want_body {
                        Some(body) => got_body.contains(body),
                        None => true,
                    };
                    if !method_ok || !body_ok {
                        ok = false;
                        notes.push(format!(
                            "submit {selector:?} {got_method} {got_body:?} != {want:?}"
                        ));
                    }
                }
                "selection" => {
                    // selection:x1,y1,x2,y2=<texto> — apply a selection and
                    // compare the text it yields. The engine owns the
                    // selection, so this is the only honest way to check it
                    // without a clipboard.
                    let Some((coords, want)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("selection malformada: {value:?}"));
                        continue;
                    };
                    let nums: Vec<f32> = coords
                        .split(',')
                        .filter_map(|v| v.trim().parse::<f32>().ok())
                        .collect();
                    if nums.len() != 4 {
                        ok = false;
                        notes.push(format!("selection sem 4 coordenadas: {coords:?}"));
                        continue;
                    }
                    engine.set_selection((nums[0], nums[1]), (nums[2], nums[3]));
                    let got = engine.selection_text();
                    if got != want {
                        ok = false;
                        notes.push(format!("seleção {got:?} != {want:?}"));
                    }
                }
                "restyle" => {
                    let want: u32 = value.parse().unwrap_or(0);
                    if engine.restyle_count != want {
                        ok = false;
                        notes.push(format!("restyle {} != {want}", engine.restyle_count));
                    }
                }
                "relayout" => {
                    let want: u32 = value.parse().unwrap_or(0);
                    if engine.relayout_count != want {
                        ok = false;
                        notes.push(format!("relayout {} != {want}", engine.relayout_count));
                    }
                }
                "box" => {
                    // box:<selector>:<w>x<h> — the first block for that
                    // selector must be that size. Unlike `blockw`, this pins
                    // the element, not "the first div on the page", so a
                    // layout change elsewhere cannot silently satisfy it.
                    let mut parts = value.rsplitn(2, ':');
                    let size = parts.next().unwrap_or("");
                    let selector = parts.next().unwrap_or("");
                    let mut it = size.split('x');
                    let want_w: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1.0);
                    let want_h: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1.0);
                    let rect = engine.doc.as_ref().and_then(|doc| {
                        naro_css::query_selector(doc, selector).and_then(|node| {
                            engine
                                .layout
                                .as_ref()
                                .and_then(|l| l.blocks.iter().find(|b| b.node == node))
                                .map(|b| b.rect)
                        })
                    });
                    match rect {
                        Some(rect) => {
                            if (rect.w - want_w).abs() > 0.6 || (rect.h - want_h).abs() > 0.6 {
                                ok = false;
                                notes.push(format!(
                                    "caixa de {selector:?} {:.1}x{:.1} != {want_w}x{want_h}",
                                    rect.w, rect.h
                                ));
                            }
                        }
                        None => {
                            ok = false;
                            notes.push(format!("caixa ausente: {selector:?}"));
                        }
                    }
                }
                "blockw" => {
                    // blockw:<tag>:<min width> — first block of that tag.
                    let mut it = value.split(':');
                    let tag = it.next().unwrap_or("");
                    let want: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let got = engine
                        .layout
                        .as_ref()
                        .and_then(|l| l.blocks.iter().find(|b| b.tag == tag))
                        .map(|b| b.rect.w)
                        .unwrap_or(0.0);
                    if got + 0.5 < want {
                        ok = false;
                        notes.push(format!("largura de {tag} {got:.1} < {want:.1}"));
                    }
                }
                "runcolor" => {
                    // runcolor:RRGGBB — some text run has exactly this colour.
                    let want = value.trim_start_matches('#').to_ascii_lowercase();
                    let got = engine
                        .layout
                        .as_ref()
                        .map(|l| {
                            l.text_runs
                                .iter()
                                .any(|r| r.color.to_hex().trim_start_matches('#') == want)
                        })
                        .unwrap_or(false);
                    if !got {
                        ok = false;
                        notes.push(format!("cor ausente: #{want}"));
                    }
                }
                "runfont" => {
                    // runfont:<family> — some text run asks for this family.
                    // The run keeps the whole `font-family` list, so the check
                    // passes when any entry matches, compared without case and
                    // without the quotes the cascade strips.
                    let want = value.trim().trim_matches(|c| c == '"' || c == '\'').to_lowercase();
                    let got = engine
                        .layout
                        .as_ref()
                        .map(|l| {
                            l.text_runs.iter().any(|r| {
                                r.family.split(',').any(|f| {
                                    f.trim()
                                        .trim_matches(|c| c == '"' || c == '\'')
                                        .to_lowercase()
                                        == want
                                })
                            })
                        })
                        .unwrap_or(false);
                    if !got {
                        ok = false;
                        notes.push(format!("família ausente: {want:?}"));
                    }
                }
                "hit" | "nothit" => {
                    // hit:<x>,<y>=<selector> — `engine.hit_node` resolves to a
                    // node matching the selector. `nothit` is the negation and
                    // passes when there is no node at all. This is what proves
                    // transformed hit-testing end to end, through the runner.
                    let Some((coords, selector)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("{key} malformado: {value:?}"));
                        continue;
                    };
                    let nums: Vec<f32> = coords
                        .split(',')
                        .filter_map(|v| v.trim().parse::<f32>().ok())
                        .collect();
                    if nums.len() != 2 {
                        ok = false;
                        notes.push(format!("{key} sem duas coordenadas: {coords:?}"));
                        continue;
                    }
                    let hit = engine.hit_node(nums[0], nums[1]);
                    let matches = match (&engine.doc, hit) {
                        (Some(doc), Some(node)) => {
                            let want = naro_css::parse_selector_list_text(selector);
                            want.iter().any(|sel| sel.matches(doc, node))
                        }
                        _ => false,
                    };
                    if key == "hit" && !matches {
                        ok = false;
                        notes.push(format!(
                            "hit {coords} não casa com {selector:?} (nó {hit:?})"
                        ));
                    } else if key == "nothit" && matches {
                        ok = false;
                        notes.push(format!("hit inesperado {coords} em {selector:?}"));
                    }
                }
                "pixel" => {
                    // pixel:<x>,<y>=RRGGBB — paint the page into a 1000x700
                    // canvas and read that pixel. The only check that proves
                    // what the rasterizer actually produced, not what the
                    // layout intended.
                    let Some((coords, want)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("pixel malformado: {value:?}"));
                        continue;
                    };
                    let nums: Vec<f32> = coords
                        .split(',')
                        .filter_map(|v| v.trim().parse::<f32>().ok())
                        .collect();
                    if nums.len() != 2 {
                        ok = false;
                        notes.push(format!("pixel sem duas coordenadas: {coords:?}"));
                        continue;
                    }
                    let want = want.trim().trim_start_matches('#').to_ascii_lowercase();
                    let want_value = u32::from_str_radix(&want, 16).unwrap_or(0);
                    let (px, py) = (nums[0].round() as i32, nums[1].round() as i32);
                    if px < 0 || py < 0 || px >= 1000 || py >= 700 {
                        ok = false;
                        notes.push(format!("pixel fora da tela: {px},{py}"));
                        continue;
                    }
                    let mut pixels = vec![0u32; 1000 * 700];
                    let mut canvas = Canvas::new(&mut pixels, 1000, 700, 1.0);
                    canvas.clear(Color::WHITE);
                    engine.paint_page(
                        &mut canvas,
                        Rect { x: 0.0, y: 0.0, w: 1000.0, h: 700.0 },
                    );
                    let got = canvas.pixels[py as usize * 1000 + px as usize] & 0x00FF_FFFF;
                    let close = |shift: u32| -> bool {
                        let a = ((got >> shift) & 0xFF) as i32;
                        let b = ((want_value >> shift) & 0xFF) as i32;
                        (a - b).abs() <= 2
                    };
                    if !(close(16) && close(8) && close(0)) {
                        ok = false;
                        notes.push(format!(
                            "pixel {px},{py} #{got:06x} != #{want_value:06x}"
                        ));
                    }
                }
                "boxdiff" => {
                    // boxdiff:<selA>:<selB>:<min px> — the two boxes differ in
                    // width by at least `min`. Proves that a family change
                    // reaches measurement without pinning a number that
                    // depends on the fonts installed.
                    let Some((pair, want)) = value.rsplit_once(':') else {
                        ok = false;
                        notes.push(format!("boxdiff malformado: {value:?}"));
                        continue;
                    };
                    let Some((sel_a, sel_b)) = pair.rsplit_once(':') else {
                        ok = false;
                        notes.push(format!("boxdiff sem dois seletores: {value:?}"));
                        continue;
                    };
                    let min: f32 = want.trim().parse().unwrap_or(0.0);
                    let width_of = |selector: &str| -> Option<f32> {
                        let doc = engine.doc.as_ref()?;
                        let node = naro_css::query_selector(doc, selector)?;
                        engine
                            .layout
                            .as_ref()?
                            .blocks
                            .iter()
                            .find(|b| b.node == node)
                            .map(|b| b.rect.w)
                    };
                    match (width_of(sel_a), width_of(sel_b)) {
                        (Some(a), Some(b)) => {
                            if (a - b).abs() < min {
                                ok = false;
                                notes.push(format!(
                                    "larguras de {sel_a:?} ({a:.1}) e {sel_b:?} ({b:.1}) diferem menos que {min}"
                                ));
                            }
                        }
                        _ => {
                            ok = false;
                            notes.push(format!("caixa ausente em boxdiff: {value:?}"));
                        }
                    }
                }
                "blockbg" => {
                    // blockbg:<tag>:RRGGBB — some block of that tag has this
                    // background, which is a paint-only property.
                    let mut it = value.split(':');
                    let tag = it.next().unwrap_or("");
                    let want = it.next().unwrap_or("").trim_start_matches('#').to_ascii_lowercase();
                    let got = engine
                        .layout
                        .as_ref()
                        .map(|l| {
                            l.blocks.iter().any(|b| {
                                b.tag == tag
                                    && b.background.to_hex().trim_start_matches('#') == want
                            })
                        })
                        .unwrap_or(false);
                    if !got {
                        ok = false;
                        notes.push(format!("fundo de {tag} ausente: #{want}"));
                    }
                }
                "noruncolor" => {
                    // noruncolor:RRGGBB — no text run has this colour.
                    let want = value.trim_start_matches('#').to_ascii_lowercase();
                    let got = engine
                        .layout
                        .as_ref()
                        .map(|l| {
                            l.text_runs
                                .iter()
                                .any(|r| r.color.to_hex().trim_start_matches('#') == want)
                        })
                        .unwrap_or(false);
                    if got {
                        ok = false;
                        notes.push(format!("cor ainda presente: #{want}"));
                    }
                }
                "computed" => {
                    // computed:<selector>:<prop>=<value> — the cascaded value
                    // of a property on the first element matching the
                    // selector. This is what `getComputedStyle` reports, so a
                    // page and its check share one formatter.
                    let Some((target, want)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("computed malformado: {value:?}"));
                        continue;
                    };
                    let Some((selector, prop)) = target.rsplit_once(':') else {
                        ok = false;
                        notes.push(format!("computed sem propriedade: {value:?}"));
                        continue;
                    };
                    let Some(node) = engine
                        .doc
                        .as_ref()
                        .and_then(|doc| naro_css::query_selector(doc, selector))
                    else {
                        ok = false;
                        notes.push(format!("seletor sem nó: {selector:?}"));
                        continue;
                    };
                    match naro_css::computed_value(&engine.styles[node], prop) {
                        Some(got) => {
                            if got != want {
                                ok = false;
                                notes.push(format!(
                                    "{selector} {prop} {got:?} != {want:?}"
                                ));
                            }
                        }
                        None => {
                            ok = false;
                            notes.push(format!("propriedade não suportada: {prop:?}"));
                        }
                    }
                }
                "attr" => {
                    // attr:<selector>:<name>=<value> — the attribute value on
                    // the first element matching the selector. `""` matches a
                    // present-but-empty attribute; use `attr?` for presence.
                    let Some((target, want)) = value.split_once('=') else {
                        ok = false;
                        notes.push(format!("attr malformado: {value:?}"));
                        continue;
                    };
                    let Some((selector, name)) = target.rsplit_once(':') else {
                        ok = false;
                        notes.push(format!("attr sem nome: {value:?}"));
                        continue;
                    };
                    let got = engine.doc.as_ref().and_then(|doc| {
                        naro_css::query_selector(doc, selector).and_then(|node| doc.attr(node, name))
                    });
                    match got {
                        Some(got) if got == want => {}
                        Some(got) => {
                            ok = false;
                            notes.push(format!("{selector} @{name} {got:?} != {want:?}"));
                        }
                        None => {
                            ok = false;
                            notes.push(format!("atributo ausente: {selector} @{name}"));
                        }
                    }
                }
                "blocked" => {
                    // blocked:<trecho> — a subresource was refused by the
                    // origin policy, and the report names it.
                    let hit = engine
                        .js
                        .errors
                        .iter()
                        .any(|e| e.contains("bloqueado") && e.contains(value));
                    if !hit {
                        ok = false;
                        notes.push(format!("recurso não bloqueado: {value:?}"));
                    }
                }
                "nojsrerror" => {
                    if let Some(err) = engine.js.errors.first() {
                        ok = false;
                        notes.push(format!("erro JS: {err}"));
                    }
                }
                _ => {}
            }
        }

        if verbose {
            for line in &engine.js.logs {
                println!("   LOG {file}: {line}");
            }
            for line in &engine.js.errors {
                println!("   ERR {file}: {line}");
            }
        }

        if is_unsupported {
            unsupported += 1;
            println!("UNSUP  {file}  ({})", notes.join("; "));
        } else if ok {
            pass += 1;
            println!("PASS   {file}");
        } else {
            fail += 1;
            let detail = notes.join("; ");
            println!("FAIL   {file}  {detail}");
            failures.push(format!("{file}: {detail}"));
        }
    }

    println!();
    println!(
        "resumo: {pass} pass, {fail} falhas, {unsupported} não suportados (total {})",
        pass + fail + unsupported
    );
    if !failures.is_empty() {
        println!("falhas:");
        for line in &failures {
            println!("  - {line}");
        }
    }
    if fail > 0 {
        1
    } else {
        0
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(|s| s.as_str()) == Some("--form") {
        // naro --form <url> [--click x y] [--type text] [--key backspace|tab|enter]
        let target = match args.get(1) {
            Some(t) => t.clone(),
            None => {
                eprintln!("uso: naro --form <url> [--click x y] [--type texto] [--key nome]");
                std::process::exit(2);
            }
        };
        let mut engine = load_for_headless(&target, 1000.0);
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--click" => {
                    let x: f32 = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let y: f32 = args.get(i + 2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let (outcome, _) = engine.click_at(x, y);
                    engine.js.errors.extend(outcome.errors);
                    engine.js.logs.extend(outcome.logs);
                    i += 3;
                }
                "--focus" => {
                    if let Some(node) = args.get(i + 1).and_then(|v| v.parse::<usize>().ok()) {
                        engine.focus_node(node);
                    }
                    i += 2;
                }
                "--type" => {
                    if let Some(text) = args.get(i + 1) {
                        engine.insert_text(text);
                    }
                    i += 2;
                }
                "--key" => {
                    match args.get(i + 1).map(|s| s.as_str()) {
                        Some("backspace") => engine.backspace(),
                        Some("tab") => engine.focus_step(true),
                        Some("enter") => engine.press_enter(),
                        Some("blur") => engine.blur(),
                        _ => {}
                    }
                    i += 2;
                }
                _ => i += 1,
            }
        }
        println!("foco: {:?}", engine.focus);
        if let Some(node) = engine.focus {
            println!("valor: {:?}", engine.control_value_of(node));
        }
        print!("{}", engine.dump_tree());
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--compat") {
        let mut dir = "tests/compat".to_string();
        let mut serve = false;
        let mut verbose = false;
        for arg in args.iter().skip(1) {
            match arg.as_str() {
                "--serve" => serve = true,
                "--verbose" => verbose = true,
                other => dir = other.to_string(),
            }
        }
        std::process::exit(run_compat(&dir, serve, verbose));
    }

    if args.first().map(|s| s.as_str()) == Some("--bench") {
        let target = args.get(1).cloned().unwrap_or_else(|| START_URL.to_string());
        let iters = args.get(2).and_then(|v| v.parse::<usize>().ok()).unwrap_or(20);
        let width = args.get(3).and_then(|v| v.parse::<f32>().ok()).unwrap_or(1000.0);

        let base = reset_peak();
        let mut engine = match Engine::new(1.0) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("naro: {err}");
                std::process::exit(1);
            }
        };
        let url = normalize_input(&target);
        let nav = engine.navigate(&url, true);
        let html = match nav.fetch {
            Some(fetch_url) => match naro_net::with_session(|net| net.fetch(&fetch_url)) {
                Ok(page) => page.html,
                Err(err) => {
                    eprintln!("naro: {err}");
                    std::process::exit(1);
                }
            },
            None => match url.as_str() {
                naro_engine::START_URL => naro_engine::START_HTML.to_string(),
                _ => {
                    let path = url::Url::parse(&url)
                        .ok()
                        .and_then(|u| u.to_file_path().ok());
                    match path.and_then(|p| std::fs::read(p).ok()) {
                        Some(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                        None => {
                            eprintln!("naro: alvo não suportado em --bench: {url}");
                            std::process::exit(1);
                        }
                    }
                }
            },
        };

        let timings = naro_engine::bench_phases(&engine.fonts, &html, width, iters);
        engine.set_viewport(width, 700.0);
        // End-to-end load, timed separately: it includes the real style
        // pipeline (with its sheet cache) and the script run, which the
        // per-phase medians above deliberately do not.
        let t_load = std::time::Instant::now();
        engine.set_html(&url, &html);
        let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;

        let mut pixels = vec![0u32; width as usize * 700];
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let mut canvas = Canvas::new(&mut pixels, width as u32, 700, 1.0);
            canvas.clear(Color::WHITE);
            engine.paint_page(&mut canvas, Rect { x: 0.0, y: 0.0, w: width, h: 700.0 });
        }
        let paint_ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;

        println!("alvo: {url}");
        println!("bytes html: {}", html.len());
        println!("parse  ms: {:.3}", timings.parse_ms);
        println!("css    ms: {:.3}", timings.css_ms);
        println!("style  ms: {:.3}", timings.style_ms);
        println!("layout ms: {:.3}", timings.layout_ms);
        println!("load   ms: {:.3}", load_ms);
        println!("paint  ms: {:.3}", paint_ms);
        println!(
            "alocs  n: {}",
            ALLOCS.load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "heap   KB: base={:.0} pico={:.0} (live após carga={:.0})",
            base as f64 / 1024.0,
            peak_bytes() as f64 / 1024.0,
            LIVE.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1024.0
        );
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--bench-shadow") {
        // Median cost of one `fill_shadow` per (blur, spread) pair. The clamp
        // is active, so the last pair reports an effective value below the
        // requested one — that is the budget doing its job, and the numbers
        // belong in `reports/milestone-v0.3.md`.
        let iters = args.get(1).and_then(|v| v.parse::<usize>().ok()).unwrap_or(20).max(1);
        let mut pixels = vec![0u32; 1000 * 700];
        let pairs = [(0.0f32, 0.0f32), (10.0, 0.0), (50.0, 25.0), (100.0, 50.0)];
        for (blur, spread) in pairs {
            let shadow = naro_css::BoxShadow {
                dx: 0.0,
                dy: 0.0,
                blur,
                spread,
                color: naro_css::Color { r: 0, g: 0, b: 0, a: 160 },
                inset: false,
            };
            let (effective_blur, effective_spread) =
                naro_paint::clamp_shadow(300.0, 300.0, blur, spread, 1.0);
            let mut samples = Vec::with_capacity(iters);
            for _ in 0..iters {
                let mut canvas = Canvas::new(&mut pixels, 1000, 700, 1.0);
                canvas.clear(Color::WHITE);
                let t = std::time::Instant::now();
                canvas.fill_shadow(350.0, 200.0, 300.0, 300.0, 0.0, &shadow);
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = samples[samples.len() / 2];
            println!(
                "shadow blur={} spread={} efetivo={},{} ms={:.3}",
                blur, spread, effective_blur, effective_spread, median
            );
        }
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--dump") {
        let target = args.get(1).cloned().unwrap_or_else(|| START_URL.to_string());
        let width = args.get(2).and_then(|w| w.parse::<f32>().ok()).unwrap_or(1000.0);
        let mut engine = load_for_headless(&target, width);
        // `--select x1 y1 x2 y2` applies a selection before dumping, so the
        // selected text can be verified without a clipboard or a mouse.
        let select_at = args
            .iter()
            .position(|a| a == "--select")
            .and_then(|at| {
                let coords: Vec<f32> = args
                    .iter()
                    .skip(at + 1)
                    .take(4)
                    .filter_map(|v| v.parse::<f32>().ok())
                    .collect();
                if coords.len() == 4 {
                    Some(((coords[0], coords[1]), (coords[2], coords[3])))
                } else {
                    None
                }
            });
        if let Some((anchor, focus)) = select_at {
            if engine.set_selection(anchor, focus) {
                println!("SELECTION: {}", engine.selection_text());
            } else {
                println!("SELECTION: ");
            }
        }
        // `--dump <url> <width> <x> <y>` dispatches a click before dumping.
        if select_at.is_none() {
            if let (Some(x), Some(y)) = (
                args.get(3).and_then(|v| v.parse::<f32>().ok()),
                args.get(4).and_then(|v| v.parse::<f32>().ok()),
            ) {
                let (outcome, consumed) = engine.click_at(x, y);
                engine.js.errors.extend(outcome.errors);
                engine.js.logs.extend(outcome.logs);
                if !consumed {
                    println!("CLICK: sem handler");
                }
            }
        }
        print!("{}", engine.dump_tree());
        if matches!(engine.status, Status::Error(_)) {
            std::process::exit(1);
        }
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--style") {
        // `--style <url> <selector>` — computed layout style of every match.
        // A read-only diagnostic: it explains why a box ended up where it did.
        let target = args.get(1).cloned().unwrap_or_else(|| START_URL.to_string());
        let selector = args.get(2).cloned().unwrap_or_else(|| "*".to_string());
        let engine = load_for_headless(&target, 1000.0);
        let doc = match &engine.doc {
            Some(doc) => doc,
            None => return,
        };
        let nodes = naro_css::query_selector_all(doc, &selector);
        if nodes.is_empty() {
            println!("nenhum nó casa com {selector:?}");
        }
        for node in nodes {
            let style = &engine.styles[node];
            println!(
                "{}{} display={:?} pos={:?} width={:?} height={:?} max-w={:?} min-w={:?}",
                doc.tag(node).unwrap_or("?"),
                doc.attr(node, "id").map(|v| format!("#{v}")).unwrap_or_default(),
                style.display,
                style.position,
                style.width.as_ref().map(|d| d.resolve(1000.0)),
                style.height.as_ref().map(|d| d.resolve(1000.0)),
                style.max_width.as_ref().map(|d| d.resolve(1000.0)),
                style.min_width.as_ref().map(|d| d.resolve(1000.0)),
            );
            println!(
                "   flex grow={} shrink={} basis={:?} dir={:?} wrap={:?} justify={:?} align={:?} align-content={:?}",
                style.flex_grow,
                style.flex_shrink,
                style.flex_basis,
                style.flex_direction,
                style.flex_wrap,
                style.justify_content,
                style.align_items,
                style.align_content,
            );
            println!(
                "   grid cols={:?} repeat={:?} gap={} rows={:?} col={:?}",
                style.grid_columns, style.grid_auto_repeat, style.grid_gap, style.grid_rows, style.grid_column,
            );
            let attrs: Vec<String> = doc
                .attrs(node)
                .map(|a| a.iter().map(|(n, v)| format!("{n}={v:?}")).collect())
                .unwrap_or_default();
            if !attrs.is_empty() {
                println!("   attrs {}", attrs.join(" "));
            }
            println!(
                "   color={} bg={} radius={} text-align={:?} margin={:?} padding={:?} border={:?} overflow={:?} table_fixed={}",
                style.color.to_hex(),
                style.background.to_hex(),
                style.border_radius,
                style.text_align,
                style.margin.resolve(1000.0),
                style.padding.resolve(1000.0),
                style.border,
                style.overflow,
                style.table_fixed,
            );
        }
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--hit") {
        // `--hit <url> <x> <y> [...]` — which node a click at those page
        // coordinates reaches, and every box under it. A read-only diagnostic
        // for hit-testing bugs.
        let target = args.get(1).cloned().unwrap_or_else(|| START_URL.to_string());
        let engine = load_for_headless(&target, 1000.0);
        let doc = match &engine.doc {
            Some(doc) => doc,
            None => return,
        };
        for pair in args[2..].chunks(2) {
            let (Some(x), Some(y)) = (
                pair.first().and_then(|v| v.parse::<f32>().ok()),
                pair.get(1).and_then(|v| v.parse::<f32>().ok()),
            ) else {
                continue;
            };
            println!("== ({x},{y})");
            if let Some(layout) = &engine.layout {
                let mut under: Vec<&naro_layout::BlockBox> = layout
                    .blocks
                    .iter()
                    .filter(|b| b.rect.contains(x, y))
                    .collect();
                under.sort_by(|a, b| (a.rect.w * a.rect.h).partial_cmp(&(b.rect.w * b.rect.h)).unwrap());
                for block in under.iter().take(6) {
                    let node = block.node;
                    println!(
                        "   {}#{} .{} rect=({:.1},{:.1},{:.1},{:.1}) paint={} clip={:?}",
                        doc.tag(node).unwrap_or("?"),
                        doc.attr(node, "id").unwrap_or(""),
                        doc.attr(node, "class").unwrap_or(""),
                        block.rect.x,
                        block.rect.y,
                        block.rect.w,
                        block.rect.h,
                        block.paint,
                        block.clip.is_some()
                    );
                }
            }
            match engine.hit_node(x, y) {
                Some(node) => println!(
                    "   HIT {}#{} class={:?}",
                    doc.tag(node).unwrap_or("?"),
                    doc.attr(node, "id").unwrap_or(""),
                    doc.attr(node, "class").unwrap_or("")
                ),
                None => println!("   HIT none"),
            }
        }
        return;
    }

    if args.first().map(|s| s.as_str()) == Some("--render") {
        let target = args.get(1).cloned().unwrap_or_else(|| START_URL.to_string());
        let output = args.get(2).cloned().unwrap_or_else(|| "out.bmp".to_string());
        let width = args.get(3).and_then(|w| w.parse::<f32>().ok()).unwrap_or(1000.0);
        let mut engine = load_for_headless(&target, width);
        // `--select x1 y1 x2 y2` applies a selection before rendering, so the
        // highlight itself can be inspected in the frame.
        let select_at = args
            .iter()
            .position(|a| a == "--select")
            .and_then(|at| {
                let coords: Vec<f32> = args
                    .iter()
                    .skip(at + 1)
                    .take(4)
                    .filter_map(|v| v.parse::<f32>().ok())
                    .collect();
                if coords.len() == 4 {
                    Some(((coords[0], coords[1]), (coords[2], coords[3])))
                } else {
                    None
                }
            });
        if let Some((anchor, focus)) = select_at {
            engine.set_selection(anchor, focus);
        }
        // `--render <url> <out> <width> <x> <y>` dispatches a click first, the
        // same way `--dump` does, so post-interaction frames can be inspected.
        if select_at.is_none() {
            if let (Some(x), Some(y)) = (
                args.get(4).and_then(|v| v.parse::<f32>().ok()),
                args.get(5).and_then(|v| v.parse::<f32>().ok()),
            ) {
                let (outcome, _) = engine.click_at(x, y);
                engine.js.errors.extend(outcome.errors);
                engine.js.logs.extend(outcome.logs);
            }
        }
        let mut pixels = vec![0u32; width as usize * 700];
        let mut canvas = Canvas::new(&mut pixels, width as u32, 700, 1.0);
        canvas.clear(Color::WHITE);
        engine.paint_page(&mut canvas, Rect { x: 0.0, y: 0.0, w: width, h: 700.0 });
        let bmp = canvas.to_bmp();
        match std::fs::write(&output, bmp) {
            Ok(()) => println!("renderizado: {output} ({width}x700)"),
            Err(err) => {
                eprintln!("naro: falha ao gravar {output}: {err}");
                std::process::exit(1);
            }
        }
        if matches!(engine.status, Status::Error(_)) {
            std::process::exit(1);
        }
        return;
    }

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(loop_) => loop_,
        Err(err) => {
            eprintln!("naro: falha ao iniciar o loop de eventos: {err}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let initial_url = args.first().map(|a| normalize_input(a));
    let mut app = match App::new(proxy, initial_url) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("naro: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("naro: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(count: usize) -> ChromeLayout {
        chrome_layout(1200.0, 800.0, count)
    }

    /// True when two rects share any area.
    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    #[test]
    fn tab_strip_occupies_the_top_and_content_starts_below_it() {
        let l = layout(1);
        // The strip is the title bar: it starts at the very top, with no band
        // above it.
        assert_eq!(l.tab_row.y, 0.0);
        assert_eq!(l.tab_row.h, TAB_ROW_H);
        // The page starts below the strip and the toolbar, and nothing is lost
        // between them.
        assert_eq!(l.toolbar.y, l.tab_row.h);
        assert_eq!(l.content.y, l.tab_row.h + l.toolbar.h);
        assert_eq!(l.content.y + l.content.h + l.status.h, 800.0);
    }

    #[test]
    fn window_controls_sit_at_the_right_of_the_tab_row() {
        let l = layout(1);
        assert!((l.win_close.x + l.win_close.w - 1200.0).abs() < 0.01);
        assert_eq!(l.win_close.y, 0.0);
        assert_eq!(l.win_close.h, TAB_ROW_H);
        // In order: minimize, maximize, close.
        assert!(l.win_min.x < l.win_max.x && l.win_max.x < l.win_close.x);
        // They never overlap each other.
        assert!(!overlaps(&l.win_min, &l.win_max));
        assert!(!overlaps(&l.win_max, &l.win_close));
    }

    #[test]
    fn tabs_never_overlap_each_other_or_the_window_controls() {
        for count in 1..=12 {
            let l = layout(count);
            assert_eq!(l.tabs.len(), count);
            for pair in l.tabs.windows(2) {
                assert!(
                    !overlaps(&pair[0], &pair[1]),
                    "tabs overlap at count {count}: {:?} {:?}",
                    pair[0],
                    pair[1]
                );
            }
            let last = l.tabs.last().unwrap();
            // The strip ends before the window controls, leaving a drag gap.
            assert!(last.x + last.w <= l.win_min.x, "count {count}");
            assert!(l.new_tab.x + l.new_tab.w <= l.win_min.x, "count {count}");
            // A close button lives inside every tab.
            for (tab, close) in l.tabs.iter().zip(l.tab_close.iter()) {
                assert!(close.x >= tab.x && close.x + close.w <= tab.x + tab.w);
                assert!(close.y >= tab.y && close.y + close.h <= tab.y + tab.h);
            }
        }
    }

    #[test]
    fn tab_width_shrinks_as_tabs_are_added() {
        // Few tabs sit at the maximum width; past that they shrink together.
        let one = layout(1).tabs[0].w;
        assert!((one - TAB_MAX_W).abs() < 0.01, "one {one}");
        let ten = layout(10).tabs[0].w;
        assert!(ten < one, "one {one} ten {ten}");
        // Never below the minimum, even with many tabs.
        let many = layout(30).tabs[0].w;
        assert!(many >= TAB_MIN_W - 0.01, "many {many}");
    }

    #[test]
    fn toolbar_buttons_do_not_intersect_the_address_bar() {
        let l = layout(3);
        assert!(!overlaps(&l.back, &l.addr));
        assert!(!overlaps(&l.reload, &l.addr));
        assert!(l.reload.x + l.reload.w <= l.addr.x);
        // The address bar stays inside the toolbar row.
        assert!(l.addr.y >= l.toolbar.y);
        assert!(l.addr.y + l.addr.h <= l.toolbar.y + l.toolbar.h);
    }

    #[test]
    fn resize_border_is_inside_the_window() {
        let l = layout(2);
        assert!(RESIZE_BORDER > 0.0);
        assert!(RESIZE_BORDER * 2.0 < l.tab_row.h);
        assert!(RESIZE_BORDER * 2.0 < l.content.h);
    }
}
