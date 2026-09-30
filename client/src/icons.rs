//! The brand icon set (`brand/icons/*.svg`), read from the SVGs themselves.
//!
//! One source: the device draws the same 24-grid monoline icons as the web
//! console by parsing the files at compile time (`include_str!`) into
//! polylines, which `ui.rs` strokes with egui — tinted to the text colour,
//! crisp at any scale. No SVG library: the set uses a small, fixed subset
//! (`path` with M/L/H/V/C/S/A/Z, `circle`, `rect` with `rx`, a dash array and
//! `fill="currentColor"` dots), and the tests hold every icon to it.

#![cfg_attr(not(feature = "gui"), allow(dead_code))]

use std::f32::consts::TAU;

/// The icons the device uses. Add one here (and its file) — never draw one
/// inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    AllowedHours,
    ArrowRight,
    Ask,
    Bell,
    Check,
    Clock,
    Close,
    EyeOff,
    Globe,
    Info,
    Laptop,
    Lock,
    Moon,
    Offline,
    Pause,
    Warning,
}

impl Icon {
    pub const ALL: [Icon; 16] = [
        Icon::AllowedHours,
        Icon::ArrowRight,
        Icon::Ask,
        Icon::Bell,
        Icon::Check,
        Icon::Clock,
        Icon::Close,
        Icon::EyeOff,
        Icon::Globe,
        Icon::Info,
        Icon::Laptop,
        Icon::Lock,
        Icon::Moon,
        Icon::Offline,
        Icon::Pause,
        Icon::Warning,
    ];

    pub fn svg(self) -> &'static str {
        match self {
            Icon::AllowedHours => include_str!("../../brand/icons/allowed-hours.svg"),
            Icon::ArrowRight => include_str!("../../brand/icons/arrow-right.svg"),
            Icon::Ask => include_str!("../../brand/icons/ask.svg"),
            Icon::Bell => include_str!("../../brand/icons/bell.svg"),
            Icon::Check => include_str!("../../brand/icons/check.svg"),
            Icon::Clock => include_str!("../../brand/icons/clock.svg"),
            Icon::Close => include_str!("../../brand/icons/close.svg"),
            Icon::EyeOff => include_str!("../../brand/icons/eye-off.svg"),
            Icon::Globe => include_str!("../../brand/icons/globe.svg"),
            Icon::Info => include_str!("../../brand/icons/info.svg"),
            Icon::Laptop => include_str!("../../brand/icons/laptop.svg"),
            Icon::Lock => include_str!("../../brand/icons/lock.svg"),
            Icon::Moon => include_str!("../../brand/icons/moon.svg"),
            Icon::Offline => include_str!("../../brand/icons/offline.svg"),
            Icon::Pause => include_str!("../../brand/icons/pause.svg"),
            Icon::Warning => include_str!("../../brand/icons/warning.svg"),
        }
    }

    /// The icon as strokes in its 24 × 24 box, parsed once.
    pub fn outline(self) -> &'static Outline {
        use std::collections::HashMap;
        use std::sync::OnceLock;
        static CACHE: OnceLock<HashMap<Icon, Outline>> = OnceLock::new();
        let all = CACHE.get_or_init(|| {
            Icon::ALL
                .iter()
                .map(|i| (*i, parse(i.svg()).unwrap_or_default()))
                .collect()
        });
        &all[&self]
    }
}

/// One stroked run of points in the 24-unit box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub points: Vec<(f32, f32)>,
    pub closed: bool,
    /// Indices of the points where one segment meets another at an angle
    /// (where a round join goes); curve samples are not corners.
    pub corners: Vec<usize>,
}

/// A filled disc (`fill="currentColor"` dots); the stroke widens it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub stroke_width: f32,
    pub runs: Vec<Run>,
    pub dots: Vec<Dot>,
}

/// Points per full turn when flattening arcs and circles.
const ARC_STEPS: f32 = 48.0;
/// Samples per cubic.
const CUBIC_STEPS: usize = 12;

fn attr<'a>(el: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = el.find(&key)? + key.len();
    let rest = &el[i..];
    Some(&rest[..rest.find('"')?])
}

fn num(el: &str, name: &str) -> Result<f32, String> {
    attr(el, name)
        .ok_or_else(|| format!("missing {name}"))?
        .parse()
        .map_err(|_| format!("bad {name}"))
}

/// Parse one icon file.
pub fn parse(svg: &str) -> Result<Outline, String> {
    let root_end = svg.find('>').ok_or("no <svg>")?;
    let root = &svg[..root_end];
    let stroke_width = attr(root, "stroke-width")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.0);
    let mut out = Outline {
        stroke_width,
        ..Default::default()
    };
    let mut rest = &svg[root_end + 1..];
    while let Some(start) = rest.find('<') {
        let end = rest[start..].find('>').ok_or("unclosed element")? + start;
        let el = &rest[start..end];
        rest = &rest[end + 1..];
        if el.starts_with("</") {
            continue;
        }
        let tag: String = el[1..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let filled = attr(el, "fill") == Some("currentColor");
        let mut runs = match tag.as_str() {
            "path" => path(attr(el, "d").ok_or("path without d")?)?,
            "circle" => {
                let (cx, cy, r) = (num(el, "cx")?, num(el, "cy")?, num(el, "r")?);
                if filled {
                    out.dots.push(Dot { cx, cy, r });
                    continue;
                }
                vec![circle(cx, cy, r)]
            }
            "rect" => {
                let (x, y, w, h) = (
                    num(el, "x")?,
                    num(el, "y")?,
                    num(el, "width")?,
                    num(el, "height")?,
                );
                let rx = attr(el, "rx").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                vec![rect(x, y, w, h, rx)]
            }
            other => return Err(format!("unsupported element <{other}>")),
        };
        if let Some(dash) = attr(el, "stroke-dasharray") {
            let pattern: Vec<f32> = dash
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<f32>().map_err(|_| "bad dash".to_string()))
                .collect::<Result<_, _>>()?;
            runs = runs.iter().flat_map(|r| dashed(r, &pattern)).collect();
        }
        out.runs.extend(runs);
    }
    Ok(out)
}

fn circle(cx: f32, cy: f32, r: f32) -> Run {
    let n = ARC_STEPS as usize;
    Run {
        points: (0..n)
            .map(|i| {
                let a = TAU * i as f32 / n as f32;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect(),
        closed: true,
        corners: Vec::new(),
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32, rx: f32) -> Run {
    let rx = rx.min(w / 2.0).min(h / 2.0);
    if rx <= 0.0 {
        return Run {
            points: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
            closed: true,
            corners: vec![0, 1, 2, 3],
        };
    }
    let mut points = Vec::new();
    // Corners clockwise from top-right, each a quarter circle.
    let centres = [
        (x + w - rx, y + rx, -TAU / 4.0),
        (x + w - rx, y + h - rx, 0.0),
        (x + rx, y + h - rx, TAU / 4.0),
        (x + rx, y + rx, TAU / 2.0),
    ];
    let steps = 8;
    for (cx, cy, a0) in centres {
        for i in 0..=steps {
            let a = a0 + TAU / 4.0 * i as f32 / steps as f32;
            points.push((cx + rx * a.cos(), cy + rx * a.sin()));
        }
    }
    Run {
        points,
        closed: true,
        corners: Vec::new(),
    }
}

/// Split a run into dashes (on, off, on, off…), measured along it.
fn dashed(run: &Run, pattern: &[f32]) -> Vec<Run> {
    if pattern.is_empty() || pattern.iter().all(|p| *p <= 0.0) {
        return vec![run.clone()];
    }
    let mut pts = run.points.clone();
    if run.closed {
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
    }
    let mut out = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut idx, mut left, mut on) = (0usize, pattern[0], true);
    if on {
        cur.push(pts[0]);
    }
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut seg = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut p = a;
        while seg > 0.0 {
            let step = seg.min(left);
            let t = step / seg;
            p = (p.0 + (b.0 - p.0) * t, p.1 + (b.1 - p.1) * t);
            seg -= step;
            left -= step;
            if left <= 1e-6 {
                if on {
                    cur.push(p);
                    out.push(Run {
                        points: std::mem::take(&mut cur),
                        closed: false,
                        corners: Vec::new(),
                    });
                } else {
                    cur.push(p);
                }
                on = !on;
                idx = (idx + 1) % pattern.len();
                left = pattern[idx];
            }
        }
        if on && cur.last() != Some(&b) {
            cur.push(b);
        }
    }
    if on && cur.len() > 1 {
        out.push(Run {
            points: cur,
            closed: false,
            corners: Vec::new(),
        });
    }
    out
}

/// A tokenizer for path data: commands and numbers, including the compact
/// forms (`3-.4`, `.5.5`) and single-character arc flags.
struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Tokens<'a> {
    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\n' | b'\t' | b'\r')
        {
            self.i += 1;
        }
    }
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }
    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }
    fn flag(&mut self) -> Result<bool, String> {
        self.skip();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err("bad arc flag".into()),
        }
    }
    fn number(&mut self) -> Result<f32, String> {
        self.skip();
        let start = self.i;
        if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| format!("bad number at {start}"))
    }
}

/// Path data → runs (one per subpath).
fn path(d: &str) -> Result<Vec<Run>, String> {
    let mut t = Tokens {
        s: d.as_bytes(),
        i: 0,
    };
    let mut runs: Vec<Run> = Vec::new();
    let mut run = Run::default();
    let (mut cur, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    // The last cubic's second control point, for S.
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut cmd = t.command().ok_or("path must start with a command")?;
    let finish = |run: &mut Run, runs: &mut Vec<Run>| {
        if run.points.len() > 1 {
            runs.push(std::mem::take(run));
        } else {
            *run = Run::default();
        }
    };
    loop {
        let rel = cmd.is_ascii_lowercase();
        let off = |p: (f32, f32), cur: (f32, f32)| if rel { (p.0 + cur.0, p.1 + cur.1) } else { p };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                finish(&mut run, &mut runs);
                let p = off((t.number()?, t.number()?), cur);
                cur = p;
                start = p;
                run.points.push(p);
                run.corners.push(0);
                last_ctrl = None;
                // Further pairs after M are implicit L.
                cmd = if rel { b'l' } else { b'L' };
                if !t.at_number() {
                    match t.command() {
                        Some(c) => {
                            cmd = c;
                            continue;
                        }
                        None => break,
                    }
                }
                continue;
            }
            b'L' | b'H' | b'V' => {
                let upper = cmd.to_ascii_uppercase();
                let p = match upper {
                    b'L' => off((t.number()?, t.number()?), cur),
                    b'H' => {
                        let x = t.number()?;
                        (if rel { cur.0 + x } else { x }, cur.1)
                    }
                    _ => {
                        let y = t.number()?;
                        (cur.0, if rel { cur.1 + y } else { y })
                    }
                };
                run.corners.push(run.points.len() - 1);
                run.points.push(p);
                cur = p;
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    off((t.number()?, t.number()?), cur)
                } else {
                    // Reflection of the previous second control point.
                    last_ctrl.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1))
                };
                let c2 = off((t.number()?, t.number()?), cur);
                let p = off((t.number()?, t.number()?), cur);
                run.corners.push(run.points.len() - 1);
                for i in 1..=CUBIC_STEPS {
                    let u = i as f32 / CUBIC_STEPS as f32;
                    let v = 1.0 - u;
                    let b = |a: f32, b: f32, c: f32, d: f32| {
                        v * v * v * a + 3.0 * v * v * u * b + 3.0 * v * u * u * c + u * u * u * d
                    };
                    run.points
                        .push((b(cur.0, c1.0, c2.0, p.0), b(cur.1, c1.1, c2.1, p.1)));
                }
                cur = p;
                last_ctrl = Some(c2);
            }
            b'A' => {
                let (rx, ry, rot) = (t.number()?, t.number()?, t.number()?);
                let (large, sweep) = (t.flag()?, t.flag()?);
                let p = off((t.number()?, t.number()?), cur);
                run.corners.push(run.points.len() - 1);
                run.points.extend(arc(cur, p, rx, ry, rot, large, sweep));
                cur = p;
                last_ctrl = None;
            }
            b'Z' => {
                run.closed = true;
                if let Some(first) = run.points.first().copied() {
                    let close = (first.0 - cur.0).abs() < 1e-3 && (first.1 - cur.1).abs() < 1e-3;
                    if close {
                        run.points.pop();
                    }
                }
                run.corners.push(run.points.len().saturating_sub(1));
                finish(&mut run, &mut runs);
                cur = start;
                last_ctrl = None;
                match t.command() {
                    Some(c) => {
                        cmd = c;
                        // A new subpath after Z starts where the last began.
                        if !matches!(c.to_ascii_uppercase(), b'M') {
                            run.points.push(cur);
                        }
                        continue;
                    }
                    None => break,
                }
            }
            other => return Err(format!("unsupported path command {}", other as char)),
        }
        if !t.at_number() {
            match t.command() {
                Some(c) => cmd = c,
                None => break,
            }
        }
    }
    finish(&mut run, &mut runs);
    for r in &mut runs {
        r.corners.retain(|i| *i < r.points.len());
        r.corners.dedup();
    }
    Ok(runs)
}

/// An SVG elliptical arc from `p0` to `p1` (endpoint → centre
/// parameterisation, SVG 1.1 F.6.5), sampled; excludes `p0`.
fn arc(
    p0: (f32, f32),
    p1: (f32, f32),
    rx: f32,
    ry: f32,
    rot_deg: f32,
    large: bool,
    sweep: bool,
) -> Vec<(f32, f32)> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        return vec![p1];
    }
    let phi = rot_deg.to_radians();
    let (cos, sin) = (phi.cos(), phi.sin());
    let (dx, dy) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large == sweep {
        co = -co;
    }
    let (cxp, cyp) = (co * rx * y1 / ry, -co * ry * x1 / rx);
    let (cx, cy) = (
        cos * cxp - sin * cyp + (p0.0 + p1.0) / 2.0,
        sin * cxp + cos * cyp + (p0.1 + p1.1) / 2.0,
    );
    let angle = |ux: f32, uy: f32| uy.atan2(ux);
    let t1 = angle((x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut dt = angle((-x1 - cxp) / rx, (-y1 - cyp) / ry) - t1;
    if sweep && dt < 0.0 {
        dt += TAU;
    } else if !sweep && dt > 0.0 {
        dt -= TAU;
    }
    let n = ((dt.abs() / TAU) * ARC_STEPS).ceil().max(2.0) as usize;
    (1..=n)
        .map(|i| {
            let a = t1 + dt * i as f32 / n as f32;
            let (x, y) = (rx * a.cos(), ry * a.sin());
            (cos * x - sin * y + cx, sin * x + cos * y + cy)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_the_device_uses_parses_inside_its_box() {
        for icon in Icon::ALL {
            let o = parse(icon.svg()).unwrap_or_else(|e| panic!("{icon:?}: {e}"));
            assert_eq!(o.stroke_width, 2.0, "{icon:?}");
            assert!(
                !o.runs.is_empty() || !o.dots.is_empty(),
                "{icon:?} is empty"
            );
            for r in &o.runs {
                assert!(r.points.len() >= 2, "{icon:?} has a degenerate run");
                for (x, y) in &r.points {
                    assert!(
                        (-0.5..=24.5).contains(x) && (-0.5..=24.5).contains(y),
                        "{icon:?}: ({x}, {y}) is outside the 24 box"
                    );
                }
            }
        }
    }

    #[test]
    fn the_whole_brand_set_stays_in_the_subset() {
        // Not just the icons wired up above: a new icon drawn in brand/gen.py
        // must still be drawable here.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../brand/icons");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let svg = std::fs::read_to_string(&p).unwrap();
            parse(&svg).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
            n += 1;
        }
        assert!(n >= 40);
    }

    #[test]
    fn shapes_land_where_the_svg_says() {
        // check: one open polyline through three corners.
        let o = parse(Icon::Check.svg()).unwrap();
        assert_eq!(o.runs.len(), 1);
        assert_eq!(
            o.runs[0].points,
            vec![(5.0, 12.5), (9.5, 17.0), (19.0, 7.0)]
        );
        assert!(!o.runs[0].closed);
        // pause: two vertical bars, relative v.
        let o = parse(Icon::Pause.svg()).unwrap();
        assert_eq!(o.runs.len(), 2);
        assert_eq!(o.runs[1].points, vec![(15.0, 6.0), (15.0, 18.0)]);
        // info: a circle, a stem and a filled dot.
        let o = parse(Icon::Info.svg()).unwrap();
        assert_eq!(o.dots.len(), 1);
        assert!(o.runs[0].closed);
        // allowed-hours: an arc plus a dashed arc (several dashes).
        let o = parse(Icon::AllowedHours.svg()).unwrap();
        assert!(o.runs.len() > 3, "{}", o.runs.len());
        // moon: arcs end where they say (the crescent closes on its start).
        let o = parse(Icon::Moon.svg()).unwrap();
        let first = o.runs[0].points[0];
        assert_eq!(first, (12.0, 3.0));
        assert!(o.runs[0].closed);
        // An arc of a 9-radius circle stays on it.
        let o = parse(Icon::Clock.svg()).unwrap();
        for (x, y) in &o.runs[0].points {
            let d = ((x - 12.0).powi(2) + (y - 12.0).powi(2)).sqrt();
            assert!((d - 9.0).abs() < 0.01);
        }
    }
}
