//! The house clock on the device: the brand board's tokens, type, ring, icons
//! and buttons as egui building blocks, shared by the app window and the lock
//! so they can't drift apart (brand/board.html, docs/DESIGN-CLIENT.md).
//! Compiled only with `--features gui`.
//!
//! One idea, carried to the last screen: the ring is time used today, filling
//! clockwise from the tick at 12. The lock is that ring completed — the gauge
//! finished, not an alarm raised.

#![cfg(feature = "gui")]
// A shared token + helper module: not every token/helper is used by every
// surface, and that's fine — they are the language, kept complete on purpose.
#![allow(dead_code)]

use crate::icons::Icon;
use crate::mark::Geometry;
use eframe::egui;
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};

// ── Tokens (brand/board.html, light — the device is always light) ────────────
pub const BG: (u8, u8, u8) = (0xf4, 0xf2, 0xee); // paper
pub const SURFACE: (u8, u8, u8) = (0xff, 0xff, 0xff);
pub const SURFACE_2: (u8, u8, u8) = (0xec, 0xeb, 0xe6);
pub const LINE: (u8, u8, u8) = (0xe6, 0xe3, 0xdd); // hairline; the ring's track on a card
pub const LINE_2: (u8, u8, u8) = (0xd5, 0xd1, 0xc9); // field edge; the ring's track on paper
pub const INK: (u8, u8, u8) = (0x1e, 0x1c, 0x19);
pub const INK_2: (u8, u8, u8) = (0x57, 0x54, 0x4e);
pub const INK_3: (u8, u8, u8) = (0x72, 0x6e, 0x66);
pub const BRAND: (u8, u8, u8) = (0x2e, 0x7d, 0x46); // the ring, the one action
pub const BRAND_STRONG: (u8, u8, u8) = (0x26, 0x6a, 0x3b);
pub const BRAND_TINT: (u8, u8, u8) = (0xe4, 0xf1, 0xe8);
pub const BRAND_INK: (u8, u8, u8) = (0x1c, 0x5c, 0x33);
pub const WARN: (u8, u8, u8) = (0x8a, 0x63, 0x00); // the one transition
pub const WARN_TINT: (u8, u8, u8) = (0xf7, 0xed, 0xd6);
pub const STOP: (u8, u8, u8) = (0xb3, 0x15, 0x1c); // the stop
pub const STOP_TINT: (u8, u8, u8) = (0xf8, 0xe3, 0xe2);

pub fn col(c: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Blend two token colours (`t` in 0..=1 toward `b`).
pub fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

// ── Type ─────────────────────────────────────────────────────────────────────

/// Figtree's weights, bundled as static instances (egui has no variable-font
/// axes: the variable file renders Light, whatever `.strong()` says).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum W {
    Regular,
    Medium,
    SemiBold,
    Bold,
    ExtraBold,
}

impl W {
    const ALL: [W; 5] = [W::Regular, W::Medium, W::SemiBold, W::Bold, W::ExtraBold];

    fn family(self) -> &'static str {
        match self {
            W::Regular => "fig-400",
            W::Medium => "fig-500",
            W::SemiBold => "fig-600",
            W::Bold => "fig-700",
            W::ExtraBold => "fig-800",
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            W::Regular => include_bytes!("../fonts/Figtree-Regular.ttf"),
            W::Medium => include_bytes!("../fonts/Figtree-Medium.ttf"),
            W::SemiBold => include_bytes!("../fonts/Figtree-SemiBold.ttf"),
            W::Bold => include_bytes!("../fonts/Figtree-Bold.ttf"),
            W::ExtraBold => include_bytes!("../fonts/Figtree-ExtraBold.ttf"),
        }
    }
}

/// Figtree at a size and weight.
pub fn font(size: f32, w: W) -> FontId {
    FontId::new(size, FontFamily::Name(w.family().into()))
}

/// Space Mono — the one place mono is allowed: codes.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// A one-line text job with the board's tracking (`em`, e.g. -0.03).
pub fn job(text: &str, font: FontId, color: Color32, tracking_em: f32) -> egui::text::LayoutJob {
    let mut j = egui::text::LayoutJob::default();
    j.append(
        text,
        0.0,
        egui::TextFormat {
            extra_letter_spacing: tracking_em * font.size,
            font_id: font,
            color,
            ..Default::default()
        },
    );
    j
}

/// Register the fonts and the light chrome. The lock runs as `ost-lock`, with
/// no home and no fontconfig to lean on, so everything is in the binary.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // egui's own proportional list, as fallbacks after Figtree (scripts and
    // symbols Figtree doesn't carry).
    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for w in W::ALL {
        let name = format!("figtree-{}", w.family());
        fonts
            .font_data
            .insert(name.clone(), egui::FontData::from_static(w.bytes()));
        let mut list = vec![name];
        list.extend(fallbacks.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(w.family().into()), list);
    }
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, format!("figtree-{}", W::Regular.family()));
    fonts.font_data.insert(
        "ost-mono".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/SpaceMono-Regular.ttf")),
    );
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "ost-mono".to_owned());
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::light();
    v.panel_fill = col(BG);
    v.window_fill = col(SURFACE);
    v.extreme_bg_color = col(SURFACE);
    v.override_text_color = Some(col(INK));
    v.selection.bg_fill = col(BRAND_TINT);
    v.selection.stroke = Stroke::new(1.0, col(BRAND));
    v.text_cursor.stroke = Stroke::new(2.0, col(INK));
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.noninteractive,
    ] {
        w.rounding = egui::Rounding::same(10.0);
    }
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, col(LINE_2));
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, col(BRAND));
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles = [
            (Heading, font(22.0, W::Bold)),
            (Body, font(15.0, W::Regular)),
            (Button, font(15.0, W::SemiBold)),
            (Small, font(12.5, W::Regular)),
            (Monospace, mono(15.0)),
        ]
        .into();
    });
}

/// The window icon: the app icon (brand/app-icon.svg), rasterised. X11 uses
/// it; Wayland finds the installed icon through the app id.
pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: crate::mark::app_icon_rgba(128),
        width: 128,
        height: 128,
    }
}

/// Every window's identity: the icon, and `openscreentime` as the Wayland
/// app id / X11 class, matching the .desktop files' `StartupWMClass`.
pub fn identity(v: egui::ViewportBuilder) -> egui::ViewportBuilder {
    v.with_title("OpenScreenTime")
        .with_app_id("openscreentime")
        .with_icon(window_icon())
}

// ── The ring (brand/gen.py geometry, every state) ────────────────────────────

pub enum RingState {
    /// Time used today as a share of the day's time, clockwise from the tick.
    Fill { frac: f32, color: (u8, u8, u8) },
    /// Complete: the day's time is used.
    Full { color: (u8, u8, u8) },
    /// A parent paused it: the dashed ink-3 ring, no track.
    Paused,
    /// No limit today, or nothing known: the track (and the tick) only.
    Track,
}

/// Draw the ring centred at `c` in a box of diameter `d`. `on_card`: the ring
/// sits on white (lighter track) rather than on paper.
pub fn ring(painter: &egui::Painter, c: Pos2, d: f32, state: &RingState, on_card: bool) {
    let g = Geometry::ui(d);
    let (r, sw) = (g.r, g.sw);
    let track = col(if on_card { LINE } else { LINE_2 });
    let top = -std::f32::consts::FRAC_PI_2;
    let tau = std::f32::consts::TAU;
    let at = |a: f32| c + Vec2::new(a.cos(), a.sin()) * r;
    let arc_pts = |a0: f32, a1: f32| -> Vec<Pos2> {
        let n = ((a1 - a0).abs() / 0.03).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| at(a0 + (a1 - a0) * i as f32 / n as f32))
            .collect()
    };
    match state {
        RingState::Paused => {
            // Dashes 1.2 % of the way round, gaps 5.5 %, round caps, from
            // 3 o'clock like the SVG.
            let circ = tau * r;
            let (on, off) = (circ * 0.012, circ * 0.055);
            let mut s = 0.0;
            while s < circ {
                let (a0, a1) = (s / r, (s + on).min(circ) / r);
                painter.add(egui::Shape::line(
                    arc_pts(a0, a1),
                    Stroke::new(sw, col(INK_3)),
                ));
                painter.circle_filled(at(a0), sw / 2.0, col(INK_3));
                painter.circle_filled(at(a1), sw / 2.0, col(INK_3));
                s += on + off;
            }
        }
        RingState::Track => {
            painter.circle_stroke(c, r, Stroke::new(sw, track));
        }
        RingState::Full { color } => {
            painter.circle_stroke(c, r, Stroke::new(sw, col(*color)));
        }
        RingState::Fill { frac, color } => {
            painter.circle_stroke(c, r, Stroke::new(sw, track));
            let f = frac.clamp(0.0, 1.0);
            if f >= 0.999 {
                painter.circle_stroke(c, r, Stroke::new(sw, col(*color)));
            } else if f > 0.0 {
                let end = top + tau * f;
                painter.add(egui::Shape::line(
                    arc_pts(top, end),
                    Stroke::new(sw, col(*color)),
                ));
                painter.circle_filled(at(end), sw / 2.0, col(*color));
            }
        }
    }
    if g.tick_len > 0.0 {
        let x = c.x;
        let (y0, y1) = (c.y - r - g.tick_len / 2.0, c.y - r + g.tick_len / 2.0);
        painter.line_segment(
            [Pos2::new(x, y0), Pos2::new(x, y1)],
            Stroke::new(g.tick_w, col(INK)),
        );
        painter.circle_filled(Pos2::new(x, y0), g.tick_w / 2.0, col(INK));
        painter.circle_filled(Pos2::new(x, y1), g.tick_w / 2.0, col(INK));
    }
}

/// The number and its label inside a ring (board `.rw .in`): the number in
/// ExtraBold with -0.03em tracking, the label under it in ink-2.
#[allow(clippy::too_many_arguments)]
pub fn ring_center(
    painter: &egui::Painter,
    c: Pos2,
    big: &str,
    big_size: f32,
    big_color: Color32,
    label: &str,
    label_size: f32,
    gap: f32,
) {
    // No number (a pause, nothing known): the label alone. Never lay out a
    // zero-size font — egui panics on it.
    let big = if big_size > 0.0 { big } else { "" };
    let lab = painter.layout_job(job(label, font(label_size, W::Regular), col(INK_2), 0.0));
    // The number's box is taller than its digits (line height ~1.2 em):
    // centre on cap height, not on the box.
    let cap = big_size * 0.72;
    let block = if big.is_empty() {
        lab.size().y
    } else {
        cap + gap + lab.size().y
    };
    let top = c.y - block / 2.0;
    if !big.is_empty() {
        let num = painter.layout_job(job(big, font(big_size, W::ExtraBold), big_color, -0.03));
        let digits_top = top - (num.size().y - big_size) / 2.0 - big_size * 0.14;
        painter.galley(
            Pos2::new(c.x - num.size().x / 2.0, digits_top),
            num,
            big_color,
        );
    }
    let label_top = if big.is_empty() { top } else { top + cap + gap };
    painter.galley(
        Pos2::new(c.x - lab.size().x / 2.0, label_top),
        lab,
        col(INK_2),
    );
}

// ── Icons (brand/icons/*.svg, via crate::icons) ──────────────────────────────

/// Stroke an icon into `rect` (square) in `color`, with round caps and joins
/// like the SVG.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let o = icon.outline();
    let s = rect.width() / 24.0;
    let w = o.stroke_width * s;
    let map = |(x, y): (f32, f32)| Pos2::new(rect.min.x + x * s, rect.min.y + y * s);
    for run in &o.runs {
        let pts: Vec<Pos2> = run.points.iter().copied().map(map).collect();
        if run.closed {
            painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(w, color)));
        } else {
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(w, color)));
            if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                painter.circle_filled(*a, w / 2.0, color);
                painter.circle_filled(*b, w / 2.0, color);
            }
        }
        for &i in &run.corners {
            painter.circle_filled(pts[i], w / 2.0, color);
        }
    }
    for d in &o.dots {
        painter.circle_filled(map((d.cx, d.cy)), (d.r + o.stroke_width / 2.0) * s, color);
    }
}

/// An icon as a widget of `size` px.
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint_icon(ui.painter(), rect, icon, color);
    resp
}

// ── The lockup: the mark + "Open" + "ScreenTime" ──────────────────────────────

/// The horizontal lockup at font size `size` (board `.lockup`). `quiet`: the
/// lock's version — mark and words in ink, so the ring is the only colour.
pub fn lockup(ui: &mut egui::Ui, size: f32, quiet: bool) -> egui::Response {
    let open_col = col(INK_2);
    let st_col = col(if quiet { INK_2 } else { INK });
    let open = ui
        .painter()
        .layout_job(job("Open", font(size, W::Medium), open_col, -0.02));
    let st = ui
        .painter()
        .layout_job(job("ScreenTime", font(size, W::Bold), st_col, -0.02));
    let mark = size * 0.96;
    let gap = size * 0.22;
    let w = mark + gap + open.size().x + st.size().x;
    let h = open.size().y.max(mark);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
    let p = ui.painter();
    let mc = Pos2::new(rect.min.x + mark / 2.0, rect.center().y);
    paint_mark(p, mc, mark, if quiet { INK_3 } else { BRAND });
    let tx = rect.min.x + mark + gap;
    let ty = rect.center().y - open.size().y / 2.0;
    let ow = open.size().x;
    p.galley(Pos2::new(tx, ty), open, open_col);
    p.galley(Pos2::new(tx + ow, ty), st, st_col);
    resp
}

/// The mark (gen.py `mark_inner`): ring at 40 % with its tick, in a box `d`.
pub fn paint_mark(p: &egui::Painter, c: Pos2, d: f32, arc: (u8, u8, u8)) {
    let g = Geometry::mark(d);
    let off = Vec2::new(c.x - d / 2.0, c.y - d / 2.0);
    let center = Pos2::new(g.cx, g.cy) + off;
    let top = -std::f32::consts::FRAC_PI_2;
    p.circle_stroke(center, g.r, Stroke::new(g.sw, col(LINE_2)));
    let end = top + std::f32::consts::TAU * 0.40;
    let n = 40;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = top + (end - top) * i as f32 / n as f32;
            center + Vec2::new(a.cos(), a.sin()) * g.r
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(g.sw, col(arc))));
    p.circle_filled(
        center + Vec2::new(end.cos(), end.sin()) * g.r,
        g.sw / 2.0,
        col(arc),
    );
    let (x, y0, y1) = (
        center.x,
        center.y - g.r - g.tick_len / 2.0,
        center.y - g.r + g.tick_len / 2.0,
    );
    p.line_segment(
        [Pos2::new(x, y0), Pos2::new(x, y1)],
        Stroke::new(g.tick_w, col(INK)),
    );
    p.circle_filled(Pos2::new(x, y0), g.tick_w / 2.0, col(INK));
    p.circle_filled(Pos2::new(x, y1), g.tick_w / 2.0, col(INK));
}

// ── Buttons (board `.btn`) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Green, white text: the screen's one verb.
    Primary,
    /// White with a line-2 edge.
    Secondary,
    /// Text only, ink-2.
    Quiet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

pub struct Button<'a> {
    pub text: &'a str,
    pub icon: Option<Icon>,
    pub kind: Kind,
    pub size: Size,
    /// Fixed width; `None` fits the label.
    pub width: Option<f32>,
    pub enabled: bool,
}

impl<'a> Button<'a> {
    pub fn new(text: &'a str, kind: Kind) -> Self {
        Button {
            text,
            icon: None,
            kind,
            size: Size::Md,
            width: None,
            enabled: true,
        }
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn size(mut self, s: Size) -> Self {
        self.size = s;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> egui::Response {
        let (h, pad, fs) = match self.size {
            Size::Sm => (36.0, 14.0, 14.0),
            Size::Md => (44.0, 20.0, 15.0),
            Size::Lg => (52.0, 28.0, 17.0),
        };
        let icon_px = 18.0;
        let fg = match self.kind {
            Kind::Primary => col(SURFACE),
            Kind::Secondary => col(INK),
            Kind::Quiet => col(INK_2),
        };
        let galley = ui
            .painter()
            .layout_job(job(self.text, font(fs, W::SemiBold), fg, 0.0));
        let content_w = galley.size().x + self.icon.map_or(0.0, |_| icon_px + 8.0);
        let w = self.width.unwrap_or(content_w + pad * 2.0);
        let sense = if self.enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), sense);
        let hovered = self.enabled && resp.hovered();
        let (fill, stroke) = match self.kind {
            Kind::Primary => (
                col(if hovered { BRAND_STRONG } else { BRAND }),
                Stroke::NONE,
            ),
            Kind::Secondary => (
                col(if hovered { SURFACE_2 } else { SURFACE }),
                Stroke::new(1.0, col(LINE_2)),
            ),
            Kind::Quiet => (
                if hovered {
                    col(SURFACE_2)
                } else {
                    Color32::TRANSPARENT
                },
                Stroke::NONE,
            ),
        };
        let alpha = if self.enabled { 1.0 } else { 0.45 };
        let fade = |c: Color32| c.gamma_multiply(alpha);
        let p = ui.painter();
        let round = egui::Rounding::same(h / 2.0);
        if self.kind == Kind::Primary && self.enabled {
            // --shadow-1: a soft 1 px drop.
            p.rect_filled(
                rect.translate(Vec2::new(0.0, 1.0)),
                round,
                Color32::from_black_alpha(18),
            );
        }
        p.rect(
            rect,
            round,
            fade(fill),
            Stroke::new(stroke.width, fade(stroke.color)),
        );
        if resp.has_focus() {
            p.rect_stroke(
                rect.expand(3.0),
                egui::Rounding::same(h / 2.0 + 3.0),
                Stroke::new(2.0, col(BRAND)),
            );
        }
        let x0 = rect.center().x - content_w / 2.0;
        let mut x = x0;
        if let Some(i) = self.icon {
            let ir = Rect::from_min_size(
                Pos2::new(x, rect.center().y - icon_px / 2.0),
                Vec2::splat(icon_px),
            );
            paint_icon(p, ir, i, fade(fg));
            x += icon_px + 8.0;
        }
        let ty = rect.center().y - galley.size().y / 2.0;
        p.galley(Pos2::new(x, ty), galley, fade(fg));
        resp
    }
}

// ── Small text helpers ───────────────────────────────────────────────────────

/// A label in a given size / weight / colour.
pub fn text(ui: &mut egui::Ui, s: &str, size: f32, w: W, c: (u8, u8, u8)) -> egui::Response {
    ui.label(egui::RichText::new(s).font(font(size, w)).color(col(c)))
}

/// A centred, wrapping paragraph at most `max_w` wide.
pub fn para(ui: &mut egui::Ui, s: &str, size: f32, w: W, c: (u8, u8, u8), max_w: f32) {
    let avail = ui.available_width();
    let width = avail.min(max_w);
    ui.allocate_ui_with_layout(
        Vec2::new(width, 0.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.set_width(width);
            ui.add(
                egui::Label::new(egui::RichText::new(s).font(font(size, w)).color(col(c))).wrap(),
            );
        },
    );
}
