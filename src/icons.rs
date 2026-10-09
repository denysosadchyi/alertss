//! Monochrome Fluent UI System Icons (Microsoft, MIT) drawn as Direct2D path geometries.
//!
//! The 24x24 filled SVGs in `assets/icons` are embedded with `include_str!`; their single
//! `<path d="...">` is parsed by a small SVG path-data parser into absolute segments and
//! turned into an `ID2D1PathGeometry` (cached per factory), filled in a tint colour.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Smile,
    Drone,
    Rocket,
    Urgent,
    Unknown,
}

impl Icon {
    fn svg(self) -> &'static str {
        match self {
            Icon::Smile => include_str!("../assets/icons/smile.svg"),
            Icon::Drone => include_str!("../assets/icons/shahed.svg"),
            Icon::Rocket => include_str!("../assets/icons/missile.svg"),
            Icon::Urgent => include_str!("../assets/icons/urgent.svg"),
            Icon::Unknown => include_str!("../assets/icons/unknown.svg"),
        }
    }
}

impl Icon {
    /// Tint colour for this status icon on a light or dark background.
    pub fn tint(self, light: bool) -> D2D1_COLOR_F {
        let (r, g, b) = match (self, light) {
            (Icon::Smile, false) => (0x6C, 0xCB, 0x5F),
            (Icon::Smile, true) => (0x16, 0xC6, 0x0C),
            (Icon::Drone, false) => (0xFF, 0xB9, 0x00),
            (Icon::Drone, true) => (0xC8, 0x87, 0x00),
            (Icon::Rocket | Icon::Urgent, _) => (0xF0, 0x50, 0x4A),
            (Icon::Unknown, _) => (0x8A, 0x8A, 0x8A),
        };
        D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }
}

/// One absolute path segment (all coordinates in icon space).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Cubic([f32; 6]),
    /// rx, ry, x-axis rotation (degrees), large-arc, sweep, end x, end y.
    Arc(f32, f32, f32, bool, bool, f32, f32),
    Close,
}

struct Scan<'a> {
    b: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }

    /// True if a number (not a command letter) comes next.
    fn at_number(&mut self) -> bool {
        self.skip_sep();
        self.i < self.b.len() && matches!(self.b[self.i], b'0'..=b'9' | b'.' | b'-' | b'+')
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let s = self.i;
        let b = self.b;
        let mut i = s;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let d0 = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let mut digits = i - d0;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let f0 = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            digits += i - f0;
        }
        if digits == 0 {
            return None;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                j += 1;
            }
            let e0 = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > e0 {
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&b[s..i]).ok()?.parse().ok()
    }

    /// Arc flag: a single '0' or '1' (may be glued to the next number).
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.b.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Parse SVG path data into absolute segments. Returns `None` on malformed data.
/// Quadratics are elevated to cubics; S/T reflect the previous control point.
pub fn parse_path(d: &str) -> Option<Vec<Seg>> {
    let mut sc = Scan { b: d.as_bytes(), i: 0 };
    let mut out = Vec::new();
    let (mut cx, mut cy) = (0f32, 0f32);
    let (mut sx, mut sy) = (0f32, 0f32);
    // Last control point for S/T reflection.
    let mut last_c: Option<(f32, f32)> = None;
    let mut last_q: Option<(f32, f32)> = None;
    let mut cmd = 0u8;
    let mut open = false;
    loop {
        sc.skip_sep();
        if sc.i >= sc.b.len() {
            break;
        }
        let c = sc.b[sc.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
            if cmd == b'Z' || cmd == b'z' {
                if open {
                    out.push(Seg::Close);
                }
                open = false;
                cx = sx;
                cy = sy;
                last_c = None;
                last_q = None;
                continue;
            }
        } else if cmd == 0 || cmd == b'Z' || cmd == b'z' || !sc.at_number() {
            return None;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        let up = cmd.to_ascii_uppercase();
        if !matches!(up, b'M' | b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A') {
            return None;
        }
        if up != b'M' && !open {
            // Drawing right after Z starts a new figure at the previous start point.
            out.push(Seg::Move(cx, cy));
            sx = cx;
            sy = cy;
            open = true;
        }
        let mut new_c = None;
        let mut new_q = None;
        match up {
            b'M' => {
                let x = sc.number()? + ox;
                let y = sc.number()? + oy;
                out.push(Seg::Move(x, y));
                cx = x;
                cy = y;
                sx = x;
                sy = y;
                open = true;
                // Further pairs are implicit lineto.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let x = sc.number()? + ox;
                let y = sc.number()? + oy;
                out.push(Seg::Line(x, y));
                cx = x;
                cy = y;
            }
            b'H' => {
                let x = sc.number()? + ox;
                out.push(Seg::Line(x, cy));
                cx = x;
            }
            b'V' => {
                let y = sc.number()? + oy;
                out.push(Seg::Line(cx, y));
                cy = y;
            }
            b'C' | b'S' => {
                let (x1, y1) = if up == b'C' {
                    (sc.number()? + ox, sc.number()? + oy)
                } else {
                    match last_c {
                        Some((px, py)) => (2.0 * cx - px, 2.0 * cy - py),
                        None => (cx, cy),
                    }
                };
                let x2 = sc.number()? + ox;
                let y2 = sc.number()? + oy;
                let x = sc.number()? + ox;
                let y = sc.number()? + oy;
                out.push(Seg::Cubic([x1, y1, x2, y2, x, y]));
                new_c = Some((x2, y2));
                cx = x;
                cy = y;
            }
            b'Q' | b'T' => {
                let (qx, qy) = if up == b'Q' {
                    (sc.number()? + ox, sc.number()? + oy)
                } else {
                    match last_q {
                        Some((px, py)) => (2.0 * cx - px, 2.0 * cy - py),
                        None => (cx, cy),
                    }
                };
                let x = sc.number()? + ox;
                let y = sc.number()? + oy;
                out.push(Seg::Cubic([
                    cx + 2.0 / 3.0 * (qx - cx),
                    cy + 2.0 / 3.0 * (qy - cy),
                    x + 2.0 / 3.0 * (qx - x),
                    y + 2.0 / 3.0 * (qy - y),
                    x,
                    y,
                ]));
                new_q = Some((qx, qy));
                cx = x;
                cy = y;
            }
            b'A' => {
                let rx = sc.number()?;
                let ry = sc.number()?;
                let rot = sc.number()?;
                let large = sc.flag()?;
                let sweep = sc.flag()?;
                let x = sc.number()? + ox;
                let y = sc.number()? + oy;
                out.push(Seg::Arc(rx, ry, rot, large, sweep, x, y));
                cx = x;
                cy = y;
            }
            _ => return None,
        }
        last_c = new_c;
        last_q = new_q;
    }
    Some(out)
}

/// Extract the `d` attribute of the first `<path>` in an SVG document.
fn path_data(svg: &str) -> Option<&str> {
    let p = svg.find("<path")?;
    let rest = &svg[p..];
    let d = rest.find(" d=\"")? + 4;
    let end = rest[d..].find('"')?;
    Some(&rest[d..d + end])
}

type CacheEntry = (ID2D1Factory, Option<ID2D1PathGeometry>);

thread_local! {
    /// (factory pointer, icon) -> geometry. The factory clone keeps the pointer from being reused.
    static CACHE: RefCell<HashMap<(usize, Icon), CacheEntry>> = RefCell::new(HashMap::new());
}

fn build(factory: &ID2D1Factory, icon: Icon) -> Option<ID2D1PathGeometry> {
    let segs = parse_path(path_data(icon.svg())?)?;
    unsafe {
        let g = factory.CreatePathGeometry().ok()?;
        let sink = g.Open().ok()?;
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        let mut open = false;
        for s in segs {
            match s {
                Seg::Move(x, y) => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    sink.BeginFigure(Vector2 { X: x, Y: y }, D2D1_FIGURE_BEGIN_FILLED);
                    open = true;
                }
                Seg::Line(x, y) => sink.AddLine(Vector2 { X: x, Y: y }),
                Seg::Cubic([a, b, c, d, e, f]) => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: Vector2 { X: a, Y: b },
                    point2: Vector2 { X: c, Y: d },
                    point3: Vector2 { X: e, Y: f },
                }),
                Seg::Arc(rx, ry, rot, large, sweep, x, y) => sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x, Y: y },
                    size: D2D_SIZE_F { width: rx, height: ry },
                    rotationAngle: rot,
                    sweepDirection: if sweep { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                    arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                }),
                Seg::Close => {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    open = false;
                }
            }
        }
        if open {
            sink.EndFigure(D2D1_FIGURE_END_CLOSED); // fills close implicitly anyway
        }
        sink.Close().ok()?;
        Some(g)
    }
}

/// Fill `icon` into `rect` (device pixels; snapped to whole pixels, square, centred) with `color`.
pub fn draw(rt: &ID2D1RenderTarget, factory: &ID2D1Factory, icon: Icon, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
    let key = (factory.as_raw() as usize, icon);
    let geom = CACHE.with(|c| {
        c.borrow_mut().entry(key).or_insert_with(|| (factory.clone(), build(factory, icon))).1.clone()
    });
    let Some(geom) = geom else { return };
    unsafe {
        let Ok(brush) = rt.CreateSolidColorBrush(&color, None) else { return };
        let side = (rect.right - rect.left).min(rect.bottom - rect.top).round().max(1.0);
        let x = ((rect.left + rect.right - side) / 2.0).round();
        let y = ((rect.top + rect.bottom - side) / 2.0).round();
        let k = side / 24.0;
        let mut old = Matrix3x2::default();
        rt.GetTransform(&mut old);
        rt.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x, M32: y });
        rt.FillGeometry(&geom, &brush, None);
        rt.SetTransform(&old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_implicit_commands() {
        let s = parse_path("M1.5.5-2e-1 3L4,5 6 7").unwrap();
        assert_eq!(s, vec![Seg::Move(1.5, 0.5), Seg::Line(-0.2, 3.0), Seg::Line(4.0, 5.0), Seg::Line(6.0, 7.0)]);
    }

    #[test]
    fn relative_to_absolute() {
        let s = parse_path("M10 10h5v5l-5 0z").unwrap();
        assert_eq!(
            s,
            vec![Seg::Move(10.0, 10.0), Seg::Line(15.0, 10.0), Seg::Line(15.0, 15.0), Seg::Line(10.0, 15.0), Seg::Close]
        );
        let s = parse_path("m1 1 2 2").unwrap(); // implicit relative lineto
        assert_eq!(s, vec![Seg::Move(1.0, 1.0), Seg::Line(3.0, 3.0)]);
    }

    #[test]
    fn smooth_and_quadratic() {
        let s = parse_path("M0 0C0 3 3 3 3 0S6 -3 6 0").unwrap();
        assert_eq!(s[2], Seg::Cubic([3.0, -3.0, 6.0, -3.0, 6.0, 0.0]));
        let s = parse_path("M0 0Q3 3 6 0T12 0").unwrap();
        assert_eq!(s.len(), 3);
        if let Seg::Cubic(c) = s[2] {
            assert!((c[0] - 8.0).abs() < 1e-5 && (c[1] + 2.0).abs() < 1e-5);
        } else {
            panic!();
        }
    }

    #[test]
    fn arcs_with_glued_flags() {
        let s = parse_path("M0 0a2 2 0 011 1A1 1 30 1 0 5 5").unwrap();
        assert_eq!(s[1], Seg::Arc(2.0, 2.0, 0.0, false, true, 1.0, 1.0));
        assert_eq!(s[2], Seg::Arc(1.0, 1.0, 30.0, true, false, 5.0, 5.0));
    }

    #[test]
    fn draw_after_close_starts_new_figure() {
        let s = parse_path("M1 1L2 2zl1 0").unwrap();
        assert_eq!(s, vec![Seg::Move(1.0, 1.0), Seg::Line(2.0, 2.0), Seg::Close, Seg::Move(1.0, 1.0), Seg::Line(2.0, 1.0)]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_path("M1").is_none());
        assert!(parse_path("M0 0X1").is_none());
    }

    #[test]
    fn all_embedded_icons_parse() {
        for i in [Icon::Smile, Icon::Drone, Icon::Rocket, Icon::Urgent, Icon::Unknown] {
            let d = path_data(i.svg()).unwrap();
            let segs = parse_path(d).unwrap();
            assert!(segs.len() > 5, "{i:?}");
            assert!(matches!(segs[0], Seg::Move(..)));
        }
    }
}
