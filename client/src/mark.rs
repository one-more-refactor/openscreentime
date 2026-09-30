//! The mark, rasterised: the house-clock ring as pixels, for the places that
//! want pixels and not a painter — the tray icon (a StatusNotifierItem pixmap
//! showing the real share of the day used), the window icon, and the image on
//! a warning notification (as SVG text).
//!
//! Same geometry as `brand/gen.py` (`ring_svg_inner`), the one source of the
//! mark: the arc starts at the 12 o'clock tick (butt end, under the tick) and
//! sweeps clockwise to a round cap; a full ring has no cap; the tick is painted
//! last. Pure Rust, no dependencies: every shape is a point-in-shape test,
//! supersampled 4×4 per pixel.

#![cfg_attr(not(all(feature = "gui", feature = "tray")), allow(dead_code))]

use std::f32::consts::{FRAC_PI_2, TAU};

/// sRGB colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba(r, g, b, 255)
    }
    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba(self.0, self.1, self.2, a)
    }
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

// The brand's light tokens (brand/gen.py `L`).
pub const PAPER: Rgba = Rgba::rgb(0xf4, 0xf2, 0xee);
pub const TILE_EDGE: Rgba = Rgba::rgb(0xe0, 0xdc, 0xd4);
pub const TRACK: Rgba = Rgba::rgb(0xd5, 0xd1, 0xc9);
pub const INK: Rgba = Rgba::rgb(0x1e, 0x1c, 0x19);
pub const BRAND: Rgba = Rgba::rgb(0x2e, 0x7d, 0x46);
pub const WARN: Rgba = Rgba::rgb(0x8a, 0x63, 0x00);
pub const STOP: Rgba = Rgba::rgb(0xb3, 0x15, 0x1c);
/// A card's track (the ring on a white surface).
pub const TRACK_CARD: Rgba = Rgba::rgb(0xe6, 0xe3, 0xdd);

/// What the ring shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ring {
    /// Time used today as a share of the day's time (0..=1), in a colour.
    Fill { frac: f32, color: Rgba },
    /// Complete: the day's time is used.
    Full { color: Rgba },
    /// A parent paused it: a dashed ring, no track.
    Paused { color: Rgba },
    /// No limit today: track only.
    Track,
}

/// Ring geometry in pixels.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub sw: f32,
    pub tick_w: f32,
    pub tick_len: f32,
}

impl Geometry {
    /// The mark in a square of `size` px (gen.py `mark_inner`: 64 box, ring
    /// r 22, stroke 7, tick 3 × 11).
    pub fn mark(size: f32) -> Geometry {
        let s = size / 64.0;
        Geometry {
            cx: 32.0 * s,
            cy: 32.0 * s,
            r: 22.0 * s,
            sw: 7.0 * s,
            tick_w: 3.0 * s,
            tick_len: 11.0 * s,
        }
    }

    /// The tray weight (gen.py `tray`: 22 box, r 7.5, stroke 3, tick 2 × 6),
    /// centred in a square of `size` px.
    pub fn tray(size: f32) -> Geometry {
        let s = size / 22.0;
        Geometry {
            cx: 11.0 * s,
            cy: 11.5 * s,
            r: 7.5 * s,
            sw: 3.0 * s,
            tick_w: 2.0 * s,
            tick_len: 6.0 * s,
        }
    }

    /// The ring a UI draws at diameter `d` (brand/build.py `ring`): stroke
    /// 9 % of the diameter at hero sizes, 8 % from 48 px, 7 % below; the ring
    /// pulled in so the tick's overhang stays inside the box.
    pub fn ui(d: f32) -> Geometry {
        let sw = (d * if d >= 120.0 {
            0.09
        } else if d >= 48.0 {
            0.08
        } else {
            0.07
        })
        .round()
        .max(2.0);
        let tick = d >= 40.0;
        let r = d / 2.0 - sw / 2.0 - if tick { sw * 0.35 } else { 0.0 };
        Geometry {
            cx: d / 2.0,
            cy: d / 2.0,
            r,
            sw,
            tick_w: if tick {
                (sw * 0.43 * 10.0).round() / 10.0
            } else {
                0.0
            },
            tick_len: if tick {
                (sw * 1.6 * 10.0).round() / 10.0
            } else {
                0.0
            },
        }
    }
}

/// The angle (radians, 0 = 12 o'clock, clockwise) of a point around the centre.
fn clock_angle(dx: f32, dy: f32) -> f32 {
    let a = dy.atan2(dx) + FRAC_PI_2; // atan2 is 0 at 3 o'clock, y grows down
    a.rem_euclid(TAU)
}

fn in_capsule(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> bool {
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 {
        (((px - ax) * vx + (py - ay) * vy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (ax + vx * t - px, ay + vy * t - py);
    qx * qx + qy * qy <= r * r
}

/// One layer of the drawing: a colour and a point test.
struct Layer<'a> {
    color: Rgba,
    hit: Box<dyn Fn(f32, f32) -> bool + 'a>,
}

/// The ring's layers, bottom to top: track, arc (+ cap), tick.
fn ring_layers(g: Geometry, ring: Ring, track: Rgba, tick: Option<Rgba>) -> Vec<Layer<'static>> {
    let mut out: Vec<Layer<'static>> = Vec::new();
    let (inner, outer) = (g.r - g.sw / 2.0, g.r + g.sw / 2.0);
    let annulus = move |x: f32, y: f32| {
        let d = ((x - g.cx).powi(2) + (y - g.cy).powi(2)).sqrt();
        d >= inner && d <= outer
    };
    match ring {
        Ring::Paused { color } => {
            // gen.py: stroke-dasharray = circumference × (1.2 % on, 5.5 % off),
            // round caps, starting at 3 o'clock like any SVG circle.
            let circ = TAU * g.r;
            let (on, off) = (circ * 0.012, circ * 0.055);
            let period = on + off;
            let n = (circ / period).floor() as usize;
            let mut dashes = Vec::with_capacity(n + 1);
            let mut s = 0.0;
            while s < circ {
                let (a0, a1) = (s / g.r, (s + on).min(circ) / g.r);
                // SVG angle 0 = 3 o'clock, clockwise (y down).
                let p = |a: f32| (g.cx + g.r * a.cos(), g.cy + g.r * a.sin());
                dashes.push((p(a0), p(a1), a0, a1));
                s += period;
            }
            out.push(Layer {
                color,
                hit: Box::new(move |x, y| {
                    dashes.iter().any(|&((ax, ay), (bx, by), a0, a1)| {
                        // The arc between the two ends (short), plus round caps.
                        let a = (y - g.cy).atan2(x - g.cx).rem_euclid(TAU);
                        (annulus(x, y) && a >= a0 && a <= a1)
                            || in_capsule(x, y, ax, ay, ax, ay, g.sw / 2.0)
                            || in_capsule(x, y, bx, by, bx, by, g.sw / 2.0)
                    })
                }),
            });
        }
        Ring::Track => {
            out.push(Layer {
                color: track,
                hit: Box::new(annulus),
            });
        }
        Ring::Full { color } => {
            out.push(Layer {
                color,
                hit: Box::new(annulus),
            });
        }
        Ring::Fill { frac, color } => {
            out.push(Layer {
                color: track,
                hit: Box::new(annulus),
            });
            let f = frac.clamp(0.0, 1.0);
            if f >= 0.999 {
                out.push(Layer {
                    color,
                    hit: Box::new(annulus),
                });
            } else if f > 0.0 {
                let sweep = TAU * f;
                let end = sweep - FRAC_PI_2; // screen angle of the cap
                let (ex, ey) = (g.cx + g.r * end.cos(), g.cy + g.r * end.sin());
                out.push(Layer {
                    color,
                    hit: Box::new(move |x, y| {
                        (annulus(x, y) && clock_angle(x - g.cx, y - g.cy) <= sweep)
                            || in_capsule(x, y, ex, ey, ex, ey, g.sw / 2.0)
                    }),
                });
            }
        }
    }
    if let Some(tick) = tick.filter(|_| g.tick_len > 0.0) {
        let (x, y0, y1) = (
            g.cx,
            g.cy - g.r - g.tick_len / 2.0,
            g.cy - g.r + g.tick_len / 2.0,
        );
        // A round-capped stroke of length tick_len (caps included, as in SVG
        // the caps extend past the ends): a capsule between the two ends.
        out.push(Layer {
            color: tick,
            hit: Box::new(move |px, py| in_capsule(px, py, x, y0, x, y1, g.tick_w / 2.0)),
        });
    }
    out
}

/// Composite layers into a `size`² straight-alpha RGBA buffer.
fn render(size: u32, bg: Option<Rgba>, layers: &[Layer<'_>]) -> Vec<u8> {
    const SS: u32 = 4;
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            // Accumulate premultiplied colour over the subsamples.
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let (mut cr, mut cg, mut cb, mut ca) = match bg {
                        Some(c) => {
                            let al = c.3 as f32 / 255.0;
                            (c.0 as f32 * al, c.1 as f32 * al, c.2 as f32 * al, al)
                        }
                        None => (0.0, 0.0, 0.0, 0.0),
                    };
                    for l in layers {
                        if (l.hit)(x, y) {
                            let al = l.color.3 as f32 / 255.0;
                            cr = l.color.0 as f32 * al + cr * (1.0 - al);
                            cg = l.color.1 as f32 * al + cg * (1.0 - al);
                            cb = l.color.2 as f32 * al + cb * (1.0 - al);
                            ca = al + ca * (1.0 - al);
                        }
                    }
                    r += cr;
                    g += cg;
                    b += cb;
                    a += ca;
                }
            }
            let n = (SS * SS) as f32;
            let (r, g, b, a) = (r / n, g / n, b / n, a / n);
            let i = ((py * size + px) * 4) as usize;
            if a > 0.0 {
                buf[i] = (r / a).round().clamp(0.0, 255.0) as u8;
                buf[i + 1] = (g / a).round().clamp(0.0, 255.0) as u8;
                buf[i + 2] = (b / a).round().clamp(0.0, 255.0) as u8;
                buf[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    buf
}

/// The app icon (brand/app-icon.svg) as RGBA, `size`² — the paper tile with
/// the mark at 40 %. For the window icon on X11.
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32 / 64.0;
    // gen.py `squircle`: a rounded square, corner radius 0.28 of the side,
    // with a 1 px edge (stroke centred on the outline).
    let (w, rad, edge) = (size as f32, 0.28 * size as f32, s.max(1.0));
    let rounded = move |x: f32, y: f32, inset: f32| {
        let (lo, hi) = (inset, w - inset);
        if x < lo || x > hi || y < lo || y > hi {
            return false;
        }
        let r = (rad - inset).max(0.0);
        let cx = x.clamp(lo + r, hi - r);
        let cy = y.clamp(lo + r, hi - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    let mut layers = vec![
        Layer {
            color: TILE_EDGE,
            hit: Box::new(move |x, y| rounded(x, y, 0.0)),
        },
        Layer {
            color: PAPER,
            hit: Box::new(move |x, y| rounded(x, y, edge)),
        },
    ];
    layers.extend(ring_layers(
        Geometry::mark(size as f32),
        Ring::Fill {
            frac: 0.40,
            color: BRAND,
        },
        TRACK,
        Some(INK),
    ));
    render(size, None, &layers)
}

/// A ring alone as RGBA (transparent background).
pub fn ring_rgba(size: u32, g: Geometry, ring: Ring, track: Rgba, tick: Option<Rgba>) -> Vec<u8> {
    render(size, None, &ring_layers(g, ring, track, tick))
}

/// What the tray shows (brand/tray-*.svg), with the real share of the day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayState {
    /// Plenty of time: green, at the share of the day used.
    Ok { frac: f32 },
    /// Fifteen minutes or less: amber, at the share used.
    Low { frac: f32 },
    /// Time's up / stopped: the full red ring.
    Stopped,
    /// A parent paused it: the dashed ring.
    Paused,
    /// No limit, or not running: the track and the tick.
    Idle,
}

/// The panel's foreground colour is unknown to a pixmap (the SVGs use
/// `currentColor`), so the neutral parts use a mid grey that reads on light
/// and dark panels alike.
const TRAY_NEUTRAL: Rgba = Rgba::rgb(0x8c, 0x87, 0x80);

/// The tray icon as ksni wants it: ARGB32, network byte order, `size`².
pub fn tray_argb(size: u32, state: TrayState) -> Vec<u8> {
    let g = Geometry::tray(size as f32);
    let ring = match state {
        TrayState::Ok { frac } => Ring::Fill { frac, color: BRAND },
        TrayState::Low { frac } => Ring::Fill { frac, color: WARN },
        TrayState::Stopped => Ring::Full { color: STOP },
        TrayState::Paused => Ring::Paused {
            color: TRAY_NEUTRAL,
        },
        TrayState::Idle => Ring::Track,
    };
    let rgba = ring_rgba(
        size,
        g,
        ring,
        TRAY_NEUTRAL.with_alpha(0x60),
        Some(TRAY_NEUTRAL),
    );
    rgba.chunks_exact(4)
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}

/// The ring as an SVG document (gen.py geometry), for a notification's image:
/// the amber ring of board 05b, drawn at the real share of the day.
pub fn ring_svg(d: f32, ring: Ring, track: Rgba) -> String {
    let g = Geometry::ui(d);
    let mut body = String::new();
    let circle = |stroke: &str, extra: &str| {
        format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="none" stroke="{stroke}" stroke-width="{:.2}"{extra}/>"#,
            g.cx, g.cy, g.r, g.sw
        )
    };
    match ring {
        Ring::Paused { color } => {
            let circ = TAU * g.r;
            body.push_str(&circle(
                &color.hex(),
                &format!(
                    r#" stroke-linecap="round" stroke-dasharray="{:.2} {:.2}""#,
                    circ * 0.012,
                    circ * 0.055
                ),
            ));
        }
        Ring::Track => body.push_str(&circle(&track.hex(), "")),
        Ring::Full { color } => {
            body.push_str(&circle(&track.hex(), ""));
            body.push_str(&circle(&color.hex(), ""));
        }
        Ring::Fill { frac, color } => {
            body.push_str(&circle(&track.hex(), ""));
            let f = frac.clamp(0.0, 1.0);
            if f >= 0.999 {
                body.push_str(&circle(&color.hex(), ""));
            } else if f > 0.0 {
                let p = |f: f32| {
                    let a = -FRAC_PI_2 + TAU * f;
                    (g.cx + g.r * a.cos(), g.cy + g.r * a.sin())
                };
                let ((sx, sy), (ex, ey)) = (p(0.0), p(f));
                let large = if f > 0.5 { 1 } else { 0 };
                body.push_str(&format!(
                    r#"<path d="M{sx:.2} {sy:.2}A{r:.2} {r:.2} 0 {large} 1 {ex:.2} {ey:.2}" fill="none" stroke="{c}" stroke-width="{sw:.2}"/><circle cx="{ex:.2}" cy="{ey:.2}" r="{h:.2}" fill="{c}"/>"#,
                    r = g.r,
                    c = color.hex(),
                    sw = g.sw,
                    h = g.sw / 2.0
                ));
            }
        }
    }
    if g.tick_len > 0.0 {
        body.push_str(&format!(
            r#"<path d="M{:.2} {:.2}v{:.2}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round" fill="none"/>"#,
            g.cx,
            g.cy - g.r - g.tick_len / 2.0,
            g.tick_len,
            INK.hex(),
            g.tick_w
        ));
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{d}" height="{d}" viewBox="0 0 {d} {d}">{body}</svg>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// The pixel on the ring's centre line at a clock position (0..1).
    fn on_ring(g: Geometry, at: f32) -> (u32, u32) {
        let a = -FRAC_PI_2 + TAU * at;
        (
            (g.cx + g.r * a.cos()).floor() as u32,
            (g.cy + g.r * a.sin()).floor() as u32,
        )
    }

    fn is(c: [u8; 4], want: Rgba) -> bool {
        c[3] == 255 && c[0] == want.0 && c[1] == want.1 && c[2] == want.2
    }

    #[test]
    fn the_arc_covers_the_share_of_the_day_clockwise_from_the_tick() {
        let size = 128;
        let g = Geometry::mark(size as f32);
        let buf = ring_rgba(
            size,
            g,
            Ring::Fill {
                frac: 0.64,
                color: BRAND,
            },
            TRACK,
            None,
        );
        for at in [0.05, 0.25, 0.5, 0.6] {
            let (x, y) = on_ring(g, at);
            assert!(is(px(&buf, size, x, y), BRAND), "{at} should be used");
        }
        for at in [0.72, 0.8, 0.95] {
            let (x, y) = on_ring(g, at);
            assert!(is(px(&buf, size, x, y), TRACK), "{at} should be left");
        }
        // The centre stays empty.
        assert_eq!(px(&buf, size, 64, 64)[3], 0);
    }

    #[test]
    fn tray_pixmaps_follow_the_state() {
        let size = 32;
        let g = Geometry::tray(size as f32);
        let argb = |s| tray_argb(size, s);
        let at = |buf: &[u8], f: f32| {
            let (x, y) = on_ring(g, f);
            let i = ((y * size + x) * 4) as usize;
            [buf[i + 1], buf[i + 2], buf[i + 3], buf[i]] // back to RGBA
        };
        let ok = argb(TrayState::Ok { frac: 0.3 });
        assert_eq!(ok.len(), (size * size * 4) as usize);
        assert!(is(at(&ok, 0.2), BRAND));
        assert!(!is(at(&ok, 0.6), BRAND), "past the share is track, not arc");
        let low = argb(TrayState::Low { frac: 0.9 });
        assert!(is(at(&low, 0.6), WARN));
        let stopped = argb(TrayState::Stopped);
        assert!(is(at(&stopped, 0.6), STOP) && is(at(&stopped, 0.95), STOP));
        // More of the day used, more of the ring coloured.
        let count = |buf: &[u8], c: Rgba| {
            buf.chunks_exact(4)
                .filter(|p| p[0] == 255 && p[1] == c.0 && p[2] == c.1 && p[3] == c.2)
                .count()
        };
        let a = count(&argb(TrayState::Ok { frac: 0.25 }), BRAND);
        let b = count(&argb(TrayState::Ok { frac: 0.75 }), BRAND);
        assert!(b > a * 2, "{a} vs {b}");
        // Paused: no red, no green.
        let paused = argb(TrayState::Paused);
        assert_eq!(count(&paused, BRAND) + count(&paused, STOP), 0);
    }

    #[test]
    fn the_app_icon_is_a_paper_tile_with_the_mark() {
        let size = 64;
        let buf = app_icon_rgba(size);
        // Tile corners are rounded away; the middle of an edge is the edge colour.
        assert_eq!(px(&buf, size, 0, 0)[3], 0);
        assert!(is(px(&buf, size, 20, 4), PAPER));
        // The tick sits at 12 o'clock over the ring, in ink.
        assert!(is(px(&buf, size, 32, 10), INK));
        // 40 % of the ring is green: 3 o'clock yes, 9 o'clock no.
        assert!(is(px(&buf, size, 54, 32), BRAND));
        assert!(is(px(&buf, size, 10, 32), TRACK));
    }

    #[test]
    fn the_svg_ring_matches_the_board() {
        let s = ring_svg(
            44.0,
            Ring::Fill {
                frac: 0.94,
                color: WARN,
            },
            TRACK_CARD,
        );
        assert!(s.starts_with("<svg"));
        assert!(s.contains("#8a6300"), "amber arc");
        assert!(s.contains("#e6e3dd"), "card track");
        assert!(s.contains(" 0 1 1 "), "a large, clockwise arc");
        assert!(s.contains("stroke-linecap=\"round\""), "the tick");
    }
}
