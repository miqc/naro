//! Minimal SVG path geometry.
//!
//! Only what a page needs to draw an icon or a wordmark: `M/L/H/V/C/S/Q/T/Z`
//! in absolute and relative form, flattened to polylines and filled with the
//! nonzero winding rule. No strokes, gradients, transforms or clipping paths —
//! those are recorded as unsupported by the caller, not silently approximated.
//!
//! Lives in `naro-layout` because it is pure geometry; `naro-paint` depends on
//! this crate and plots the result.

/// A closed or open subpath in user space.
#[derive(Clone, Debug, Default)]
pub struct SubPath {
    pub points: Vec<(f32, f32)>,
}

/// Parse an SVG `d` attribute into subpaths.
///
/// Curves are flattened into `STEPS` line segments each. Malformed input stops
/// parsing and keeps whatever was already built, so a single bad command does
/// not blank the whole icon.
pub fn parse_path(d: &str) -> Vec<SubPath> {
    const STEPS: usize = 8;
    let mut out: Vec<SubPath> = Vec::new();
    let mut current = SubPath::default();
    let mut tokens = Tokens::new(d);
    let mut cursor = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    // Last control point, for the smooth variants.
    let mut last_control: Option<(f32, f32)> = None;
    let mut command = 'M';

    while let Some(cmd) = tokens.next_command() {
        command = cmd;
        loop {
            match command {
                'M' | 'm' => {
                    let Some((mut x, mut y)) = tokens.pair() else { return finish(out, current) };
                    if command == 'm' {
                        x += cursor.0;
                        y += cursor.1;
                    }
                    if !current.points.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    cursor = (x, y);
                    start = cursor;
                    current.points.push(cursor);
                    // Further pairs after `M` are implicit line-tos.
                    command = if command == 'm' { 'l' } else { 'L' };
                    last_control = None;
                }
                'L' | 'l' => {
                    let Some((mut x, mut y)) = tokens.pair() else { return finish(out, current) };
                    if command == 'l' {
                        x += cursor.0;
                        y += cursor.1;
                    }
                    cursor = (x, y);
                    current.points.push(cursor);
                    last_control = None;
                }
                'H' | 'h' => {
                    let Some(mut x) = tokens.number() else { return finish(out, current) };
                    if command == 'h' {
                        x += cursor.0;
                    }
                    cursor = (x, cursor.1);
                    current.points.push(cursor);
                    last_control = None;
                }
                'V' | 'v' => {
                    let Some(mut y) = tokens.number() else { return finish(out, current) };
                    if command == 'v' {
                        y += cursor.1;
                    }
                    cursor = (cursor.0, y);
                    current.points.push(cursor);
                    last_control = None;
                }
                'C' | 'c' => {
                    let Some(values) = tokens.numbers(6) else { return finish(out, current) };
                    let rel = command == 'c';
                    let p = |i: usize| -> (f32, f32) {
                        let (dx, dy) = (values[i], values[i + 1]);
                        if rel {
                            (cursor.0 + dx, cursor.1 + dy)
                        } else {
                            (dx, dy)
                        }
                    };
                    let (c1, c2, end) = (p(0), p(2), p(4));
                    flatten_cubic(cursor, c1, c2, end, STEPS, &mut current.points);
                    last_control = Some(c2);
                    cursor = end;
                }
                'S' | 's' => {
                    let Some(values) = tokens.numbers(4) else { return finish(out, current) };
                    let rel = command == 's';
                    let p = |i: usize| -> (f32, f32) {
                        let (dx, dy) = (values[i], values[i + 1]);
                        if rel {
                            (cursor.0 + dx, cursor.1 + dy)
                        } else {
                            (dx, dy)
                        }
                    };
                    let c1 = match last_control {
                        Some(prev) => (2.0 * cursor.0 - prev.0, 2.0 * cursor.1 - prev.1),
                        None => cursor,
                    };
                    let (c2, end) = (p(0), p(2));
                    flatten_cubic(cursor, c1, c2, end, STEPS, &mut current.points);
                    last_control = Some(c2);
                    cursor = end;
                }
                'Q' | 'q' => {
                    let Some(values) = tokens.numbers(4) else { return finish(out, current) };
                    let rel = command == 'q';
                    let p = |i: usize| -> (f32, f32) {
                        let (dx, dy) = (values[i], values[i + 1]);
                        if rel {
                            (cursor.0 + dx, cursor.1 + dy)
                        } else {
                            (dx, dy)
                        }
                    };
                    let (control, end) = (p(0), p(2));
                    flatten_quadratic(cursor, control, end, STEPS, &mut current.points);
                    last_control = Some(control);
                    cursor = end;
                }
                'T' | 't' => {
                    let Some((mut x, mut y)) = tokens.pair() else { return finish(out, current) };
                    if command == 't' {
                        x += cursor.0;
                        y += cursor.1;
                    }
                    let control = match last_control {
                        Some(prev) => (2.0 * cursor.0 - prev.0, 2.0 * cursor.1 - prev.1),
                        None => cursor,
                    };
                    flatten_quadratic(cursor, control, (x, y), STEPS, &mut current.points);
                    last_control = Some(control);
                    cursor = (x, y);
                }
                'Z' | 'z' => {
                    cursor = start;
                    if !current.points.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    last_control = None;
                }
                // `A` (elliptic arc) is not implemented; treat it as a line to
                // its endpoint so the rest of the shape still draws.
                'A' | 'a' => {
                    let Some(values) = tokens.numbers(7) else { return finish(out, current) };
                    let (mut x, mut y) = (values[5], values[6]);
                    if command == 'a' {
                        x += cursor.0;
                        y += cursor.1;
                    }
                    cursor = (x, y);
                    current.points.push(cursor);
                    last_control = None;
                }
                _ => return finish(out, current),
            }
            // A command repeats until the next command letter or end of input.
            if !tokens.has_number_ahead() {
                break;
            }
        }
    }
    finish(out, current)
}

fn finish(mut out: Vec<SubPath>, current: SubPath) -> Vec<SubPath> {
    if current.points.len() >= 2 {
        out.push(current);
    }
    out
}

fn flatten_cubic(
    from: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    to: (f32, f32),
    steps: usize,
    out: &mut Vec<(f32, f32)>,
) {
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let u = 1.0 - t;
        let x = u * u * u * from.0
            + 3.0 * u * u * t * c1.0
            + 3.0 * u * t * t * c2.0
            + t * t * t * to.0;
        let y = u * u * u * from.1
            + 3.0 * u * u * t * c1.1
            + 3.0 * u * t * t * c2.1
            + t * t * t * to.1;
        out.push((x, y));
    }
}

fn flatten_quadratic(
    from: (f32, f32),
    control: (f32, f32),
    to: (f32, f32),
    steps: usize,
    out: &mut Vec<(f32, f32)>,
) {
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let u = 1.0 - t;
        let x = u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0;
        let y = u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1;
        out.push((x, y));
    }
}

struct Tokens<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Tokens<'a> {
    fn new(src: &'a str) -> Tokens<'a> {
        Tokens { bytes: src.as_bytes(), pos: 0 }
    }

    fn skip_separators(&mut self) {
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c.is_ascii_whitespace() || c == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn next_command(&mut self) -> Option<char> {
        self.skip_separators();
        let c = *self.bytes.get(self.pos)?;
        if c.is_ascii_alphabetic() {
            self.pos += 1;
            Some(c as char)
        } else {
            // A number where a command was expected: repeat the last one.
            None
        }
    }

    fn has_number_ahead(&mut self) -> bool {
        self.skip_separators();
        matches!(self.bytes.get(self.pos), Some(c) if *c == b'-' || *c == b'+' || *c == b'.' || c.is_ascii_digit())
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let start = self.pos;
        if matches!(self.bytes.get(self.pos), Some(b'-') | Some(b'+')) {
            self.pos += 1;
        }
        let mut seen_digit = false;
        while let Some(&c) = self.bytes.get(self.pos) {
            if c.is_ascii_digit() {
                seen_digit = true;
                self.pos += 1;
            } else if c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if !seen_digit {
            self.pos = start;
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse()
            .ok()
    }

    fn pair(&mut self) -> Option<(f32, f32)> {
        let x = self.number()?;
        let y = self.number()?;
        Some((x, y))
    }

    /// `count` consecutive numbers.
    fn numbers(&mut self, count: usize) -> Option<Vec<f32>> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.number()?);
        }
        Some(out)
    }
}

/// Fill `subpaths` (already in device space) with `color`.
///
/// Scanline fill with the nonzero winding rule: for each device row the
/// crossings of every edge are collected with their direction, then spans
/// between accumulated nonzero runs are painted.
pub fn fill_subpaths(
    subpaths: &[SubPath],
    origin: (f32, f32),
    scale: (f32, f32),
    width: u32,
    height: u32,
    mut plot: impl FnMut(u32, u32),
) {
    if subpaths.is_empty() || width == 0 || height == 0 {
        return;
    }
    let mut edges: Vec<(f32, f32, f32, f32, i32)> = Vec::new();
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for sub in subpaths {
        let n = sub.points.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (x0, y0) = sub.points[i];
            let (x1, y1) = sub.points[(i + 1) % n];
            let p0 = (origin.0 + x0 * scale.0, origin.1 + y0 * scale.1);
            let p1 = (origin.0 + x1 * scale.0, origin.1 + y1 * scale.1);
            if p0.1 == p1.1 {
                continue;
            }
            let dir = if p1.1 > p0.1 { 1 } else { -1 };
            let (top, bottom) = if p0.1 < p1.1 { (p0, p1) } else { (p1, p0) };
            min_y = min_y.min(top.1);
            max_y = max_y.max(bottom.1);
            edges.push((top.0, top.1, bottom.0, bottom.1, dir));
        }
    }
    if edges.is_empty() {
        return;
    }
    let y_start = (min_y.floor().max(0.0) as u32).min(height.saturating_sub(1));
    let y_end = (max_y.ceil().max(0.0) as u32).min(height.saturating_sub(1));
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for y in y_start..=y_end {
        let sample = y as f32 + 0.5;
        crossings.clear();
        for &(x0, y0, x1, y1, dir) in &edges {
            if sample >= y0 && sample < y1 {
                let t = (sample - y0) / (y1 - y0);
                crossings.push((x0 + (x1 - x0) * t, dir));
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut winding = 0i32;
        for pair in crossings.windows(2) {
            winding += pair[0].1;
            if winding != 0 {
                let x_from = pair[0].0.max(0.0).ceil() as u32;
                let x_to = pair[1].0.min(width as f32).max(0.0) as u32;
                for x in x_from..x_to.min(width) {
                    plot(x, y);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_and_relative_commands() {
        let path = parse_path("M10 10 L20 10 L20 20 Z");
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].points.len(), 3);
        assert_eq!(path[0].points[0], (10.0, 10.0));

        let relative = parse_path("m10 10 l10 0 l0 10 z");
        assert_eq!(relative[0].points.len(), 3);
        assert_eq!(relative[0].points[2], (20.0, 20.0));
    }

    #[test]
    fn horizontal_and_vertical_shorthand() {
        let path = parse_path("M0 0 H10 V10 H0 Z");
        assert_eq!(path[0].points.len(), 4);
        assert_eq!(path[0].points[1], (10.0, 0.0));
        assert_eq!(path[0].points[2], (10.0, 10.0));
    }

    #[test]
    fn curves_are_flattened() {
        let path = parse_path("M0 0 C0 10 10 10 10 0");
        assert!(path[0].points.len() > 4, "curve became a polyline");
        assert_eq!(*path[0].points.last().unwrap(), (10.0, 0.0));
    }

    #[test]
    fn multiple_subpaths_are_separate() {
        let path = parse_path("M0 0 L5 0 Z M10 0 L15 0 Z");
        assert_eq!(path.len(), 2);
    }

    #[test]
    fn malformed_input_keeps_what_was_parsed() {
        let path = parse_path("M0 0 L10 0 L10 garbage");
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].points.len(), 2);
        assert!(parse_path("").is_empty());
    }

    #[test]
    fn fill_covers_the_inside_of_a_square() {
        let subpaths = parse_path("M0 0 L10 0 L10 10 L0 10 Z");
        let mut painted = Vec::new();
        fill_subpaths(&subpaths, (0.0, 0.0), (1.0, 1.0), 20, 20, |x, y| painted.push((x, y)));
        assert!(painted.contains(&(5, 5)), "inside is filled");
        assert!(!painted.contains(&(15, 15)), "outside is not");
    }

    #[test]
    fn winding_rule_leaves_a_hole() {
        // Outer square clockwise, inner square counter-clockwise.
        let subpaths = parse_path("M0 0 L10 0 L10 10 L0 10 Z M3 3 L3 7 L7 7 L7 3 Z");
        let mut painted = Vec::new();
        fill_subpaths(&subpaths, (0.0, 0.0), (1.0, 1.0), 20, 20, |x, y| painted.push((x, y)));
        assert!(painted.contains(&(1, 5)), "border is filled");
        assert!(!painted.contains(&(5, 5)), "hole stays empty");
    }
}
