//! Font loading, text measurement and glyph rasterization.
//!
//! Built on `ab_glyph`, which keeps the font file and builds a glyph outline
//! only when that glyph is rasterized. `fontdue` instead materializes the
//! outline of *every* glyph in the face up front: for Segoe UI (3.952 glyphs)
//! that is ~18.7 MB of heap per face, which dominated NARO's whole footprint.

use ab_glyph::{Font, FontVec, Glyph, PxScale, ScaleFont};
use std::cell::RefCell;
use std::collections::HashMap;

/// One requested face: the `font-family` list as written, plus weight and
/// slant. Resolution against the registered families happens inside
/// [`FontSet`]; callers only describe what the cascade asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFace<'a> {
    /// Families as written in `font-family`, in cascade order. `""` means the
    /// system default.
    pub family: &'a str,
    pub weight: u16,
    pub italic: bool,
}

impl<'a> FontFace<'a> {
    pub const fn new(family: &'a str, weight: u16, italic: bool) -> Self {
        FontFace { family, weight, italic }
    }
}

/// `derive(Default)` would give `weight: 0`, which is not a valid CSS weight
/// and would make `resolve` look for the face closest to zero — the regular
/// one. The default has to be written out by hand to be 400.
impl<'a> Default for FontFace<'a> {
    fn default() -> Self {
        FontFace { family: "", weight: 400, italic: false }
    }
}

pub trait TextMeasure {
    fn measure(&self, text: &str, size_px: f32, face: FontFace<'_>) -> f32;
    /// (ascent, descent), both positive, in pixels.
    fn line_metrics(&self, size_px: f32, face: FontFace<'_>) -> (f32, f32);
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

const SERIF_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\times.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
];

const MONO_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
];

/// Hard cap on cached glyph bitmaps; the cache is cleared when exceeded so a
/// pathological page cannot grow it without bound.
const GLYPH_CACHE_LIMIT: usize = 4096;

/// Registered faces per family. A family normally has two (regular + bold).
const MAX_FACES_PER_FAMILY: usize = 8;
/// Registered families, system ones included. Bounds both the lookup table and
/// the per-document cost of a page that declares hundreds of `@font-face`.
const MAX_FAMILIES: usize = 16;
/// Extra advance a synthesized bold adds, as a fraction of the em size. One
/// pixel of dilation at 16 px is 1/16 of the em, and half of that reads as
/// bold without the text becoming wider than the design expects.
const SYNTHETIC_BOLD_ADVANCE: f32 = 0.03;
/// Italic shear: horizontal run per vertical unit. The CSS oblique default is
/// 14°, whose tangent is 0.249; 0.21 keeps synthesized italics a touch more
/// upright, which reads better at UI sizes.
const SYNTHETIC_ITALIC_SHEAR: f32 = 0.21;

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

/// One loaded font file. Shared between families: `sans-serif`, `system-ui` and
/// the file's own name all point at the same two faces instead of parsing
/// Segoe UI three times.
struct Face {
    font: FontVec,
    weight: u16,
    italic: bool,
    /// System path or URL it came from, for diagnostics and for deduplicating
    /// a re-registration of the same file.
    source: String,
}

/// A named family: face ids into [`FontSet::faces`], so one file can back
/// several families without a second copy.
struct Family {
    faces: Vec<u16>,
}

pub struct FontSet {
    faces: Vec<Face>,
    families: Vec<Family>,
    /// Key: family name, lowercased. The value is an index into `families`.
    family_ids: HashMap<String, u16>,
    /// Family used when nothing matches; always `"sans-serif"`.
    default_family: String,
    /// Fallback faces, loaded the first time a glyph is missing from the
    /// primary face. The CJK candidate on Windows is `msyh.ttc`, ~20 MB
    /// resident and slow to parse; a page that never shows a CJK codepoint
    /// must not pay for it.
    fallbacks: RefCell<Vec<FontVec>>,
    /// Candidate paths already opened (or attempted), so a missing glyph does
    /// not re-read the same files on every call.
    tried_fallbacks: RefCell<Vec<&'static str>>,
    name: String,
    /// Last request resolved, with its family index and the chosen face. Layout
    /// calls `measure` once per word with the same style, and resolution scans
    /// the registered families and their faces; memoizing the last answer
    /// removes both scans from the hot path without hashing anything.
    last_face: RefCell<Option<(String, u16, bool, u16, bool, bool)>>,
    /// Key: (char, px, family, weight, italic). The family index makes the key
    /// exact — no hashing of family names, no collision between two families
    /// that differ only in case.
    cache: HashMap<(u32, u32, u16, u16, bool), RasterGlyph>,
    /// Advance widths, memoized: `h_advance` re-resolves the glyph and applies
    /// scaling on every call, which dominated layout on text-heavy pages.
    advances: RefCell<HashMap<(u32, u32, u16, u16, bool), f32>>,
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

/// `C:\Windows\Fonts\segoeui.ttf` -> `segoeui`, `DejaVuSans-Bold.ttf` ->
/// `dejavusans bold`. The CSS name of a system font is not in the file name,
/// so this is the best guess available without reading the `name` table.
fn family_name_from_path(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("system");
    stem.to_lowercase().replace(['-', '_'], " ")
}

/// Iterator over a `font-family` list, split on commas that are not inside
/// quotes. Borrows the input: `resolve` runs on every measurement, so it must
/// not allocate.
fn families(list: &str) -> impl Iterator<Item = &str> {
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut parts: Vec<(usize, usize)> = Vec::new();
    for (i, c) in list.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ',' => {
                parts.push((start, i));
                start = i + c.len_utf8();
            }
            None => {}
        }
    }
    parts.push((start, list.len()));
    parts.into_iter().map(move |(a, b)| &list[a..b])
}

/// One family name as written: unquoted, trimmed.
fn clean_family(name: &str) -> &str {
    name.trim().trim_matches(|c| c == '"' || c == '\'').trim()
}

/// Lowercase, unquoted, trimmed family name as stored in `family_ids`.
fn normalize_family(name: &str) -> String {
    name.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_lowercase()
}

/// Generic CSS families that alias another registered key.
fn generic_alias(name: &str) -> Option<&'static str> {
    match name {
        "ui-sans-serif" => Some("sans-serif"),
        "ui-serif" => Some("serif"),
        "ui-monospace" => Some("monospace"),
        _ => None,
    }
}

struct ResolvedFace<'a> {
    font: &'a FontVec,
    synthetic_bold: bool,
    synthetic_italic: bool,
    /// Family index, part of the glyph/advance cache key.
    family: u16,
}

impl FontSet {
    pub fn load_system() -> Result<FontSet, String> {
        let (regular, path) = load_first(REGULAR_CANDIDATES)
            .ok_or_else(|| "nenhuma fonte do sistema encontrada".to_string())?;
        let bold = load_first(BOLD_CANDIDATES);
        let derived = family_name_from_path(&path);

        let mut set = FontSet {
            faces: Vec::new(),
            families: Vec::new(),
            family_ids: HashMap::new(),
            default_family: "sans-serif".to_string(),
            fallbacks: RefCell::new(Vec::new()),
            tried_fallbacks: RefCell::new(Vec::new()),
            name: derived.clone(),
            last_face: RefCell::new(None),
            cache: HashMap::new(),
            advances: RefCell::new(HashMap::new()),
        };

        let regular_id = set.push_face(Face {
            font: regular,
            weight: 400,
            italic: false,
            source: path,
        });
        let bold_id = bold.map(|(font, path)| {
            set.push_face(Face { font, weight: 700, italic: false, source: path })
        });
        let mut system_faces = vec![regular_id];
        if let Some(id) = bold_id {
            system_faces.push(id);
        }
        for name in ["sans-serif", "system-ui", derived.as_str()] {
            set.push_family(name, &system_faces);
        }

        // Generic families the page can name. Only the regular file is loaded;
        // a bold request is synthesized (A4.3), because neither candidate list
        // carries a bold file.
        for (name, candidates) in [("serif", SERIF_CANDIDATES), ("monospace", MONO_CANDIDATES)] {
            let id = match load_first(candidates) {
                Some((font, path)) => {
                    set.push_face(Face { font, weight: 400, italic: false, source: path })
                }
                // A generic family is never absent: without a dedicated file it
                // is the system regular under that name.
                None => regular_id,
            };
            set.push_family(name, &[id]);
        }
        Ok(set)
    }

    fn push_face(&mut self, face: Face) -> u16 {
        self.faces.push(face);
        (self.faces.len() - 1) as u16
    }

    /// Register `name` as a family, or add the faces to an existing one.
    fn push_family(&mut self, name: &str, faces: &[u16]) {
        let key = normalize_family(name);
        let idx = match self.family_ids.get(&key) {
            Some(&i) => i as usize,
            None => {
                self.families.push(Family { faces: Vec::new() });
                let i = (self.families.len() - 1) as u16;
                self.family_ids.insert(key, i);
                i as usize
            }
        };
        let family = &mut self.families[idx];
        for id in faces {
            if !family.faces.contains(id) {
                family.faces.push(*id);
            }
        }
    }

    /// Register a face that came from the network. Accepts TTF and OTF.
    ///
    /// WOFF2 is rejected: the `woff2` crate (0.3.0, the only release) does not
    /// compile against rustc 1.99 — its `SafeBuf::try_get_u8` calls now collide
    /// with `std::io::Buf`'s. See `## Bloqueios` in `reports/ROADMAP-STATE.md`.
    pub fn register_web_family(
        &mut self,
        family: &str,
        weight: u16,
        italic: bool,
        bytes: Vec<u8>,
        source: &str,
    ) -> Result<(), String> {
        let key = normalize_family(family);
        if key.is_empty() {
            return Err("família vazia".to_string());
        }
        // Same file, same weight and slant: one face, shared by every family
        // that names it. A second `@font-face` with the same `src` for another
        // family therefore costs no extra memory.
        let existing = self
            .faces
            .iter()
            .position(|f| f.weight == weight && f.italic == italic && f.source == source);
        let face_id = match existing {
            Some(i) => i as u16,
            None => {
                if bytes.starts_with(b"wOF2") {
                    return Err("WOFF2 não suportado nesta build".to_string());
                }
                let font = FontVec::try_from_vec(bytes)
                    .map_err(|e| format!("fonte inválida: {e}"))?;
                if self.faces.len() >= u16::MAX as usize {
                    return Err("limite de faces".to_string());
                }
                self.push_face(Face {
                    font,
                    weight,
                    italic,
                    source: source.to_string(),
                })
            }
        };

        let known = self.family_ids.contains_key(&key);
        if !known && self.families.len() >= MAX_FAMILIES {
            return Err("limite de famílias".to_string());
        }
        if let Some(&idx) = self.family_ids.get(&key) {
            let family = &self.families[idx as usize];
            if !family.faces.contains(&face_id) && family.faces.len() >= MAX_FACES_PER_FAMILY {
                return Err("limite de faces".to_string());
            }
        }
        self.push_family(&key, &[face_id]);
        self.cache.clear();
        self.advances.borrow_mut().clear();
        *self.last_face.borrow_mut() = None;
        Ok(())
    }

    pub fn font_name(&self) -> &str {
        &self.name
    }

    /// True when `family` is registered (system or web).
    pub fn has_family(&self, family: &str) -> bool {
        self.family_ids.contains_key(&normalize_family(family))
    }

    fn default_id(&self) -> u16 {
        self.family_ids.get(&self.default_family).copied().unwrap_or(0)
    }

    /// Index of a registered family, compared without allocating.
    fn find_family(&self, name: &str) -> Option<u16> {
        let name = clean_family(name);
        if name.is_empty() {
            return None;
        }
        for (key, &id) in &self.family_ids {
            if key.eq_ignore_ascii_case(name) {
                return Some(id);
            }
        }
        // `ui-sans-serif` and friends name another generic.
        if let Some(alias) = generic_alias(&name.to_ascii_lowercase()) {
            return self.family_ids.get(alias).copied();
        }
        None
    }

    /// First family of the list that is registered, or the default family.
    fn pick_family(&self, family: &str) -> u16 {
        if family.trim().is_empty() {
            return self.default_id();
        }
        for part in families(family) {
            if let Some(id) = self.find_family(part) {
                return id;
            }
        }
        self.default_id()
    }

    /// Exact (weight, italic), then same weight with the other slant, then the
    /// closest weight (ties to the lighter face).
    fn best_face(faces: &[u16], pool: &[Face], weight: u16, italic: bool) -> Option<u16> {
        if let Some(&id) = faces
            .iter()
            .find(|&&id| pool[id as usize].weight == weight && pool[id as usize].italic == italic)
        {
            return Some(id);
        }
        if let Some(&id) = faces
            .iter()
            .find(|&&id| pool[id as usize].weight == weight && pool[id as usize].italic != italic)
        {
            return Some(id);
        }
        let mut best: Option<(u16, (u16, u8, u16))> = None;
        for &id in faces {
            let face = &pool[id as usize];
            let key = (
                face.weight.abs_diff(weight),
                if face.italic == italic { 0 } else { 1 },
                face.weight,
            );
            if best.is_none_or(|(_, best_key)| key < best_key) {
                best = Some((id, key));
            }
        }
        best.map(|(id, _)| id)
    }

    fn resolve(&self, face: FontFace<'_>) -> ResolvedFace<'_> {
        // The borrow must end before the miss path writes, hence the block.
        let cached = {
            let last = self.last_face.borrow();
            match last.as_ref() {
                Some((name, weight, italic, family, bold, slanted))
                    if name == face.family && *weight == face.weight && *italic == face.italic =>
                {
                    Some((*family, *bold, *slanted))
                }
                _ => None,
            }
        };
        let (family_id, synthetic_bold, synthetic_italic) = match cached {
            Some(hit) => hit,
            None => {
                let family_id = self.pick_family(face.family);
                let family = &self.families[family_id as usize];
                let chosen = Self::best_face(
                    &family.faces,
                    &self.faces,
                    face.weight,
                    face.italic,
                )
                .map(|id| &self.faces[id as usize])
                .unwrap_or(&self.faces[0]);
                let bold = face.weight >= 600 && chosen.weight < 600;
                let slanted = face.italic && !chosen.italic;
                *self.last_face.borrow_mut() = Some((
                    face.family.to_string(),
                    face.weight,
                    face.italic,
                    family_id,
                    bold,
                    slanted,
                ));
                (family_id, bold, slanted)
            }
        };
        // The chosen face is what the cache hit skipped, so resolve it here
        // once: `best_face` on a two-face family is two comparisons.
        let family = &self.families[family_id as usize];
        let chosen = Self::best_face(&family.faces, &self.faces, face.weight, face.italic)
            .map(|id| &self.faces[id as usize])
            .unwrap_or(&self.faces[0]);
        ResolvedFace {
            font: &chosen.font,
            synthetic_bold,
            synthetic_italic,
            family: family_id,
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
    /// the resolved face so missing codepoints still get the `.notdef` box.
    ///
    /// A closure rather than a returned reference: the fallback list is behind
    /// a `RefCell` because it loads on demand.
    fn with_font_for<R>(&self, ch: char, primary: &FontVec, f: impl FnOnce(&FontVec) -> R) -> R {
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
    pub fn glyph(&mut self, ch: char, px: f32, face: FontFace<'_>) -> RasterGlyph {
        let resolved = self.resolve(face);
        let px_key = px.round().max(1.0) as u32;
        let key = (ch as u32, px_key, resolved.family, face.weight, face.italic);
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let glyph = self.rasterize(ch, px_key as f32, &resolved);
        if self.cache.len() >= GLYPH_CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(key, glyph.clone());
        glyph
    }

    fn rasterize(&self, ch: char, px: f32, resolved: &ResolvedFace<'_>) -> RasterGlyph {
        let (bold, italic) = (resolved.synthetic_bold, resolved.synthetic_italic);
        self.with_font_for(ch, resolved.font, |font| {
            Self::rasterize_with(font, ch, px, bold, italic)
        })
    }

    fn rasterize_with(
        font: &FontVec,
        ch: char,
        px: f32,
        synthetic_bold: bool,
        synthetic_italic: bool,
    ) -> RasterGlyph {
        let scaled = font.as_scaled(px);
        let glyph_id = font.glyph_id(ch);
        let mut advance_width = scaled.h_advance(glyph_id);

        let glyph: Glyph =
            glyph_id.with_scale_and_position(PxScale::from(px), ab_glyph::point(0.0, 0.0));
        let outline = match font.outline_glyph(glyph) {
            Some(o) => o,
            None => {
                if synthetic_bold {
                    advance_width += px * SYNTHETIC_BOLD_ADVANCE;
                }
                return RasterGlyph {
                    metrics: GlyphMetrics { advance_width, ..Default::default() },
                    bitmap: Vec::new(),
                };
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
        let mut xmin = x0;
        let mut width = width;
        // Italic first, then bold: the stem thickens along the already slanted
        // rows instead of being sheared after it grew.
        if synthetic_italic && width > 0 && height > 0 {
            let (sheared, new_width, new_xmin) = shear_italic(bitmap, width, height, xmin);
            bitmap = sheared;
            width = new_width;
            xmin = new_xmin;
        }
        if synthetic_bold {
            dilate_bold(&mut bitmap, width, height);
            advance_width += px * SYNTHETIC_BOLD_ADVANCE;
        }
        RasterGlyph {
            metrics: GlyphMetrics { xmin, ymin: y0, width, height, advance_width },
            bitmap,
        }
    }

    pub fn advance(&mut self, ch: char, px: f32, face: FontFace<'_>) -> f32 {
        self.measure(&ch.to_string(), px, face)
    }
}

/// Skew every row right by `(height - y) * SYNTHETIC_ITALIC_SHEAR`, widening
/// the bitmap by the top row's shift. `xmin` follows the top row, so the glyph
/// still lands on the pen position.
fn shear_italic(bitmap: Vec<u8>, width: usize, height: usize, xmin: i32) -> (Vec<u8>, usize, i32) {
    let max_dx = ((height as f32) * SYNTHETIC_ITALIC_SHEAR).round().max(0.0) as usize;
    if max_dx == 0 {
        return (bitmap, width, xmin);
    }
    let new_width = width + max_dx;
    let mut out = vec![0u8; new_width * height];
    for y in 0..height {
        let dx = (((height - y) as f32) * SYNTHETIC_ITALIC_SHEAR).round().max(0.0) as usize;
        for x in 0..width {
            out[y * new_width + x + dx] = bitmap[y * width + x];
        }
    }
    (out, new_width, xmin + max_dx as i32)
}

/// One-pixel horizontal dilation: `out[x] = max(in[x], in[x-1])`, left edge
/// preserved. Iterated right to left so a pixel is never copied twice.
fn dilate_bold(bitmap: &mut [u8], width: usize, height: usize) {
    for y in 0..height {
        let row = y * width;
        for x in (1..width).rev() {
            bitmap[row + x] = bitmap[row + x].max(bitmap[row + x - 1]);
        }
    }
}

impl TextMeasure for FontSet {
    fn measure(&self, text: &str, size_px: f32, face: FontFace<'_>) -> f32 {
        let resolved = self.resolve(face);
        let px_key = size_px.round().max(1.0) as u32;
        let mut cache = self.advances.borrow_mut();
        let mut total = 0.0;
        for ch in text.chars() {
            let key = (ch as u32, px_key, resolved.family, face.weight, face.italic);
            total += match cache.get(&key) {
                Some(a) => *a,
                None => {
                    let mut a = self.with_font_for(ch, resolved.font, |font| {
                        font.as_scaled(px_key as f32).h_advance(font.glyph_id(ch))
                    });
                    if resolved.synthetic_bold {
                        a += px_key as f32 * SYNTHETIC_BOLD_ADVANCE;
                    }
                    cache.insert(key, a);
                    a
                }
            };
        }
        total
    }

    fn line_metrics(&self, size_px: f32, face: FontFace<'_>) -> (f32, f32) {
        let resolved = self.resolve(face);
        let scaled = resolved.font.as_scaled(size_px);
        (scaled.ascent(), -scaled.descent())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_face() -> FontFace<'static> {
        FontFace::default()
    }

    #[test]
    fn font_face_default_is_regular_upright_default_family() {
        let face = FontFace::default();
        assert_eq!(face.family, "");
        assert_eq!(face.weight, 400);
        assert!(!face.italic);
        // A named family is a different request even at the same weight.
        assert_ne!(FontFace::new("x", 400, false), FontFace::default());
    }

    #[test]
    fn measures_monotonically() {
        let fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let short = fonts.measure("ab", 16.0, default_face());
        let long = fonts.measure("abcd", 16.0, default_face());
        assert!(long > short);
        let (ascent, descent) = fonts.line_metrics(16.0, default_face());
        assert!(ascent > 0.0 && descent >= 0.0);
    }

    #[test]
    fn rasterizes_ascii_and_accents() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        for ch in ['A', 'ç', 'ã', '→'] {
            let g = fonts.glyph(ch, 16.0, default_face());
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
            fonts.glyph(ch, 10.0 + (i % 7) as f32, default_face());
        }
        assert!(fonts.cache.len() <= GLYPH_CACHE_LIMIT);
    }

    fn system_font_bytes() -> Option<Vec<u8>> {
        for path in REGULAR_CANDIDATES {
            if let Ok(data) = std::fs::read(path) {
                return Some(data);
            }
        }
        None
    }

    #[test]
    fn registers_web_family_from_ttf_bytes() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let Some(bytes) = system_font_bytes() else { return };
        fonts
            .register_web_family("Naro Fixture", 400, false, bytes, "https://x/f.ttf")
            .expect("registro");
        assert!(fonts.has_family("naro fixture"));
        assert!(fonts.measure("MMMM", 16.0, FontFace::new("Naro Fixture", 400, false)) > 0.0);
    }

    #[test]
    fn rejects_woff2_and_empty_family() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        assert!(fonts.register_web_family("  ", 400, false, vec![1, 2, 3], "s").is_err());
        let err = fonts
            .register_web_family("x", 400, false, b"wOF2deadbeef".to_vec(), "s")
            .unwrap_err();
        assert!(err.contains("WOFF2"), "{err}");
    }

    #[test]
    fn unknown_family_falls_back_to_default() {
        let fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let default_w = fonts.measure("abc", 16.0, default_face());
        let unknown_w = fonts.measure("abc", 16.0, FontFace::new("No Such Face", 400, false));
        assert_eq!(default_w, unknown_w);
        // An empty family and an unmatched list both mean "system default".
        assert_eq!(
            fonts.measure("abc", 16.0, FontFace::new("No Such Face, sans-serif", 400, false)),
            default_w
        );
    }

    #[test]
    fn monospace_differs_from_sans() {
        let fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let mono = fonts.measure("MMMM", 16.0, FontFace::new("monospace", 400, false));
        let sans = fonts.measure("MMMM", 16.0, FontFace::new("sans-serif", 400, false));
        assert!(mono > 0.0 && sans > 0.0);
        // Both families exist; whether the widths differ depends on the
        // installed files, so only the resolution is asserted here.
        assert!(fonts.has_family("monospace") && fonts.has_family("serif"));
        assert_ne!(
            fonts.resolve(FontFace::new("monospace", 400, false)).family,
            fonts.resolve(FontFace::new("serif", 400, false)).family
        );
    }

    #[test]
    fn synthesizes_bold_when_family_has_only_regular() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        // `monospace` carries a single 400 face, so 700 must be synthesized.
        let resolved = fonts.resolve(FontFace::new("monospace", 700, false));
        assert!(resolved.synthetic_bold);
        let regular = fonts.measure("MMMM", 16.0, FontFace::new("monospace", 400, false));
        let bold = fonts.measure("MMMM", 16.0, FontFace::new("monospace", 700, false));
        assert!(bold > regular, "bold {bold} vs regular {regular}");
        let g = fonts.glyph('M', 16.0, FontFace::new("monospace", 700, false));
        assert!(g.metrics.advance_width > 0.0);
    }

    #[test]
    fn synthesizes_italic_when_family_has_no_italic() {
        let fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let resolved = fonts.resolve(FontFace::new("monospace", 400, true));
        assert!(resolved.synthetic_italic);
        assert!(!resolved.synthetic_bold);
        assert!(!fonts.resolve(FontFace::new("monospace", 400, false)).synthetic_italic);
    }

    #[test]
    fn face_limit_is_enforced() {
        let mut fonts = match FontSet::load_system() {
            Ok(f) => f,
            Err(_) => return,
        };
        let Some(bytes) = system_font_bytes() else { return };
        for i in 0..MAX_FACES_PER_FAMILY {
            let weight = 100 + (i as u16) * 50;
            fonts
                .register_web_family("Big", weight, false, bytes.clone(), &format!("u{i}"))
                .expect("within limit");
        }
        assert!(fonts
            .register_web_family("Big", 900, false, bytes, "overflow")
            .is_err());
    }

    #[test]
    fn splits_family_lists_respecting_quotes() {
        assert_eq!(
            families(r#""A, B", serif"#).collect::<Vec<_>>(),
            vec![r#""A, B""#, " serif"]
        );
        assert_eq!(families("sans-serif").collect::<Vec<_>>(), vec!["sans-serif"]);
        assert_eq!(clean_family(r#"  "Naro Fixture" "#), "Naro Fixture");
    }
}
