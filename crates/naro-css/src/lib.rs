//! Minimal CSS engine: parser, selectors, cascade and inheritance.
//!
//! Selectors cover the four combinators, attribute selectors, `:not`, the
//! structural pseudo-classes and `!important`. Deliberately partial: no
//! `@layer`, no animations, no pseudo-elements, and interactive pseudo-classes
//! parse but never match a static paint.

use naro_html::{Document, NodeId};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Default for Color {
    fn default() -> Self {
        Color::TRANSPARENT
    }
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim().to_ascii_lowercase();
        if let Some(hex) = s.strip_prefix('#') {
            return match hex.len() {
                3 => {
                    let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                    let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                    let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                    Some(Color::rgb(r * 17, g * 17, b * 17))
                }
                4 => {
                    // `#rgba`: the fourth digit is alpha.
                    let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                    let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                    let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                    let a = u8::from_str_radix(&hex[3..4], 16).ok()?;
                    Some(Color { r: r * 17, g: g * 17, b: b * 17, a: a * 17 })
                }
                6 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                    Some(Color::rgb(r, g, b))
                }
                8 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                    let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                    Some(Color { r, g, b, a })
                }
                _ => None,
            };
        }
        if let Some(inner) = s.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
            let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
            if parts.len() == 3 {
                let r = parts[0].parse::<u8>().ok()?;
                let g = parts[1].parse::<u8>().ok()?;
                let b = parts[2].parse::<u8>().ok()?;
                return Some(Color::rgb(r, g, b));
            }
            return None;
        }
        if let Some(inner) = s.strip_prefix("rgba(").and_then(|v| v.strip_suffix(')')) {
            let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
            if parts.len() == 4 {
                let r = parts[0].parse::<u8>().ok()?;
                let g = parts[1].parse::<u8>().ok()?;
                let b = parts[2].parse::<u8>().ok()?;
                // `0.5` and `.5` are both valid; anything else is invalid CSS.
                let a = parts[3].parse::<f32>().ok()?.clamp(0.0, 1.0);
                return Some(Color { r, g, b, a: (a * 255.0).round() as u8 });
            }
            return None;
        }
        for (prefix, has_alpha) in [("hsl(", false), ("hsla(", true)] {
            if let Some(inner) = s.strip_prefix(prefix).and_then(|v| v.strip_suffix(')')) {
                let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
                if parts.len() != if has_alpha { 4 } else { 3 } {
                    return None;
                }
                let hue = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let sat = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                let light = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                let (r, g, b) = hsl_to_rgb(hue, sat.clamp(0.0, 1.0), light.clamp(0.0, 1.0));
                let a = if has_alpha {
                    (parts[3].parse::<f32>().ok()?.clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    255
                };
                return Some(Color { r, g, b, a });
            }
        }
        let named = match s.as_str() {
            "transparent" => Color::TRANSPARENT,
            "black" => Color::BLACK,
            "white" => Color::WHITE,
            "red" => Color::rgb(255, 0, 0),
            "green" => Color::rgb(0, 128, 0),
            "blue" => Color::rgb(0, 0, 255),
            "gray" | "grey" => Color::rgb(128, 128, 128),
            "silver" => Color::rgb(192, 192, 192),
            "yellow" => Color::rgb(255, 255, 0),
            "orange" => Color::rgb(255, 165, 0),
            "purple" => Color::rgb(128, 0, 128),
            "navy" => Color::rgb(0, 0, 128),
            "teal" => Color::rgb(0, 128, 128),
            "maroon" => Color::rgb(128, 0, 0),
            "olive" => Color::rgb(128, 128, 0),
            "lime" => Color::rgb(0, 255, 0),
            "aqua" => Color::rgb(0, 255, 255),
            "fuchsia" => Color::rgb(255, 0, 255),
            "darkgray" | "darkgrey" => Color::rgb(169, 169, 169),
            "lightgray" | "lightgrey" => Color::rgb(211, 211, 211),
            "whitesmoke" => Color::rgb(245, 245, 245),
            _ => return None,
        };
        Some(named)
    }

    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// How a gradient's colour varies across the box.
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    /// Angle in CSS degrees: 0 = up, 90 = right, 180 = down, 270 = left.
    Linear { angle_deg: f32 },
    Radial { center: (f32, f32), radius: f32 },
}

/// A `linear-gradient()`/`radial-gradient()` reduced to what the rasterizer
/// needs: a direction and stops already placed in `0..=1`.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Positions in `0..=1`, ordered.
    pub stops: Vec<(f32, Color)>,
}

/// One `box-shadow`: offsets, blur, spread, colour and `inset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// A 2D affine transform in CSS order: `[a c e; b d f]`, column-vector
/// convention, so `apply` is `M · p`.
///
/// Every method takes `self` by value: it is six `f32`, `Copy`, and passing a
/// reference would force each caller to write `&` in a place where the type is
/// already cheap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine =
        Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// `self · other` in the column-vector convention.
    pub fn multiply(self, other: Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn determinant(self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// Pure translation with positive scale: the only case where `device_rect`
    /// can map two corners and stay on the fast path.
    pub fn is_translation_scale(self) -> bool {
        self.b == 0.0 && self.c == 0.0 && self.a > 0.0 && self.d > 0.0
    }

    /// `None` when `determinant().abs() < 1e-6` (singular matrix).
    pub fn inverse(self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-6 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn translate(dx: f32, dy: f32) -> Affine {
        Affine { e: dx, f: dy, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    /// Clockwise on screen, matching CSS `rotate()` with `y` pointing down.
    pub fn rotate(deg: f32) -> Affine {
        let (sin, cos) = deg.to_radians().sin_cos();
        Affine { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 }
    }
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

/// `object-fit` for a replaced element inside its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ObjectFit {
    #[default]
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

/// `float`. A float is taken out of the normal flow and placed against one
/// side of its containing block; line boxes beside it are shortened.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

/// `clear`: the element must start below the floats on the given side.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
}

impl Clear {
    /// Does this value clear a float on `side`?
    pub fn clears(self, side: Float) -> bool {
        match self {
            Clear::None => false,
            Clear::Both => true,
            Clear::Left => side == Float::Left,
            Clear::Right => side == Float::Right,
        }
    }
}

/// `vertical-align`, reduced to what the line builder can place.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Middle,
    Top,
    Bottom,
    Sub,
    Super,
    /// A `<length>` or `<percentage>` of the element's own font size, in
    /// pixels. Positive moves the box down, which is the CSS sign.
    Length(f32),
}

/// Standard HSL to RGB. Hue in degrees (any range), saturation and lightness in
/// `0..=1`.
fn hsl_to_rgb(hue: f32, sat: f32, light: f32) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0) / 360.0;
    if sat <= 0.0 {
        let v = (light * 255.0).round().clamp(0.0, 255.0) as u8;
        return (v, v, v);
    }
    let q = if light < 0.5 {
        light * (1.0 + sat)
    } else {
        light + sat - light * sat
    };
    let p = 2.0 * light - q;
    let channel = |t: f32| -> u8 {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (channel(hue + 1.0 / 3.0), channel(hue), channel(hue - 1.0 / 3.0))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    Block,
    Inline,
    /// `inline-block`: inline-level box that shrink-wraps like a block.
    InlineBlock,
    Flex,
    Grid,
    Table,
    TableRow,
    TableCell,
    /// `display: contents`: the box vanishes, children join the parent's flow.
    Contents,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlignContent {
    Start,
    Center,
    End,
    Stretch,
    SpaceBetween,
    SpaceAround,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overflow {
    Visible,
    Hidden,
    Auto,
    Scroll,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    /// `fixed`: absolutely positioned against the viewport, not an ancestor.
    Fixed,
}

/// One grid track: a fixed size plus a flexible share (`fr`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Track {
    pub px: f32,
    /// Percentage of the container width, resolved during layout.
    pub percent: f32,
    pub fr: f32,
    pub min_px: f32,
    pub max_px: Option<f32>,
}

/// `repeat(auto-fit|auto-fill, <track>)`, resolved against the container width
/// at layout time (the parser cannot know it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoRepeat {
    pub fill: bool,
    pub track: Track,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridPlacement {
    /// 1-based start line; `None` means auto.
    pub start: Option<u16>,
    pub span: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// A CSS length. Units that do not depend on layout (`px`, `em`, `rem`, `vw`,
/// `vh`, `vmin`, `vmax`) are already folded into `px` when the style is
/// computed; a percentage stays symbolic because only layout knows the
/// containing block. `min()`/`max()`/`clamp()` stay as a tree when their
/// arguments mix percentages with absolute lengths, because the ordering of
/// the arguments can change once the base is known.
#[derive(Clone, Debug, PartialEq)]
pub enum Dim {
    /// `px` pixels plus `percent` per cent of the containing block.
    Linear { px: f32, percent: f32 },
    Min(Box<Dim>, Box<Dim>),
    Max(Box<Dim>, Box<Dim>),
    Clamp(Box<Dim>, Box<Dim>, Box<Dim>),
}

impl Dim {
    pub const fn px(px: f32) -> Dim {
        Dim::Linear { px, percent: 0.0 }
    }

    pub const fn percent(percent: f32) -> Dim {
        Dim::Linear { px: 0.0, percent }
    }

    /// Resolve against a containing-block base (in pixels).
    pub fn resolve(&self, base: f32) -> f32 {
        match self {
            Dim::Linear { px, percent } => px + base * percent / 100.0,
            Dim::Min(a, b) => a.resolve(base).min(b.resolve(base)),
            Dim::Max(a, b) => a.resolve(base).max(b.resolve(base)),
            Dim::Clamp(low, value, high) => {
                value.resolve(base).max(low.resolve(base)).min(high.resolve(base))
            }
        }
    }

    /// Exact pixel value, when no percentage remains.
    pub fn absolute(&self) -> Option<f32> {
        match self {
            Dim::Linear { px, percent } => {
                if *percent == 0.0 {
                    Some(*px)
                } else {
                    None
                }
            }
            Dim::Min(a, b) => Some(a.absolute()?.min(b.absolute()?)),
            Dim::Max(a, b) => Some(a.absolute()?.max(b.absolute()?)),
            Dim::Clamp(low, value, high) => {
                Some(value.absolute()?.max(low.absolute()?).min(high.absolute()?))
            }
        }
    }

    /// Multiply by a scalar (`calc(2 * 10px)`, `calc(10px / 2)`).
    fn scale(&self, factor: f32) -> Dim {
        match self {
            Dim::Linear { px, percent } => {
                Dim::Linear { px: px * factor, percent: percent * factor }
            }
            Dim::Min(a, b) => Dim::Min(Box::new(a.scale(factor)), Box::new(b.scale(factor))),
            Dim::Max(a, b) => Dim::Max(Box::new(a.scale(factor)), Box::new(b.scale(factor))),
            Dim::Clamp(low, value, high) => Dim::Clamp(
                Box::new(low.scale(factor)),
                Box::new(value.scale(factor)),
                Box::new(high.scale(factor)),
            ),
        }
    }

    /// `self + other` (`sign = 1`) or `self - other` (`sign = -1`).
    /// Only sums of linear terms are representable; anything else is invalid.
    fn combine(&self, other: &Dim, sign: f32) -> Option<Dim> {
        match (self, other) {
            (Dim::Linear { px: a, percent: pa }, Dim::Linear { px: b, percent: pb }) => {
                Some(Dim::Linear { px: a + sign * b, percent: pa + sign * pb })
            }
            _ => None,
        }
    }
}

/// Context needed to turn a `<length>` into pixels at style time.
#[derive(Clone, Copy, Debug)]
pub struct LengthCtx {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Computed font-size of the root element (`rem`).
    pub root_font_size: f32,
    /// Font-size `em` refers to for the property being parsed.
    pub em_base: f32,
    /// `opacity` inherited from the ancestors, so a child inside a
    /// `opacity: 0` subtree is not painted either.
    pub parent_opacity: f32,
}

impl Default for LengthCtx {
    fn default() -> Self {
        LengthCtx {
            viewport_width: 1000.0,
            viewport_height: 700.0,
            root_font_size: 16.0,
            em_base: 16.0,
            parent_opacity: 1.0,
        }
    }
}

/// Document-level inputs for style computation.
#[derive(Clone, Copy, Debug)]
pub struct StyleContext {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Font-size of the root element before any author rule applies.
    pub root_font_size: f32,
}

impl Default for StyleContext {
    fn default() -> Self {
        StyleContext { viewport_width: 1000.0, viewport_height: 700.0, root_font_size: 16.0 }
    }
}

fn parse_dim(value: &str, ctx: LengthCtx) -> Option<Dim> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("auto") {
        return None;
    }
    parse_length_value(v, ctx)
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// Margin or padding: each side is a `Dim`, because a percentage there is a
/// share of the containing block's **width** (CSS 2.1 §8.3/8.4 — also for the
/// vertical sides), which only layout knows.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgesDim {
    pub top: Dim,
    pub right: Dim,
    pub bottom: Dim,
    pub left: Dim,
}

impl Default for EdgesDim {
    fn default() -> Self {
        EdgesDim {
            top: Dim::px(0.0),
            right: Dim::px(0.0),
            bottom: Dim::px(0.0),
            left: Dim::px(0.0),
        }
    }
}

impl EdgesDim {
    /// All four sides as pixels, against a containing-block width.
    pub fn resolve(&self, base: f32) -> Edges {
        Edges {
            top: self.top.resolve(base),
            right: self.right.resolve(base),
            bottom: self.bottom.resolve(base),
            left: self.left.resolve(base),
        }
    }

    pub fn top_at(&self, base: f32) -> f32 {
        self.top.resolve(base)
    }
    pub fn right_at(&self, base: f32) -> f32 {
        self.right.resolve(base)
    }
    pub fn bottom_at(&self, base: f32) -> f32 {
        self.bottom.resolve(base)
    }
    pub fn left_at(&self, base: f32) -> f32 {
        self.left.resolve(base)
    }
    pub fn horizontal_at(&self, base: f32) -> f32 {
        self.left.resolve(base) + self.right.resolve(base)
    }
    pub fn vertical_at(&self, base: f32) -> f32 {
        self.top.resolve(base) + self.bottom.resolve(base)
    }

    pub fn uniform(dim: Dim) -> EdgesDim {
        EdgesDim { top: dim.clone(), right: dim.clone(), bottom: dim.clone(), left: dim }
    }
}

impl Edges {
    pub fn uniform(v: f32) -> Edges {
        Edges { top: v, right: v, bottom: v, left: v }
    }
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Clone, Debug)]
pub struct ComputedStyle {
    pub display: Display,
    pub color: Color,
    pub background: Color,
    pub font_size: f32,
    pub font_weight: u16,
    /// `font-family` list as written; `""` means the system default.
    pub font_family: String,
    /// `font-style: italic`/`oblique`.
    pub italic: bool,
    pub margin: EdgesDim,
    pub padding: EdgesDim,
    pub border: Edges,
    pub border_color: Color,
    pub text_align: TextAlign,
    pub line_height: f32,
    pub flex_direction: FlexDirection,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub flex_wrap: FlexWrap,
    pub gap: f32,
    pub width: Option<Dim>,
    pub max_width: Option<Dim>,
    pub margin_auto_left: bool,
    pub margin_auto_right: bool,
    pub flex_grow: f32,
    /// `flex-shrink`; defaults to 1, so an item may shrink below its basis.
    pub flex_shrink: f32,
    /// `flex-basis` in pixels, `None` for `auto` (use the item's own width).
    pub flex_basis: Option<f32>,
    /// `align-content` for wrapped flex lines and grid rows.
    pub align_content: AlignContent,
    /// `justify-items`: horizontal placement of a grid item inside its area.
    pub justify_items: AlignItems,
    pub min_height: Option<Dim>,
    pub max_height: Option<Dim>,
    /// `table-layout: fixed`: column widths come from the first row only.
    pub table_fixed: bool,
    /// `opacity`; 0 means the element is laid out but not painted.
    pub opacity: f32,
    /// `visibility: visible`; `hidden` keeps the box and skips the paint.
    pub visible: bool,
    pub align_self: Option<AlignItems>,
    pub underline: bool,
    /// `false` when `list-style(-type): none` removes the marker.
    pub list_marker: bool,
    pub height: Option<Dim>,
    /// Absolute URL of a `background-image`, resolved by the engine.
    pub background_image: Option<String>,
    /// `true` for `box-sizing: border-box`: width/height include padding+border.
    pub border_box: bool,
    pub min_width: Option<Dim>,
    pub overflow: Overflow,
    pub position: Position,
    pub top: Option<Dim>,
    pub right: Option<Dim>,
    pub bottom: Option<Dim>,
    pub left: Option<Dim>,
    pub grid_columns: Vec<Track>,
    pub grid_rows: Vec<Track>,
    pub grid_gap: f32,
    pub grid_auto_repeat: Option<AutoRepeat>,
    pub grid_column: GridPlacement,
    pub grid_row: GridPlacement,
    pub border_radius: f32,
    /// `border-radius` given in `%`, resolved against the box in layout.
    pub border_radius_percent: f32,
    /// `box-shadow`, single shadow.
    pub box_shadow: Option<BoxShadow>,
    /// `linear-gradient()`/`radial-gradient()` in `background`/`background-image`.
    pub background_gradient: Option<Gradient>,
    /// `transform`, already composed left to right.
    pub transform: Option<Affine>,
    /// `transform-origin` as fractions of the box.
    pub transform_origin: (f32, f32),
    pub object_fit: ObjectFit,
    /// `aspect-ratio` as `w / h`.
    pub aspect_ratio: Option<f32>,
    pub float: Float,
    pub clear: Clear,
    pub vertical_align: VerticalAlign,
    /// `border-collapse: collapse` on a table.
    pub border_collapse: bool,
    /// `order` inside a flex or grid container.
    pub order: i32,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Inline,
            color: Color::rgb(0x1a, 0x1a, 0x1a),
            background: Color::TRANSPARENT,
            font_size: 16.0,
            font_weight: 400,
            font_family: String::new(),
            italic: false,
            margin: EdgesDim::default(),
            padding: EdgesDim::default(),
            border: Edges::default(),
            border_color: Color::rgb(0xcc, 0xcc, 0xcc),
            text_align: TextAlign::Left,
            line_height: 1.35,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            flex_wrap: FlexWrap::NoWrap,
            gap: 0.0,
            width: None,
            max_width: None,
            margin_auto_left: false,
            margin_auto_right: false,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: None,
            align_content: AlignContent::Start,
            justify_items: AlignItems::Stretch,
            min_height: None,
            max_height: None,
            table_fixed: false,
            opacity: 1.0,
            visible: true,
            align_self: None,
            underline: false,
            list_marker: true,
            height: None,
            background_image: None,
            border_box: false,
            min_width: None,
            overflow: Overflow::Visible,
            position: Position::Static,
            top: None,
            right: None,
            bottom: None,
            left: None,
            grid_columns: Vec::new(),
            grid_rows: Vec::new(),
            grid_gap: 0.0,
            grid_auto_repeat: None,
            grid_column: GridPlacement::default(),
            grid_row: GridPlacement::default(),
            border_radius: 0.0,
            border_radius_percent: 0.0,
            box_shadow: None,
            background_gradient: None,
            transform: None,
            transform_origin: (0.5, 0.5),
            object_fit: ObjectFit::Fill,
            aspect_ratio: None,
            float: Float::None,
            clear: Clear::None,
            vertical_align: VerticalAlign::Baseline,
            border_collapse: false,
            order: 0,
        }
    }
}

/// A number the way CSS renders it: no trailing `.0`, at most two decimals.
fn css_num(v: f32) -> String {
    if (v - v.round()).abs() < 0.001 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}").trim_end_matches('0').to_string()
    }
}

/// A length the way CSS renders it, keeping percentages symbolic.
fn css_dim(d: &Dim) -> String {
    match d {
        Dim::Linear { px, percent } => {
            if *percent == 0.0 {
                format!("{}px", css_num(*px))
            } else if *px == 0.0 {
                format!("{}%", css_num(*percent))
            } else {
                format!("calc({}px + {}%)", css_num(*px), css_num(*percent))
            }
        }
        Dim::Min(a, b) => format!("min({}, {})", css_dim(a), css_dim(b)),
        Dim::Max(a, b) => format!("max({}, {})", css_dim(a), css_dim(b)),
        Dim::Clamp(lo, value, hi) => {
            format!("clamp({}, {}, {})", css_dim(lo), css_dim(value), css_dim(hi))
        }
    }
}

/// The computed value of `prop` on `style`, formatted the way CSS does.
///
/// Single source of truth for both the `computed:` compat check and the JS
/// `getComputedStyle()` host function, so a page and a check can never
/// disagree about what a property is worth. `None` means the property is not
/// modelled here; callers turn that into a failure or an empty string.
pub fn computed_value(style: &ComputedStyle, prop: &str) -> Option<String> {
    let value = match prop {
        "display" => match style.display {
            Display::Block => "block",
            Display::Inline => "inline",
            Display::InlineBlock => "inline-block",
            Display::Flex => "flex",
            Display::Grid => "grid",
            Display::Table => "table",
            Display::TableRow => "table-row",
            Display::TableCell => "table-cell",
            Display::Contents => "contents",
            Display::None => "none",
        }
        .to_string(),
        "color" => style.color.to_hex(),
        "background-color" => style.background.to_hex(),
        "font-size" => format!("{}px", css_num(style.font_size)),
        "font-weight" => style.font_weight.to_string(),
        "font-family" => style.font_family.clone(),
        "float" => match style.float {
            Float::None => "none",
            Float::Left => "left",
            Float::Right => "right",
        }
        .to_string(),
        "clear" => match style.clear {
            Clear::None => "none",
            Clear::Left => "left",
            Clear::Right => "right",
            Clear::Both => "both",
        }
        .to_string(),
        "order" => style.order.to_string(),
        "object-fit" => match style.object_fit {
            ObjectFit::Fill => "fill",
            ObjectFit::Contain => "contain",
            ObjectFit::Cover => "cover",
            ObjectFit::None => "none",
            ObjectFit::ScaleDown => "scale-down",
        }
        .to_string(),
        "font-style" => if style.italic { "italic" } else { "normal" }.to_string(),
        "line-height" => css_num(style.line_height * style.font_size) + "px",
        "width" => style.width.as_ref().map(css_dim).unwrap_or_else(|| "auto".to_string()),
        "height" => style.height.as_ref().map(css_dim).unwrap_or_else(|| "auto".to_string()),
        "margin-top" => css_dim(&style.margin.top),
        "margin-left" => css_dim(&style.margin.left),
        "padding-top" => css_dim(&style.padding.top),
        "padding-left" => css_dim(&style.padding.left),
        "position" => match style.position {
            Position::Static => "static",
            Position::Relative => "relative",
            Position::Absolute => "absolute",
            Position::Fixed => "fixed",
        }
        .to_string(),
        "opacity" => css_num(style.opacity),
        "text-align" => match style.text_align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        }
        .to_string(),
        "visibility" => if style.visible { "visible" } else { "hidden" }.to_string(),
        "border-radius" => {
            if style.border_radius_percent > 0.0 {
                format!("{}%", css_num(style.border_radius_percent))
            } else {
                format!("{}px", css_num(style.border_radius))
            }
        }
        _ => return None,
    };
    Some(value)
}

impl ComputedStyle {
    /// True when the two styles can produce different box geometry.
    ///
    /// Used to decide whether a restyle also needs a relayout: a change to
    /// `color` only repaints, while `width`/`font-size`/`display` move boxes.
    /// Purely visual properties (`color`, `background`, `border_color`,
    /// `border_radius`, `box_shadow`, `underline`, `background_image`) are
    /// deliberately excluded.
    pub fn geometry_differs(&self, other: &ComputedStyle) -> bool {
        self.display != other.display
            || self.font_size != other.font_size
            || self.font_weight != other.font_weight
            // A different family or slant changes every advance, so it moves
            // boxes exactly like a size change.
            || self.font_family != other.font_family
            || self.italic != other.italic
            || self.margin != other.margin
            || self.padding != other.padding
            || self.border != other.border
            || self.text_align != other.text_align
            || self.line_height != other.line_height
            || self.flex_direction != other.flex_direction
            || self.justify_content != other.justify_content
            || self.align_items != other.align_items
            || self.flex_wrap != other.flex_wrap
            || self.gap != other.gap
            || self.width != other.width
            || self.max_width != other.max_width
            || self.margin_auto_left != other.margin_auto_left
            || self.margin_auto_right != other.margin_auto_right
            || self.flex_grow != other.flex_grow
            || self.flex_shrink != other.flex_shrink
            || self.flex_basis != other.flex_basis
            || self.align_content != other.align_content
            || self.justify_items != other.justify_items
            || self.min_height != other.min_height
            || self.max_height != other.max_height
            || self.table_fixed != other.table_fixed
            || self.align_self != other.align_self
            || self.list_marker != other.list_marker
            || self.height != other.height
            || self.border_box != other.border_box
            || self.min_width != other.min_width
            || self.overflow != other.overflow
            || self.position != other.position
            || self.top != other.top
            || self.right != other.right
            || self.bottom != other.bottom
            || self.left != other.left
            || self.grid_columns != other.grid_columns
            || self.grid_rows != other.grid_rows
            || self.grid_gap != other.grid_gap
            || self.grid_column != other.grid_column
            || self.grid_row != other.grid_row
            // `aspect-ratio` changes the height.
            || self.aspect_ratio != other.aspect_ratio
            // The paint matrix and the `object-fit` sub-rect live in the
            // `Layout`, not in the style, so a restyle that only changes them
            // must rebuild the layout — otherwise the page keeps painting the
            // old matrix and no check would catch it.
            || self.transform != other.transform
            || self.transform_origin != other.transform_origin
            || self.object_fit != other.object_fit
            // Floats, `clear` and `order` all move boxes.
            || self.float != other.float
            || self.clear != other.clear
            || self.order != other.order
    }
}

/// True when `next` cannot reuse the geometry computed for `previous`.
///
/// A different node count is enough on its own: a node was inserted or
/// removed, so boxes must be rebuilt even if the surviving styles match.
pub fn geometry_changed(previous: &[ComputedStyle], next: &[ComputedStyle]) -> bool {
    if previous.len() != next.len() {
        return true;
    }
    previous
        .iter()
        .zip(next.iter())
        .any(|(a, b)| a.geometry_differs(b))
}

#[derive(Clone, Debug)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    /// `!important` was stripped from `value` and recorded here. The cascade
    /// sorts on this before specificity, so an important author declaration
    /// outranks any normal one — including a normal inline `style=""`.
    pub important: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Combinator {
    Descendant,
    Child,
    Adjacent,
    Sibling,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttrOp {
    Present,
    Equals,
    Includes,
    Prefix,
    Suffix,
    Substring,
    Dash,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
    pub case_insensitive: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    /// `(a, b)` from `an+b`, signed: `-n+3` is `(-1, 3)`.
    NthChild(i32, i32),
    Not(Box<Compound>),
    /// `:root`; matches only `html`. Counts as a class for specificity, like
    /// any other pseudo-class.
    Root,
    /// `:host` / `:host(<compound>)`. Matches the host element of the shadow
    /// tree whose sheet declares it — never from outside that tree.
    Host(Option<Box<Compound>>),
    /// `:hover`, `:focus`, `:active`, `:visited`, `:link`, `:disabled`,
    /// `:target`, `:focus-within`, `:focus-visible`.
    /// Parsed and valid, but never matches a static paint.
    Interactive,
    /// `:checked` — a real state, not an interaction: it matches a
    /// `checked` input and a `selected` option, which is what a page
    /// inspecting its own form needs.
    Checked,
    /// Valid pseudo-class the engine does not implement. Never matches.
    Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub compounds: Vec<Compound>,
    /// `combinators.len() == compounds.len() - 1`; `combinators[i]` links
    /// `compounds[i]` to `compounds[i + 1]`.
    pub combinators: Vec<Combinator>,
}

/// Why a rule never reached the cascade. Every dropped rule is counted, so a
/// stylesheet that loses half its rules cannot look identical to a correct one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkipReason {
    /// Invalid syntax (unclosed attribute, malformed pseudo, empty part).
    InvalidSyntax,
    /// Valid selector using a pseudo-element (`::before`, `::marker`…).
    PseudoElement,
    /// `:not()` with a combinator or a complex compound.
    UnsupportedNot,
    /// At-rule the engine does not implement (`@supports`, `@container`…).
    UnsupportedAtRule,
}

#[derive(Clone, Debug, Default)]
pub struct ParseReport {
    /// Selector parts that entered the cascade.
    pub rules: usize,
    pub skipped: Vec<(SkipReason, String)>,
    /// Interactive pseudo-classes neutralised (parsed, never match).
    pub interactive: usize,
    /// Unknown pseudo-classes neutralised.
    pub unsupported_pseudo: usize,
}

impl ParseReport {
    pub fn count(&self, reason: SkipReason) -> usize {
        self.skipped.iter().filter(|(r, _)| *r == reason).count()
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// Custom properties (`--name: value`) collected from every rule. MDN,
    /// GitHub and most modern sites define their palette this way; without
    /// substitution every `var(--color-text)` is an invalid value.
    pub variables: std::collections::HashMap<String, String>,
    /// What the parser accepted, dropped and neutralised.
    pub report: ParseReport,
    /// `@font-face` rules, in source order.
    pub font_faces: Vec<FontFaceRule>,
}

/// One `@font-face` block: a family name plus the files that can supply it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFaceRule {
    pub family: String,
    /// URLs in source order; `local(...)` is discarded — NARO cannot resolve a
    /// locally installed face by its PostScript name.
    pub srcs: Vec<String>,
    pub weight: u16,
    pub italic: bool,
}

/// A `font-family` value as the computed style stores it: one entry per name,
/// unquoted, joined with `", "`. Quotes are a CSS syntax detail the rest of the
/// engine has no use for, and `getComputedStyle` reports the list without them.
fn parse_family_list(value: &str) -> String {
    value
        .split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .filter(|f| !f.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse a `linear-gradient()`/`radial-gradient()` value.
///
/// Returns `None` for anything it does not model (conic gradients, repeating
/// variants, fewer than two stops), so the caller keeps its previous behaviour
/// instead of painting a wrong thing.
pub fn parse_gradient(value: &str) -> Option<Gradient> {
    let lower = value.trim().to_ascii_lowercase();
    let (kind, inner) = if let Some(rest) = lower.strip_prefix("linear-gradient(") {
        let inner = rest.strip_suffix(')')?;
        let mut parts: Vec<String> = split_top_level_commas(inner)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        if parts.is_empty() {
            return None;
        }
        // The first argument is a direction only when it is a keyword or an
        // angle; otherwise it is already a colour stop.
        let first = parts[0].trim().to_string();
        let angle = if let Some(deg) = first.strip_suffix("deg") {
            parts.remove(0);
            deg.trim().parse::<f32>().ok()?
        } else if first.starts_with("to ") {
            parts.remove(0);
            match first.as_str() {
                "to top" => 0.0,
                "to right" => 90.0,
                "to bottom" => 180.0,
                "to left" => 270.0,
                "to top right" | "to right top" => 45.0,
                "to bottom right" | "to right bottom" => 135.0,
                "to bottom left" | "to left bottom" => 225.0,
                "to top left" | "to left top" => 315.0,
                _ => 180.0,
            }
        } else if first.ends_with("rad") {
            parts.remove(0);
            let rad = first.trim_end_matches("rad").trim().parse::<f32>().ok()?;
            rad.to_degrees()
        } else {
            // No direction: CSS defaults to `to bottom`.
            180.0
        };
        (GradientKind::Linear { angle_deg: angle }, parts)
    } else if let Some(rest) = lower.strip_prefix("radial-gradient(") {
        let inner = rest.strip_suffix(')')?;
        let mut parts: Vec<String> = split_top_level_commas(inner)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        if parts.is_empty() {
            return None;
        }
        let mut center = (0.5f32, 0.5f32);
        let mut radius = 0.5f32;
        // An optional prelude: `circle`, `ellipse`, `at <pos>`, or both.
        let first = parts[0].trim().to_string();
        let is_color = Color::parse(&first).is_some();
        if !is_color {
            parts.remove(0);
            let mut words = first.split_whitespace();
            while let Some(word) = words.next() {
                match word {
                    "at" => {
                        let mut pos = words.next().unwrap_or("center");
                        let mut second = words.next();
                        // `at left top` is two words; `at 20% 30%` too.
                        if let Some(next) = second {
                            if !next.starts_with(|c: char| c.is_ascii_digit())
                                && next != "center"
                                && next != "left"
                                && next != "right"
                                && next != "top"
                                && next != "bottom"
                            {
                                second = None;
                            }
                        }
                        let (x, y) = position_fractions(pos, second);
                        center = (x, y);
                        pos = "";
                        let _ = pos;
                        break;
                    }
                    "circle" => radius = 0.5,
                    _ => {}
                }
            }
        }
        (GradientKind::Radial { center, radius }, parts)
    } else {
        return None;
    };

    let stops = parse_gradient_stops(&inner)?;
    if stops.len() < 2 {
        return None;
    }
    Some(Gradient { kind, stops })
}

/// `left`, `top`, `20%`, `30px`… as fractions of the box.
fn position_fractions(first: &str, second: Option<&str>) -> (f32, f32) {
    let one = |token: &str| -> Option<f32> {
        let token = token.trim();
        if token == "center" {
            return Some(0.5);
        }
        if token == "left" || token == "top" {
            return Some(0.0);
        }
        if token == "right" || token == "bottom" {
            return Some(1.0);
        }
        if let Some(pct) = token.strip_suffix('%') {
            return pct.trim().parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0));
        }
        None
    };
    match (one(first), second.and_then(one)) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => {
            // A single keyword is a horizontal position unless it is vertical.
            if first == "top" || first == "bottom" {
                (0.5, a)
            } else {
                (a, 0.5)
            }
        }
        _ => (0.5, 0.5),
    }
}

/// Colour stops with optional positions. Missing positions are spread evenly
/// between the neighbours that have one, and the ends default to 0 and 1.
fn parse_gradient_stops(parts: &[String]) -> Option<Vec<(f32, Color)>> {
    let mut stops: Vec<(Option<f32>, Color)> = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // `<color> [<position>]`, or `<position>` alone for a two-position stop.
        let mut color = None;
        let mut position = None;
        for token in part.split_whitespace() {
            if let Some(c) = Color::parse(token) {
                color = Some(c);
            } else if let Some(pct) = token.strip_suffix('%') {
                position = pct.trim().parse::<f32>().ok().map(|v| v / 100.0);
            }
        }
        let color = color?;
        stops.push((position, color));
    }
    if stops.len() < 2 {
        return None;
    }
    // First and last default to 0 and 1 so a bare `red, blue` fills the box.
    if stops[0].0.is_none() {
        stops[0].0 = Some(0.0);
    }
    let last = stops.len() - 1;
    if stops[last].0.is_none() {
        stops[last].0 = Some(1.0);
    }
    // Interior gaps are interpolated between the nearest fixed neighbours.
    let mut i = 0usize;
    while i < stops.len() {
        if stops[i].0.is_some() {
            i += 1;
            continue;
        }
        let start = i;
        while i < stops.len() && stops[i].0.is_none() {
            i += 1;
        }
        let before = stops[start - 1].0.unwrap_or(0.0);
        let after = stops[i].0.unwrap_or(1.0);
        let count = (i - start) as f32;
        for k in 0..(i - start) {
            let t = (k as f32 + 1.0) / (count + 1.0);
            stops[start + k].0 = Some(before + (after - before) * t);
        }
    }
    let mut out: Vec<(f32, Color)> = stops
        .into_iter()
        .map(|(pos, color)| (pos.unwrap_or(0.0).clamp(0.0, 1.0), color))
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Some(out)
}

/// `vertical-align`, with lengths and percentages resolved against the
/// element's own font size (which is what CSS uses for this property).
fn parse_vertical_align(value: &str, ctx: LengthCtx) -> VerticalAlign {
    let lower = value.trim().to_ascii_lowercase();
    match lower.as_str() {
        "baseline" => VerticalAlign::Baseline,
        "middle" => VerticalAlign::Middle,
        "top" | "text-top" => VerticalAlign::Top,
        "bottom" | "text-bottom" => VerticalAlign::Bottom,
        "sub" => VerticalAlign::Sub,
        "super" => VerticalAlign::Super,
        _ => match parse_length(&lower, ctx) {
            Some(px) => VerticalAlign::Length(px),
            None => VerticalAlign::Baseline,
        },
    }
}

/// `grid-area: <row-start> / <column-start> / <row-end> / <column-end>`.
///
/// Named areas and named lines are not modelled; only the numeric form (and
/// `auto`/`span N` in each slot) is understood. Each pair becomes the same
/// `GridPlacement` the longhands would have produced.
fn parse_grid_area(value: &str) -> Option<(GridPlacement, GridPlacement)> {
    let parts: Vec<&str> = value.split('/').map(|p| p.trim()).collect();
    let (row_part, column_part) = match parts.len() {
        1 => (parts[0], "auto"),
        2 => (parts[0], parts[1]),
        4 => {
            let row = format!("{} / {}", parts[0], parts[2]);
            let column = format!("{} / {}", parts[1], parts[3]);
            return Some((
                parse_placement(&row)?,
                parse_placement(&column)?,
            ));
        }
        _ => return None,
    };
    Some((parse_placement(row_part)?, parse_placement(column_part)?))
}

/// Parse a `transform` list. Any function this does not model (`skew`,
/// `matrix`, `perspective`) discards the whole declaration — a partial
/// application would silently paint something the page never asked for.
pub fn parse_transform(value: &str) -> Option<Affine> {
    let lower = value.trim().to_ascii_lowercase();
    if lower.is_empty() || lower == "none" {
        return None;
    }
    let mut rest = lower.as_str();
    let mut acc = Affine::IDENTITY;
    let mut any = false;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim();
        let close = rest[open..].find(')')? + open;
        let args: Vec<f32> = rest[open + 1..close]
            .split(',')
            .map(|a| a.trim())
            .map(|a| {
                let a = a.trim_end_matches("deg").trim_end_matches("rad");
                a.trim_end_matches("px").trim_end_matches('%').trim().parse::<f32>()
            })
            .collect::<Result<Vec<f32>, _>>()
            .ok()?;
        let piece = match name {
            "translate" => {
                let (x, y) = (args.first().copied()?, args.get(1).copied().unwrap_or(0.0));
                Affine::translate(x, y)
            }
            "translatex" => Affine::translate(args.first().copied()?, 0.0),
            "translatey" => Affine::translate(0.0, args.first().copied()?),
            "scale" => {
                let sx = args.first().copied()?;
                Affine::scale(sx, args.get(1).copied().unwrap_or(sx))
            }
            "scalex" => Affine::scale(args.first().copied()?, 1.0),
            "scaley" => Affine::scale(1.0, args.first().copied()?),
            "rotate" => {
                let raw = rest[open + 1..close].trim();
                let deg = if let Some(rad) = raw.strip_suffix("rad") {
                    rad.trim().parse::<f32>().ok()?.to_degrees()
                } else {
                    args.first().copied()?
                };
                Affine::rotate(deg)
            }
            _ => return None,
        };
        // `transform: A B C` is `A · B · C`: the rightmost function acts on the
        // point first.
        acc = acc.multiply(piece);
        any = true;
        rest = &rest[close + 1..];
    }
    // Trailing text that is not another function means an unparsed tail.
    if !any || !rest.trim().is_empty() {
        return None;
    }
    Some(acc)
}

/// `transform-origin` as fractions of the box. A `<length>` cannot become a
/// fraction here — only layout knows the box — so a px value falls back to the
/// default and the fraction form is the single representation the painter sees.
fn parse_transform_origin(value: &str) -> (f32, f32) {
    let mut parts = value.split_whitespace();
    let one = |token: &str| -> Option<f32> {
        let token = token.trim();
        match token {
            "center" => Some(0.5),
            "left" | "top" => Some(0.0),
            "right" | "bottom" => Some(1.0),
            _ => token
                .strip_suffix('%')
                .and_then(|p| p.trim().parse::<f32>().ok())
                .map(|v| v / 100.0)
                // `0` and `0px` are exact; any other length would need the box,
                // which only layout has, so it falls back to the default.
                .or_else(|| {
                    let bare = token.strip_suffix("px").unwrap_or(token).trim();
                    (bare.parse::<f32>().ok() == Some(0.0)).then_some(0.0)
                }),
        }
    };
    let first = parts.next().unwrap_or("center");
    let second = parts.next();
    match (one(first), second.and_then(one)) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => {
            if first == "top" || first == "bottom" {
                (0.5, a)
            } else {
                (a, 0.5)
            }
        }
        _ => (0.5, 0.5),
    }
}

/// `aspect-ratio: 16 / 9` (or `16/9`) as `w / h`. `auto` and anything with a
/// zero side mean "no ratio".
pub fn parse_aspect_ratio(value: &str) -> Option<f32> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("auto") {
        return None;
    }
    // `auto 16 / 9` and `16 / 9` both carry a usable ratio.
    let numeric = v.trim_start_matches("auto").trim();
    let (w, h) = match numeric.split_once('/') {
        Some((w, h)) => (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?),
        None => (numeric.parse::<f32>().ok()?, 1.0),
    };
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(w / h)
}

/// A numeric weight in a `font` shorthand: `100`..=`900` in steps of 100.
fn shorthand_weight(token: &str) -> Option<u16> {
    let n = token.parse::<u16>().ok()?;
    if (100..=900).contains(&n) && n % 100 == 0 {
        Some(n)
    } else {
        None
    }
}

/// The family list out of a `font` shorthand value.
///
/// The shorthand is `<style>? <weight>? <size>[/<line-height>] <family-list>`:
/// the family list starts at the first token that is neither a style keyword,
/// a weight keyword, nor a length. Everything from there on, joined back with
/// single spaces, is the list.
fn font_shorthand_families(value: &str) -> String {
    let mut tokens: Vec<&str> = value.split_whitespace().collect();
    let mut start = None;
    for (i, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        let is_keyword = matches!(
            lower.as_str(),
            "italic" | "oblique" | "normal" | "bold" | "bolder" | "lighter"
                | "small-caps" | "ultra-condensed" | "extra-condensed" | "condensed"
                | "semi-condensed" | "semi-expanded" | "expanded" | "extra-expanded"
                | "ultra-expanded"
        );
        if is_keyword || lower.parse::<u16>().is_ok() {
            continue;
        }
        // A size is the last non-keyword token before the family list, so
        // every token from here that still parses as a length is skipped and
        // the first one that does not begins the list.
        let looks_like_size = lower
            .split('/')
            .next()
            .map(|head| {
                head.ends_with("px")
                    || head.ends_with("em")
                    || head.ends_with("rem")
                    || head.ends_with('%')
                    || head.ends_with("pt")
                    || head.parse::<f32>().is_ok()
            })
            .unwrap_or(false);
        if looks_like_size {
            continue;
        }
        start = Some(i);
        break;
    }
    match start {
        Some(i) => parse_family_list(&tokens.split_off(i).join(" ")),
        None => String::new(),
    }
}

/// Remove `@media`/`@supports`/`@keyframes`/`@font-face` blocks (including
/// their nested braces) plus `@import` statements. NARO has no media-query or
/// animation support, and leaving them in produced garbage declarations.
/// Evaluate a media query against the viewport width. Only the forms NARO can
/// answer are honoured; anything unrecognised is treated as non-matching so a
/// desktop-only rule cannot leak into every layout.
fn media_matches(query: &str, viewport_width: f32) -> bool {
    if std::env::var_os("NARO_NO_MEDIA").is_some() {
        return false;
    }
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    // Comma is an OR list; only the first matching branch applies. Evaluate
    // each branch independently instead of treating the list as a conjunction.
    if q.contains(',') {
        return q
            .split(',')
            .any(|branch| media_matches(branch, viewport_width));
    }
    // `screen`, `all`, `only screen`; `print` and `speech` never match.
    let mut matched_type = false;
    for part in q.split(" and ") {
        let part = part.trim();
        if part == "screen" || part == "all" || part == "only screen" {
            matched_type = true;
            continue;
        }
        if part == "print" || part == "speech" || part == "only print" {
            return false;
        }
        if let Some(v) = part.strip_prefix("(max-width:") {
            match parse_media_length(v.trim_end_matches(')')) {
                Some(px) => {
                    if viewport_width > px {
                        return false;
                    }
                    matched_type = true;
                }
                // A unit we cannot resolve must not silently match.
                None => return false,
            }
            continue;
        }
        if let Some(v) = part.strip_prefix("(min-width:") {
            match parse_media_length(v.trim_end_matches(')')) {
                Some(px) => {
                    if viewport_width < px {
                        return false;
                    }
                    matched_type = true;
                }
                None => return false,
            }
            continue;
        }
        if part.starts_with('(') {
            // Unsupported feature (orientation, prefers-*, aspect-ratio…).
            return false;
        }
    }
    matched_type
}

/// `px`/`em`/`rem` (16px base). Anything else is unsupported.
fn parse_media_length(value: &str) -> Option<f32> {
    let v = value.trim();
    if let Some(px) = v.strip_suffix("px") {
        return px.trim().parse::<f32>().ok();
    }
    if let Some(em) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        return em.trim().parse::<f32>().ok().map(|n| n * 16.0);
    }
    v.parse::<f32>().ok()
}

fn strip_at_rules(
    src: &str,
    viewport_width: f32,
    report: &mut ParseReport,
    faces: &mut Vec<FontFaceRule>,
) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // `@media` is evaluated against the viewport: matching blocks keep
            // their contents (so the inner rules join the cascade), the rest
            // are dropped entirely.
            let prelude_start = i + 1;
            let mut j = prelude_start;
            while j < bytes.len() && bytes[j] != b'{' && bytes[j] != b';' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'{' {
                let prelude = src[prelude_start..j].trim();
                if prelude.to_ascii_lowercase().starts_with("media") {
                    let query = prelude[5..].trim();
                    let end = match matching_brace(bytes, j) {
                        Some(end) => end,
                        None => bytes.len(),
                    };
                    if media_matches(query, viewport_width) {
                        let inner = &src[j + 1..end];
                        out.push(' ');
                        out.push_str(&strip_at_rules(inner, viewport_width, report, faces));
                        out.push(' ');
                    }
                    i = (end + 1).min(bytes.len());
                    continue;
                }
            }
            // At-rules the engine cannot evaluate are recorded, not silently
            // dropped. `@import`/`@charset`/`@namespace` have no effect on the
            // cascade either, so they stay uncounted.
            let name: String = src[prelude_start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
                .to_ascii_lowercase();
            // `@font-face` is parsed rather than skipped, so it no longer
            // counts as an unsupported at-rule.
            const COUNTED: [&str; 6] = [
                "supports",
                "container",
                "layer",
                "keyframes",
                "-webkit-keyframes",
                "page",
            ];
            if COUNTED.contains(&name.as_str()) {
                report
                    .skipped
                    .push((SkipReason::UnsupportedAtRule, format!("@{name}")));
            }
            if name == "font-face" {
                if let Some(end) = matching_brace(bytes, j) {
                    if let Some(rule) = parse_font_face(&src[j + 1..end]) {
                        faces.push(rule);
                    }
                    i = (end + 1).min(bytes.len());
                    out.push(' ');
                    continue;
                }
            }
            // Other at-rules are skipped wholesale.
            let mut j = prelude_start;
            while j < bytes.len() && bytes[j] != b'{' && bytes[j] != b';' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b';' {
                i = j + 1;
            } else if j < bytes.len() {
                let mut depth = 0usize;
                while j < bytes.len() {
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
            } else {
                break;
            }
            out.push(' ');
        } else {
            // Copy the run up to the next `@` as a slice. Pushing
            // `bytes[i] as char` one byte at a time reinterpreted every
            // non-ASCII byte as a Latin-1 character, which turned `.ação`
            // into `.aÃ§Ã£o` before the selector parser ever saw it.
            let start = i;
            while i < bytes.len() && bytes[i] != b'@' {
                i += 1;
            }
            out.push_str(&src[start..i]);
        }
    }
    out
}

/// Index of the `}` that closes the `{` at `open`.
fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn parse_stylesheet(src: &str) -> Stylesheet {
    parse_stylesheet_for(src, 1000.0)
}

/// Reuses the last parsed stylesheet when the same text is parsed again.
///
/// A page load parses its `<style>` text more than once — the engine builds a
/// provisional sheet so scripts can measure the page, then rebuilds it after
/// they run — and a restyle after a mutation parses it again. When the text
/// has not changed, all of that is the same work over the same bytes.
///
/// The key is the exact source text plus the viewport width: everything
/// `parse_stylesheet_for` depends on. Any edit to the text is a miss, and the
/// previous entry is dropped, so a cache hit cannot return a stale sheet.
#[derive(Default)]
pub struct SheetCache {
    last: std::cell::RefCell<Option<(String, f32, std::rc::Rc<Stylesheet>)>>,
}

impl SheetCache {
    /// Parse `src`, or return the cached result when it is the same text and
    /// the same viewport.
    pub fn parse(&self, src: &str, viewport_width: f32) -> std::rc::Rc<Stylesheet> {
        if let Some((text, width, sheet)) = self.last.borrow().as_ref() {
            if *width == viewport_width && text == src {
                return sheet.clone();
            }
        }
        let sheet = std::rc::Rc::new(parse_stylesheet_for(src, viewport_width));
        *self.last.borrow_mut() = Some((src.to_string(), viewport_width, sheet.clone()));
        sheet
    }
}

/// Read one `@font-face` body. A rule without a family or without any `url()`
/// is useless to the engine, so it is dropped rather than kept as a face that
/// can never load.
fn parse_font_face(body: &str) -> Option<FontFaceRule> {
    let mut family = String::new();
    let mut srcs: Vec<String> = Vec::new();
    let mut weight = 400u16;
    let mut italic = false;
    for decl in parse_declarations(body) {
        match decl.name.to_ascii_lowercase().as_str() {
            "font-family" => family = parse_family_list(&decl.value),
            "src" => {
                // Every `url(...)`, in order. `local(...)` names an installed
                // face and cannot be resolved here.
                let mut rest = decl.value.as_str();
                while let Some(start) = rest.find("url(") {
                    rest = &rest[start + 4..];
                    let Some(end) = rest.find(')') else { break };
                    let url = rest[..end]
                        .trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .trim();
                    if !url.is_empty() {
                        srcs.push(url.to_string());
                    }
                    rest = &rest[end..];
                }
            }
            "font-weight" => {
                let lower = decl.value.trim().to_ascii_lowercase();
                weight = match lower.as_str() {
                    "normal" => 400,
                    "bold" => 700,
                    _ => lower.parse::<u16>().unwrap_or(400),
                };
            }
            "font-style" => {
                italic = matches!(lower_style(&decl.value).as_str(), "italic" | "oblique");
            }
            _ => {}
        }
    }
    if family.is_empty() || srcs.is_empty() {
        return None;
    }
    Some(FontFaceRule { family, srcs, weight, italic })
}

fn lower_style(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Parse with a viewport width, so `@media` rules can be evaluated.
pub fn parse_stylesheet_for(src: &str, viewport_width: f32) -> Stylesheet {
    let mut report = ParseReport::default();
    let mut font_faces = Vec::new();
    let src = strip_at_rules(
        &strip_comments(src),
        viewport_width,
        &mut report,
        &mut font_faces,
    );
    let mut rules = Vec::new();
    let mut variables: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut rest = src.as_str();
    while let Some(open) = rest.find('{') {
        let selector_text = &rest[..open];
        let close = match rest[open + 1..].find('}') {
            Some(pos) => open + 1 + pos,
            None => break,
        };
        let body = &rest[open + 1..close];
        rest = &rest[close + 1..];
        let declarations = parse_declarations(body);
        if declarations.is_empty() {
            continue;
        }
        for declaration in &declarations {
            if declaration.name.starts_with("--") {
                variables.insert(declaration.name.clone(), declaration.value.clone());
            }
        }
        for selector in parse_selector_list_reporting(selector_text, &mut report) {
            rules.push(Rule { selector, declarations: declarations.clone() });
        }
    }
    Stylesheet { rules, variables, report, font_faces }
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(pos) = rest.find("/*") {
        out.push_str(&rest[..pos]);
        rest = match rest[pos + 2..].find("*/") {
            Some(end) => &rest[pos + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn parse_declarations(body: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for part in body.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let colon = match part.find(':') {
            Some(pos) => pos,
            None => continue,
        };
        // Property names are case-insensitive, but custom properties are not:
        // `--xhUGwc` and `--xhugwc` are different. Lowercasing the name made
        // every camelCase variable unresolvable, which silently dropped
        // `background: var(--xhUGwc)` and similar declarations.
        let raw_name = part[..colon].trim();
        let name = if raw_name.starts_with("--") {
            raw_name.to_string()
        } else {
            raw_name.to_ascii_lowercase()
        };
        let mut value = part[colon + 1..].trim().to_string();
        let mut important = false;
        // `!important` is a cascade flag, not part of the value: leaving it in
        // made every colour/length parse fail and the declaration vanish.
        //
        // The suffix is compared case-insensitively in place. Lowercasing the
        // whole value first — which is what this did — copied every
        // declaration's value on every sheet parse, and `!important` is rare.
        if let Some(tail) = value.get(value.len().saturating_sub(9)..) {
            if tail.eq_ignore_ascii_case("important") {
                let head = value[..value.len() - 9].trim_end();
                if let Some(stripped) = head.strip_suffix('!') {
                    value.truncate(stripped.trim_end().len());
                    important = true;
                }
            }
        }
        if name.is_empty() || value.is_empty() {
            continue;
        }
        out.push(Declaration { name, value, important });
    }
    out
}

/// Split a selector list on `,` only at nesting depth zero, so commas inside
/// `[data-x="a,b"]` or `:not(.a, .b)` do not split the list.
fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (index, c) in text.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth <= 0 => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || (c as u32) >= 0x80
}

/// Parse a selector list, recording every part in `report`: accepted parts
/// bump `report.rules`, rejected ones land in `report.skipped` with a reason.
fn parse_selector_list_reporting(text: &str, report: &mut ParseReport) -> Vec<Selector> {
    let mut out = Vec::new();
    for part in split_top_level_commas(text) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match parse_complex(part, report) {
            Ok(selector) => {
                report.rules += 1;
                out.push(selector);
            }
            Err(reason) => report.skipped.push((reason, part.to_string())),
        }
    }
    out
}

/// Valid parts of a comma-separated selector list; invalid parts are dropped.
pub fn parse_selector_list_text(text: &str) -> Vec<Selector> {
    let mut report = ParseReport::default();
    parse_selector_list_reporting(text, &mut report)
}

/// The char at byte offset `index`, when the offset is on a boundary.
fn char_at(text: &str, index: usize) -> Option<char> {
    text.get(index..).and_then(|rest| rest.chars().next())
}

/// Skip whitespace from `index`.
fn skip_space_at(text: &str, mut index: usize) -> usize {
    while let Some(c) = char_at(text, index) {
        if !c.is_whitespace() {
            break;
        }
        index += c.len_utf8();
    }
    index
}

/// Read a run of name characters starting at `index`.
///
/// Returns the borrowed slice and the index just past it. The selector parser
/// used to collect the whole compound into a `Vec<char>` and then rebuild a
/// `String` char by char; both are gone, and only the final `String` a
/// `Compound` stores is allocated.
fn take_name(text: &str, mut index: usize) -> (&str, usize) {
    let start = index;
    while let Some(c) = char_at(text, index) {
        if !is_name_char(c) {
            break;
        }
        index += c.len_utf8();
    }
    (&text[start..index], index)
}

fn parse_complex(text: &str, report: &mut ParseReport) -> Result<Selector, SkipReason> {
    let mut compounds: Vec<Compound> = Vec::new();
    let mut combinators: Vec<Combinator> = Vec::new();
    // The compound currently being read is the slice `text[start..index]`:
    // no per-character buffer, no `Vec<char>`.
    let mut start = 0usize;
    let mut current_empty = true;
    let mut explicit: Option<Combinator> = None;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut index = 0usize;

    // Pushes the compound ending at `$end`, linking it with the pending
    // combinator.
    macro_rules! flush {
        ($end:expr) => {
            if !current_empty {
                let compound = parse_compound_part(&text[start..$end], report)?;
                if compounds.is_empty() {
                    if explicit.is_some() {
                        return Err(SkipReason::InvalidSyntax);
                    }
                } else {
                    combinators.push(explicit.take().unwrap_or(Combinator::Descendant));
                }
                compounds.push(compound);
            }
        };
    }

    while let Some(c) = char_at(text, index) {
        let len = c.len_utf8();
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            index += len;
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                current_empty = false;
            }
            '[' | '(' => {
                depth += 1;
                current_empty = false;
            }
            ']' | ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(SkipReason::InvalidSyntax);
                }
                current_empty = false;
            }
            c if c.is_whitespace() && depth == 0 => {
                flush!(index);
                index += len;
                start = index;
                current_empty = true;
                continue;
            }
            '>' | '+' | '~' if depth == 0 => {
                flush!(index);
                if explicit.is_some() {
                    return Err(SkipReason::InvalidSyntax);
                }
                explicit = Some(match c {
                    '>' => Combinator::Child,
                    '+' => Combinator::Adjacent,
                    _ => Combinator::Sibling,
                });
                index += len;
                start = index;
                current_empty = true;
                continue;
            }
            _ => current_empty = false,
        }
        index += len;
    }
    flush!(text.len());
    if explicit.is_some() || compounds.is_empty() {
        return Err(SkipReason::InvalidSyntax);
    }
    if combinators.len() + 1 != compounds.len() {
        return Err(SkipReason::InvalidSyntax);
    }
    Ok(Selector { compounds, combinators })
}

fn parse_compound_part(text: &str, report: &mut ParseReport) -> Result<Compound, SkipReason> {
    let mut compound = Compound::default();
    let mut index = 0usize;
    let mut saw_any = false;

    // Optional leading type/universal selector.
    if let Some(c) = char_at(text, index) {
        if c == '*' {
            index += 1;
            saw_any = true;
        } else if is_name_char(c) {
            let (name, next) = take_name(text, index);
            compound.tag = Some(name.to_ascii_lowercase());
            index = next;
            saw_any = true;
        }
    }

    while let Some(c) = char_at(text, index) {
        match c {
            '#' => {
                index += 1;
                let (name, next) = take_name(text, index);
                if name.is_empty() {
                    return Err(SkipReason::InvalidSyntax);
                }
                index = next;
                if compound.id.is_none() {
                    compound.id = Some(name.to_string());
                }
                saw_any = true;
            }
            '.' => {
                index += 1;
                let (name, next) = take_name(text, index);
                if name.is_empty() {
                    return Err(SkipReason::InvalidSyntax);
                }
                index = next;
                compound.classes.push(name.to_string());
                saw_any = true;
            }
            '[' => {
                let (attr, next) = parse_attr_selector(text, index)?;
                compound.attrs.push(attr);
                index = next;
                saw_any = true;
            }
            ':' => {
                let (pseudo, next) = parse_pseudo(text, index, report)?;
                compound.pseudos.push(pseudo);
                index = next;
                saw_any = true;
            }
            '*' => {
                index += 1;
                saw_any = true;
            }
            _ => return Err(SkipReason::InvalidSyntax),
        }
    }
    if !saw_any {
        return Err(SkipReason::InvalidSyntax);
    }
    Ok(compound)
}

/// Parses `[...]` starting at `text[open]`; returns the selector and the index
/// just past the closing `]`.
fn parse_attr_selector(text: &str, open: usize) -> Result<(AttrSelector, usize), SkipReason> {
    let mut index = open + 1;
    let (name, next) = take_name(text, index);
    if name.is_empty() {
        return Err(SkipReason::InvalidSyntax);
    }
    index = skip_space_at(text, next);
    let c = match char_at(text, index) {
        Some(c) => c,
        None => return Err(SkipReason::InvalidSyntax),
    };
    if c == ']' {
        return Ok((
            AttrSelector {
                name: name.to_ascii_lowercase(),
                op: AttrOp::Present,
                value: String::new(),
                case_insensitive: false,
            },
            index + 1,
        ));
    }
    let op = match c {
        '=' => {
            index += 1;
            AttrOp::Equals
        }
        '~' | '^' | '$' | '*' | '|' => {
            if char_at(text, index + 1) != Some('=') {
                return Err(SkipReason::InvalidSyntax);
            }
            index += 2;
            match c {
                '~' => AttrOp::Includes,
                '^' => AttrOp::Prefix,
                '$' => AttrOp::Suffix,
                '*' => AttrOp::Substring,
                _ => AttrOp::Dash,
            }
        }
        _ => return Err(SkipReason::InvalidSyntax),
    };
    index = skip_space_at(text, index);
    let value;
    if let Some(q) = char_at(text, index).filter(|c| *c == '"' || *c == '\'') {
        index += 1;
        let start = index;
        loop {
            match char_at(text, index) {
                Some(c) if c == q => break,
                Some(c) => index += c.len_utf8(),
                None => return Err(SkipReason::InvalidSyntax),
            }
        }
        value = text[start..index].to_string();
        index += 1;
    } else {
        let start = index;
        while let Some(c) = char_at(text, index) {
            if c.is_whitespace() || c == ']' {
                break;
            }
            index += c.len_utf8();
        }
        value = text[start..index].to_string();
    }
    if value.is_empty() {
        return Err(SkipReason::InvalidSyntax);
    }
    index = skip_space_at(text, index);
    let mut case_insensitive = false;
    if let Some(c) = char_at(text, index) {
        if c == 'i' || c == 'I' {
            case_insensitive = true;
            index = skip_space_at(text, index + 1);
        } else if c == 's' || c == 'S' {
            index = skip_space_at(text, index + 1);
        }
    }
    if char_at(text, index) != Some(']') {
        return Err(SkipReason::InvalidSyntax);
    }
    Ok((
        AttrSelector {
            name: name.to_ascii_lowercase(),
            op,
            value,
            case_insensitive,
        },
        index + 1,
    ))
}

/// Parses `:name` / `:name(arg)` starting at `text[open]` (which is `:`).
fn parse_pseudo(
    text: &str,
    open: usize,
    report: &mut ParseReport,
) -> Result<(Pseudo, usize), SkipReason> {
    let mut index = open + 1;
    // `::before` and friends never apply to a static paint: the rule is
    // reported as a pseudo-element instead of silently matching the element.
    if char_at(text, index) == Some(':') {
        return Err(SkipReason::PseudoElement);
    }
    let (name, next) = take_name(text, index);
    if name.is_empty() {
        return Err(SkipReason::InvalidSyntax);
    }
    index = next;
    let lower = name.to_ascii_lowercase();
    // Legacy single-colon pseudo-elements.
    if matches!(
        lower.as_str(),
        "before"
            | "after"
            | "first-line"
            | "first-letter"
            | "marker"
            | "placeholder"
            | "selection"
            | "backdrop"
    ) {
        return Err(SkipReason::PseudoElement);
    }
    let mut argument = String::new();
    let mut has_argument = false;
    if char_at(text, index) == Some('(') {
        has_argument = true;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        index += 1;
        let start = index;
        while let Some(c) = char_at(text, index) {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                index += c.len_utf8();
                continue;
            }
            match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            index += c.len_utf8();
        }
        if index >= text.len() {
            return Err(SkipReason::InvalidSyntax);
        }
        argument = text[start..index].to_string();
        index += 1;
    }

    let pseudo = match lower.as_str() {
        "root" => Pseudo::Root,
        "host" => {
            if !has_argument {
                Pseudo::Host(None)
            } else {
                let inner = argument.trim();
                // A compound only: combinators, nested pseudos and
                // pseudo-elements have no meaning on the host itself.
                if inner.is_empty()
                    || inner.contains(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~')
                    || inner.contains(':')
                {
                    return Err(SkipReason::InvalidSyntax);
                }
                let mut inner_report = ParseReport::default();
                let compound = parse_compound_part(inner, &mut inner_report)?;
                Pseudo::Host(Some(Box::new(compound)))
            }
        }
        "first-child" => Pseudo::FirstChild,
        "last-child" => Pseudo::LastChild,
        "nth-child" => {
            if !has_argument {
                return Err(SkipReason::InvalidSyntax);
            }
            match parse_nth(&argument) {
                Some((a, b)) => Pseudo::NthChild(a, b),
                None => return Err(SkipReason::InvalidSyntax),
            }
        }
        "not" => {
            if !has_argument {
                return Err(SkipReason::InvalidSyntax);
            }
            let inner = argument.trim();
            // Only a single simple compound is supported: `:not(.a .b)` and
            // nested `:not` are reported rather than mis-matched.
            if inner.is_empty()
                || inner.contains(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~')
                || inner.contains(':')
            {
                return Err(SkipReason::UnsupportedNot);
            }
            let mut inner_report = ParseReport::default();
            let compound = parse_compound_part(inner, &mut inner_report)?;
            Pseudo::Not(Box::new(compound))
        }
        "checked" => Pseudo::Checked,
        "hover" | "focus" | "active" | "visited" | "link" | "disabled" | "target"
        | "focus-within" | "focus-visible" => {
            report.interactive += 1;
            Pseudo::Interactive
        }
        _ => {
            report.unsupported_pseudo += 1;
            Pseudo::Unsupported
        }
    };
    Ok((pseudo, index))
}

/// `an+b` → `(a, b)` with `n >= 0`.
fn parse_nth(argument: &str) -> Option<(i32, i32)> {
    let s: String = argument
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if s == "odd" {
        return Some((2, 1));
    }
    if s == "even" {
        return Some((2, 0));
    }
    match s.find('n') {
        Some(position) => {
            let a_part = &s[..position];
            let b_part = &s[position + 1..];
            let a = match a_part {
                "" | "+" => 1,
                "-" => -1,
                other => other.parse::<i32>().ok()?,
            };
            let b = if b_part.is_empty() {
                0
            } else {
                if !(b_part.starts_with('+') || b_part.starts_with('-')) {
                    return None;
                }
                b_part.parse::<i32>().ok()?
            };
            Some((a, b))
        }
        None => Some((0, s.parse::<i32>().ok()?)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Specificity {
    inline: bool,
    ids: u32,
    classes: u32,
    tags: u32,
}

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut spec = Specificity { inline: false, ids: 0, classes: 0, tags: 0 };
        for compound in &self.compounds {
            if compound.id.is_some() {
                spec.ids += 1;
            }
            // Attribute selectors and pseudo-classes weigh the same as a class.
            // `:not(x)` counts as one class rather than the specificity of its
            // argument — a deliberate simplification.
            spec.classes += compound.classes.len() as u32;
            spec.classes += compound.attrs.len() as u32;
            spec.classes += compound.pseudos.len() as u32;
            if compound.tag.is_some() {
                spec.tags += 1;
            }
        }
        spec
    }

    /// True when the selector matches `id` from outside any shadow tree.
    pub fn matches(&self, doc: &Document, id: NodeId) -> bool {
        self.matches_scoped(doc, id, None)
    }

    /// Match with an optional shadow scope.
    ///
    /// `scope` is the shadow root whose sheet is being applied. It exists for
    /// `:host`, which names the host element of *that* tree and must never
    /// match from the document's own sheets.
    pub fn matches_scoped(&self, doc: &Document, id: NodeId, scope: Option<NodeId>) -> bool {
        if self.compounds.is_empty() {
            return false;
        }
        self.matches_at(doc, id, self.compounds.len() - 1, scope)
    }

    /// True when the selector names the host (`:host`, `:host(...)`).
    ///
    /// Shadow-tree rules are invisible to nodes outside the tree, with one
    /// exception: a rule whose selector mentions `:host` styles the host.
    pub fn has_host_pseudo(&self) -> bool {
        self.compounds.iter().any(|compound| {
            compound
                .pseudos
                .iter()
                .any(|pseudo| matches!(pseudo, Pseudo::Host(_)))
        })
    }

    /// Matching runs right to left: the last compound must match `id`, then the
    /// combinator to its left decides which node must satisfy `index - 1`.
    fn matches_at(
        &self,
        doc: &Document,
        id: NodeId,
        index: usize,
        scope: Option<NodeId>,
    ) -> bool {
        if !compound_matches(doc, id, &self.compounds[index], scope) {
            return false;
        }
        if index == 0 {
            return true;
        }
        match self.combinators[index - 1] {
            Combinator::Descendant => {
                let mut ancestor = doc.node(id).parent;
                while let Some(parent) = ancestor {
                    if self.matches_at(doc, parent, index - 1, scope) {
                        return true;
                    }
                    ancestor = doc.node(parent).parent;
                }
                false
            }
            Combinator::Child => doc
                .node(id)
                .parent
                .map(|parent| self.matches_at(doc, parent, index - 1, scope))
                .unwrap_or(false),
            Combinator::Adjacent => doc
                .previous_element_sibling(id)
                .map(|sibling| self.matches_at(doc, sibling, index - 1, scope))
                .unwrap_or(false),
            Combinator::Sibling => {
                let mut sibling = doc.previous_element_sibling(id);
                while let Some(previous) = sibling {
                    if self.matches_at(doc, previous, index - 1, scope) {
                        return true;
                    }
                    sibling = doc.previous_element_sibling(previous);
                }
                false
            }
        }
    }
}

/// `index` is 1-based among element siblings.
fn nth_matches(index: usize, a: i32, b: i32) -> bool {
    let i = index as i32;
    if a == 0 {
        return i == b;
    }
    let diff = i - b;
    // Some `n >= 0` satisfies `i = a*n + b` iff `diff/a >= 0` and `diff % a == 0`.
    if a > 0 {
        diff >= 0 && diff % a == 0
    } else {
        diff <= 0 && diff % a == 0
    }
}

fn attr_matches(doc: &Document, id: NodeId, attr: &AttrSelector) -> bool {
    let actual = match doc.attr(id, &attr.name) {
        Some(value) => value,
        None => return false,
    };
    if attr.op == AttrOp::Present {
        return true;
    }
    let (actual_owned, want_owned);
    let (actual, want) = if attr.case_insensitive {
        actual_owned = actual.to_ascii_lowercase();
        want_owned = attr.value.to_ascii_lowercase();
        (actual_owned.as_str(), want_owned.as_str())
    } else {
        (actual, attr.value.as_str())
    };
    match attr.op {
        AttrOp::Present => true,
        AttrOp::Equals => actual == want,
        AttrOp::Includes => actual.split_whitespace().any(|token| token == want),
        AttrOp::Prefix => actual.starts_with(want),
        AttrOp::Suffix => actual.ends_with(want),
        AttrOp::Substring => actual.contains(want),
        AttrOp::Dash => {
            actual == want
                || (actual.len() > want.len()
                    && actual.starts_with(want)
                    && actual.as_bytes()[want.len()] == b'-')
        }
    }
}

fn pseudo_matches(doc: &Document, id: NodeId, pseudo: &Pseudo, scope: Option<NodeId>) -> bool {
    match pseudo {
        Pseudo::Root => doc.tag(id) == Some("html"),
        // `:host` names the element that hosts the tree the rule came from.
        // Without a scope there is no such element, so it never matches — the
        // document's own sheets cannot reach into a shadow tree.
        Pseudo::Host(inner) => match scope {
            Some(root) => {
                doc.shadow_host(root) == Some(id)
                    && inner
                        .as_ref()
                        .map(|compound| compound_matches(doc, id, compound, scope))
                        .unwrap_or(true)
            }
            None => false,
        },
        Pseudo::FirstChild => doc.previous_element_sibling(id).is_none(),
        Pseudo::LastChild => doc.next_element_sibling(id).is_none(),
        Pseudo::NthChild(a, b) => doc
            .element_index(id)
            .map(|index| nth_matches(index, *a, *b))
            .unwrap_or(false),
        Pseudo::Not(compound) => !compound_matches(doc, id, compound, scope),
        // A static paint has no hover, focus or visited state: applying these
        // rules unconditionally painted interaction colours into every page.
        Pseudo::Interactive | Pseudo::Unsupported => false,
        // `:checked` is state, not interaction: the attribute is the state.
        Pseudo::Checked => {
            matches!(doc.tag(id), Some("input")) && doc.attr(id, "checked").is_some()
                || matches!(doc.tag(id), Some("option")) && doc.attr(id, "selected").is_some()
        }
    }
}

fn compound_matches(
    doc: &Document,
    id: NodeId,
    compound: &Compound,
    scope: Option<NodeId>,
) -> bool {
    let tag = match doc.tag(id) {
        Some(t) => t,
        None => return false,
    };
    if let Some(want) = &compound.tag {
        if want != tag {
            return false;
        }
    }
    if let Some(want) = &compound.id {
        if doc.attr(id, "id") != Some(want.as_str()) {
            return false;
        }
    }
    if !compound.classes.is_empty() {
        let class_attr = doc.attr(id, "class").unwrap_or("");
        // No `Vec` here: this runs once per (rule, node) pair — over two
        // million times on a page like rust-lang.org — and collecting the
        // class list allocated on every one of them.
        for want in &compound.classes {
            if !class_attr.split_whitespace().any(|class| class == want) {
                return false;
            }
        }
    }
    for attr in &compound.attrs {
        if !attr_matches(doc, id, attr) {
            return false;
        }
    }
    for pseudo in &compound.pseudos {
        if !pseudo_matches(doc, id, pseudo, scope) {
            return false;
        }
    }
    true
}

pub const UA_CSS: &str = r#"
html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, section, article, header, footer, nav, main, aside, blockquote, pre, table, form, fieldset, figure, hr, address { display: block; }
img { display: inline; }
table { display: table; border-spacing: 2px; }
tr { display: table-row; }
td, th { display: table-cell; padding: 1px; }
thead, tbody, tfoot, colgroup, caption { display: block; }
center { display: block; text-align: center; }
[hidden] { display: none; }
input, textarea, button { display: inline; padding: 1px 2px; border: 1px solid #767676; background-color: #ffffff; color: #1a1a1a; }
button { background-color: #eef0f4; }
/* A checkbox and a radio are fixed-size boxes, not text fields: they must not
   take the 160px a text input defaults to. */
input[type=checkbox], input[type=radio] { width: 13px; height: 13px; padding: 0; margin: 3px 3px 3px 4px; vertical-align: middle; }
input[type=radio] { border-radius: 7px; }
select { display: inline; padding: 1px 2px; border: 1px solid #767676; background-color: #ffffff; color: #1a1a1a; }
option, optgroup { display: none; }
label { display: inline; }
textarea { display: block; }
input[type=submit], input[type=button], button { cursor: pointer; }
head, script, style, title, meta, link, base, noscript, template { display: none; }
body { margin: 8px; font-size: 16px; color: #1a1a1a; background-color: #ffffff; }
h1 { font-size: 32px; font-weight: 700; margin: 21px 0; }
h2 { font-size: 24px; font-weight: 700; margin: 19px 0; }
h3 { font-size: 19px; font-weight: 700; margin: 18px 0; }
h4 { font-size: 16px; font-weight: 700; margin: 21px 0; }
h5 { font-size: 13px; font-weight: 700; margin: 22px 0; }
h6 { font-size: 11px; font-weight: 700; margin: 24px 0; }
p { margin: 16px 0; }
ul, ol { margin: 16px 0; padding-left: 40px; }
li { margin: 4px 0; }
blockquote { margin: 16px 40px; }
pre { margin: 16px 0; }
hr { margin: 8px 0; border: 1px solid #cccccc; }
a { color: #1a56db; text-decoration: underline; }
strong, b { font-weight: 700; }
small { font-size: 13px; }
code, pre { font-size: 14px; }
"#;

/// The user-agent stylesheet, parsed once per process.
///
/// It is a constant string, and parsing it on every style pass was pure
/// overhead: a page that restyles on each mutation re-parsed sixty rules.
pub fn ua_stylesheet() -> &'static Stylesheet {
    static UA: std::sync::OnceLock<Stylesheet> = std::sync::OnceLock::new();
    UA.get_or_init(|| parse_stylesheet(UA_CSS))
}

pub fn is_inline_element(doc: &Document, id: NodeId) -> bool {
    doc.tag(id).is_some()
}

/// First descendant of `root` (never `root` itself) matching `selector`.
pub fn query_selector_within(doc: &Document, root: NodeId, selector: &str) -> Option<NodeId> {
    let selectors = parse_selector_list_text(selector);
    if selectors.is_empty() {
        return None;
    }
    let mut stack: Vec<NodeId> = doc.children(root).iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        if doc.tag(node).is_some() && selectors.iter().any(|s| s.matches(doc, node)) {
            return Some(node);
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
    None
}

/// Every descendant of `root` matching `selector`, in document order.
pub fn query_selector_all_within(doc: &Document, root: NodeId, selector: &str) -> Vec<NodeId> {
    let selectors = parse_selector_list_text(selector);
    if selectors.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc.children(root).iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        if doc.tag(node).is_some() && selectors.iter().any(|s| s.matches(doc, node)) {
            out.push(node);
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
    out
}

/// First element in document order matching any part of `selector`.
pub fn query_selector(doc: &Document, selector: &str) -> Option<NodeId> {
    let selectors = parse_selector_list_text(selector);
    if selectors.is_empty() {
        return None;
    }
    let mut stack = vec![doc.root];
    while let Some(node) = stack.pop() {
        if doc.tag(node).is_some() && selectors.iter().any(|s| s.matches(doc, node)) {
            return Some(node);
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
    None
}

/// Every element matching any part of `selector`, in document order, without
/// duplicates (an element matching two parts appears once).
pub fn query_selector_all(doc: &Document, selector: &str) -> Vec<NodeId> {
    let selectors = parse_selector_list_text(selector);
    if selectors.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![doc.root];
    while let Some(node) = stack.pop() {
        if doc.tag(node).is_some() && selectors.iter().any(|s| s.matches(doc, node)) {
            if seen.insert(node) {
                out.push(node);
            }
        }
        for &child in doc.children(node).iter().rev() {
            stack.push(child);
        }
    }
    out
}

/// Compute a style for every node; index equals `NodeId`.
///
/// Uses the engine's default viewport (1000×700) for `vw`/`vh`/`vmin`/`vmax`
/// and a 16px root font size for `rem`. Callers that know the real viewport
/// must use [`compute_styles_with`].
pub fn compute_styles(doc: &Document, author: &Stylesheet) -> Vec<ComputedStyle> {
    compute_styles_with(doc, author, StyleContext::default())
}

/// Rules grouped by the tag their rightmost compound requires.
///
/// The cascade tests every rule against every element, which on a page with
/// two thousand rules and a thousand nodes is two million selector matches.
/// Grouping by the rightmost tag removes most of them before the matcher runs:
/// `.nav a` cannot match a `<div>` whatever the rest of the selector says.
/// Both lists stay in source order so the `order` counter still breaks ties.
#[derive(Debug, Default)]
struct RuleIndex {
    /// Rules whose rightmost compound names a tag.
    by_tag: std::collections::HashMap<String, Vec<usize>>,
    /// Rules whose rightmost compound names an id but no tag.
    by_id: std::collections::HashMap<String, Vec<usize>>,
    /// Rules whose rightmost compound names a class but no tag or id, keyed by
    /// their first class. Any rule that can match a node must have all of its
    /// classes, so keying on one is a safe pre-filter.
    by_class: std::collections::HashMap<String, Vec<usize>>,
    /// Rules that constrain nothing: `*`, `:hover`, `:not(.a)`.
    universal: Vec<usize>,
}

impl RuleIndex {
    fn build(sheet: &Stylesheet) -> RuleIndex {
        let mut index = RuleIndex::default();
        for (i, rule) in sheet.rules.iter().enumerate() {
            let Some(compound) = rule.selector.compounds.last() else {
                index.universal.push(i);
                continue;
            };
            if let Some(tag) = &compound.tag {
                index.by_tag.entry(tag.clone()).or_default().push(i);
            } else if let Some(id) = &compound.id {
                index.by_id.entry(id.clone()).or_default().push(i);
            } else if let Some(class) = compound.classes.first() {
                index.by_class.entry(class.clone()).or_default().push(i);
            } else {
                index.universal.push(i);
            }
        }
        index
    }

    /// Every rule that could match a node with this tag, id and class list.
    ///
    /// Fills `out` in source order, without duplicates. The caller reuses one
    /// buffer for the whole document: a `Vec` per node was thousands of
    /// allocations for a list that is only read once.
    fn fill_candidates(&self, out: &mut Vec<usize>, tag: &str, id: Option<&str>, classes: &str) {
        out.clear();
        if let Some(rules) = self.by_tag.get(tag) {
            out.extend_from_slice(rules);
        }
        if let Some(id) = id {
            if let Some(rules) = self.by_id.get(id) {
                out.extend_from_slice(rules);
            }
        }
        for class in classes.split_whitespace() {
            if let Some(rules) = self.by_class.get(class) {
                out.extend_from_slice(rules);
            }
        }
        out.extend_from_slice(&self.universal);
        out.sort_unstable();
        out.dedup();
    }
}

/// Compute a style for every node with an explicit viewport.
pub fn compute_styles_with(
    doc: &Document,
    author: &Stylesheet,
    context: StyleContext,
) -> Vec<ComputedStyle> {
    compute_styles_scoped(doc, author, &std::collections::HashMap::new(), context)
}

/// Compute a style for every node, honouring shadow-tree scoping.
///
/// `shadow` maps each shadow root to the stylesheet parsed from the `<style>`
/// elements inside it. Two rules make the encapsulation real:
///
/// - a node inside a shadow tree is styled by the UA sheet plus **its own
///   tree's** sheet; the document's author sheet does not reach it;
/// - a rule from a shadow tree reaches the host only through `:host`, so
///   `.card { … }` inside a component cannot restyle the page.
///
/// Inherited properties still flow from the host, because the shadow root
/// node's parent is the host.
pub fn compute_styles_scoped(
    doc: &Document,
    author: &Stylesheet,
    shadow: &std::collections::HashMap<NodeId, Stylesheet>,
    context: StyleContext,
) -> Vec<ComputedStyle> {
    let ua = ua_stylesheet();
    // Which rules can even match a node, keyed by the tag its **rightmost**
    // compound requires. A selector like `.nav a` can only match an `<a>`, so
    // the whole rule is skipped for every other tag without touching the
    // matching code. Rules whose rightmost compound has no tag (`.card`,
    // `#id`, `*`) go in the universal bucket. Semantics are unchanged: the
    // matcher checks tag equality first anyway.
    let ua_index = RuleIndex::build(ua);
    let author_index = RuleIndex::build(author);
    let shadow_indexes: std::collections::HashMap<NodeId, RuleIndex> =
        shadow.iter().map(|(root, sheet)| (*root, RuleIndex::build(sheet))).collect();
    // Custom properties inherit through the host, so a shadow tree sees the
    // document's variables and its own, its own winning on a clash.
    let shadow_vars: std::collections::HashMap<NodeId, std::collections::HashMap<String, String>> =
        shadow
            .iter()
            .map(|(root, sheet)| {
                let mut merged = author.variables.clone();
                for (name, value) in &sheet.variables {
                    merged.insert(name.clone(), value.clone());
                }
                (*root, merged)
            })
            .collect();
    let mut styles = vec![ComputedStyle::default(); doc.nodes.len()];
    let mut order = 0usize;
    // `rem` follows the root element's computed font-size; before the root is
    // styled (and on documents without `<html>`) it is the initial value.
    let mut root_font_size = context.root_font_size;

    let mut candidates: Vec<usize> = Vec::with_capacity(64);
    for id in 0..doc.nodes.len() {
        let is_element = doc.tag(id).is_some();
        // The inherited properties are all `Copy`; cloning the whole
        // `ComputedStyle` here copied its `String`s and `Vec`s for every node
        // in the document just to read nine scalars.
        let parent_style = doc
            .node(id)
            .parent
            .map(|p| styles[p].clone())
            .unwrap_or_default();

        if !is_element {
            let mut inherited = ComputedStyle::default();
            inherited.color = parent_style.color;
            inherited.font_size = parent_style.font_size;
            inherited.font_weight = parent_style.font_weight;
            inherited.font_family = parent_style.font_family.clone();
            inherited.italic = parent_style.italic;
            inherited.text_align = parent_style.text_align;
            inherited.line_height = parent_style.line_height;
            inherited.underline = parent_style.underline;
            inherited.list_marker = parent_style.list_marker;
            inherited.visible = parent_style.visible;
            inherited.opacity = parent_style.opacity;
            styles[id] = inherited;
            continue;
        }

        let mut style = ComputedStyle::default();
        style.color = parent_style.color;
        style.font_size = parent_style.font_size;
        style.font_weight = parent_style.font_weight;
        style.font_family = parent_style.font_family.clone();
        style.italic = parent_style.italic;
        style.text_align = parent_style.text_align;
        style.line_height = parent_style.line_height;
        // `text-decoration` is not inherited per spec, but it propagates to
        // inline descendants; inheriting matches what users see.
        style.underline = parent_style.underline;
        // `list-style-type` is inherited; `ul { list-style: none }` disables
        // the marker for every item below it.
        style.list_marker = parent_style.list_marker;
        // `visibility` is inherited; `opacity` is not, but it applies to the
        // whole subtree, so the parent's factor is carried down (an
        // approximation: real compositing would multiply the layers).
        style.visible = parent_style.visible;
        style.opacity = parent_style.opacity;

        // Five priority levels, weakest first: UA normal, author normal,
        // inline normal, author !important, inline !important. Sorting on
        // `important` before specificity is what makes an important author
        // rule beat a normal inline one.
        const SPEC_INLINE: Specificity =
            Specificity { inline: true, ids: 0, classes: 0, tags: 0 };
        let inline_decls = doc
            .attr(id, "style")
            .map(parse_declarations)
            .unwrap_or_default();
        // Which sheets apply to this node. `host_only` keeps every rule of a
        // shadow sheet off the host except the `:host` ones.
        let scope = doc.shadow_scope(id);
        let mut sources: Vec<(&Stylesheet, &RuleIndex, Option<NodeId>, bool)> =
            Vec::with_capacity(3);
        sources.push((ua, &ua_index, None, false));
        match scope {
            None => {
                // The host's own sheet is applied **before** the document's:
                // when a document rule and a `:host` rule set the same
                // property, the outer tree wins for normal declarations and
                // the inner one wins for `!important`, per CSS Scoping.
                if let Some(root) = doc.shadow_root(id) {
                    if let (Some(sheet), Some(index)) = (shadow.get(&root), shadow_indexes.get(&root))
                    {
                        sources.push((sheet, index, Some(root), true));
                    }
                }
                sources.push((author, &author_index, None, false));
            }
            Some(root) => {
                if let (Some(sheet), Some(index)) = (shadow.get(&root), shadow_indexes.get(&root)) {
                    sources.push((sheet, index, Some(root), false));
                }
            }
        }
        let variables = scope
            .and_then(|root| shadow_vars.get(&root))
            .unwrap_or(&author.variables);

        // A `:host` declaration that is `!important` must beat an important
        // document rule, and the ordering key is `order`, so those entries are
        // pushed past every document declaration.
        const INNER_IMPORTANT_BASE: usize = 1 << 20;
        let mut entries: Vec<(bool, Specificity, usize, &Declaration)> = Vec::new();
        let tag = doc.tag(id).unwrap_or("");
        let node_id = doc.attr(id, "id");
        let node_classes = doc.attr(id, "class").unwrap_or("");
        for (sheet, index, rule_scope, host_only) in sources {
            // Candidates are visited in source order, because `order` is what
            // resolves a specificity tie.
            index.fill_candidates(&mut candidates, tag, node_id, node_classes);
            for &rule_index in &candidates {
                let rule = &sheet.rules[rule_index];
                if host_only && !rule.selector.has_host_pseudo() {
                    continue;
                }
                if !rule.selector.matches_scoped(doc, id, rule_scope) {
                    continue;
                }
                let spec = rule.selector.specificity();
                for decl in &rule.declarations {
                    let key = if host_only && decl.important {
                        INNER_IMPORTANT_BASE + order
                    } else {
                        order
                    };
                    entries.push((decl.important, spec, key, decl));
                    order += 1;
                }
            }
        }
        for decl in &inline_decls {
            entries.push((decl.important, SPEC_INLINE, order, decl));
            order += 1;
        }
        entries.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });

        // Pass 1 settles the font size, because `em` everywhere else depends
        // on it. In `font-size` itself `em`/`%` use the parent's font size.
        let font_ctx = LengthCtx {
            viewport_width: context.viewport_width,
            viewport_height: context.viewport_height,
            root_font_size,
            em_base: parent_style.font_size,
            parent_opacity: parent_style.opacity,
        };
        for (_, _, _, decl) in &entries {
            apply_declaration(&mut style, decl, variables, font_ctx, true);
        }

        // Pass 2 resolves every other property against the element's own
        // computed font size.
        let length_ctx = LengthCtx { em_base: style.font_size, ..font_ctx };
        for (_, _, _, decl) in &entries {
            apply_declaration(&mut style, decl, variables, length_ctx, false);
        }

        if doc.tag(id) == Some("html") {
            root_font_size = style.font_size;
        }
        styles[id] = style;
    }

    styles
}

/// Parse a `<length>` (or `<length-percentage>`) into a `Dim`.
///
/// Understands `px`, `em`, `rem`, `vw`, `vh`, `vmin`, `vmax`, `%`, the unitless
/// zero, and the `calc()`/`min()`/`max()`/`clamp()` functions. Returns `None`
/// for anything else — including values that are only invalid in the property
/// being parsed, which the caller then rejects as a whole declaration.
fn parse_length_value(text: &str, ctx: LengthCtx) -> Option<Dim> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(open) = text.find('(') {
        let name = text[..open].trim().to_ascii_lowercase();
        if !matches!(name.as_str(), "calc" | "min" | "max" | "clamp") {
            return None;
        }
        let mut parser = CalcParser { bytes: text.as_bytes(), pos: 0, ctx };
        let value = parser.parse_value()?;
        parser.skip_ws();
        if parser.pos != parser.bytes.len() {
            return None;
        }
        match value {
            CalcValue::Length(dim) => Some(dim),
            // A bare number is not a length, except for the unitless zero.
            CalcValue::Number(n) if n == 0.0 => Some(Dim::px(0.0)),
            CalcValue::Number(_) => None,
        }
    } else {
        parse_dimension_token(text, ctx)
    }
}

/// `12px`, `1.5rem`, `50%`, `0`…
fn parse_dimension_token(token: &str, ctx: LengthCtx) -> Option<Dim> {
    let token = token.trim();
    let split = token
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(token.len());
    let (number, unit) = token.split_at(split);
    let n: f32 = number.trim().parse().ok()?;
    dimension(n, unit, ctx)
}

fn dimension(n: f32, unit: &str, ctx: LengthCtx) -> Option<Dim> {
    let unit = unit.trim();
    if unit.is_empty() {
        return if n == 0.0 { Some(Dim::px(0.0)) } else { None };
    }
    let dim = match unit.to_ascii_lowercase().as_str() {
        "px" => Dim::px(n),
        "em" => Dim::px(n * ctx.em_base),
        "rem" => Dim::px(n * ctx.root_font_size),
        "vw" => Dim::px(n * ctx.viewport_width / 100.0),
        "vh" => Dim::px(n * ctx.viewport_height / 100.0),
        "vmin" => Dim::px(n * ctx.viewport_width.min(ctx.viewport_height) / 100.0),
        "vmax" => Dim::px(n * ctx.viewport_width.max(ctx.viewport_height) / 100.0),
        "%" => Dim::percent(n),
        _ => return None,
    };
    Some(dim)
}

enum CalcValue {
    Length(Dim),
    Number(f32),
}

/// Recursive-descent parser for `calc()`/`min()`/`max()`/`clamp()`.
///
/// Grammar: `expr := term (('+'|'-') term)*`, `term := factor (('*'|'/') factor)*`,
/// `factor := number [unit] | function | '(' expr ')' | ('+'|'-') factor`.
/// Dimensional rules follow CSS: `+`/`-` need matching kinds, `*` needs at
/// least one number, `/` needs a number divisor.
struct CalcParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    ctx: LengthCtx,
}

impl CalcParser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Entry point: an expression whose result must be the whole input.
    fn parse_value(&mut self) -> Option<CalcValue> {
        self.parse_expression()
    }

    fn parse_expression(&mut self) -> Option<CalcValue> {
        let mut left = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let right = self.parse_term()?;
                    left = calc_add(left, right, 1.0)?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    let right = self.parse_term()?;
                    left = calc_add(left, right, -1.0)?;
                }
                _ => return Some(left),
            }
        }
    }

    fn parse_term(&mut self) -> Option<CalcValue> {
        let mut left = self.parse_factor()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    let right = self.parse_factor()?;
                    left = calc_mul(left, right)?;
                }
                Some(b'/') => {
                    self.pos += 1;
                    let right = self.parse_factor()?;
                    left = calc_div(left, right)?;
                }
                _ => return Some(left),
            }
        }
    }

    fn parse_factor(&mut self) -> Option<CalcValue> {
        self.skip_ws();
        match self.peek()? {
            b'(' => {
                self.pos += 1;
                let value = self.parse_expression()?;
                self.skip_ws();
                if self.peek()? != b')' {
                    return None;
                }
                self.pos += 1;
                Some(value)
            }
            b'+' => {
                self.pos += 1;
                self.parse_factor()
            }
            b'-' => {
                self.pos += 1;
                match self.parse_factor()? {
                    CalcValue::Length(dim) => Some(CalcValue::Length(dim.scale(-1.0))),
                    CalcValue::Number(n) => Some(CalcValue::Number(-n)),
                }
            }
            c if c.is_ascii_alphabetic() => self.parse_function(),
            _ => self.parse_number(),
        }
    }

    fn parse_number(&mut self) -> Option<CalcValue> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return None;
        }
        let n: f32 = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?.parse().ok()?;
        let unit_start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let unit = std::str::from_utf8(&self.bytes[unit_start..self.pos]).ok()?;
        if unit.is_empty() {
            // `50%` keeps its percentage sign.
            if self.peek() == Some(b'%') {
                self.pos += 1;
                return Some(CalcValue::Length(Dim::percent(n)));
            }
            return Some(CalcValue::Number(n));
        }
        dimension(n, unit, self.ctx).map(CalcValue::Length)
    }

    fn parse_function(&mut self) -> Option<CalcValue> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() || c == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?.to_ascii_lowercase();
        self.skip_ws();
        if self.peek()? != b'(' {
            return None;
        }
        self.pos += 1;
        match name.as_str() {
            "calc" => {
                let value = self.parse_expression()?;
                self.skip_ws();
                if self.peek()? != b')' {
                    return None;
                }
                self.pos += 1;
                Some(value)
            }
            "min" | "max" => {
                let mut args = vec![self.parse_length_argument()?];
                loop {
                    self.skip_ws();
                    match self.peek()? {
                        b',' => {
                            self.pos += 1;
                            args.push(self.parse_length_argument()?);
                        }
                        b')' => {
                            self.pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(CalcValue::Length(if name == "min" {
                    fold_min(args)
                } else {
                    fold_max(args)
                }))
            }
            "clamp" => {
                let low = self.parse_length_argument()?;
                self.skip_ws();
                if self.peek()? != b',' {
                    return None;
                }
                self.pos += 1;
                let value = self.parse_length_argument()?;
                self.skip_ws();
                if self.peek()? != b',' {
                    return None;
                }
                self.pos += 1;
                let high = self.parse_length_argument()?;
                self.skip_ws();
                if self.peek()? != b')' {
                    return None;
                }
                self.pos += 1;
                Some(CalcValue::Length(fold_clamp(low, value, high)))
            }
            _ => None,
        }
    }

    /// An argument of `min()`/`max()`/`clamp()`, which must be a length.
    fn parse_length_argument(&mut self) -> Option<Dim> {
        match self.parse_expression()? {
            CalcValue::Length(dim) => Some(dim),
            CalcValue::Number(n) if n == 0.0 => Some(Dim::px(0.0)),
            CalcValue::Number(_) => None,
        }
    }
}

fn calc_add(left: CalcValue, right: CalcValue, sign: f32) -> Option<CalcValue> {
    match (left, right) {
        (CalcValue::Number(a), CalcValue::Number(b)) => Some(CalcValue::Number(a + sign * b)),
        (CalcValue::Length(a), CalcValue::Length(b)) => {
            a.combine(&b, sign).map(CalcValue::Length)
        }
        // Mixing a number with a length is invalid in CSS.
        _ => None,
    }
}

fn calc_mul(left: CalcValue, right: CalcValue) -> Option<CalcValue> {
    match (left, right) {
        (CalcValue::Number(a), CalcValue::Number(b)) => Some(CalcValue::Number(a * b)),
        (CalcValue::Length(dim), CalcValue::Number(n))
        | (CalcValue::Number(n), CalcValue::Length(dim)) => {
            Some(CalcValue::Length(dim.scale(n)))
        }
        // `px * px` is not a length.
        (CalcValue::Length(_), CalcValue::Length(_)) => None,
    }
}

fn calc_div(left: CalcValue, right: CalcValue) -> Option<CalcValue> {
    match (left, right) {
        (CalcValue::Number(a), CalcValue::Number(b)) if b != 0.0 => {
            Some(CalcValue::Number(a / b))
        }
        (CalcValue::Length(dim), CalcValue::Number(n)) if n != 0.0 => {
            Some(CalcValue::Length(dim.scale(1.0 / n)))
        }
        _ => None,
    }
}

/// Fold `min()` when every argument is already absolute.
fn fold_min(mut args: Vec<Dim>) -> Dim {
    if args.iter().all(|d| d.absolute().is_some()) {
        return Dim::px(args.iter().filter_map(|d| d.absolute()).fold(f32::INFINITY, f32::min));
    }
    let mut acc = args.remove(0);
    for arg in args {
        acc = Dim::Min(Box::new(acc), Box::new(arg));
    }
    acc
}

fn fold_max(mut args: Vec<Dim>) -> Dim {
    if args.iter().all(|d| d.absolute().is_some()) {
        return Dim::px(
            args.iter()
                .filter_map(|d| d.absolute())
                .fold(f32::NEG_INFINITY, f32::max),
        );
    }
    let mut acc = args.remove(0);
    for arg in args {
        acc = Dim::Max(Box::new(acc), Box::new(arg));
    }
    acc
}

fn fold_clamp(low: Dim, value: Dim, high: Dim) -> Dim {
    if let (Some(l), Some(v), Some(h)) = (low.absolute(), value.absolute(), high.absolute()) {
        return Dim::px(v.max(l).min(h));
    }
    Dim::Clamp(Box::new(low), Box::new(value), Box::new(high))
}

/// A length that must be resolvable without layout (font-size, padding,
/// border, gap…). A percentage here has no containing block yet, so the whole
/// declaration is rejected instead of guessing a base.
fn parse_length(value: &str, ctx: LengthCtx) -> Option<f32> {
    parse_length_value(value, ctx)?.absolute()
}

fn parse_edges(value: &str, ctx: LengthCtx) -> Option<EdgesDim> {
    let mut parts: Vec<Dim> = Vec::new();
    // Split at depth zero, so `calc(1em + 4px)` stays a single component.
    for token in split_top_level(value) {
        // A single unparseable component invalidates the whole shorthand,
        // instead of silently collapsing to zero.
        parts.push(parse_dim(&token, ctx)?);
    }
    match parts.len() {
        1 => Some(EdgesDim::uniform(parts[0].clone())),
        2 => Some(EdgesDim {
            top: parts[0].clone(),
            right: parts[1].clone(),
            bottom: parts[0].clone(),
            left: parts[1].clone(),
        }),
        3 => Some(EdgesDim {
            top: parts[0].clone(),
            right: parts[1].clone(),
            bottom: parts[2].clone(),
            left: parts[1].clone(),
        }),
        4 => Some(EdgesDim {
            top: parts[0].clone(),
            right: parts[1].clone(),
            bottom: parts[2].clone(),
            left: parts[3].clone(),
        }),
        _ => None,
    }
}

/// Replace `var(--name, fallback)` with the value declared for `--name`.
/// Unresolvable references fall back to the provided default, or are dropped.
fn resolve_vars(value: &str, vars: &std::collections::HashMap<String, String>) -> String {
    if !value.contains("var(") {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find("var(") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        let end = match after.find(')') {
            Some(e) => e,
            None => {
                out.push_str(&rest[pos..]);
                return out;
            }
        };
        let inner = &after[..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        match vars.get(name) {
            Some(v) => out.push_str(v),
            None => {
                if let Some(f) = fallback {
                    out.push_str(&resolve_vars(f, vars));
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `100px 1fr 2fr`, `repeat(3, 1fr)` and `auto` are accepted; anything else
/// is treated as an equal `1fr` track.
fn parse_tracks(value: &str, ctx: LengthCtx) -> (Vec<Track>, Option<AutoRepeat>) {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix("repeat(") {
        if let Some(inner) = inner.strip_suffix(')') {
            let mut parts = inner.splitn(2, ',');
            let head = parts.next().unwrap_or("1").trim();
            let spec = parts.next().unwrap_or("1fr").trim();
            let track = parse_track(spec, ctx).unwrap_or_default();
            if head.starts_with("auto-fit") || head.starts_with("auto-fill") {
                return (
                    Vec::new(),
                    Some(AutoRepeat { fill: head.starts_with("auto-fill"), track }),
                );
            }
            let count = head.parse::<usize>().unwrap_or(1).min(12);
            return (vec![track; count], None);
        }
    }
    let tracks: Vec<Track> = split_top_level(value)
        .into_iter()
        .filter_map(|t| parse_track(&t, ctx))
        .collect();
    (tracks, None)
}

/// Split on whitespace but keep `minmax(a, b)` together.
fn split_top_level(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in value.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// `grid-column: 2`, `grid-column: span 2`, `grid-column: 1 / 3`,
/// `grid-column: 1 / span 2`.
fn parse_placement(value: &str) -> Option<GridPlacement> {
    let value = value.trim();
    let (start_part, end_part) = match value.split_once('/') {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (value, None),
    };
    let parse_one = |part: &str| -> Option<(Option<u16>, u16)> {
        if let Some(span) = part.strip_prefix("span ") {
            return Some((None, span.trim().parse().ok()?));
        }
        // `auto` and named lines have no line number; they auto-place.
        if part.eq_ignore_ascii_case("auto") {
            return Some((None, 1));
        }
        let n: u16 = part.parse().ok()?;
        Some((Some(n), 1))
    };
    let (start, mut span) = parse_one(start_part)?;
    if let Some(end) = end_part {
        let (end_line, end_span) = parse_one(end)?;
        span = match (start, end_line) {
            (Some(s), Some(e)) if e > s => e - s,
            _ => end_span,
        };
    }
    Some(GridPlacement { start, span: span.max(1) })
}

/// One track, or `None` when the token is not a size the engine understands.
///
/// Dropping it is important: `grid-template-columns: var(--missing)` used to
/// leave a 0px track per unresolved token, which produced invisible columns and
/// squeezed real items to zero width.
fn parse_track(token: &str, ctx: LengthCtx) -> Option<Track> {
    let token = token.trim();
    if let Some(inner) = token.strip_prefix("minmax(").and_then(|t| t.strip_suffix(')')) {
        let mut parts = inner.splitn(2, ',');
        let min = parts.next().unwrap_or("0").trim();
        let max = parts.next().unwrap_or("auto").trim();
        let mut track = parse_track(max, ctx).unwrap_or_default();
        track.min_px = if min.eq_ignore_ascii_case("auto") {
            0.0
        } else {
            parse_track(min, ctx).map(|t| t.px).unwrap_or(0.0)
        };
        if track.fr == 0.0 && !max.eq_ignore_ascii_case("auto") {
            track.max_px = Some(track.px);
        }
        return Some(track);
    }
    if let Some(fr) = token.strip_suffix("fr") {
        return Some(Track {
            px: 0.0,
            percent: 0.0,
            fr: fr.trim().parse().unwrap_or(1.0),
            min_px: 0.0,
            max_px: None,
        });
    }
    // `auto` and the content keywords all mean "size from the content".
    // A track resolved without content knowledge behaves like `auto` (it takes
    // a share of the free space) instead of collapsing to zero, which used to
    // leave grid items with a 0px width.
    if token.eq_ignore_ascii_case("auto")
        || token.is_empty()
        || token.eq_ignore_ascii_case("min-content")
        || token.eq_ignore_ascii_case("max-content")
        || token.starts_with("fit-content")
    {
        return Some(Track { px: 0.0, percent: 0.0, fr: 1.0, min_px: 0.0, max_px: None });
    }
    if let Some(pct) = token.strip_suffix('%') {
        let percent = pct.trim().parse::<f32>().unwrap_or(0.0);
        return Some(Track { px: 0.0, percent, fr: 0.0, min_px: 0.0, max_px: None });
    }
    let px = parse_length(token, ctx)?;
    Some(Track { px, percent: 0.0, fr: 0.0, min_px: 0.0, max_px: None })
}

/// Map a CSS logical property to its physical equivalent for a left-to-right
/// document.
///
/// Real stylesheets position and space elements with `inset-inline-*` and
/// `margin-inline-*`; without this mapping those declarations are dropped as
/// unknown and elements that rely on them collapse onto each other. The engine
/// has no `direction`/`writing-mode` support, so RTL and vertical text are not
/// modelled — for LTR horizontal text the mapping is exact.
fn logical_to_physical(name: &str) -> &str {
    match name {
        "inset-inline-start" | "inset-inline" => "left",
        "inset-inline-end" => "right",
        "inset-block-start" | "inset-block" => "top",
        "inset-block-end" => "bottom",
        "margin-inline-start" | "margin-inline" => "margin-left",
        "margin-inline-end" => "margin-right",
        "margin-block-start" | "margin-block" => "margin-top",
        "margin-block-end" => "margin-bottom",
        "padding-inline-start" | "padding-inline" => "padding-left",
        "padding-inline-end" => "padding-right",
        "padding-block-start" | "padding-block" => "padding-top",
        "padding-block-end" => "padding-bottom",
        "border-inline-start-width" => "border-left-width",
        "border-inline-end-width" => "border-right-width",
        other => other,
    }
}

fn apply_declaration(
    style: &mut ComputedStyle,
    decl: &Declaration,
    variables: &std::collections::HashMap<String, String>,
    ctx: LengthCtx,
    font_phase: bool,
) {
    // `em` in any property refers to the element's own computed font-size, so
    // the font size has to be settled first: the cascade runs twice, once for
    // `font-size`/`font` and once for everything else. The phase check comes
    // before the value is materialized, so the pass that does not want this
    // declaration costs one comparison.
    let name = logical_to_physical(&decl.name);
    let is_font = matches!(name, "font-size" | "font");
    if is_font != font_phase {
        return;
    }
    // `var()` needs substitution, which allocates. The common case — a plain
    // value — borrows: cloning the string here copied every declaration of
    // every matching rule, on both passes.
    let raw: std::borrow::Cow<'_, str> = if decl.value.contains("var(") {
        std::borrow::Cow::Owned(resolve_vars(&decl.value, variables))
    } else {
        std::borrow::Cow::Borrowed(decl.value.as_str())
    };
    let value = raw.trim();
    match name {
        "display" => {
            style.display = match value.to_ascii_lowercase().as_str() {
                "block" => Display::Block,
                "inline" => Display::Inline,
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" => Display::Grid,
                "table" | "inline-table" => Display::Table,
                "table-row" => Display::TableRow,
                "table-cell" => Display::TableCell,
                "inline-block" => Display::InlineBlock,
                "contents" => Display::Contents,
                "none" => Display::None,
                _ => return,
            };
        }
        "color" => {
            if let Some(c) = Color::parse(value) {
                style.color = c;
            }
        }
        "background-color" => {
            if let Some(c) = Color::parse(value) {
                style.background = c;
            }
        }
        "background" => {
            // A gradient wins over the colour: `background: linear-gradient(...)
            // #fff` paints the gradient, and the colour is only the fallback
            // behind a transparent stop.
            if let Some(gradient) = parse_gradient(value) {
                style.background_gradient = Some(gradient);
            } else {
                style.background_gradient = None;
                for token in value.split_whitespace() {
                    if let Some(c) = Color::parse(token) {
                        style.background = c;
                        break;
                    }
                }
            }
        }
        "font-size" => {
            // `em` and `%` in `font-size` refer to the parent's font size,
            // which is what `ctx.em_base` carries during the font phase.
            let base = ctx.em_base;
            if value.eq_ignore_ascii_case("inherit") {
                style.font_size = base;
            } else if let Some(dim) = parse_length_value(value, ctx) {
                let px = match &dim {
                    Dim::Linear { .. } => Some(dim.resolve(base)),
                    _ => dim.absolute(),
                };
                if let Some(px) = px {
                    style.font_size = px.max(1.0);
                }
            }
        }
        "font-weight" => {
            let lower = value.to_ascii_lowercase();
            style.font_weight = match lower.as_str() {
                "normal" => 400,
                "bold" => 700,
                "bolder" => 700,
                "lighter" => 300,
                _ => value.parse::<u16>().unwrap_or(style.font_weight),
            };
        }
        "font-family" => {
            style.font_family = parse_family_list(value);
        }
        "font-style" => {
            style.italic = matches!(
                value.to_ascii_lowercase().as_str(),
                "italic" | "oblique"
            );
        }
        "transform" => {
            style.transform = parse_transform(value);
        }
        "transform-origin" => {
            style.transform_origin = parse_transform_origin(value);
        }
        "object-fit" => {
            style.object_fit = match value.to_ascii_lowercase().as_str() {
                "contain" => ObjectFit::Contain,
                "cover" => ObjectFit::Cover,
                "none" => ObjectFit::None,
                "scale-down" => ObjectFit::ScaleDown,
                _ => ObjectFit::Fill,
            };
        }
        "aspect-ratio" => {
            style.aspect_ratio = parse_aspect_ratio(value);
        }
        "float" => {
            style.float = match value.to_ascii_lowercase().as_str() {
                "left" => Float::Left,
                "right" => Float::Right,
                _ => Float::None,
            };
        }
        "clear" => {
            style.clear = match value.to_ascii_lowercase().as_str() {
                "left" => Clear::Left,
                "right" => Clear::Right,
                "both" => Clear::Both,
                _ => Clear::None,
            };
        }
        "vertical-align" => {
            style.vertical_align = parse_vertical_align(value, ctx);
        }
        "border-collapse" => {
            style.border_collapse = value.eq_ignore_ascii_case("collapse");
        }
        "order" => {
            if let Ok(n) = value.trim().parse::<i32>() {
                style.order = n;
            }
        }
        "grid-area" => {
            if let Some((row, column)) = parse_grid_area(value) {
                style.grid_row = row;
                style.grid_column = column;
            }
        }
        "margin" => {
            let parts: Vec<String> = split_top_level(value);
            let has_auto = parts.iter().any(|p| p.eq_ignore_ascii_case("auto"));
            if has_auto {
                // `margin: <v> auto` is the common centering idiom.
                let v = parts
                    .iter()
                    .find(|p| !p.eq_ignore_ascii_case("auto"))
                    .and_then(|p| parse_dim(p, ctx))
                    .unwrap_or(Dim::px(0.0));
                let (left_auto, right_auto) = match parts.len() {
                    1 => (true, true),
                    2 => (parts[1].eq_ignore_ascii_case("auto"), parts[1].eq_ignore_ascii_case("auto")),
                    4 => (
                        parts[3].eq_ignore_ascii_case("auto"),
                        parts[1].eq_ignore_ascii_case("auto"),
                    ),
                    _ => (true, true),
                };
                style.margin = EdgesDim {
                    top: v.clone(),
                    right: Dim::px(0.0),
                    bottom: v,
                    left: Dim::px(0.0),
                };
                style.margin_auto_left = left_auto;
                style.margin_auto_right = right_auto;
            } else if let Some(e) = parse_edges(value, ctx) {
                style.margin = e;
                style.margin_auto_left = false;
                style.margin_auto_right = false;
            }
        }
        "box-sizing" => {
            style.border_box = value.eq_ignore_ascii_case("border-box");
        }
        "min-width" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.min_width = Some(d);
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            style.overflow = match value.to_ascii_lowercase().as_str() {
                "hidden" => Overflow::Hidden,
                "auto" => Overflow::Auto,
                "scroll" => Overflow::Scroll,
                _ => Overflow::Visible,
            };
        }
        "position" => {
            style.position = match value.to_ascii_lowercase().as_str() {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                _ => Position::Static,
            };
        }
        "top" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.top = Some(d);
            }
        }
        "right" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.right = Some(d);
            }
        }
        "bottom" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.bottom = Some(d);
            }
        }
        "left" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.left = Some(d);
            }
        }
        "grid-template-columns" | "grid-template-rows" => {
            let (tracks, repeat) = parse_tracks(value, ctx);
            if name == "grid-template-columns" {
                style.grid_columns = tracks;
                style.grid_auto_repeat = repeat;
            } else {
                style.grid_rows = tracks;
            }
        }
        "border-radius" => {
            let first = value.split('/').next().unwrap_or(value).trim();
            // `50%` cannot be resolved here: the percentage is of the box, and
            // only layout knows the box. The fraction is kept and turned into
            // pixels in `Flow::block`.
            if let Some(pct) = first.strip_suffix('%') {
                if let Ok(v) = pct.trim().parse::<f32>() {
                    style.border_radius_percent = v.max(0.0);
                    style.border_radius = 0.0;
                }
            } else if let Some(v) = parse_length(first, ctx) {
                style.border_radius = v.max(0.0);
                style.border_radius_percent = 0.0;
            }
        }
        "box-shadow" => {
            style.box_shadow = None;
            if !value.eq_ignore_ascii_case("none") {
                // Only the first shadow of a list: `a, b` is rare and a second
                // shadow is not worth a second mask per box. Split at top level
                // so the commas inside `rgba()` do not cut the value.
                let first = split_top_level_commas(value)
                    .first()
                    .copied()
                    .unwrap_or(value);
                let mut nums: Vec<f32> = Vec::new();
                let mut color: Option<Color> = None;
                let mut inset = false;
                for token in first.split_whitespace() {
                    if token.eq_ignore_ascii_case("inset") {
                        inset = true;
                    } else if let Some(c) = Color::parse(token) {
                        color = Some(c);
                    } else if let Some(v) = parse_length(token, ctx) {
                        nums.push(v);
                    }
                }
                if nums.len() >= 2 {
                    style.box_shadow = Some(BoxShadow {
                        dx: nums[0],
                        dy: nums[1],
                        blur: nums.get(2).copied().unwrap_or(0.0).max(0.0),
                        spread: nums.get(3).copied().unwrap_or(0.0),
                        // No colour means a soft black, which is what a page
                        // that omits it expects to see.
                        color: color.unwrap_or(Color { r: 0, g: 0, b: 0, a: 60 }),
                        inset,
                    });
                }
            }
        }
        "grid-column" | "grid-column-start" => {
            style.grid_column = parse_placement(value).unwrap_or_default();
        }
        "grid-row" | "grid-row-start" => {
            style.grid_row = parse_placement(value).unwrap_or_default();
        }
        "grid-gap" | "grid-row-gap" | "grid-column-gap" => {
            if let Some(v) = parse_length(value, ctx) {
                style.grid_gap = v;
            }
        }
        "list-style" | "list-style-type" => {
            style.list_marker = !value.to_ascii_lowercase().contains("none");
        }
        "text-decoration" | "text-decoration-line" => {
            style.underline = value.to_ascii_lowercase().contains("underline");
        }
        // An unparseable value is ignored, not treated as `auto`: otherwise a
        // later unsupported value would wipe out an earlier valid one.
        "height" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.height = Some(d);
            }
        }
        "width" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.width = Some(d);
            }
        }
        "max-width" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.max_width = Some(d);
            }
        }
        "flex-grow" => {
            if let Ok(n) = value.parse::<f32>() {
                style.flex_grow = n.max(0.0);
            }
        }
        "flex-shrink" => {
            if let Ok(n) = value.parse::<f32>() {
                style.flex_shrink = n.max(0.0);
            }
        }
        "flex-basis" => {
            // `auto` keeps the item's own width; `content` is max-content.
            style.flex_basis = if value.eq_ignore_ascii_case("auto")
                || value.eq_ignore_ascii_case("content")
            {
                None
            } else {
                parse_dim(value, ctx).and_then(|d| d.absolute())
            };
        }
        "flex" => {
            // `flex: <grow> [<shrink>] [<basis>]`, plus the keywords.
            let lower = value.to_ascii_lowercase();
            match lower.as_str() {
                "none" => {
                    style.flex_grow = 0.0;
                    style.flex_shrink = 0.0;
                    style.flex_basis = None;
                }
                "auto" => {
                    style.flex_grow = 1.0;
                    style.flex_shrink = 1.0;
                    style.flex_basis = None;
                }
                "initial" => {
                    style.flex_grow = 0.0;
                    style.flex_shrink = 1.0;
                    style.flex_basis = None;
                }
                _ => {
                    let mut numbers = 0usize;
                    for token in split_top_level(value) {
                        if let Ok(n) = token.parse::<f32>() {
                            // The first number is grow, the second shrink.
                            if numbers == 0 {
                                style.flex_grow = n.max(0.0);
                            } else if numbers == 1 {
                                style.flex_shrink = n.max(0.0);
                            }
                            numbers += 1;
                        } else if token.eq_ignore_ascii_case("auto")
                            || token.eq_ignore_ascii_case("content")
                        {
                            style.flex_basis = None;
                        } else if let Some(d) = parse_dim(&token, ctx).and_then(|d| d.absolute()) {
                            style.flex_basis = Some(d);
                        }
                    }
                    // A bare `flex: 1` means `1 1 0%`, not `1 1 auto`.
                    if numbers == 1 && style.flex_basis.is_none() {
                        style.flex_shrink = 1.0;
                        style.flex_basis = Some(0.0);
                    }
                }
            }
        }
        "justify-items" | "justify-self" => {
            style.justify_items = match value.to_ascii_lowercase().as_str() {
                "center" => AlignItems::Center,
                "flex-end" | "end" | "right" => AlignItems::End,
                "flex-start" | "start" | "left" => AlignItems::Start,
                _ => AlignItems::Stretch,
            };
        }
        "place-items" => {
            // `<align-items> <justify-items>`; one value applies to both.
            let mut parts = split_top_level(value).into_iter();
            let first = parts.next().unwrap_or_default().to_ascii_lowercase();
            let second = parts.next().unwrap_or_else(|| first.clone()).to_ascii_lowercase();
            let map = |v: &str| match v {
                "center" => AlignItems::Center,
                "flex-end" | "end" | "right" => AlignItems::End,
                "flex-start" | "start" | "left" => AlignItems::Start,
                _ => AlignItems::Stretch,
            };
            style.align_items = map(&first);
            style.justify_items = map(&second);
        }
        "align-content" => {
            style.align_content = match value.to_ascii_lowercase().as_str() {
                "center" => AlignContent::Center,
                "flex-end" | "end" => AlignContent::End,
                "space-between" => AlignContent::SpaceBetween,
                "space-around" | "space-evenly" => AlignContent::SpaceAround,
                "stretch" => AlignContent::Stretch,
                _ => AlignContent::Start,
            };
        }
        "min-height" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.min_height = Some(d);
            }
        }
        "max-height" => {
            if let Some(d) = parse_dim(value, ctx) {
                style.max_height = Some(d);
            }
        }
        "table-layout" => {
            style.table_fixed = value.eq_ignore_ascii_case("fixed");
        }
        "opacity" => {
            if let Ok(n) = value.trim().parse::<f32>() {
                style.opacity = (n.clamp(0.0, 1.0) * ctx.parent_opacity).clamp(0.0, 1.0);
            }
        }
        "visibility" => {
            style.visible = !matches!(
                value.to_ascii_lowercase().as_str(),
                "hidden" | "collapse"
            );
        }
        "align-self" => {
            style.align_self = Some(match value.to_ascii_lowercase().as_str() {
                "center" => AlignItems::Center,
                "flex-end" | "end" => AlignItems::End,
                "flex-start" | "start" => AlignItems::Start,
                "stretch" => AlignItems::Stretch,
                _ => return,
            });
        }
        "background-image" => {
            if let Some(gradient) = parse_gradient(value) {
                style.background_gradient = Some(gradient);
                return;
            }
            style.background_gradient = None;
            if let Some(start) = value.find("url(") {
                let rest = &value[start + 4..];
                if let Some(end) = rest.find(')') {
                    let url = rest[..end].trim().trim_matches(|c| c == '"' || c == '\'');
                    if !url.is_empty() && !url.starts_with("data:") {
                        style.background_image = Some(url.to_string());
                    }
                }
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            // `margin-left: auto` is how a flex row pushes an item to the end.
            if value.eq_ignore_ascii_case("auto") {
                match name {
                    "margin-right" => style.margin_auto_right = true,
                    "margin-left" => style.margin_auto_left = true,
                    // Auto vertical margins do not center in a flex row.
                    _ => {}
                }
            } else if let Some(v) = parse_dim(value, ctx) {
                match name {
                    "margin-top" => style.margin.top = v,
                    "margin-right" => {
                        style.margin.right = v;
                        style.margin_auto_right = false;
                    }
                    "margin-bottom" => style.margin.bottom = v,
                    _ => {
                        style.margin.left = v;
                        style.margin_auto_left = false;
                    }
                }
            }
        }
        "padding" => {
            if let Some(e) = parse_edges(&resolve_vars(value, variables), ctx) {
                style.padding = e;
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_dim(value, ctx) {
                match name {
                    "padding-top" => style.padding.top = v,
                    "padding-right" => style.padding.right = v,
                    "padding-bottom" => style.padding.bottom = v,
                    _ => style.padding.left = v,
                }
            }
        }
        "border" => {
            let mut width = 0.0f32;
            let mut color = style.border_color;
            for token in value.split_whitespace() {
                if let Some(c) = Color::parse(token) {
                    color = c;
                } else if let Some(w) = parse_length(token, ctx) {
                    width = w;
                }
            }
            style.border = Edges::uniform(width);
            style.border_color = color;
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            // Longhands: `<width> <style> <color>`, any order. Only a visible
            // border contributes, but a transparent 8px border still takes
            // space, which is how pages pad a text field.
            let mut width = 0.0f32;
            let mut color = style.border_color;
            for token in value.split_whitespace() {
                if let Some(c) = Color::parse(token) {
                    color = c;
                } else if let Some(w) = parse_length(token, ctx) {
                    width = w;
                }
            }
            match name {
                "border-top" => style.border.top = width,
                "border-right" => style.border.right = width,
                "border-bottom" => style.border.bottom = width,
                _ => style.border.left = width,
            }
            if width > 0.0 {
                style.border_color = color;
            }
        }
        "border-width" => {
            let mut widths: Vec<f32> = Vec::new();
            for token in split_top_level(value) {
                widths.push(parse_length(&token, ctx).unwrap_or(0.0));
            }
            style.border = match widths.len() {
                1 => Edges::uniform(widths[0]),
                2 => Edges { top: widths[0], right: widths[1], bottom: widths[0], left: widths[1] },
                3 => Edges { top: widths[0], right: widths[1], bottom: widths[2], left: widths[1] },
                4 => Edges { top: widths[0], right: widths[1], bottom: widths[2], left: widths[3] },
                _ => style.border,
            };
        }
        "border-color" => {
            if let Some(c) = Color::parse(value) {
                style.border_color = c;
            }
        }
        "flex-direction" => {
            style.flex_direction = match value.to_ascii_lowercase().as_str() {
                "column" | "column-reverse" => FlexDirection::Column,
                _ => FlexDirection::Row,
            };
        }
        "justify-content" => {
            style.justify_content = match value.to_ascii_lowercase().as_str() {
                "center" => JustifyContent::Center,
                "flex-end" | "end" | "right" => JustifyContent::End,
                "space-between" => JustifyContent::SpaceBetween,
                "space-around" | "space-evenly" => JustifyContent::SpaceAround,
                _ => JustifyContent::Start,
            };
        }
        "align-items" => {
            style.align_items = match value.to_ascii_lowercase().as_str() {
                "center" => AlignItems::Center,
                "flex-end" | "end" => AlignItems::End,
                "flex-start" | "start" => AlignItems::Start,
                _ => AlignItems::Stretch,
            };
        }
        "flex-wrap" => {
            style.flex_wrap = match value.to_ascii_lowercase().as_str() {
                "wrap" | "wrap-reverse" => FlexWrap::Wrap,
                _ => FlexWrap::NoWrap,
            };
        }
        "gap" | "column-gap" | "row-gap" => {
            if let Some(v) = parse_length(value, ctx) {
                style.gap = v;
            }
        }
        "font" => {
            // Minimal `font` shorthand: `16px/1.6 system-ui` or `700 24px Arial`.
            for token in value.split_whitespace() {
                if let Some((size, lh)) = token.split_once('/') {
                    if let Some(px) = parse_length(size, ctx) {
                        style.font_size = px.max(1.0);
                    }
                    if let Ok(n) = lh.parse::<f32>() {
                        style.line_height = n.max(0.5);
                    }
                } else if token.eq_ignore_ascii_case("bold") {
                    style.font_weight = 700;
                } else if let Some(weight) = shorthand_weight(token) {
                    // A bare `700` in the shorthand is a weight, not a size:
                    // `font: 700 24px Arial` would otherwise measure 700px.
                    style.font_weight = weight;
                } else if let Some(px) = parse_length(token, ctx) {
                    if px >= 8.0 {
                        style.font_size = px;
                    }
                }
            }
            // The rest of the shorthand — everything after the size, the
            // optional `/line-height`, the weight and the style — is the
            // family list. `16px/1.4 Arial, sans-serif` must yield
            // `font_family == "Arial, sans-serif"`.
            let rest = font_shorthand_families(value);
            if !rest.is_empty() {
                style.font_family = rest;
            }
            let lower = value.to_ascii_lowercase();
            let words: Vec<&str> = lower.split_whitespace().collect();
            if words.iter().any(|w| *w == "italic" || *w == "oblique") {
                style.italic = true;
            } else if words.iter().any(|w| *w == "normal") {
                // `font: normal 16px Arial` resets the slant.
                style.italic = false;
            }
        }
        "text-align" => {
            style.text_align = match value.to_ascii_lowercase().as_str() {
                "center" => TextAlign::Center,
                "right" => TextAlign::Right,
                _ => TextAlign::Left,
            };
        }
        "line-height" => {
            let lower = value.to_ascii_lowercase();
            if lower == "normal" {
                style.line_height = 1.35;
            } else if let Ok(n) = lower.parse::<f32>() {
                // A unitless number is a multiplier, not a length.
                style.line_height = n.max(0.5);
            } else if let Some(dim) = parse_length_value(&lower, ctx) {
                // `px`, `em`, `rem` and `%` all resolve to a used length that
                // is then expressed as a multiple of the font size.
                let px = match &dim {
                    Dim::Linear { .. } => Some(dim.resolve(ctx.em_base)),
                    _ => dim.absolute(),
                };
                if let Some(px) = px {
                    style.line_height = (px / style.font_size.max(1.0)).max(0.5);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naro_html::Document;

    fn style_of(html: &str, css: &str, tag: &str) -> ComputedStyle {
        let doc = Document::parse(html);
        let author = parse_stylesheet(css);
        let styles = compute_styles(&doc, &author);
        let id = doc.find_tag(tag).unwrap();
        styles[id].clone()
    }

    /// A host with a shadow tree, styled with both sheets.
    ///
    /// Returns the document (so nodes can be found), the computed styles, and
    /// the host id. `page` is the document sheet, `component` the shadow one.
    fn shadow_styles(
        html: &str,
        page: &str,
        component: &str,
    ) -> (Document, Vec<ComputedStyle>, NodeId) {
        let mut doc = Document::parse(html);
        let host = doc.find_tag("div").expect("host");
        let root = doc.attach_shadow(host, naro_html::ShadowMode::Open);
        let style = doc.push_detached_element("style".into(), root);
        doc.attach(root, style);
        doc.push_text_node(style, component);
        let author = parse_stylesheet(page);
        let mut shadow = std::collections::HashMap::new();
        shadow.insert(
            root,
            parse_stylesheet_for(&doc.collect_style_text_from(root), 1000.0),
        );
        let styles = compute_styles_scoped(&doc, &author, &shadow, StyleContext::default());
        (doc, styles, host)
    }

    fn styles_for(html: &str, css: &str) -> (Document, Vec<ComputedStyle>) {
        let doc = Document::parse(html);
        let styles = compute_styles(&doc, &parse_stylesheet(css));
        (doc, styles)
    }

    /// Style with an explicit viewport, so `vw`/`vh`/`vmin`/`vmax` are
    /// deterministic in tests.
    fn styles_ctx(html: &str, css: &str, context: StyleContext) -> (Document, Vec<ComputedStyle>) {
        let doc = Document::parse(html);
        let styles = compute_styles_with(&doc, &parse_stylesheet(css), context);
        (doc, styles)
    }

    fn ctx_1000x700() -> StyleContext {
        StyleContext { viewport_width: 1000.0, viewport_height: 700.0, root_font_size: 16.0 }
    }

    /// Width of `#id` resolved against `base`.
    fn width_px(html: &str, css: &str, id: &str, context: StyleContext, base: f32) -> Option<f32> {
        let (doc, styles) = styles_ctx(html, css, context);
        styles[node_by_id(&doc, id)].width.as_ref().map(|w| w.resolve(base))
    }

    fn width_of(html: &str, css: &str, id: &str, context: StyleContext) -> Option<f32> {
        width_px(html, css, id, context, 1000.0)
    }

    fn node_by_id(doc: &Document, id: &str) -> NodeId {
        (0..doc.nodes.len())
            .find(|&n| doc.attr(n, "id") == Some(id))
            .expect("id no DOM")
    }

    fn color_at(html: &str, css: &str, id: &str) -> Color {
        let (doc, styles) = styles_for(html, css);
        styles[node_by_id(&doc, id)].color
    }

    #[test]
    fn hover_never_applies_statically() {
        let (doc, styles) = styles_for(
            "<p><a id=\"l\" href=\"#x\">link</a></p>",
            "a { color: #000000 } a:hover { color: #ff0000 }",
        );
        assert_eq!(styles[node_by_id(&doc, "l")].color, Color::rgb(0, 0, 0));

        // High specificity does not make interaction state win either.
        assert_eq!(
            color_at(
                "<a id=\"l\" href=\"#x\">l</a>",
                "a { color: #000000 } #l:hover { color: #ff0000 }",
                "l"
            ),
            Color::rgb(0, 0, 0)
        );
        assert_eq!(
            color_at(
                "<a id=\"l\" href=\"#x\">l</a>",
                "a { color: #000000 } a:focus { color: #ff0000 } a:active { color: #00ff00 }",
                "l"
            ),
            Color::rgb(0, 0, 0)
        );
    }

    #[test]
    fn attribute_presence_and_value() {
        let (doc, styles) = styles_for(
            "<div id=\"h\" hidden>x</div>\
             <input id=\"t\" type=\"text\">\
             <input id=\"s\" type=\"submit\">",
            "[hidden] { display: none }\
             input[type=text] { color: #123456 }",
        );
        assert_eq!(styles[node_by_id(&doc, "h")].display, Display::None);
        assert_eq!(styles[node_by_id(&doc, "t")].color, Color::rgb(0x12, 0x34, 0x56));
        assert_ne!(styles[node_by_id(&doc, "s")].color, Color::rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn attribute_operators() {
        let html = "<p id=\"a\" class=\"x b y\" href=\"https://example.org\" lang=\"pt-BR\" data-x=\"a b\">t</p>";
        let css = "[class~=b] { color: #000001 }";
        assert_eq!(color_at(html, css, "a"), Color::rgb(0, 0, 1));
        assert_eq!(
            color_at(html, "[href^=https] { color: #000002 }", "a"),
            Color::rgb(0, 0, 2)
        );
        assert_eq!(
            color_at(html, "[href$=.org] { color: #000003 }", "a"),
            Color::rgb(0, 0, 3)
        );
        assert_eq!(
            color_at(html, "[href*=ex] { color: #000004 }", "a"),
            Color::rgb(0, 0, 4)
        );
        assert_eq!(
            color_at(html, "[lang|=pt] { color: #000005 }", "a"),
            Color::rgb(0, 0, 5)
        );
        assert_eq!(
            color_at(html, "[data-x=\"a b\"] { color: #000006 }", "a"),
            Color::rgb(0, 0, 6)
        );
        // Non-matching operators leave the inherited colour untouched.
        assert_ne!(
            color_at(html, "[class~=z] { color: #000007 }", "a"),
            Color::rgb(0, 0, 7)
        );
        assert_ne!(
            color_at(html, "[href^=ftp] { color: #000008 }", "a"),
            Color::rgb(0, 0, 8)
        );
        assert_ne!(
            color_at(html, "[lang|=en] { color: #000009 }", "a"),
            Color::rgb(0, 0, 9)
        );
        assert_ne!(
            color_at(html, "[data-x=a] { color: #00000a }", "a"),
            Color::rgb(0, 0, 10)
        );
    }

    #[test]
    fn child_combinator_is_direct() {
        let html = "<div class=\"a\"><p class=\"b\" id=\"direct\">x</p>\
                    <section><p class=\"b\" id=\"nested\">y</p></section></div>";
        let css = ".a > .b { color: #000001 }";
        assert_eq!(color_at(html, css, "direct"), Color::rgb(0, 0, 1));
        assert_ne!(color_at(html, css, "nested"), Color::rgb(0, 0, 1));
    }

    #[test]
    fn adjacent_and_general_siblings() {
        // `far` is separated from the first `.a` by an `<i>`; `near` follows a
        // `.a` directly.
        let html = "<div><p class=\"a\">a</p><i>i</i><p class=\"b\" id=\"far\">b</p>\
                    <p class=\"a\">a2</p><p class=\"b\" id=\"near\">b</p></div>";
        // `+` needs the immediately preceding element sibling.
        assert_ne!(color_at(html, ".a + .b { color: #000001 }", "far"), Color::rgb(0, 0, 1));
        assert_eq!(
            color_at(html, ".a + .b { color: #000002 }", "near"),
            Color::rgb(0, 0, 2)
        );
        // `~` accepts any preceding sibling.
        assert_eq!(color_at(html, ".a ~ .b { color: #000003 }", "far"), Color::rgb(0, 0, 3));
    }

    #[test]
    fn mixed_combinators_and_whitespace() {
        let spaced = parse_selector_list_text("div .a > p + span");
        let tight = parse_selector_list_text(".a>b");
        assert_eq!(spaced.len(), 1);
        assert_eq!(
            spaced[0].combinators,
            vec![Combinator::Descendant, Combinator::Child, Combinator::Adjacent]
        );
        assert_eq!(spaced[0].compounds.len(), 4);
        assert_eq!(tight[0].combinators, vec![Combinator::Child]);
        let padded = parse_selector_list_text(".a   >   b");
        assert_eq!(padded[0].combinators, vec![Combinator::Child]);
        assert_eq!(padded[0].compounds.len(), 2);
        // Trailing spaces produce no extra combinator.
        let trailing = parse_selector_list_text("  .a > b  ");
        assert_eq!(trailing[0].compounds.len(), 2);
    }

    #[test]
    fn structural_pseudos_first_last() {
        let html = "<ul><li id=\"one\">1</li><li id=\"two\">2</li><li id=\"three\">3</li></ul>";
        assert_eq!(
            color_at(html, "li:first-child { color: #000001 }", "one"),
            Color::rgb(0, 0, 1)
        );
        assert_ne!(
            color_at(html, "li:first-child { color: #000001 }", "two"),
            Color::rgb(0, 0, 1)
        );
        assert_eq!(
            color_at(html, "li:last-child { color: #000002 }", "three"),
            Color::rgb(0, 0, 2)
        );
        assert_ne!(
            color_at(html, "li:last-child { color: #000002 }", "one"),
            Color::rgb(0, 0, 2)
        );
    }

    #[test]
    fn nth_child_positive_cases() {
        let html = "<ul>\
            <li id=\"i1\">1</li><li id=\"i2\">2</li><li id=\"i3\">3</li>\
            <li id=\"i4\">4</li><li id=\"i5\">5</li><li id=\"i6\">6</li>\
            <li id=\"i7\">7</li><li id=\"i8\">8</li></ul>";
        let odd = "li:nth-child(2n+1) { color: #000001 }";
        for id in ["i1", "i3", "i5"] {
            assert_eq!(color_at(html, odd, id), Color::rgb(0, 0, 1), "{id} is odd");
        }
        for id in ["i2", "i4"] {
            assert_ne!(color_at(html, odd, id), Color::rgb(0, 0, 1), "{id} is even");
        }
        let even = "li:nth-child(2n) { color: #000002 }";
        for id in ["i2", "i4"] {
            assert_eq!(color_at(html, even, id), Color::rgb(0, 0, 2), "{id} is even");
        }
        assert_ne!(color_at(html, even, "i1"), Color::rgb(0, 0, 2));
        // `odd`/`even` keywords.
        assert_eq!(
            color_at(html, "li:nth-child(odd) { color: #000003 }", "i1"),
            Color::rgb(0, 0, 3)
        );
        assert_eq!(
            color_at(html, "li:nth-child(even) { color: #000004 }", "i2"),
            Color::rgb(0, 0, 4)
        );
        // `0n+3` matches only the third child.
        let only_third = "li:nth-child(0n+3) { color: #000005 }";
        assert_eq!(color_at(html, only_third, "i3"), Color::rgb(0, 0, 5));
        assert_ne!(color_at(html, only_third, "i1"), Color::rgb(0, 0, 5));
        assert_ne!(color_at(html, only_third, "i6"), Color::rgb(0, 0, 5));
        // `n+2` matches 2, 3, 4…
        let from_two = "li:nth-child(n+2) { color: #000006 }";
        assert_ne!(color_at(html, from_two, "i1"), Color::rgb(0, 0, 6));
        for id in ["i2", "i3", "i4"] {
            assert_eq!(color_at(html, from_two, id), Color::rgb(0, 0, 6), "{id} >= 2");
        }
        // `3n-1` matches 2, 5, 8…
        let stepped = "li:nth-child(3n-1) { color: #000007 }";
        for id in ["i2", "i5", "i8"] {
            assert_eq!(color_at(html, stepped, id), Color::rgb(0, 0, 7), "{id} = 3n-1");
        }
        for id in ["i1", "i3", "i4"] {
            assert_ne!(color_at(html, stepped, id), Color::rgb(0, 0, 7), "{id} != 3n-1");
        }
    }

    #[test]
    fn nth_child_negative_cases() {
        let html = "<ul>\
            <li id=\"i1\">1</li><li id=\"i2\">2</li><li id=\"i3\">3</li>\
            <li id=\"i4\">4</li><li id=\"i5\">5</li></ul>";
        // `-n+3` matches 1, 2, 3 and stops.
        let first_three = "li:nth-child(-n+3) { color: #000001 }";
        for id in ["i1", "i2", "i3"] {
            assert_eq!(color_at(html, first_three, id), Color::rgb(0, 0, 1), "{id} <= 3");
        }
        assert_ne!(color_at(html, first_three, "i4"), Color::rgb(0, 0, 1));
        // `2n` never matches index 1.
        assert_ne!(
            color_at(html, "li:nth-child(2n) { color: #000002 }", "i1"),
            Color::rgb(0, 0, 2)
        );
        // `0n+0` matches nothing.
        let none = "li:nth-child(0n+0) { color: #000003 }";
        for id in ["i1", "i2", "i3", "i4", "i5"] {
            assert_ne!(color_at(html, none, id), Color::rgb(0, 0, 3), "{id} never matches");
        }
        // `-2n+4` matches 2 and 4 only.
        let evens_to_four = "li:nth-child(-2n+4) { color: #000004 }";
        for id in ["i2", "i4"] {
            assert_eq!(
                color_at(html, evens_to_four, id),
                Color::rgb(0, 0, 4),
                "{id} = -2n+4"
            );
        }
        for id in ["i1", "i5"] {
            assert_ne!(
                color_at(html, evens_to_four, id),
                Color::rgb(0, 0, 4),
                "{id} != -2n+4"
            );
        }
    }

    #[test]
    fn not_pseudo() {
        let html = "<div><p class=\"x\" id=\"a\">a</p><p class=\"y\" id=\"b\">b</p>\
                    <input id=\"c\" disabled><input id=\"d\"></div>";
        let exclude_class = "p:not(.x) { color: #000001 }";
        assert_ne!(color_at(html, exclude_class, "a"), Color::rgb(0, 0, 1));
        assert_eq!(color_at(html, exclude_class, "b"), Color::rgb(0, 0, 1));
        let exclude_attr = "input:not([disabled]) { color: #000002 }";
        assert_ne!(color_at(html, exclude_attr, "c"), Color::rgb(0, 0, 2));
        assert_eq!(color_at(html, exclude_attr, "d"), Color::rgb(0, 0, 2));
    }

    #[test]
    fn important_beats_id_specificity() {
        let html = "<p id=\"i\" class=\"c\">x</p>";
        assert_eq!(
            color_at(html, "#i { color: #ff0000 } .c { color: #00ff00 !important }", "i"),
            Color::rgb(0, 255, 0)
        );
    }

    #[test]
    fn inline_normal_loses_to_author_important() {
        let html = "<p id=\"i\" class=\"c\" style=\"color: #0000ff\">x</p>";
        assert_eq!(
            color_at(html, ".c { color: #00ff00 !important }", "i"),
            Color::rgb(0, 255, 0)
        );
    }

    #[test]
    fn inline_important_beats_author_important() {
        let html = "<p id=\"i\" class=\"c\" style=\"color: #0000ff !important\">x</p>";
        assert_eq!(
            color_at(html, ".c { color: #00ff00 !important }", "i"),
            Color::rgb(0, 0, 255)
        );
    }

    #[test]
    fn inline_normal_beats_author_normal() {
        let html = "<p id=\"i\" style=\"color: #0000ff\">x</p>";
        assert_eq!(color_at(html, "#i { color: #ff0000 }", "i"), Color::rgb(0, 0, 255));
    }

    #[test]
    fn specificity_counts_attributes_and_pseudos() {
        // An attribute selector weighs the same as a class and outranks a tag.
        let html = "<p id=\"p\" data-x=\"1\">x</p>";
        assert_eq!(
            color_at(html, "p { color: #ff0000 } [data-x] { color: #00ff00 }", "p"),
            Color::rgb(0, 255, 0)
        );
        // A pseudo-class weighs the same as a class.
        let html2 = "<div><p id=\"p\">x</p></div>";
        assert_eq!(
            color_at(html2, "p { color: #ff0000 } p:first-child { color: #0000ff }", "p"),
            Color::rgb(0, 0, 255)
        );
        // Attribute and class tie: the later rule wins.
        assert_eq!(
            color_at(
                "<p id=\"p\" class=\"k\" data-x=\"1\">x</p>",
                ".k { color: #ff0000 } [data-x] { color: #00ff00 }",
                "p"
            ),
            Color::rgb(0, 255, 0)
        );
    }

    #[test]
    fn pseudo_element_rules_do_not_leak() {
        let sheet = parse_stylesheet("p::before { color: #ff0000 }");
        let doc = Document::parse("<p id=\"p\">x</p>");
        let styles = compute_styles(&doc, &sheet);
        assert_ne!(styles[node_by_id(&doc, "p")].color, Color::rgb(255, 0, 0));
        assert_eq!(sheet.report.count(SkipReason::PseudoElement), 1);
        assert!(sheet.rules.is_empty());
    }

    #[test]
    fn comma_list_with_not() {
        let html = "<div><p class=\"a\" id=\"a\">a</p><p class=\"b\" id=\"b\">b</p></div>";
        let css = "p:not(.a), .b { color: #123456 }";
        assert_ne!(color_at(html, css, "a"), Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(color_at(html, css, "b"), Color::rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn query_selector_matches_css() {
        let html = "<div class=\"a\"><p class=\"b\" id=\"hit\">x</p></div>\
                    <p data-state=\"on\" id=\"state\">y</p>";
        let doc = Document::parse(html);
        let css = ".a > .b { color: #123456 } [data-state=on] { color: #654321 }";
        let styles = compute_styles(&doc, &parse_stylesheet(css));
        let child = query_selector(&doc, ".a > .b").expect("child match");
        assert_eq!(child, node_by_id(&doc, "hit"));
        assert_eq!(styles[child].color, Color::rgb(0x12, 0x34, 0x56));
        let state = query_selector(&doc, "[data-state=on]").expect("attribute match");
        assert_eq!(state, node_by_id(&doc, "state"));
        assert_eq!(styles[state].color, Color::rgb(0x65, 0x43, 0x21));
        assert_eq!(query_selector(&doc, ".a > .b p"), None);
        assert_eq!(query_selector(&doc, ""), None);
        assert_eq!(query_selector(&doc, "[unclosed"), None);
    }

    #[test]
    fn query_selector_all_dedupes_and_orders() {
        let html = "<div><p class=\"b\" id=\"one\">1</p><span id=\"two\">2</span>\
                    <p class=\"b\" id=\"three\">3</p></div>";
        let doc = Document::parse(html);
        let all = query_selector_all(&doc, "p, .b");
        let ids: Vec<&str> = all.iter().map(|&n| doc.attr(n, "id").unwrap_or("")).collect();
        assert_eq!(ids, vec!["one", "three"], "document order, no duplicates");
        let both = query_selector_all(&doc, "p, .b, #one");
        assert_eq!(both.iter().filter(|&&n| doc.attr(n, "id") == Some("one")).count(), 1);
        assert!(query_selector_all(&doc, "").is_empty());
        assert!(query_selector_all(&doc, "[unclosed").is_empty());
    }

    #[test]
    fn absolute_units_resolve_against_viewport_and_root() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        assert_eq!(width_of(html, "#x { width: 10rem }", "x", ctx), Some(160.0));
        assert_eq!(width_of(html, "#x { width: 50vw }", "x", ctx), Some(500.0));
        assert_eq!(width_of(html, "#x { width: 50vh }", "x", ctx), Some(350.0));
        // vmin/vmax use the smaller/larger viewport axis.
        assert_eq!(width_of(html, "#x { width: 10vmin }", "x", ctx), Some(70.0));
        assert_eq!(width_of(html, "#x { width: 10vmax }", "x", ctx), Some(100.0));
        // A different viewport proves vw is not a fixed pixel value.
        let wide = StyleContext { viewport_width: 1200.0, viewport_height: 800.0, root_font_size: 16.0 };
        assert_eq!(width_of(html, "#x { width: 10vw }", "x", wide), Some(120.0));
        assert_eq!(width_of(html, "#x { width: 10vmin }", "x", wide), Some(80.0));
    }

    #[test]
    fn rem_follows_root_font_size() {
        let html = "<html><body><div id=\"x\"></div></body></html>";
        let ctx = ctx_1000x700();
        // `html { font-size: 20px }` moves the rem base for descendants.
        assert_eq!(
            width_of(html, "html { font-size: 20px } #x { width: 10rem }", "x", ctx),
            Some(200.0)
        );
        // `rem` in the root's own font-size uses the initial 16px.
        let (doc, styles) = styles_ctx(html, "html { font-size: 2rem }", ctx);
        let root = doc.find_tag("html").unwrap();
        assert_eq!(styles[root].font_size, 32.0);
        // Without an author rule the root stays at the initial size.
        assert_eq!(width_of(html, "#x { width: 1rem }", "x", ctx), Some(16.0));
    }

    #[test]
    fn em_in_font_size_uses_parent() {
        let html = "<div style=\"font-size: 20px\"><p id=\"p\"></p></div>";
        let ctx = ctx_1000x700();
        let (doc, styles) = styles_ctx(html, "#p { font-size: 1.5em }", ctx);
        assert_eq!(styles[node_by_id(&doc, "p")].font_size, 30.0);
        // `%` in font-size is also relative to the parent.
        let (doc, styles) = styles_ctx(html, "#p { font-size: 150% }", ctx);
        assert_eq!(styles[node_by_id(&doc, "p")].font_size, 30.0);
    }

    #[test]
    fn em_in_lengths_uses_own_font_size() {
        let html = "<div id=\"p\"></div>";
        let ctx = ctx_1000x700();
        // `padding` is declared before `font-size` and must still use 20px.
        let (doc, styles) = styles_ctx(
            html,
            "#p { padding: 2em; font-size: 20px; width: 3em }",
            ctx,
        );
        let style = &styles[node_by_id(&doc, "p")];
        assert_eq!(style.padding.top.resolve(1000.0), 40.0);
        assert_eq!(style.padding.left.resolve(1000.0), 40.0);
        assert_eq!(style.width.as_ref().map(|w| w.resolve(1000.0)), Some(60.0));
    }

    #[test]
    fn calc_addition_and_subtraction() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        assert_eq!(
            width_px(html, "#x { width: calc(100% - 32px) }", "x", ctx, 1000.0),
            Some(968.0)
        );
        assert_eq!(
            width_px(html, "#x { width: calc(100% - 32px) }", "x", ctx, 500.0),
            Some(468.0)
        );
        assert_eq!(width_of(html, "#x { width: calc(10px + 5px) }", "x", ctx), Some(15.0));
        assert_eq!(width_of(html, "#x { width: calc(50vw - 10px) }", "x", ctx), Some(490.0));
        // `em` inside `calc()` uses the element's font size.
        let (doc, styles) = styles_ctx(html, "#x { padding: calc(1em + 4px) }", ctx);
        assert_eq!(styles[node_by_id(&doc, "x")].padding.top.resolve(1000.0), 20.0);
    }

    #[test]
    fn calc_multiplication_and_division() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        assert_eq!(width_of(html, "#x { width: calc(2 * 10px) }", "x", ctx), Some(20.0));
        assert_eq!(width_of(html, "#x { width: calc(100px / 4) }", "x", ctx), Some(25.0));
        assert_eq!(width_of(html, "#x { width: calc(10px * 2 / 4) }", "x", ctx), Some(5.0));
        assert_eq!(
            width_px(html, "#x { width: calc(100% / 4) }", "x", ctx, 1000.0),
            Some(250.0)
        );
    }

    #[test]
    fn calc_precedence_and_parentheses() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        assert_eq!(width_of(html, "#x { width: calc(2px + 3px * 4) }", "x", ctx), Some(14.0));
        assert_eq!(width_of(html, "#x { width: calc((2px + 3px) * 4) }", "x", ctx), Some(20.0));
        assert_eq!(
            width_px(html, "#x { width: calc(100% - (10px + 20px)) }", "x", ctx, 1000.0),
            Some(970.0)
        );
        assert_eq!(
            width_of(html, "#x { width: calc((10px - 4px) * 2 + 1px) }", "x", ctx),
            Some(13.0)
        );
    }

    #[test]
    fn min_max_clamp_functions() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        // Mixed percentage/absolute: the winner depends on the base.
        assert_eq!(
            width_px(html, "#x { width: min(800px, 90%) }", "x", ctx, 1000.0),
            Some(800.0)
        );
        assert_eq!(
            width_px(html, "#x { width: min(800px, 90%) }", "x", ctx, 500.0),
            Some(450.0)
        );
        assert_eq!(
            width_px(html, "#x { width: max(320px, 50%) }", "x", ctx, 400.0),
            Some(320.0)
        );
        assert_eq!(
            width_px(html, "#x { width: max(320px, 50%) }", "x", ctx, 1000.0),
            Some(500.0)
        );
        assert_eq!(
            width_of(html, "#x { width: clamp(320px, 80vw, 1200px) }", "x", ctx),
            Some(800.0)
        );
        assert_eq!(
            width_of(html, "#x { width: clamp(320px, 10vw, 1200px) }", "x", ctx),
            Some(320.0)
        );
        // clamp() in font-size folds to a single pixel value.
        let (doc, styles) = styles_ctx(html, "#x { font-size: clamp(14px, 1.2rem, 20px) }", ctx);
        assert!((styles[node_by_id(&doc, "x")].font_size - 19.2).abs() < 0.01);
    }

    #[test]
    fn percentages_stay_symbolic_until_layout() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        let (doc, styles) = styles_ctx(html, "#x { width: 50% }", ctx);
        let width = styles[node_by_id(&doc, "x")].width.clone().unwrap();
        assert_eq!(width.absolute(), None, "a percentage has no pixel value yet");
        assert_eq!(width.resolve(400.0), 200.0);
        assert_eq!(width.resolve(800.0), 400.0);
    }

    #[test]
    fn invalid_lengths_do_not_break_the_sheet() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        let (doc, styles) = styles_ctx(
            html,
            "#x { width: 10furlong; height: 10px }\
             #x { width: calc(10px * 10px) }\
             #x { width: calc(10px + 5) }\
             #x { width: min() }\
             #x { width: calc(10px / 0) }\
             #x { width: 10 }",
            ctx,
        );
        let style = &styles[node_by_id(&doc, "x")];
        assert!(style.width.is_none(), "every invalid width was rejected");
        assert_eq!(style.height.as_ref().map(|h| h.resolve(0.0)), Some(10.0));
        // A percentage in padding is kept and resolved against the
        // containing-block width, not silently dropped.
        let (doc, styles) = styles_ctx(html, "#x { padding: 10px 5% }", ctx);
        let padding = &styles[node_by_id(&doc, "x")].padding;
        assert_eq!(padding.top.resolve(1000.0), 10.0);
        assert_eq!(padding.right.resolve(1000.0), 50.0);
        assert_eq!(padding.right.resolve(400.0), 20.0);
        // A genuinely unparseable component still drops the shorthand.
        let (doc, styles) = styles_ctx(html, "#x { padding: 10px furlong }", ctx);
        assert_eq!(styles[node_by_id(&doc, "x")].padding.top.resolve(1000.0), 0.0);
    }

    #[test]
    fn line_height_accepts_units_and_numbers() {
        let html = "<div id=\"x\">x</div>";
        let ctx = ctx_1000x700();
        let (doc, styles) = styles_ctx(html, "#x { font-size: 20px; line-height: 1.5 }", ctx);
        assert!((styles[node_by_id(&doc, "x")].line_height - 1.5).abs() < 0.001);
        // 30px on a 20px font is 1.5, whatever unit produced it.
        for css in ["#x { font-size: 20px; line-height: 30px }",
                    "#x { font-size: 20px; line-height: 1.5em }",
                    "#x { font-size: 20px; line-height: 150% }"] {
            let (doc, styles) = styles_ctx(html, css, ctx);
            assert!((styles[node_by_id(&doc, "x")].line_height - 1.5).abs() < 0.001, "{css}");
        }
    }

    #[test]
    fn px_and_percent_keep_working() {
        let html = "<div id=\"x\"></div>";
        let ctx = ctx_1000x700();
        assert_eq!(width_of(html, "#x { width: 100px }", "x", ctx), Some(100.0));
        assert_eq!(width_px(html, "#x { width: 25% }", "x", ctx, 800.0), Some(200.0));
        let (doc, styles) = styles_ctx(html, "#x { padding: 4px 8px; margin: 2px }", ctx);
        let style = &styles[node_by_id(&doc, "x")];
        assert_eq!(
            style.padding.resolve(1000.0),
            Edges { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 }
        );
        assert_eq!(style.margin.resolve(1000.0), Edges::uniform(2.0));
    }

    #[test]
    fn grid_percentage_tracks_keep_their_percent() {
        let html = "<div id=\"g\"></div>";
        let ctx = ctx_1000x700();
        let (doc, styles) = styles_ctx(html, "#g { grid-template-columns: 25% 75% }", ctx);
        let tracks = &styles[node_by_id(&doc, "g")].grid_columns;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].percent, 25.0);
        assert_eq!(tracks[0].px, 0.0);
        assert_eq!(tracks[1].percent, 75.0);
    }

    #[test]
    fn geometry_differs_only_for_layout_properties() {
        let (doc, styles) = styles_for(
            "<p id=\"a\">x</p>",
            "p { color: #000000; background-color: #ffffff; border-radius: 4px }",
        );
        let node = node_by_id(&doc, "a");
        let mut changed = styles[node].clone();
        changed.color = Color::rgb(255, 0, 0);
        changed.background = Color::rgb(0, 0, 0);
        changed.border_radius = 0.0;
        assert!(
            !changed.geometry_differs(&styles[node]),
            "colour and radius are paint-only"
        );
        let mut wider = styles[node].clone();
        wider.width = Some(Dim::px(10.0));
        assert!(wider.geometry_differs(&styles[node]));
        let mut bigger = styles[node].clone();
        bigger.font_size = 20.0;
        assert!(bigger.geometry_differs(&styles[node]));
        let mut padded = styles[node].clone();
        padded.padding.left = Dim::px(4.0);
        assert!(padded.geometry_differs(&styles[node]));
        let mut hidden = styles[node].clone();
        hidden.display = Display::None;
        assert!(hidden.geometry_differs(&styles[node]));
    }

    #[test]
    fn geometry_changed_detects_a_different_node_count() {
        let (doc, styles) = styles_for("<p id=\"a\">x</p>", "p { color: #000000 }");
        assert!(!geometry_changed(&styles, &styles.clone()));
        let mut shorter = styles.clone();
        shorter.pop();
        assert!(geometry_changed(&styles, &shorter), "a node disappeared");
        let mut longer = styles.clone();
        longer.push(styles[0].clone());
        assert!(geometry_changed(&styles, &longer), "a node appeared");
        let _ = doc;
    }

    #[test]
    fn custom_properties_keep_their_case() {
        // `--xhUGwc` and `--xhugwc` are different variables; lowercasing the
        // name made every camelCase variable unresolvable.
        let style = style_of(
            "<div class=\"a\">x</div>",
            ":root{--xhUGwc:#fff} .a{background:var(--xhUGwc)}",
            "div",
        );
        assert_eq!(style.background, Color::rgb(255, 255, 255));
        // A lowercase reference does not resolve the camelCase definition.
        let style = style_of(
            "<div class=\"a\">x</div>",
            ":root{--xhUGwc:#fff} .a{background:var(--xhugwc)}",
            "div",
        );
        assert_ne!(style.background, Color::rgb(255, 255, 255));
    }

    #[test]
    fn justify_items_centers_a_narrow_grid_item() {
        let style = style_of(
            "<div class=\"g\">x</div>",
            ".g{display:grid; justify-items:center; align-items:center}",
            "div",
        );
        assert_eq!(style.justify_items, AlignItems::Center);
        assert_eq!(style.align_items, AlignItems::Center);
        // `place-items` sets both axes.
        let style = style_of("<div class=\"g\">x</div>", ".g{place-items:center end}", "div");
        assert_eq!(style.align_items, AlignItems::Center);
        assert_eq!(style.justify_items, AlignItems::End);
    }

    #[test]
    fn parse_report_classifies_skips() {
        let sheet = parse_stylesheet(
            "p::before { color: #ff0000 }\
             [x= { color: #00ff00 }\
             p:not(.a .b) { color: #0000ff }\
             a:hover { color: #123456 }\
             p { color: #654321 }",
        );
        assert_eq!(sheet.report.count(SkipReason::PseudoElement), 1);
        assert_eq!(sheet.report.count(SkipReason::InvalidSyntax), 1);
        assert_eq!(sheet.report.count(SkipReason::UnsupportedNot), 1);
        assert_eq!(sheet.report.interactive, 1);
        // Two valid parts entered the cascade: `a:hover` and `p`.
        assert_eq!(sheet.report.rules, 2);
        assert_eq!(sheet.rules.len(), 2);
    }

    #[test]
    fn id_beats_class_beats_tag() {
        let html = "<div id=\"a\" class=\"c\">x</div>";
        let css = "div { color: red } .c { color: green } #a { color: blue }";
        assert_eq!(style_of(html, css, "div").color, Color::rgb(0, 0, 255));

        let css2 = "div { color: red } .c { color: green }";
        assert_eq!(style_of(html, css2, "div").color, Color::rgb(0, 128, 0));

        let css3 = "div { color: red }";
        assert_eq!(style_of(html, css3, "div").color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn inline_style_wins_over_author_rules() {
        let html = "<div id=\"a\" style=\"color: #123456\">x</div>";
        let css = "#a { color: red }";
        assert_eq!(style_of(html, css, "div").color, Color::rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn inherits_color_and_font_size() {
        let doc = Document::parse("<div style=\"color: red; font-size: 20px\"><span>x</span></div>");
        let styles = compute_styles(&doc, &Stylesheet::default());
        let span = doc.find_tag("span").unwrap();
        assert_eq!(styles[span].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[span].font_size, 20.0);
    }

    #[test]
    fn margin_shorthand_expands() {
        let style = style_of("<div>x</div>", "div { margin: 1px 2px 3px 4px }", "div");
        assert_eq!(
            style.margin.resolve(1000.0),
            Edges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 }
        );
        let style = style_of("<p>x</p>", "p { margin: 10px 0 }", "p");
        assert_eq!(
            style.margin.resolve(1000.0),
            Edges { top: 10.0, right: 0.0, bottom: 10.0, left: 0.0 }
        );
    }

    #[test]
    fn media_queries_gate_rules_by_viewport() {
        let css = "@media screen and (min-width: 600px) { .a { color: red } }\
                   @media (max-width: 400px) { .a { color: blue } }\
                   @media print { .a { color: green } }\
                   .a { background-color: #ffffff }";
        let html = "<div class=\"a\">x</div>";

        let doc = naro_html::Document::parse(html);
        let wide = compute_styles(&doc, &parse_stylesheet_for(css, 800.0));
        let id = doc.find_tag("div").unwrap();
        assert_eq!(wide[id].color, Color::rgb(255, 0, 0), "min-width matched");
        assert_eq!(wide[id].background, Color::rgb(255, 255, 255), "outside @media kept");

        let narrow = compute_styles(&doc, &parse_stylesheet_for(css, 300.0));
        assert_eq!(narrow[id].color, Color::rgb(0, 0, 255), "max-width matched");

        let print = compute_styles(&doc, &parse_stylesheet_for(css, 1200.0));
        assert_ne!(print[id].color, Color::rgb(0, 128, 0), "print never applies");
    }

    #[test]
    fn unsupported_media_features_do_not_leak() {
        let css = "@media (prefers-color-scheme: dark) { .a { color: red } }";
        let doc = naro_html::Document::parse("<div class=\"a\">x</div>");
        let styles = compute_styles(&doc, &parse_stylesheet_for(css, 800.0));
        assert_ne!(styles[doc.find_tag("div").unwrap()].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn custom_properties_are_substituted() {
        let html = "<div class=\"a\">x</div>";
        let css = ":root { --brand: #1a56db; --gap: 12px } .a { color: var(--brand); margin: var(--missing, 8px) }";
        let style = style_of(html, css, "div");
        assert_eq!(style.color, Color::rgb(0x1a, 0x56, 0xdb));
        assert_eq!(style.margin.top.resolve(1000.0), 8.0);
    }

    #[test]
    fn line_height_numbers_and_pixels() {
        let style = style_of("<p>x</p>", "p { line-height: 1.5 }", "p");
        assert!((style.line_height - 1.5).abs() < 0.001);
        let style = style_of("<p>x</p>", "p { font-size: 20px; line-height: 30px }", "p");
        assert!((style.line_height - 1.5).abs() < 0.001);
    }

    #[test]
    fn parses_hex_colors() {
        assert_eq!(Color::parse("#1a56db"), Some(Color::rgb(0x1a, 0x56, 0xdb)));
        assert_eq!(Color::parse("#fff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::parse("rgb(1, 2, 3)"), Some(Color::rgb(1, 2, 3)));
    }

    #[test]
    fn descendant_selectors_require_ancestors() {
        let html = "<div class=\"hero\"><p>x</p></div>";
        let css = ".hero p { color: red }";
        let style = style_of(html, css, "p");
        assert_eq!(style.color, Color::rgb(255, 0, 0));
        let doc = Document::parse("<p>x</p>");
        let styles = compute_styles(&doc, &parse_stylesheet(css));
        assert_ne!(styles[doc.find_tag("p").unwrap()].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn ua_stylesheet_sets_block_display() {
        let doc = Document::parse("<body><div>x</div><span>y</span></body>");
        let styles = compute_styles(&doc, &Stylesheet::default());
        assert_eq!(styles[doc.find_tag("div").unwrap()].display, Display::Block);
        assert_eq!(styles[doc.find_tag("span").unwrap()].display, Display::Inline);
    }

    #[test]
    fn host_pseudo_matches_the_host_only_from_its_tree() {
        let (doc, styles, host) = shadow_styles(
            "<body><div id=\"host\"></div></body>",
            "",
            ":host { color: #0a7d3f }",
        );
        assert_eq!(styles[host].color, Color::rgb(0x0a, 0x7d, 0x3f));
        // Outside the tree there is no scope, so `:host` never matches.
        let selector = parse_selector_list_text(":host");
        assert!(!selector[0].matches(&doc, host));
    }

    #[test]
    fn host_pseudo_takes_a_compound_argument() {
        let (doc, styles, host) = shadow_styles(
            "<body><div id=\"host\" class=\"card\"></div></body>",
            "",
            ":host(.card) { color: #1558d6 }",
        );
        assert_eq!(styles[host].color, Color::rgb(0x15, 0x58, 0xd6));
        let (doc2, styles2, host2) = shadow_styles(
            "<body><div id=\"host\" class=\"other\"></div></body>",
            "",
            ":host(.card) { color: #1558d6 }",
        );
        assert_ne!(styles2[host2].color, Color::rgb(0x15, 0x58, 0xd6));
        let _ = doc2;
        let _ = doc;
    }

    #[test]
    fn shadow_rules_style_only_their_own_tree() {
        // Same class on both sides: only the node inside the tree is styled by
        // the shadow sheet, and only the node outside it by the page sheet.
        let mut doc = Document::parse(
            "<body><div id=\"host\"></div><p id=\"page-p\" class=\"inner\">fora</p></body>",
        );
        let host = doc.find_tag("div").expect("host");
        let root = doc.attach_shadow(host, naro_html::ShadowMode::Open);
        let inner = doc.push_detached_element("p".into(), root);
        doc.attach(root, inner);
        doc.set_attr(inner, "class", "inner");

        let author = parse_stylesheet(".inner { color: #8a1f1f }");
        let mut shadow = std::collections::HashMap::new();
        shadow.insert(root, parse_stylesheet(".inner { color: #0a7d3f }"));
        let styles = compute_styles_scoped(&doc, &author, &shadow, StyleContext::default());

        assert_eq!(styles[inner].color, Color::rgb(0x0a, 0x7d, 0x3f), "inside the tree");
        let page_p = node_by_id(&doc, "page-p");
        assert_eq!(styles[page_p].color, Color::rgb(0x8a, 0x1f, 0x1f), "outside the tree");
    }

    #[test]
    fn document_rules_do_not_reach_shadow_content() {
        let (mut doc, styles, _host) = shadow_styles(
            "<body><div id=\"host\"></div></body>",
            ".inner { color: #8a1f1f }",
            "",
        );
        let root = doc.shadow_root(doc.find_tag("div").unwrap()).expect("root");
        let inner = doc.push_detached_element("p".into(), root);
        doc.attach(root, inner);
        doc.set_attr(inner, "class", "inner");
        // Recompute: the node was added after the first pass.
        let author = parse_stylesheet(".inner { color: #8a1f1f }");
        let mut shadow = std::collections::HashMap::new();
        shadow.insert(root, parse_stylesheet(""));
        let styles = compute_styles_scoped(&doc, &author, &shadow, StyleContext::default());
        assert_ne!(styles[inner].color, Color::rgb(0x8a, 0x1f, 0x1f));
        let _ = styles;
    }

    #[test]
    fn inherited_properties_flow_from_the_host_into_the_tree() {
        let (mut doc, styles, host) = shadow_styles(
            "<body><div id=\"host\"></div></body>",
            "#host { color: #681da8; font-size: 20px }",
            "",
        );
        let root = doc.shadow_root(host).expect("root");
        let inner = doc.push_detached_element("p".into(), root);
        doc.attach(root, inner);
        let author = parse_stylesheet("#host { color: #681da8; font-size: 20px }");
        let mut shadow = std::collections::HashMap::new();
        shadow.insert(root, parse_stylesheet(""));
        let styles = compute_styles_scoped(&doc, &author, &shadow, StyleContext::default());
        assert_eq!(styles[inner].color, Color::rgb(0x68, 0x1d, 0xa8));
        assert!((styles[inner].font_size - 20.0).abs() < 0.01);
        let _ = styles;
    }

    #[test]
    fn shadow_sheet_variables_apply_inside_the_tree() {
        let (mut doc, styles, host) = shadow_styles(
            "<body><div id=\"host\"></div></body>",
            "",
            ":host { --accent: #0e7490 } .inner { color: var(--accent) }",
        );
        let root = doc.shadow_root(host).expect("root");
        let inner = doc.push_detached_element("p".into(), root);
        doc.attach(root, inner);
        doc.set_attr(inner, "class", "inner");
        let author = parse_stylesheet("");
        let mut shadow = std::collections::HashMap::new();
        shadow.insert(
            root,
            parse_stylesheet(":host { --accent: #0e7490 } .inner { color: var(--accent) }"),
        );
        let styles = compute_styles_scoped(&doc, &author, &shadow, StyleContext::default());
        assert_eq!(styles[inner].color, Color::rgb(0x0e, 0x74, 0x90));
        let _ = styles;
    }

    #[test]
    fn host_rules_lose_to_document_rules_and_win_when_important() {
        let html = "<body><div id=\"host\" class=\"card\"></div></body>";
        // Equal specificity: `.card` and `:host` both weigh one class, so the
        // encapsulation rule is what decides.
        let (doc, styles, host) = shadow_styles(
            html,
            ".card { background-color: #8a1f1f }",
            ":host { background-color: #681da8 }",
        );
        assert_eq!(
            styles[host].background,
            Color::rgb(0x8a, 0x1f, 0x1f),
            "outer tree wins a normal declaration"
        );
        let _ = doc;

        // `!important` inside the tree wins instead.
        let (_doc, styles, host) = shadow_styles(
            html,
            ".card { background-color: #8a1f1f !important }",
            ":host { background-color: #681da8 !important }",
        );
        assert_eq!(
            styles[host].background,
            Color::rgb(0x68, 0x1d, 0xa8),
            "inner tree wins an important declaration"
        );
    }

    #[test]
    fn nested_shadow_trees_do_not_leak_into_each_other() {
        // A host inside a shadow tree can host another tree. Each node is
        // styled by the innermost tree it belongs to; the outer component's
        // sheet must not reach the inner one.
        let mut doc = Document::parse("<body><div id=\"outer\"></div></body>");
        let outer_host = doc.find_tag("div").expect("outer host");
        let outer = doc.attach_shadow(outer_host, naro_html::ShadowMode::Open);
        let inner_host = doc.push_detached_element("div".into(), outer);
        doc.attach(outer, inner_host);
        let inner = doc.attach_shadow(inner_host, naro_html::ShadowMode::Open);
        let leaf = doc.push_detached_element("p".into(), inner);
        doc.attach(inner, leaf);
        doc.set_attr(leaf, "class", "x");

        let mut shadow = std::collections::HashMap::new();
        shadow.insert(outer, parse_stylesheet(".x { color: #8a1f1f }"));
        shadow.insert(inner, parse_stylesheet(".x { color: #0a7d3f }"));
        let styles = compute_styles_scoped(
            &doc,
            &parse_stylesheet(""),
            &shadow,
            StyleContext::default(),
        );

        assert_eq!(styles[leaf].color, Color::rgb(0x0a, 0x7d, 0x3f));
        // The inner host belongs to the outer tree, so the outer sheet does
        // apply to it — but only through selectors that match it.
        assert_eq!(doc.shadow_scope(inner_host), Some(outer));
        assert_eq!(doc.shadow_scope(leaf), Some(inner));
    }

    #[test]
    fn sheet_cache_returns_the_same_rules_for_the_same_text() {
        let cache = SheetCache::default();
        let css = ".a { color: #0a7d3f } .b > .c { margin: 0 }";
        let first = cache.parse(css, 1000.0);
        let second = cache.parse(css, 1000.0);
        // Same text, same viewport: the cached sheet, not a re-parse.
        assert!(std::rc::Rc::ptr_eq(&first, &second));
        assert_eq!(first.rules.len(), second.rules.len());
        assert_eq!(first.rules.len(), 2);
    }

    #[test]
    fn sheet_cache_misses_when_the_text_or_viewport_changes() {
        let cache = SheetCache::default();
        let first = cache.parse(".a { color: #0a7d3f }", 1000.0);
        // Different text: a new sheet, not the cached one.
        let second = cache.parse(".a { color: #8a1f1f }", 1000.0);
        assert!(!std::rc::Rc::ptr_eq(&first, &second));
        assert_eq!(second.rules.len(), 1);

        // Same text but a different viewport: `@media` could resolve
        // differently, so the cache must not reuse it.
        let wide = cache.parse("@media (min-width: 900px) { .a { color: #0a7d3f } }", 1000.0);
        let narrow = cache.parse("@media (min-width: 900px) { .a { color: #0a7d3f } }", 500.0);
        assert!(!std::rc::Rc::ptr_eq(&wide, &narrow));
        assert_eq!(wide.rules.len(), 1, "the rule applies at 1000px");
        assert_eq!(narrow.rules.len(), 0, "and not at 500px");
    }

    #[test]
    fn font_face_collects_urls_and_skips_local() {
        let sheet = parse_stylesheet(
            r#"@font-face {
                font-family: "Naro Fixture";
                src: local("Fixture"), url(a.woff2) format("woff2"), url("b.ttf") format("truetype");
                font-weight: 700;
                font-style: italic;
            }
            p { color: #000000 }"#,
        );
        assert_eq!(sheet.font_faces.len(), 1);
        let face = &sheet.font_faces[0];
        assert_eq!(face.family, "Naro Fixture");
        assert_eq!(face.srcs, vec!["a.woff2".to_string(), "b.ttf".to_string()]);
        assert_eq!(face.weight, 700);
        assert!(face.italic);
        // The at-rule no longer shows up as unsupported.
        assert!(!sheet
            .report
            .skipped
            .iter()
            .any(|(reason, detail)| *reason == SkipReason::UnsupportedAtRule
                && detail == "@font-face"));
    }

    #[test]
    fn font_face_without_family_or_src_is_dropped() {
        assert!(parse_stylesheet("@font-face { src: url(a.woff2) }").font_faces.is_empty());
        assert!(parse_stylesheet("@font-face { font-family: X }").font_faces.is_empty());
    }

    #[test]
    fn font_family_is_inherited_and_normalized() {
        let style = style_of(
            r#"<div><p>t</p></div>"#,
            r#"div { font-family: "Arial", 'Helvetica Neue' , sans-serif }"#,
            "p",
        );
        assert_eq!(style.font_family, "Arial, Helvetica Neue, sans-serif");
    }

    #[test]
    fn font_shorthand_carries_size_line_height_and_family() {
        let style = style_of("<p>t</p>", "p { font: italic 700 16px/1.4 Arial, sans-serif }", "p");
        assert_eq!(style.font_size, 16.0);
        assert_eq!(style.line_height, 1.4);
        assert_eq!(style.font_weight, 700);
        assert!(style.italic);
        assert_eq!(style.font_family, "Arial, sans-serif");
    }

    #[test]
    fn font_style_and_computed_values() {
        let style = style_of("<p>t</p>", "p { font-style: oblique; font-family: monospace }", "p");
        assert!(style.italic);
        assert_eq!(computed_value(&style, "font-style").as_deref(), Some("italic"));
        assert_eq!(computed_value(&style, "font-family").as_deref(), Some("monospace"));
        // A family change must force a relayout: it changes every advance.
        let mut other = style.clone();
        other.font_family = "sans-serif".to_string();
        assert!(style.geometry_differs(&other));
    }

    #[test]
    fn selector_parsing_survives_multibyte_names() {
        // The parser works on byte offsets; a name character above U+007F is
        // accepted by `is_name_char`, so slicing must stay on boundaries.
        let sheet = parse_stylesheet(".ação { color: #0a7d3f } [data-x=\"ção\"] { margin: 0 }");
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[0].selector.compounds[0].classes, vec!["ação".to_string()]);
        assert_eq!(sheet.rules[1].selector.compounds[0].attrs[0].value, "ção");
    }

    #[test]
    fn color_parses_alpha_forms() {
        assert_eq!(Color::parse("rgba(0, 0, 0, 0.5)").unwrap().a, 128);
        assert_eq!(Color::parse("rgba(0,0,0,.5)").unwrap().a, 128);
        assert_eq!(Color::parse("#ff000080").unwrap().a, 128);
        assert_eq!(Color::parse("#f008").unwrap().a, 136);
        // `to_hex` still reports the RGB channels only.
        assert_eq!(Color::parse("#ff000080").unwrap().to_hex(), "#ff0000");
    }

    #[test]
    fn color_parses_hsl() {
        let red = Color::parse("hsl(0, 100%, 50%)").unwrap();
        assert_eq!((red.r, red.g, red.b), (255, 0, 0));
        let green = Color::parse("hsl(120deg, 100%, 50%)").unwrap();
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        let gray = Color::parse("hsl(0, 0%, 50%)").unwrap();
        assert_eq!((gray.r, gray.g, gray.b), (128, 128, 128));
        assert_eq!(Color::parse("hsla(240, 100%, 50%, 0.5)").unwrap().a, 128);
    }

    #[test]
    fn linear_gradient_parses_direction_and_stops() {
        let g = parse_gradient("linear-gradient(to right, #000000, #ffffff)").expect("gradient");
        assert_eq!(g.kind, GradientKind::Linear { angle_deg: 90.0 });
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].0, 0.0);
        assert_eq!(g.stops[1].0, 1.0);
        // A bare angle, and the CSS default direction.
        assert_eq!(
            parse_gradient("linear-gradient(45deg, red, blue)").unwrap().kind,
            GradientKind::Linear { angle_deg: 45.0 }
        );
        assert_eq!(
            parse_gradient("linear-gradient(red, blue)").unwrap().kind,
            GradientKind::Linear { angle_deg: 180.0 }
        );
        // Fewer than two stops is not a gradient.
        assert!(parse_gradient("linear-gradient(red)").is_none());
        assert!(parse_gradient("#ffffff").is_none());
    }

    #[test]
    fn linear_gradient_stops_interpolate_and_respect_positions() {
        let g = parse_gradient("linear-gradient(red 10%, blue 60%, lime)").unwrap();
        assert_eq!(g.stops.len(), 3);
        assert!((g.stops[0].0 - 0.1).abs() < 1e-6);
        assert!((g.stops[1].0 - 0.6).abs() < 1e-6);
        assert!((g.stops[2].0 - 1.0).abs() < 1e-6);
        // Positions are sorted even when the source is not.
        let sorted = parse_gradient("linear-gradient(red 80%, blue 20%)").unwrap();
        assert!(sorted.stops[0].0 <= sorted.stops[1].0);
    }

    #[test]
    fn radial_gradient_parses_centre() {
        let g = parse_gradient("radial-gradient(circle at 50% 50%, #ffffff, #000000)").unwrap();
        match g.kind {
            GradientKind::Radial { center, radius } => {
                assert!((center.0 - 0.5).abs() < 1e-6);
                assert!((center.1 - 0.5).abs() < 1e-6);
                assert!((radius - 0.5).abs() < 1e-6);
            }
            other => panic!("esperava radial, veio {other:?}"),
        }
        let corner = parse_gradient("radial-gradient(at left top, red, blue)").unwrap();
        assert_eq!(
            corner.kind,
            GradientKind::Radial { center: (0.0, 0.0), radius: 0.5 }
        );
        // A prelude that is already a colour must not be eaten.
        let plain = parse_gradient("radial-gradient(red, blue)").unwrap();
        assert_eq!(plain.stops.len(), 2);
    }

    #[test]
    fn box_shadow_parses_spread_inset_and_alpha() {
        let style = style_of(
            "<p>t</p>",
            "p { box-shadow: 0 2px 4px 2px rgba(0,0,0,.3) inset }",
            "p",
        );
        let shadow = style.box_shadow.expect("sombra");
        assert_eq!((shadow.dx, shadow.dy, shadow.blur, shadow.spread), (0.0, 2.0, 4.0, 2.0));
        assert!(shadow.inset);
        assert_eq!(shadow.color.a, 77);
        // No colour means a soft black, the historical default.
        let plain = style_of("<p>t</p>", "p { box-shadow: 0 1px 0 }", "p");
        assert_eq!(plain.box_shadow.unwrap().color.a, 60);
        assert!(style_of("<p>t</p>", "p { box-shadow: none }", "p").box_shadow.is_none());
    }

    #[test]
    fn border_radius_percent_is_kept_symbolic() {
        let style = style_of("<p>t</p>", "p { border-radius: 50% }", "p");
        assert_eq!(style.border_radius_percent, 50.0);
        assert_eq!(style.border_radius, 0.0);
        assert_eq!(computed_value(&style, "border-radius").as_deref(), Some("50%"));
        let px = style_of("<p>t</p>", "p { border-radius: 4px }", "p");
        assert_eq!(computed_value(&px, "border-radius").as_deref(), Some("4px"));
    }

    #[test]
    fn gradient_reaches_the_computed_style() {
        let style = style_of(
            "<p>t</p>",
            "p { background: linear-gradient(to right, #000000, #ffffff) }",
            "p",
        );
        assert!(style.background_gradient.is_some());
        // `background-image` takes the same path.
        let via_image = style_of(
            "<p>t</p>",
            "p { background-image: linear-gradient(red, blue) }",
            "p",
        );
        assert!(via_image.background_gradient.is_some());
    }

    fn transform_of(css: &str) -> Affine {
        style_of("<p>t</p>", css, "p").transform.expect("transform")
    }

    #[test]
    fn transform_composes_left_to_right() {
        // `translate(10px, 0) rotate(90deg)` applied to (1, 0) is (10, 1): the
        // rotation acts first because it is on the right. Swapping `multiply`
        // gives (0, 11) and fails here.
        let m = transform_of("p { transform: translate(10px, 0) rotate(90deg) }");
        let (x, y) = m.apply(1.0, 0.0);
        assert!((x - 10.0).abs() < 1e-4, "x = {x}");
        assert!((y - 1.0).abs() < 1e-4, "y = {y}");

        // The scale also scales the translation, because the translation is on
        // the right of the scale.
        let m = transform_of("p { transform: scale(2) translate(5px, 0) }");
        let (x, y) = m.apply(1.0, 0.0);
        assert!((x - 12.0).abs() < 1e-4, "x = {x}");
        assert!(y.abs() < 1e-4);
    }

    #[test]
    fn transform_origin_shifts_the_local_matrix() {
        // A 100x100 box rotated about its top-left corner keeps (0,0) fixed;
        // about its centre, (0,0) maps to (50, -50).
        let m = transform_of("p { transform: rotate(90deg); transform-origin: left top }");
        let (x, y) = m.apply(0.0, 0.0);
        assert!(x.abs() < 1e-4 && y.abs() < 1e-4, "origem left top: {x},{y}");

        let style = style_of(
            "<p>t</p>",
            "p { transform: rotate(90deg); transform-origin: center }",
            "p",
        );
        assert_eq!(style.transform_origin, (0.5, 0.5));
        let m = style.transform.unwrap();
        // Conjugating by the origin is a layout step; the origin itself is
        // what differs, and `Flow::block` applies it.
        assert_ne!(
            style_of("<p>t</p>", "p { transform-origin: left top }", "p").transform_origin,
            style.transform_origin
        );
        let (x, y) = m.apply(0.0, 0.0);
        assert!(x.abs() < 1e-4 && y.abs() < 1e-4);
    }

    #[test]
    fn mirrored_and_singular_transforms() {
        let m = transform_of("p { transform: scale(-1, 1) }");
        assert!(m.inverse().is_some());
        assert!(!m.is_translation_scale());
        let (x, _) = m.apply(1.0, 0.0);
        assert!((x + 1.0).abs() < 1e-4);

        let zero = transform_of("p { transform: scale(0) }");
        assert!(zero.inverse().is_none());
    }

    #[test]
    fn unknown_transform_functions_discard_the_declaration() {
        assert_eq!(parse_transform("skew(10deg)"), None);
        assert_eq!(parse_transform("matrix(1,0,0,1,0,0)"), None);
        assert_eq!(parse_transform("perspective(100px)"), None);
        // `none` clears the transform instead of keeping the previous one.
        let style = style_of(
            "<p>t</p>",
            "p { transform: rotate(10deg) } p { transform: none }",
            "p",
        );
        assert!(style.transform.is_none());
    }

    #[test]
    fn object_fit_and_aspect_ratio_parse() {
        let style = style_of(
            "<p>t</p>",
            "p { object-fit: contain; aspect-ratio: 16 / 9 }",
            "p",
        );
        assert_eq!(style.object_fit, ObjectFit::Contain);
        assert!((style.aspect_ratio.unwrap() - 16.0 / 9.0).abs() < 1e-5);
        assert_eq!(computed_value(&style, "object-fit").as_deref(), Some("contain"));
        assert_eq!(parse_aspect_ratio("auto"), None);
        assert!((parse_aspect_ratio("4/2").unwrap() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn transform_change_forces_a_relayout() {
        // The paint matrix lives in the `Layout`, so a restyle that only
        // changes `transform` must rebuild the boxes.
        let a = style_of("<p>t</p>", "p { transform: translate(10px, 0) }", "p");
        let b = style_of("<p>t</p>", "p { transform: translate(20px, 0) }", "p");
        assert!(a.geometry_differs(&b));
    }
}
