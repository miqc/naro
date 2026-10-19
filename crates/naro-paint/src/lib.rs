//! Software rasterizer: an RGBA-less 32-bit framebuffer plus the few drawing
//! primitives NARO needs. Kept separate from layout so a GPU compositor can
//! replace it later without touching the engine.

use naro_css::Color;
use naro_layout::Rect;
use naro_text::{FontSet, TextMeasure};

/// A framebuffer borrowed from the caller. Painting straight into the
/// presentation buffer avoids one full-screen allocation and one copy per frame.
pub struct Canvas<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a mut [u32],
    pub scale: f32,
    clip: (i32, i32, i32, i32),
    /// CSS transforms pushed by the caller, innermost last. Empty in the
    /// common case, which is why it is a `Vec` and not a fixed field.
    transforms: Vec<naro_css::Affine>,
    /// How many blurred shadow masks were actually built. The fast path and a
    /// box outside the clip do not count, which is what lets a test prove the
    /// skip without timing anything.
    pub shadows_painted: usize,
}

/// Box-blur passes behind one shadow. Three is the usual choice (a quadratic
/// B-spline approximates a Gaussian), and with `radius = blur / passes` the
/// total support is exactly `blur`.
const BLUR_PASSES: usize = 3;

/// Peak bytes one shadow mask may allocate. The paint builds exactly two `u8`
/// buffers of the mask size (the mask and the horizontal blur temporary), so
/// this is a ceiling on the allocation, not only on the mask.
pub const MAX_SHADOW_PIXELS: usize = 4_000_000;

/// Bytes the mask of one shadow needs, with the parameters already effective.
///
/// The single source of the formula: `clamp_shadow` uses it to bound, and the
/// tests use it to verify, so the two can never drift apart.
pub fn shadow_mask_bytes(w: f32, h: f32, blur: f32, spread: f32, scale: f32) -> usize {
    // The blur tail is `BLUR_PASSES * radius` with `radius = blur / BLUR_PASSES`,
    // so it is exactly `blur`. The padding therefore is `spread + blur` per
    // side — the earlier factor of 3 over-reserved and doubled the mask area.
    let grow = 2.0 * (spread.max(0.0) + blur.max(0.0));
    let s = if scale > 0.0 { scale } else { 1.0 };
    let mw = ((w + grow).max(1.0) * s).ceil();
    let mh = ((h + grow).max(1.0) * s).ceil();
    // Two buffers, both `u8`, of the mask size.
    (2.0 * mw * mh) as usize
}


/// Shrink `blur` and `spread` until the mask — and every buffer it needs — fits
/// in [`MAX_SHADOW_PIXELS`]. Returns `(blur, spread)` already bounded; never
/// grows either value.
pub fn clamp_shadow(w: f32, h: f32, blur: f32, spread: f32, scale: f32) -> (f32, f32) {
    if shadow_mask_bytes(w, h, blur, spread, scale) <= MAX_SHADOW_PIXELS {
        return (blur, spread);
    }
    // Solve for `k` on `(spread, blur)`: both scale linearly, and the mask area
    // grows with `(w + 2k(spread + blur)) * (h + 2k(spread + blur))`, which is
    // monotone in `k`, so a bisection converges without a closed form.
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if shadow_mask_bytes(w, h, blur * mid, spread * mid, scale) <= MAX_SHADOW_PIXELS {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (blur * lo, spread * lo)
}

impl<'a> Canvas<'a> {
    /// `pixels` must hold at least `width * height` entries.
    pub fn new(pixels: &'a mut [u32], width_px: u32, height_px: u32, scale: f32) -> Canvas<'a> {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let width = width_px.max(1);
        let height = height_px.max(1);
        let needed = (width as usize) * (height as usize);
        let pixels = if pixels.len() >= needed { &mut pixels[..needed] } else { pixels };
        Canvas {
            width,
            height,
            pixels,
            scale,
            clip: (0, 0, width as i32, height as i32),
            transforms: Vec::new(),
            shadows_painted: 0,
        }
    }

    pub fn clear(&mut self, color: Color) {
        let px = pack(color);
        self.pixels.iter_mut().for_each(|p| *p = px);
    }

    pub fn set_clip(&mut self, rect: Rect) {
        // The clip follows the transform: a rotated block must not be cropped
        // by its ancestor's unrotated box, so the clip is the device-space
        // bounding box of the four mapped corners.
        let d = self.device_matrix();
        if d.is_translation_scale() {
            let (x0, y0, x1, y1) = self.device_rect(rect.x, rect.y, rect.w, rect.h);
            self.clip = (
                x0.max(0),
                y0.max(0),
                x1.min(self.width as i32),
                y1.min(self.height as i32),
            );
            return;
        }
        let corners = [
            d.apply(rect.x, rect.y),
            d.apply(rect.x + rect.w, rect.y),
            d.apply(rect.x, rect.y + rect.h),
            d.apply(rect.x + rect.w, rect.y + rect.h),
        ];
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (x, y) in corners {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        self.clip = (
            (min_x.floor() as i32).max(0),
            (min_y.floor() as i32).max(0),
            (max_x.ceil() as i32).min(self.width as i32),
            (max_y.ceil() as i32).min(self.height as i32),
        );
    }

    /// Push a CSS transform, in layout pixels. The canvas scale is applied on
    /// top of it, exactly once.
    pub fn push_transform(&mut self, t: naro_css::Affine) {
        self.transforms.push(t);
    }

    pub fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    /// The CSS transform currently in effect, identity when the stack is empty.
    fn current(&self) -> naro_css::Affine {
        self.transforms
            .last()
            .copied()
            .unwrap_or(naro_css::Affine::IDENTITY)
    }

    /// Layout space to device space: the CSS transform, then the canvas scale.
    /// One application of `scale`, so a `translate(10px)` moves 10 CSS pixels
    /// (20 device pixels at 2x), never 40.
    fn device_matrix(&self) -> naro_css::Affine {
        let s = self.scale;
        naro_css::Affine::scale(s, s).multiply(self.current())
    }

    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, self.width as i32, self.height as i32);
    }

    fn device_rect(&self, x: f32, y: f32, w: f32, h: f32) -> (i32, i32, i32, i32) {
        if self.transforms.is_empty() {
            let s = self.scale;
            return (
                (x * s).floor() as i32,
                (y * s).floor() as i32,
                ((x + w) * s).ceil() as i32,
                ((y + h) * s).ceil() as i32,
            );
        }
        let d = self.device_matrix();
        if d.is_translation_scale() {
            let (x0, y0) = d.apply(x, y);
            let (x1, y1) = d.apply(x + w, y + h);
            return (
                x0.floor() as i32,
                y0.floor() as i32,
                x1.ceil() as i32,
                y1.ceil() as i32,
            );
        }
        let corners = [
            d.apply(x, y),
            d.apply(x + w, y),
            d.apply(x, y + h),
            d.apply(x + w, y + h),
        ];
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (cx, cy) in corners {
            min_x = min_x.min(cx);
            min_y = min_y.min(cy);
            max_x = max_x.max(cx);
            max_y = max_y.max(cy);
        }
        (
            min_x.floor() as i32,
            min_y.floor() as i32,
            max_x.ceil() as i32,
            max_y.ceil() as i32,
        )
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || color.a == 0 {
            return;
        }
        if !self.transforms.is_empty() && !self.device_matrix().is_translation_scale() {
            self.fill_transformed(x, y, w, h, 0.0, color);
            return;
        }
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        self.fill_device(x0, y0, x1, y1, color);
    }

    /// Rasterize an axis-aligned layout rect (optionally rounded) through the
    /// current matrix, by inverse mapping each device pixel.
    ///
    /// Interior pixels are solid and only edge pixels are supersampled 4x4, so
    /// the cost is `area + perimeter*16` instead of `area*16`. A singular
    /// matrix paints nothing: `scale(0)` is invisible.
    fn fill_transformed(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
        let d = self.device_matrix();
        let Some(inv) = d.inverse() else { return };
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        let x0 = (x0 - 1).max(self.clip.0).max(0);
        let y0 = (y0 - 1).max(self.clip.1).max(0);
        let x1 = (x1 + 1).min(self.clip.2).min(self.width as i32);
        let y1 = (y1 + 1).min(self.clip.3).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let inside = |px: i32, py: i32| -> bool {
            let (lx, ly) = inv.apply(px as f32 + 0.5, py as f32 + 0.5);
            if r > 0.0 {
                rounded_contains_layout(lx, ly, x, y, w, h, r)
            } else {
                lx >= x && ly >= y && lx < x + w && ly < y + h
            }
        };
        for py in y0..y1 {
            for px in x0..x1 {
                let here = inside(px, py);
                let edge = inside(px - 1, py) != here
                    || inside(px + 1, py) != here
                    || inside(px, py - 1) != here
                    || inside(px, py + 1) != here;
                if !here && !edge {
                    continue;
                }
                if edge {
                    let mut hits = 0u32;
                    for sy in 0..4 {
                        for sx in 0..4 {
                            let fx = px as f32 + (sx as f32 + 0.5) / 4.0;
                            let fy = py as f32 + (sy as f32 + 0.5) / 4.0;
                            let (lx, ly) = inv.apply(fx, fy);
                            let ok = if r > 0.0 {
                                rounded_contains_layout(lx, ly, x, y, w, h, r)
                            } else {
                                lx >= x && ly >= y && lx < x + w && ly < y + h
                            };
                            if ok {
                                hits += 1;
                            }
                        }
                    }
                    if hits > 0 {
                        self.blend_at(px, py, color, (hits * 255 / 16) as u8);
                    }
                } else {
                    self.blend_at(px, py, color, 255);
                }
            }
        }
    }

    fn fill_device(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let x0 = x0.max(self.clip.0).max(0);
        let y0 = y0.max(self.clip.1).max(0);
        let x1 = x1.min(self.clip.2).min(self.width as i32);
        let y1 = y1.min(self.clip.3).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        if color.a == 255 {
            let px = pack(color);
            for y in y0..y1 {
                let row = (y as usize) * (self.width as usize);
                for x in x0..x1 {
                    self.pixels[row + x as usize] = px;
                }
            }
        } else {
            for y in y0..y1 {
                for x in x0..x1 {
                    self.blend_at(x, y, color, 255);
                }
            }
        }
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: Color) {
        if width <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        self.fill_rect(x, y, w, width, color);
        self.fill_rect(x, y + h - width, w, width, color);
        self.fill_rect(x, y + width, width, h - 2.0 * width, color);
        self.fill_rect(x + w - width, y + width, width, h - 2.0 * width, color);
    }

    /// Distance from a point to a segment, in device pixels.
    fn dist_to_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
        let vx = bx - ax;
        let vy = by - ay;
        let wx = px - ax;
        let wy = py - ay;
        let len2 = vx * vx + vy * vy;
        let t = if len2 > 0.0 {
            ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dx = px - (ax + vx * t);
        let dy = py - (ay + vy * t);
        (dx * dx + dy * dy).sqrt()
    }

    /// Anti-aliased polyline of `width` logical pixels.
    ///
    /// `fill_rect` snaps to whole device pixels, so a diagonal drawn by
    /// stamping squares comes out as a staircase — at 1x a "1.5px" stroke is a
    /// 2px blocky step. This builds a coverage mask from the distance to each
    /// segment and blends once, which is what makes an icon look drawn instead
    /// of pixelated. Joins are covered by taking the maximum coverage, so
    /// overlapping segments do not darken each other.
    pub fn stroke_path(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        if points.len() < 2 || width <= 0.0 || color.a == 0 {
            return;
        }
        let s = self.scale;
        let radius = (width * s) / 2.0;
        let pad = radius + 1.0;

        let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
        let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in points {
            min_x = min_x.min(x * s);
            min_y = min_y.min(y * s);
            max_x = max_x.max(x * s);
            max_y = max_y.max(y * s);
        }
        let x0 = ((min_x - pad).floor() as i32).max(self.clip.0).max(0);
        let y0 = ((min_y - pad).floor() as i32).max(self.clip.1).max(0);
        let x1 = ((max_x + pad).ceil() as i32).min(self.clip.2).min(self.width as i32);
        let y1 = ((max_y + pad).ceil() as i32).min(self.clip.3).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut mask = vec![0f32; w * h];

        for pair in points.windows(2) {
            let (ax, ay) = (pair[0].0 * s, pair[0].1 * s);
            let (bx, by) = (pair[1].0 * s, pair[1].1 * s);
            for py in 0..h {
                let cy = y0 as f32 + py as f32 + 0.5;
                for px in 0..w {
                    let cx = x0 as f32 + px as f32 + 0.5;
                    let d = Self::dist_to_segment(cx, cy, ax, ay, bx, by);
                    let cov = (radius + 0.5 - d).clamp(0.0, 1.0);
                    let slot = &mut mask[py * w + px];
                    if cov > *slot {
                        *slot = cov;
                    }
                }
            }
        }

        for py in 0..h {
            for px in 0..w {
                let cov = mask[py * w + px];
                if cov > 0.0 {
                    let a = (cov * 255.0).round() as u8;
                    self.blend_at(x0 + px as i32, y0 + py as i32, color, a);
                }
            }
        }
    }

    /// Anti-aliased filled triangle, by 4x4 supersampling.
    pub fn fill_triangle(
        &mut self,
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        color: Color,
    ) {
        if color.a == 0 {
            return;
        }
        let s = self.scale;
        let (ax, ay) = (p0.0 * s, p0.1 * s);
        let (bx, by) = (p1.0 * s, p1.1 * s);
        let (cx, cy) = (p2.0 * s, p2.1 * s);
        let x0 = (ax.min(bx).min(cx).floor() as i32 - 1).max(self.clip.0).max(0);
        let y0 = (ay.min(by).min(cy).floor() as i32 - 1).max(self.clip.1).max(0);
        let x1 = (ax.max(bx).max(cx).ceil() as i32 + 1).min(self.clip.2).min(self.width as i32);
        let y1 = (ay.max(by).max(cy).ceil() as i32 + 1).min(self.clip.3).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let edge = |px: f32, py: f32, qx: f32, qy: f32, rx: f32, ry: f32| -> f32 {
            (qx - px) * (ry - py) - (qy - py) * (rx - px)
        };
        let area = edge(ax, ay, bx, by, cx, cy);
        if area == 0.0 {
            return;
        }
        for py in y0..y1 {
            for px in x0..x1 {
                let mut inside = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let x = px as f32 + (sx as f32 + 0.5) / 4.0;
                        let y = py as f32 + (sy as f32 + 0.5) / 4.0;
                        let w0 = edge(bx, by, cx, cy, x, y);
                        let w1 = edge(cx, cy, ax, ay, x, y);
                        let w2 = edge(ax, ay, bx, by, x, y);
                        let ok = if area > 0.0 {
                            w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0
                        } else {
                            w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0
                        };
                        if ok {
                            inside += 1;
                        }
                    }
                }
                if inside > 0 {
                    let a = (inside * 255 / 16) as u8;
                    self.blend_at(px, py, color, a);
                }
            }
        }
    }

    /// Circular arc in logical pixels, drawn by sampling; angles in degrees,
    /// measured clockwise from 3 o'clock (screen coordinates).
    pub fn stroke_arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        width: f32,
        start_deg: f32,
        end_deg: f32,
        color: Color,
    ) {
        let steps = ((radius * 8.0).max(24.0)) as i32;
        let sweep = end_deg - start_deg;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let angle = (start_deg + sweep * t).to_radians();
            let x = cx + radius * angle.cos();
            let y = cy + radius * angle.sin();
            self.fill_rect(x - width / 2.0, y - width / 2.0, width, width, color);
        }
    }

    /// Filled rectangle with rounded corners.
    pub fn fill_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
        if r <= 0.5 || w <= 0.0 || h <= 0.0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        if !self.transforms.is_empty() && !self.device_matrix().is_translation_scale() {
            self.fill_transformed(x, y, w, h, r, color);
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0);
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        let s = self.scale;
        let rr = r * s;
        let (lx, ty) = (x0 as f32, y0 as f32);
        let (rx, by) = (x1 as f32, y1 as f32);
        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let cx = if fx < lx + rr {
                    lx + rr
                } else if fx > rx - rr {
                    rx - rr
                } else {
                    fx
                };
                let cy = if fy < ty + rr {
                    ty + rr
                } else if fy > by - rr {
                    by - rr
                } else {
                    fy
                };
                let (dx, dy) = (fx - cx, fy - cy);
                if dx * dx + dy * dy <= rr * rr {
                    self.blend_at(px, py, color, 255);
                }
            }
        }
    }

    /// Draw an RGBA image scaled into `w × h` logical pixels (bilinear).
    /// Fill one SVG shape produced by the layout.
    ///
    /// The geometry comes from `naro_layout::svg`; this only maps device pixels
    /// back to the canvas, honouring the current clip.
    pub fn fill_svg_shape(&mut self, shape: &naro_layout::SvgShape) {
        if shape.color.a == 0 {
            return;
        }
        let (width, height) = (self.width, self.height);
        let scale = self.scale;
        // The fill walks device rows, so the canvas has to hand out its buffer
        // for the duration; the clip is re-applied per pixel through `blend_at`.
        let mut hits: Vec<(u32, u32)> = Vec::new();
        naro_layout::svg::fill_subpaths(
            &shape.subpaths,
            (shape.origin.0 * scale, shape.origin.1 * scale),
            (shape.scale.0 * scale, shape.scale.1 * scale),
            width,
            height,
            |x, y| hits.push((x, y)),
        );
        for (x, y) in hits {
            self.blend_at(x as i32, y as i32, shape.color, 255);
        }
    }

    /// Fill a rectangle with a parsed CSS gradient.
    ///
    /// Interpolates in sRGB between the neighbouring stops. A linear gradient
    /// projects each pixel onto the gradient axis; a radial one measures the
    /// distance to the centre. `border_radius` is deliberately ignored — the
    /// clip is the rectangle.
    pub fn fill_gradient_rect(&mut self, x: f32, y: f32, w: f32, h: f32, gradient: &naro_css::Gradient) {
        if w <= 0.0 || h <= 0.0 || gradient.stops.len() < 2 {
            return;
        }
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        let cx0 = x0.max(self.clip.0).max(0);
        let cy0 = y0.max(self.clip.1).max(0);
        let cx1 = x1.min(self.clip.2).min(self.width as i32);
        let cy1 = y1.min(self.clip.3).min(self.height as i32);
        if cx1 <= cx0 || cy1 <= cy0 {
            return;
        }
        let dw = (x1 - x0).max(1) as f32;
        let dh = (y1 - y0).max(1) as f32;
        let stops = &gradient.stops;
        for py in cy0..cy1 {
            for px in cx0..cx1 {
                // Normalized position inside the box, in `0..=1`.
                let u = ((px - x0) as f32 + 0.5) / dw;
                let v = ((py - y0) as f32 + 0.5) / dh;
                let t = match &gradient.kind {
                    naro_css::GradientKind::Linear { angle_deg } => {
                        // CSS: 0 = up, 90 = right. Project the offset from the
                        // centre onto the unit vector and normalize by the
                        // box's extent along that direction, so a horizontal
                        // gradient spans the width and a diagonal one spans
                        // the diagonal.
                        let rad = angle_deg.to_radians();
                        let (dx, dy) = (rad.sin(), -rad.cos());
                        let (ox, oy) = (u - 0.5, v - 0.5);
                        let proj = ox * dx + oy * dy;
                        let extent = 0.5 * (dx.abs() + dy.abs()).max(1e-6);
                        (0.5 + proj / (2.0 * extent)).clamp(0.0, 1.0)
                    }
                    naro_css::GradientKind::Radial { center, radius } => {
                        // Distance in layout pixels from the centre, divided by
                        // `radius * max(w, h)` — so `radius: 0.5` (the default)
                        // reaches the far edge of the box's longer side.
                        let (lx, ly) = ((u - center.0) * w, (v - center.1) * h);
                        let dist = (lx * lx + ly * ly).sqrt();
                        (dist / (radius.max(1e-6) * w.max(h))).clamp(0.0, 1.0)
                    }
                };
                let color = sample_gradient(stops, t);
                self.blend_at(px, py, color, 255);
            }
        }
    }

    /// Draw a `box-shadow` for the rounded rect `(x, y, w, h, r)`.
    ///
    /// `blur == 0` and `spread == 0` takes a fast path that draws the offset
    /// rounded rect directly — that is the common `0 1px 0 #000` case and it
    /// must not allocate a mask. Anything else builds an alpha mask of the
    /// expanded, offset rect, blurs it with three separable box passes and
    /// composites `color` through it. `inset` draws the inverted mask inside
    /// the box.
    pub fn fill_shadow(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        shadow: &naro_css::BoxShadow,
    ) {
        if w <= 0.0 || h <= 0.0 || shadow.color.a == 0 {
            return;
        }
        // Clamp against the *visible* part of the box: the mask is cropped to
        // the clip, so a 5000x5000 box on a 400x300 canvas needs a 400x300
        // mask, and its blur does not have to shrink as if it covered the
        // whole box.
        let s = self.scale;
        let clip_w = ((self.clip.2 - self.clip.0).max(0) as f32) / s;
        let clip_h = ((self.clip.3 - self.clip.1).max(0) as f32) / s;
        let (blur, spread) =
            clamp_shadow(w.min(clip_w), h.min(clip_h), shadow.blur, shadow.spread, s);
        if blur <= 0.0 && spread <= 0.0 {
            // Fast path: no mask, no allocation.
            let (dx, dy) = (shadow.dx, shadow.dy);
            self.fill_round_rect(x + dx, y + dy, w, h, r, shadow.color);
            return;
        }
        let grow = spread + blur;
        // The mask is cropped to the clip. Without that, a 5000x5000 box would
        // allocate 50 MB for pixels nobody can see; with it, the allocation is
        // bounded by the visible area. The trade-off is that the blur edge
        // clamps at the clip boundary instead of at the shape's own edge, which
        // only affects the outermost blur radius of the visible region.
        let mask_x = ((x + shadow.dx - grow) * s).floor() as i32;
        let mask_y = ((y + shadow.dy - grow) * s).floor() as i32;
        let mask_x1 = (((x + w + shadow.dx + grow) * s).ceil() as i32)
            .min(self.clip.2)
            .min(self.width as i32);
        let mask_y1 = (((y + h + shadow.dy + grow) * s).ceil() as i32)
            .min(self.clip.3)
            .min(self.height as i32);
        let mask_x0 = mask_x.max(self.clip.0).max(0);
        let mask_y0 = mask_y.max(self.clip.1).max(0);
        let mask_w = mask_x1 - mask_x0;
        let mask_h = mask_y1 - mask_y0;
        if mask_w <= 0 || mask_h <= 0 {
            // Entirely outside the visible area: no mask is built at all.
            return;
        }
        self.shadows_painted += 1;
        let (mask_x, mask_y) = (mask_x0, mask_y0);
        // The mask covers the box expanded by `spread`, positioned inside the
        // padded mask so the blur tail has room.
        let inner_x = ((x + shadow.dx - spread) * s).round() as i32 - mask_x;
        let inner_y = ((y + shadow.dy - spread) * s).round() as i32 - mask_y;
        let inner_w = ((w + 2.0 * spread) * s).round().max(1.0) as i32;
        let inner_h = ((h + 2.0 * spread) * s).round().max(1.0) as i32;
        // A square box keeps square shadow corners: spread grows the shape, it
        // does not round it.
        let rr = (if r > 0.0 { (r + spread).max(0.0) } else { 0.0 } * s).round() as i32;

        let (mw, mh) = (mask_w as usize, mask_h as usize);
        let mut mask = vec![0u8; mw * mh];
        // Row spans instead of a per-pixel point test: the shape is convex, so
        // each row is one contiguous run and the expensive part (the square
        // root of the corner circle) happens once per row.
        if shadow.inset {
            // Inset: the box, minus the shrunken offset rect.
            fill_rounded_span(&mut mask, mw, mh, 0, 0, mask_w, mask_h, 0, 255);
            fill_rounded_span(&mut mask, mw, mh, inner_x, inner_y, inner_w, inner_h, rr, 0);
            // Restore the part of the inner rect that falls outside the mask.
            for my in 0..mh as i32 {
                for mx in 0..mw as i32 {
                    if rounded_contains(mx, my, 0, 0, mask_w, mask_h, 0)
                        && !rounded_contains(mx, my, inner_x, inner_y, inner_w, inner_h, rr)
                    {
                        mask[my as usize * mw + mx as usize] = 255;
                    }
                }
            }
        } else {
            fill_rounded_span(&mut mask, mw, mh, inner_x, inner_y, inner_w, inner_h, rr, 255);
        }
        let radius = if blur <= 0.0 {
            0
        } else {
            ((blur * s / BLUR_PASSES as f32).max(1.0)) as usize
        };
        for _ in 0..BLUR_PASSES {
            if radius == 0 {
                break;
            }
            blur_horizontal(&mut mask, mw, mh, radius);
            blur_vertical(&mut mask, mw, mh, radius);
        }
        let color = shadow.color;
        let stride = self.width as usize;
        // Every mask pixel is inside the clip by construction, so the composite
        // writes straight into the buffer: no clip test, no call per pixel.
        for my in 0..mh {
            let py = (mask_y + my as i32) as usize;
            let row = py * stride;
            let mask_row = &mask[my * mw..(my + 1) * mw];
            for (mx, &cov) in mask_row.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                let idx = row + (mask_x as usize + mx);
                let a = (cov as u32 * color.a as u32 + 127) / 255;
                if a == 0 {
                    continue;
                }
                if a >= 255 {
                    self.pixels[idx] = pack(color);
                    continue;
                }
                let dst = self.pixels[idx];
                let inv = 255 - a;
                let r = (color.r as u32 * a + ((dst >> 16) & 0xFF) * inv + 127) / 255;
                let g = (color.g as u32 * a + ((dst >> 8) & 0xFF) * inv + 127) / 255;
                let b = (color.b as u32 * a + (dst & 0xFF) * inv + 127) / 255;
                self.pixels[idx] = (r << 16) | (g << 8) | b;
            }
        }
    }

    pub fn draw_image(&mut self, x: f32, y: f32, w: f32, h: f32, image: &naro_image::Image) {
        if w <= 0.0 || h <= 0.0 || image.width == 0 || image.height == 0 {
            return;
        }
        if !self.transforms.is_empty() && !self.device_matrix().is_translation_scale() {
            self.draw_image_transformed(x, y, w, h, image);
            return;
        }
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        let (x0c, y0c) = (x0.max(self.clip.0).max(0), y0.max(self.clip.1).max(0));
        let (x1c, y1c) = (
            x1.min(self.clip.2).min(self.width as i32),
            y1.min(self.clip.3).min(self.height as i32),
        );
        if x1c <= x0c || y1c <= y0c {
            return;
        }
        let sw = image.width as f32;
        let sh = image.height as f32;
        let dw = (x1 - x0).max(1) as f32;
        let dh = (y1 - y0).max(1) as f32;

        for py in y0c..y1c {
            let v = ((py - y0) as f32 + 0.5) / dh;
            let sy = (v * sh - 0.5).clamp(0.0, sh - 1.0);
            let y0i = sy.floor() as u32;
            let y1i = (y0i + 1).min(image.height - 1);
            let fy = sy - y0i as f32;
            for px in x0c..x1c {
                let u = ((px - x0) as f32 + 0.5) / dw;
                let sx = (u * sw - 0.5).clamp(0.0, sw - 1.0);
                let x0i = sx.floor() as u32;
                let x1i = (x0i + 1).min(image.width - 1);
                let fx = sx - x0i as f32;

                let at = |ix: u32, iy: u32| -> (f32, f32, f32, f32) {
                    let i = ((iy * image.width + ix) * 4) as usize;
                    (
                        image.rgba[i] as f32,
                        image.rgba[i + 1] as f32,
                        image.rgba[i + 2] as f32,
                        image.rgba[i + 3] as f32,
                    )
                };
                let (r00, g00, b00, a00) = at(x0i, y0i);
                let (r10, g10, b10, a10) = at(x1i, y0i);
                let (r01, g01, b01, a01) = at(x0i, y1i);
                let (r11, g11, b11, a11) = at(x1i, y1i);
                let mix = |p00: f32, p10: f32, p01: f32, p11: f32| {
                    let top = p00 + (p10 - p00) * fx;
                    let bottom = p01 + (p11 - p01) * fx;
                    top + (bottom - top) * fy
                };
                let a = mix(a00, a10, a01, a11);
                if a < 1.0 {
                    continue;
                }
                let color = Color {
                    r: mix(r00, r10, r01, r11).round().clamp(0.0, 255.0) as u8,
                    g: mix(g00, g10, g01, g11).round().clamp(0.0, 255.0) as u8,
                    b: mix(b00, b10, b01, b11).round().clamp(0.0, 255.0) as u8,
                    a: a.round().clamp(0.0, 255.0) as u8,
                };
                self.blend_at(px, py, color, 255);
            }
        }
    }

    /// Draw an image through the current matrix, sampling bilinearly in
    /// layout space: every device pixel is mapped back into the image.
    fn draw_image_transformed(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        image: &naro_image::Image,
    ) {
        let d = self.device_matrix();
        let Some(inv) = d.inverse() else { return };
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        let x0 = x0.max(self.clip.0).max(0);
        let y0 = y0.max(self.clip.1).max(0);
        let x1 = x1.min(self.clip.2).min(self.width as i32);
        let y1 = y1.min(self.clip.3).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let sw = image.width as f32;
        let sh = image.height as f32;
        for py in y0..y1 {
            for px in x0..x1 {
                let (lx, ly) = inv.apply(px as f32 + 0.5, py as f32 + 0.5);
                if lx < x || ly < y || lx >= x + w || ly >= y + h {
                    continue;
                }
                let u = (lx - x) / w;
                let v = (ly - y) / h;
                let sx = (u * sw - 0.5).clamp(0.0, sw - 1.0);
                let sy = (v * sh - 0.5).clamp(0.0, sh - 1.0);
                let x0i = sx.floor() as u32;
                let y0i = sy.floor() as u32;
                let x1i = (x0i + 1).min(image.width - 1);
                let y1i = (y0i + 1).min(image.height - 1);
                let fx = sx - x0i as f32;
                let fy = sy - y0i as f32;
                let at = |ix: u32, iy: u32| -> (f32, f32, f32, f32) {
                    let i = ((iy * image.width + ix) * 4) as usize;
                    (
                        image.rgba[i] as f32,
                        image.rgba[i + 1] as f32,
                        image.rgba[i + 2] as f32,
                        image.rgba[i + 3] as f32,
                    )
                };
                let (r00, g00, b00, a00) = at(x0i, y0i);
                let (r10, g10, b10, a10) = at(x1i, y0i);
                let (r01, g01, b01, a01) = at(x0i, y1i);
                let (r11, g11, b11, a11) = at(x1i, y1i);
                let mix = |p00: f32, p10: f32, p01: f32, p11: f32| {
                    let top = p00 + (p10 - p00) * fx;
                    let bottom = p01 + (p11 - p01) * fx;
                    top + (bottom - top) * fy
                };
                let a = mix(a00, a10, a01, a11);
                if a < 1.0 {
                    continue;
                }
                let color = Color {
                    r: mix(r00, r10, r01, r11).round().clamp(0.0, 255.0) as u8,
                    g: mix(g00, g10, g01, g11).round().clamp(0.0, 255.0) as u8,
                    b: mix(b00, b10, b01, b11).round().clamp(0.0, 255.0) as u8,
                    a: a.round().clamp(0.0, 255.0) as u8,
                };
                self.blend_at(px, py, color, 255);
            }
        }
    }

    pub fn draw_text(
        &mut self,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        face: naro_text::FontFace<'_>,
        fonts: &mut FontSet,
    ) {
        if color.a == 0 || size <= 0.0 {
            return;
        }
        let scale = self.scale;
        let px = (size * scale).round().max(1.0);
        let mut pen_x = x * scale;
        let baseline_px = baseline * scale;
        let d = self.device_matrix();
        let transformed = !d.is_translation_scale();
        for ch in text.chars() {
            let glyph = fonts.glyph(ch, px, face);
            let m = glyph.metrics;
            if m.width > 0 && m.height > 0 {
                let gx = pen_x as i32 + m.xmin;
                let gy = baseline_px as i32 + m.ymin;
                let gw = m.width as i32;
                let gh = m.height as i32;
                if transformed {
                    // Map every coverage pixel through the matrix instead of
                    // blitting an offscreen buffer: exact for any affine,
                    // including a shear, and it allocates nothing. A rotated
                    // glyph loses its edge antialiasing (1x point sampling).
                    if d.inverse().is_some() {
                        for row in 0..gh {
                            for col in 0..gw {
                                let cov = glyph.bitmap[(row * gw + col) as usize];
                                if cov == 0 {
                                    continue;
                                }
                                let (dx, dy) = d.apply(
                                    (gx + col) as f32 + 0.5,
                                    (gy + row) as f32 + 0.5,
                                );
                                self.blend_at(
                                    dx.floor() as i32,
                                    dy.floor() as i32,
                                    color,
                                    cov,
                                );
                            }
                        }
                    }
                } else {
                    for row in 0..gh {
                        for col in 0..gw {
                            let cov = glyph.bitmap[(row * gw + col) as usize];
                            if cov != 0 {
                                self.blend_at(gx + col, gy + row, color, cov);
                            }
                        }
                    }
                }
            }
            pen_x += m.advance_width;
        }
    }

    pub fn text_width(
        &self,
        text: &str,
        size: f32,
        face: naro_text::FontFace<'_>,
        fonts: &mut FontSet,
    ) -> f32 {
        fonts.measure(text, size * self.scale, face) / self.scale
    }

    fn blend_at(&mut self, x: i32, y: i32, color: Color, coverage: u8) {
        if x < self.clip.0 || y < self.clip.1 || x >= self.clip.2 || y >= self.clip.3 {
            return;
        }
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let a = (coverage as u32 * color.a as u32 + 127) / 255;
        if a == 0 {
            return;
        }
        let idx = (y as usize) * (self.width as usize) + x as usize;
        if a >= 255 {
            self.pixels[idx] = pack(color);
            return;
        }
        let dst = self.pixels[idx];
        let inv = 255 - a;
        let r = (color.r as u32 * a + ((dst >> 16) & 0xFF) * inv + 127) / 255;
        let g = (color.g as u32 * a + ((dst >> 8) & 0xFF) * inv + 127) / 255;
        let b = (color.b as u32 * a + (dst & 0xFF) * inv + 127) / 255;
        self.pixels[idx] = (r << 16) | (g << 8) | b;
    }

    /// 24-bit bottom-up BMP (the format every viewer understands).
    pub fn to_bmp(&self) -> Vec<u8> {
        let row_bytes = (self.width as usize * 3 + 3) & !3usize;
        let image_size = row_bytes * self.height as usize;
        let file_size = 54 + image_size;
        let mut out = Vec::with_capacity(file_size);

        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(file_size as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(self.height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(image_size as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        for y in (0..self.height as usize).rev() {
            let row = &self.pixels[y * self.width as usize..(y + 1) * self.width as usize];
            for px in row {
                out.push((px & 0xFF) as u8);
                out.push(((px >> 8) & 0xFF) as u8);
                out.push(((px >> 16) & 0xFF) as u8);
            }
            for _ in self.width as usize * 3..row_bytes {
                out.push(0);
            }
        }
        out
    }
}

/// Fill the rows of the rounded rect `(x, y, w, h, r)` inside `mask` with
/// `value`. One span per row: the shape is convex, so each row is a single
/// contiguous run.
fn fill_rounded_span(
    mask: &mut [u8],
    mw: usize,
    mh: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    value: u8,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = r.max(0).min(w / 2).min(h / 2) as f32;
    let y0 = y.max(0);
    let y1 = (y + h).min(mh as i32);
    for py in y0..y1 {
        // Distance from the row centre to the nearest cap, 0 in the middle.
        let fy = py as f32 + 0.5;
        let dy = if fy < (y as f32 + r) {
            (y as f32 + r) - fy
        } else if fy > (y + h) as f32 - r {
            fy - ((y + h) as f32 - r)
        } else {
            0.0
        };
        let inset = if dy > 0.0 {
            r - (r * r - dy * dy).max(0.0).sqrt()
        } else {
            0.0
        };
        let x0 = (x as f32 + inset).round() as i32;
        let x1 = ((x + w) as f32 - inset).round() as i32;
        let x0 = x0.max(0);
        let x1 = x1.min(mw as i32);
        if x1 <= x0 {
            continue;
        }
        let row = py as usize * mw;
        mask[row + x0 as usize..row + x1 as usize].fill(value);
    }
}

/// `rounded_contains` for continuous layout coordinates.
fn rounded_contains_layout(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    let r = r.max(0.0).min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return true;
    }
    let cx = px.clamp(x + r, x + w - r);
    let cy = py.clamp(y + r, y + h - r);
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy <= r * r
}

/// Is `(px, py)` inside the rounded rect `(x, y, w, h, r)`?
fn rounded_contains(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    let r = r.max(0).min(w / 2).min(h / 2);
    if r == 0 {
        return true;
    }
    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
    let cx = if fx < (x + r) as f32 {
        (x + r) as f32
    } else if fx > (x + w - r) as f32 {
        (x + w - r) as f32
    } else {
        fx
    };
    let cy = if fy < (y + r) as f32 {
        (y + r) as f32
    } else if fy > (y + h - r) as f32 {
        (y + h - r) as f32
    } else {
        fy
    };
    let (dx, dy) = (fx - cx, fy - cy);
    dx * dx + dy * dy <= (r as f32) * (r as f32)
}

/// Separable box blur, horizontal pass, in place over a `u8` mask.
///
/// A running sum, so the cost is one add and one subtract per pixel whatever
/// the radius is. Summing the window per pixel made `blur: 100` cost 200 M
/// operations for a 300x300 box, which is 190 ms — the whole budget.
fn blur_horizontal(mask: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let window = (radius * 2 + 1) as u32;
    let mut out = vec![0u8; w];
    for y in 0..h {
        let base = y * w;
        // Seed the sum for x = 0: indices `-radius..=radius`, clamped to the
        // edges, so the mask does not darken at the border.
        let mut sum: u32 = mask[base] as u32 * (radius as u32 + 1);
        for i in 1..=radius.min(w - 1) {
            sum += mask[base + i] as u32;
        }
        for x in 0..w {
            out[x] = (sum / window) as u8;
            let drop = base + x.saturating_sub(radius);
            let add = base + (x + radius + 1).min(w - 1);
            sum = sum + mask[add] as u32 - mask[drop] as u32;
        }
        mask[base..base + w].copy_from_slice(&out);
    }
}

/// Separable box blur, vertical pass, in place over a `u8` mask.
///
/// Keeps one running sum per column and walks the rows in order, so both the
/// reads and the writes are sequential. Summing down a column per pixel made
/// the pass stride through memory `w` bytes at a time and dominated the cost.
fn blur_vertical(mask: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let window = (radius * 2 + 1) as u32;
    let mut sums = vec![0u32; w];
    let mut out = vec![0u8; w * h];
    // Seed the window for row 0: rows `-radius..=radius`, clamped to the edges.
    let seed = radius as u32 + 1;
    for x in 0..w {
        sums[x] = mask[x] as u32 * seed;
    }
    for i in 1..=radius.min(h - 1) {
        let row = i * w;
        for x in 0..w {
            sums[x] += mask[row + x] as u32;
        }
    }
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            out[row + x] = (sums[x] / window) as u8;
        }
        let drop = y.saturating_sub(radius) * w;
        let add = (y + radius + 1).min(h - 1) * w;
        for x in 0..w {
            sums[x] = sums[x] + mask[add + x] as u32 - mask[drop + x] as u32;
        }
    }
    mask.copy_from_slice(&out);
}

/// The colour of a gradient at position `t`, interpolating in sRGB.
fn sample_gradient(stops: &[(f32, Color)], t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t <= stops[0].0 {
        return stops[0].1;
    }
    let last = stops.len() - 1;
    if t >= stops[last].0 {
        return stops[last].1;
    }
    for pair in stops.windows(2) {
        let (p0, c0) = pair[0];
        let (p1, c1) = pair[1];
        if t >= p0 && t <= p1 {
            let span = (p1 - p0).max(1e-6);
            let k = (t - p0) / span;
            let mix = |a: u8, b: u8| -> u8 {
                (a as f32 + (b as f32 - a as f32) * k).round().clamp(0.0, 255.0) as u8
            };
            return Color {
                r: mix(c0.r, c1.r),
                g: mix(c0.g, c1.g),
                b: mix(c0.b, c1.b),
                a: mix(c0.a, c1.a),
            };
        }
    }
    stops[last].1
}

fn pack(c: Color) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use naro_css::{BoxShadow, Gradient, GradientKind};

    fn channel(pixel: u32, shift: u32) -> u8 {
        ((pixel >> shift) & 0xFF) as u8
    }

    #[test]
    fn gradient_runs_left_to_right() {
        let mut pixels = vec![0u32; 200 * 100];
        let mut canvas = Canvas::new(&mut pixels, 200, 100, 1.0);
        canvas.clear(Color::WHITE);
        let gradient = Gradient {
            kind: GradientKind::Linear { angle_deg: 90.0 },
            stops: vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
        };
        canvas.fill_gradient_rect(0.0, 0.0, 200.0, 100.0, &gradient);
        let row = 50 * 200;
        let mut previous = 0u8;
        for x in 0..200usize {
            let value = channel(canvas.pixels[row + x], 16);
            assert!(value >= previous, "canal decrescente em x={x}: {value} < {previous}");
            previous = value;
        }
        // Midpoint is roughly half way, and the ends are the stop colours.
        assert!((channel(canvas.pixels[row + 100], 16) as i32 - 128).abs() <= 3);
        // The first and last pixel sample at half a pixel inside the box, so
        // they are within one level of the stop, not exactly on it.
        assert!(channel(canvas.pixels[row], 16) <= 1);
        assert!(channel(canvas.pixels[row + 199], 16) >= 254);
    }

    #[test]
    fn shadow_darkens_outside_and_leaves_the_box_alone() {
        let mut pixels = vec![0u32; 200 * 200];
        let mut canvas = Canvas::new(&mut pixels, 200, 200, 1.0);
        canvas.clear(Color::WHITE);
        let shadow = BoxShadow {
            dx: 0.0,
            dy: 0.0,
            blur: 8.0,
            spread: 4.0,
            color: Color { r: 0, g: 0, b: 0, a: 255 },
            inset: false,
        };
        canvas.fill_shadow(50.0, 50.0, 100.0, 100.0, 0.0, &shadow);
        let at = |x: usize, y: usize| canvas.pixels[y * 200 + x];
        // Outside the box, inside the spread + blur tail: darkened.
        assert_ne!(at(46, 100), 0x00FF_FFFF, "a sombra não escureceu fora da caixa");
        // Far away: untouched.
        assert_eq!(at(5, 100), 0x00FF_FFFF);
        // Inside the box the mask is solid black, which is what a spread-only
        // shadow does — the element's own background is painted over it later.
        assert_eq!(at(100, 100), 0x0000_0000);
        assert_eq!(canvas.shadows_painted, 1);
    }

    #[test]
    fn zero_blur_zero_spread_takes_the_fast_path() {
        // Byte-identical to a plain offset `fill_round_rect`, which proves the
        // common case allocates no mask.
        let shadow = BoxShadow {
            dx: 6.0,
            dy: 4.0,
            blur: 0.0,
            spread: 0.0,
            color: Color { r: 0, g: 0, b: 0, a: 255 },
            inset: false,
        };
        let mut fast = vec![0u32; 200 * 200];
        let mut fast_canvas = Canvas::new(&mut fast, 200, 200, 1.0);
        fast_canvas.clear(Color::WHITE);
        fast_canvas.fill_shadow(50.0, 50.0, 60.0, 40.0, 8.0, &shadow);
        assert_eq!(fast_canvas.shadows_painted, 0);

        let mut slow = vec![0u32; 200 * 200];
        let mut slow_canvas = Canvas::new(&mut slow, 200, 200, 1.0);
        slow_canvas.clear(Color::WHITE);
        slow_canvas.fill_round_rect(56.0, 54.0, 60.0, 40.0, 8.0, shadow.color);

        assert_eq!(fast_canvas.pixels, slow_canvas.pixels);
    }

    #[test]
    fn clamp_shadow_respects_the_total_allocation() {
        // A blur-dominated shadow: the formula is what the clamp bounds.
        let (w, h, blur, spread, scale) = (300.0, 300.0, 5000.0, 50.0, 1.0);
        let (blur2, spread2) = clamp_shadow(w, h, blur, spread, scale);
        assert!(blur2 <= blur && spread2 <= spread);
        assert!(
            shadow_mask_bytes(w, h, blur2, spread2, scale) <= MAX_SHADOW_PIXELS,
            "máscara acima do teto: {}",
            shadow_mask_bytes(w, h, blur2, spread2, scale)
        );
        // Unclamped, the same box would blow past the ceiling.
        assert!(shadow_mask_bytes(w, h, blur, spread, scale) > MAX_SHADOW_PIXELS);
    }

    #[test]
    fn clamp_shadow_never_grows_and_tightens_with_scale() {
        assert_eq!(clamp_shadow(10.0, 10.0, 0.0, 0.0, 1.0), (0.0, 0.0));
        let one = clamp_shadow(300.0, 300.0, 1000.0, 100.0, 1.0);
        let two = clamp_shadow(300.0, 300.0, 1000.0, 100.0, 2.0);
        assert!(one.0 < 1000.0, "escala 1 também limita: {one:?}");
        assert!(two.0 < one.0, "escala 2 deve limitar mais: {two:?} vs {one:?}");
        assert!(two.1 < one.1);
    }

    #[test]
    fn huge_shadow_stays_within_budget() {
        let (w, h, blur, spread) = (5000.0, 5000.0, 500.0, 200.0);
        // The clamp bounds the blur tail; the mask itself is cropped to the
        // clip, so the allocation is the visible area and not the box.
        let (blur2, spread2) = clamp_shadow(w, h, blur, spread, 1.0);
        assert!(blur2 <= blur && spread2 <= spread);
        assert!(shadow_mask_bytes(400.0, 300.0, blur2, spread2, 1.0) <= MAX_SHADOW_PIXELS);
        // And the paint itself completes without exhausting memory.
        let mut pixels = vec![0u32; 400 * 300];
        let mut canvas = Canvas::new(&mut pixels, 400, 300, 1.0);
        canvas.clear(Color::WHITE);
        let shadow = BoxShadow {
            dx: 0.0,
            dy: 0.0,
            blur,
            spread,
            color: Color { r: 0, g: 0, b: 0, a: 200 },
            inset: false,
        };
        canvas.fill_shadow(10.0, 10.0, w, h, 0.0, &shadow);
        assert_eq!(canvas.shadows_painted, 1);
    }

    fn canvas_at(w: u32, h: u32) -> (Vec<u32>, Canvas<'static>) {
        let pixels: &'static mut Vec<u32> = Box::leak(Box::new(vec![0u32; (w * h) as usize]));
        let mut canvas = Canvas::new(pixels, w, h, 1.0);
        canvas.clear(Color::WHITE);
        (Vec::new(), canvas)
    }

    #[test]
    fn translation_moves_the_paint_not_the_layout() {
        let (_, mut canvas) = canvas_at(400, 100);
        canvas.push_transform(naro_css::Affine::translate(200.0, 0.0));
        canvas.fill_rect(0.0, 0.0, 40.0, 20.0, Color::rgb(255, 0, 0));
        canvas.pop_transform();
        let at = |x: usize, y: usize| canvas.pixels[y * 400 + x] & 0xFFFFFF;
        assert_eq!(at(210, 10), 0xFF0000, "pintou no destino");
        assert_eq!(at(10, 10), 0xFFFFFF, "não pintou na posição de layout");
    }

    #[test]
    fn rotation_around_the_centre_keeps_the_centre() {
        let (_, mut canvas) = canvas_at(200, 200);
        // Rotate 90° about (20, 10): the box (0,0,40,20) becomes x 10..30,
        // y -10..30 — shifted into the canvas here by the 100px offset.
        let m = naro_css::Affine::translate(100.0, 100.0)
            .multiply(naro_css::Affine::translate(20.0, 10.0))
            .multiply(naro_css::Affine::rotate(90.0))
            .multiply(naro_css::Affine::translate(-20.0, -10.0));
        canvas.push_transform(m);
        canvas.fill_rect(0.0, 0.0, 40.0, 20.0, Color::rgb(0, 0, 255));
        canvas.pop_transform();
        let at = |x: usize, y: usize| canvas.pixels[y * 200 + x] & 0xFFFFFF;
        assert_eq!(at(110, 110), 0x0000FF, "centro girado");
        assert_eq!(at(110, 95), 0x0000FF, "dentro da área girada");
        assert_eq!(at(95, 110), 0xFFFFFF, "fora da área girada");
    }

    #[test]
    fn singular_transform_paints_nothing() {
        let (_, mut canvas) = canvas_at(50, 50);
        canvas.push_transform(naro_css::Affine::scale(0.0, 0.0));
        canvas.fill_rect(0.0, 0.0, 40.0, 20.0, Color::rgb(255, 0, 0));
        canvas.pop_transform();
        assert!(canvas.pixels.iter().all(|p| *p == 0x00FF_FFFF));
    }

    #[test]
    fn clip_follows_the_rotation() {
        let (_, mut canvas) = canvas_at(200, 200);
        let m = naro_css::Affine::translate(100.0, 100.0)
            .multiply(naro_css::Affine::rotate(90.0));
        canvas.push_transform(m);
        canvas.set_clip(Rect { x: -20.0, y: -20.0, w: 40.0, h: 40.0 });
        canvas.fill_rect(-50.0, -50.0, 100.0, 100.0, Color::rgb(255, 0, 0));
        let at = |x: usize, y: usize| canvas.pixels[y * 200 + x] & 0xFFFFFF;
        // The rotated clip is a 40x40 box around (100,100).
        assert_eq!(at(100, 100), 0xFF0000);
        assert_eq!(at(20, 20), 0xFFFFFF, "fora do bbox girado");
        canvas.pop_transform();
    }

    #[test]
    fn scale_is_applied_once() {
        // HiDPI: `translate(10px)` is 10 CSS pixels, so 20 device pixels.
        let pixels: &'static mut Vec<u32> = Box::leak(Box::new(vec![0u32; 200 * 100]));
        let mut canvas = Canvas::new(pixels, 200, 100, 2.0);
        canvas.clear(Color::WHITE);
        canvas.push_transform(naro_css::Affine::translate(10.0, 0.0));
        canvas.fill_rect(0.0, 0.0, 10.0, 10.0, Color::rgb(255, 0, 0));
        canvas.pop_transform();
        let at = |x: usize, y: usize| canvas.pixels[y * 200 + x] & 0xFFFFFF;
        assert_eq!(at(25, 5), 0xFF0000, "20 px de dispositivo");
        assert_eq!(at(5, 5), 0xFFFFFF);
        assert_eq!(at(45, 5), 0xFFFFFF, "não 40 px");
    }
}
