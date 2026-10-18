//! Font loading, text measurement and glyph rasterization.
//!
//! Built on `ab_glyph`, which keeps the font file and builds a glyph outline
//! only when that glyph is rasterized. `fontdue` instead materializes the
//! outline of *every* glyph in the face up front: for Segoe UI (3.952 glyphs)
//! that is ~18.7 MB of heap per face, which dominated NARO's whole footprint.

use ab_glyph::{Font, FontVec, Glyph, PxScale, ScaleFont};
use std::cell::RefCell;
use std::collections::HashMap;

pub trait TextMeasure {
    fn measure(&self, text: &str, size_px: f32, bold: bool) -> f32;
    /// (ascent, descent), both positive, in pixels.
    fn line_metrics(&self, size_px: f32, bold: bool) -> (f32, f32);
}

const REGULAR_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
];

/// Fallback faces for codepoints the primary font lacks (CJK, symbols).
/// Segoe UI covers Latin/Greek/Cyrillic but not CJK, which used to render as
/// empty boxes on real pages.
const FALLBACK_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\seguisym.ttf",
    r"C:\Windows\Fonts\msyh.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
];

const BOLD_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\segoeuib.ttf",
    r"C:\Windows\Fonts\arialbd.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
];

/// Hard cap on cached glyph bitmaps; the cache is cleared when exceeded so a
/// pathological page cannot grow it without bound.
const GLYPH_CACHE_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphMetrics {
    /// Bitmap left edge, relative to the pen position.
    pub xmin: i32,
    /// Bitmap top edge, relative to the baseline (negative above it).
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub advance_width: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RasterGlyph {
    pub metrics: GlyphMetrics,
    pub bitmap: Vec<u8>,
}

pub struct FontSet {
    regular: FontVec,
    bold: Option<FontVec>,
    /// Fallback faces, loaded the first time a glyph is missing from the
    /// primary face. The CJK candidate on Windows is `msyh.ttc`, ~20 MB
    /// resident and slow to parse; a page that never shows a CJK codepoint
    /// must not pay for it.
    fallbacks: RefCell<Vec<FontVec>>,
    /// Candidate paths already opened (or attempted), so a missing glyph does
    /// not re-read the same files on every call.
    tried_fallbacks: RefCell<Vec<&'static str>>,
    name: String,
    cache: HashMap<(u32, u32, bool), RasterGlyph>,
    /// Advance widths, memoized: `h_advance` re-resolves the glyph and applies
    /// scaling on every call, which dominated layout on text-heavy pages.
    advances: RefCell<HashMap<(u32, u32, bool), f32>>,
}

fn load_first(paths: &[&str]) -> Option<(FontVec, String)> {
    for path in paths {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        if let Ok(font) = FontVec::try_from_vec(data) {
            return Some((font, (*path).to_string()));
        }
    }
    None
}

impl FontSet {
    pub fn load_system() -> Result<FontSet, String> {
        let (regular, name) = load_first(REGULAR_CANDIDATES)
            .ok_or_else(|| "nenhuma fonte do sistema encontrada".to_string())?;
        let bold = load_first(BOLD_CANDIDATES).map(|(font, _)| font);
        Ok(FontSet {
            regular,
            bold,
            fallbacks: RefCell::new(Vec::new()),
            tried_fallbacks: RefCell::new(Vec::new()),
            name,
            cache: HashMap::new(),
            advances: RefCell::new(HashMap::new()),
        })
    }

    pub fn font_name(&self) -> &str {
        &self.name
    }

    fn font(&self, bold: bool) -> &FontVec {
        match (bold, &self.bold) {
            (true, Some(font)) => font,
            _ => &self.regular,
        }
    }

    /// Load fallback faces until one covers `ch`, or every candidate has been
    /// tried. Called only when the primary face has no glyph for `ch`.
    fn load_fallback_for(&self, ch: char) {
        if self.fallbacks.borrow().iter().any(|font| font.glyph_id(ch).0 != 0) {
            return;
        }
        for path in FALLBACK_CANDIDATES {
            if self.tried_fallbacks.borrow().contains(path) {
                continue;
            }
            self.tried_fallbacks.borrow_mut().push(path);
            let Ok(data) = std::fs::read(path) else { continue };
            let Ok(font) = FontVec::try_from_vec(data) else { continue };
            let covers = font.glyph_id(ch).0 != 0;
            // Kept even when it does not cover `ch`: the next missing glyph
            // would otherwise re-read and re-parse the same file.
            self.fallbacks.borrow_mut().push(font);
            if covers {
                return;
            }
        }
    }

    /// Run `f` with the first face that has a glyph for `ch`, falling back to
    /// the primary face so missing codepoints still get the `.notdef` box.
    ///
    /// A closure rather than a returned reference: the fallback list is behind
    /// a `RefCell` because it loads on demand.
    fn with_font_for<R>(&self, ch: char, bold: bool, f: impl FnOnce(&FontVec) -> R) -> R {
        let primary = self.font(bold);
        if primary.glyph_id(ch).0 == 0 {
            self.load_fallback_for(ch);
            let fallbacks = self.fallbacks.borrow();
            if let Some(font) = fallbacks.iter().find(|font| font.glyph_id(ch).0 != 0) {
                return f(font);
            }
        }
        f(primary)
    }

    /// Rasterize on demand. The returned glyph is owned by the cache.
    pub fn glyph(&mut self, ch: char, px: f32, bold: bool) -> RasterGlyph {
        let px_key = px.round().max(1.0) as u32;
        let key = (ch as u32, px_key, bold);
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let glyph = self.rasterize(ch, px_key as f32, bold);
        if self.cache.len() >= GLYPH_CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(key, glyph.clone());
        glyph
    }

    fn rasterize(&self, ch: char, px: f32, bold: bool) -> RasterGlyph {
        self.with_font_for(ch, bold, |font| Self::rasterize_with(font, ch, px))
    }

    fn rasterize_with(font: &FontVec, ch: char, px: f32) -> RasterGlyph {
        let scaled = font.as_scaled(px);
        let glyph_id = font.glyph_id(ch);
        let advance_width = scaled.h_advance(glyph_id);

        let glyph: Glyph = glyph_id.with_scale_and_position(PxScale::from(px), ab_glyph::point(0.0, 0.0));
        let outline = match font.outline_glyph(glyph) {
            Some(o) => o,
            None => {
                return RasterGlyph {
                    metrics: GlyphMetrics { advance_width, ..Default::default() },
                    bitmap: Vec::new(),
                }
            }
        };
        let bounds = outline.px_bounds();
        let x0 = bounds.min.x.floor() as i32;
        let y0 = bounds.min.y.floor() as i32;
        let x1 = bounds.max.x.ceil() as i32;
        let y1 = bounds.max.y.ceil() as i32;
        let width = (x1 - x0).max(0) as usize;
        let height = (y1 - y0).max(0) as usize;
        let mut bitmap = vec![0u8; width * height];
        if width > 0 && height > 0 {
            outline.draw(|x, y, coverage| {
                let (x, y) = (x as usize, y as usize);
                if x < width && y < height {
                    bitmap[y * width + x] = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            });
        }
        RasterGlyph {
            metrics: GlyphMetrics {
                xmin: x0,
                ymin: y0,
                width,
                height,
                advance_width,
            },
            bitmap,
        }
    }

    pub fn advance(&mut self, ch: char, px: f32, bold: bool) -> f32 {
        self.measure(&ch.to_string(), px, bold)
    }
}

impl TextMeasure for FontSet {
    fn measure(&self, text: &str, size_px: f32, bold: bool) -> f32 {
        let px_key = size_px.round().max(1.0) as u32;
        let mut cache = self.advances.borrow_mut();
        let mut total = 0.0;
        for ch in text.chars() {
            let key = (ch as u32, px_key, bold);
            total += match cache.get(&key) {
                Some(a) => *a,
                None => {
                    let a = self.with_font_for(ch, bold, |font| {
                        font.as_scaled(px_key as f32).h_advance(font.glyph_id(ch))
                    });
                    cache.insert(key, a);
                    a
                }
            };
        }
        total
    }

    fn line_metrics(&self, size_px: f32, bold: bool) -> (f32, f32) {
        let font = self.font(bold);
        let scaled = font.as_scaled(size_px);
        (scaled.ascent(), -scaled.descent())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_monotonically() {
        let fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let short = fonts.measure("ab", 16.0, false);
        let long = fonts.measure("abcd", 16.0, false);
        assert!(long > short);
        let (ascent, descent) = fonts.line_metrics(16.0, false);
        assert!(ascent > 0.0 && descent >= 0.0);
    }

    #[test]
    fn rasterizes_ascii_and_accents() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        for ch in ['A', 'ç', 'ã', '→'] {
            let g = fonts.glyph(ch, 16.0, false);
            assert!(g.metrics.advance_width > 0.0, "advance for {ch}");
            assert_eq!(g.bitmap.len(), g.metrics.width * g.metrics.height);
        }
    }

    #[test]
    fn cache_is_bounded() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        for i in 0..(GLYPH_CACHE_LIMIT + 50) {
            let ch = char::from_u32(0x20 + (i as u32 % 90)).unwrap();
            fonts.glyph(ch, 10.0 + (i % 7) as f32, false);
        }
        assert!(fonts.cache.len() <= GLYPH_CACHE_LIMIT);
    }
}
