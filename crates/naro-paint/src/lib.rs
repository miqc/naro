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
        }
    }

    pub fn clear(&mut self, color: Color) {
        let px = pack(color);
        self.pixels.iter_mut().for_each(|p| *p = px);
    }

    pub fn set_clip(&mut self, rect: Rect) {
        let (x0, y0, x1, y1) = self.device_rect(rect.x, rect.y, rect.w, rect.h);
        self.clip = (
            x0.max(0),
            y0.max(0),
            x1.min(self.width as i32),
            y1.min(self.height as i32),
        );
    }

    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, self.width as i32, self.height as i32);
    }

    fn device_rect(&self, x: f32, y: f32, w: f32, h: f32) -> (i32, i32, i32, i32) {
        let s = self.scale;
        (
            (x * s).floor() as i32,
            (y * s).floor() as i32,
            ((x + w) * s).ceil() as i32,
            ((y + h) * s).ceil() as i32,
        )
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || color.a == 0 {
            return;
        }
        let (x0, y0, x1, y1) = self.device_rect(x, y, w, h);
        self.fill_device(x0, y0, x1, y1, color);
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

    pub fn draw_image(&mut self, x: f32, y: f32, w: f32, h: f32, image: &naro_image::Image) {
        if w <= 0.0 || h <= 0.0 || image.width == 0 || image.height == 0 {
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

    pub fn draw_text(
        &mut self,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        fonts: &mut FontSet,
    ) {
        if color.a == 0 || size <= 0.0 {
            return;
        }
        let scale = self.scale;
        let px = (size * scale).round().max(1.0);
        let mut pen_x = x * scale;
        let baseline_px = baseline * scale;
        for ch in text.chars() {
            let glyph = fonts.glyph(ch, px, bold);
            let m = glyph.metrics;
            if m.width > 0 && m.height > 0 {
                let gx = pen_x as i32 + m.xmin;
                let gy = baseline_px as i32 + m.ymin;
                let gw = m.width as i32;
                let gh = m.height as i32;
                for row in 0..gh {
                    for col in 0..gw {
                        let cov = glyph.bitmap[(row * gw + col) as usize];
                        if cov != 0 {
                            self.blend_at(gx + col, gy + row, color, cov);
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
        bold: bool,
        fonts: &mut FontSet,
    ) -> f32 {
        fonts.measure(text, size * self.scale, bold) / self.scale
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

fn pack(c: Color) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)
}
