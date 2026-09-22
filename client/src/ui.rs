//! The OpenScreenTime ring language, as shared egui building blocks — so the
//! app window, the wind-down, the lock and the intro can never drift apart
//! again (docs/DESIGN-CLIENT.md). Compiled only with `--features gui`.
//!
//! One idea, carried to the last screen: time is a ring you fill. The same ring
//! that shows how much of the day is left is the one that fills full and closes
//! when it runs out — enforcement is the gauge completed, not an alarm raised.

#![cfg(feature = "gui")]
// A shared token + helper module: not every token/helper is used by every
// surface, and that's fine — they are the language, kept complete on purpose.
#![allow(dead_code)]

use eframe::egui;

// ── Tokens (verbatim from DESIGN.md §2 / DESIGN-CLIENT.md §0) ────────────────
pub const BG: (u8, u8, u8) = (0xf4, 0xf2, 0xee); // warm paper canvas
pub const SURFACE: (u8, u8, u8) = (0xff, 0xff, 0xff);
pub const SUNKEN: (u8, u8, u8) = (0xec, 0xeb, 0xe6); // ring / bar track
pub const LINE: (u8, u8, u8) = (0xe6, 0xe3, 0xdd);
pub const LINE_2: (u8, u8, u8) = (0xd5, 0xd1, 0xc9);
pub const INK: (u8, u8, u8) = (0x1e, 0x1c, 0x19);
pub const INK_2: (u8, u8, u8) = (0x57, 0x54, 0x4e);
pub const INK_3: (u8, u8, u8) = (0x72, 0x6e, 0x66);
pub const BRAND: (u8, u8, u8) = (0x2e, 0x7d, 0x46); // THE RING, primary action
pub const BRAND_STRONG: (u8, u8, u8) = (0x26, 0x6a, 0x3b);
pub const BRAND_TINT: (u8, u8, u8) = (0xe4, 0xf1, 0xe8);
pub const BRAND_INK: (u8, u8, u8) = (0x1c, 0x5c, 0x33);
pub const WARN: (u8, u8, u8) = (0x8a, 0x63, 0x00); // wind-down, soft offline
pub const WARN_TINT: (u8, u8, u8) = (0xf7, 0xed, 0xd6);
pub const STOP: (u8, u8, u8) = (0xb3, 0x15, 0x1c); // the interrupt
pub const STOP_TINT: (u8, u8, u8) = (0xf8, 0xe3, 0xe2);

pub fn col(c: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

/// Blend two token colours (`t` in 0..=1 toward `b`) — the one motion, amber→red.
pub fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Bundle Figtree + Space Mono into the binary and register them, so the client
/// looks the same as the console with no network and no fontconfig — the
/// overlay runs as root with a scrubbed $HOME and cannot use system fonts.
pub fn install(cc: &eframe::CreationContext<'_>) {
    let ctx = &cc.egui_ctx;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "figtree".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/Figtree.ttf")),
    );
    fonts.font_data.insert(
        "ost-mono".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/SpaceMono-Regular.ttf")),
    );
    // Figtree is the whole product's voice.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "figtree".to_owned());
    // Space Mono survives only for literal codes.
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "ost-mono".to_owned());
    ctx.set_fonts(fonts);

    // Light chrome that matches the ring language (DESIGN-CLIENT.md §0).
    let mut v = egui::Visuals::light();
    v.panel_fill = col(BG);
    v.window_fill = col(SURFACE);
    v.override_text_color = Some(col(INK));
    v.selection.bg_fill = col(BRAND_TINT);
    v.selection.stroke = egui::Stroke::new(1.0, col(BRAND));
    v.widgets.inactive.rounding = egui::Rounding::same(10.0);
    v.widgets.hovered.rounding = egui::Rounding::same(10.0);
    v.widgets.active.rounding = egui::Rounding::same(10.0);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, col(LINE_2));
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, col(BRAND));
    ctx.set_visuals(v);
}

/// A Figtree text style at a given size.
pub fn font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Proportional)
}
/// Space Mono, for literal codes only.
pub fn mono(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Monospace)
}

// ── The ring — one painter, every state (DESIGN-CLIENT.md §1) ────────────────

pub enum RingState {
    /// Fills clockwise from 12 o'clock by `frac` (0..=1) in `color`.
    Fill { frac: f32, color: (u8, u8, u8) },
    /// A complete ring in `color` — the day is spent (stop) or a full grace (amber).
    Full { color: (u8, u8, u8) },
    /// Parent-paused: a dashed ink-3 ring.
    Paused,
    /// No daily limit / not running: track only, no arc.
    Track,
}

/// Draw the activity ring centred at `center` with the given outer `diameter`.
/// The same geometry as the favicon and the console: a sunken track with a
/// round-capped arc growing clockwise from the top.
pub fn ring(painter: &egui::Painter, center: egui::Pos2, diameter: f32, state: &RingState) {
    let stroke_w = (diameter * 0.09).round().max(4.0);
    let radius = diameter / 2.0 - stroke_w / 2.0;

    // Track: a full circle in the sunken colour.
    painter.circle_stroke(center, radius, egui::Stroke::new(stroke_w, col(SUNKEN)));

    let start = -std::f32::consts::FRAC_PI_2; // 12 o'clock
    let full = std::f32::consts::TAU;

    match state {
        RingState::Track => {}
        RingState::Paused => {
            // A dashed ink-3 ring: short on-segments around the circle.
            let n = 26;
            let seg = full / n as f32;
            for i in 0..n {
                let a0 = start + seg * i as f32;
                let a1 = a0 + seg * 0.42; // ~42% duty → "on/off" dash
                arc(painter, center, radius, stroke_w, col(INK_3), a0, a1, false);
            }
        }
        RingState::Full { color } => {
            arc(
                painter,
                center,
                radius,
                stroke_w,
                col(*color),
                start,
                start + full,
                true,
            );
        }
        RingState::Fill { frac, color } => {
            let f = frac.clamp(0.0, 1.0);
            if f > 0.0 {
                arc(
                    painter,
                    center,
                    radius,
                    stroke_w,
                    col(*color),
                    start,
                    start + full * f,
                    true,
                );
            }
        }
    }
}

/// One arc as a polyline, with round caps drawn as filled discs at each end.
#[allow(clippy::too_many_arguments)]
fn arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    width: f32,
    color: egui::Color32,
    a0: f32,
    a1: f32,
    caps: bool,
) {
    let steps = ((a1 - a0).abs() / 0.06).ceil().max(2.0) as usize;
    let pts: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * (i as f32 / steps as f32);
            center + egui::vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(
        pts.clone(),
        egui::Stroke::new(width, color),
    ));
    if caps {
        if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
            painter.circle_filled(*first, width / 2.0, color);
            painter.circle_filled(*last, width / 2.0, color);
        }
    }
}

/// A brand-green primary pill button, sized. Returns whether it was clicked.
pub fn primary_button(ui: &mut egui::Ui, text: &str, size: egui::Vec2, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(
            size,
            egui::Button::new(
                egui::RichText::new(text)
                    .font(font(16.0))
                    .strong()
                    .color(col(SURFACE)),
            )
            .fill(col(BRAND))
            .rounding(egui::Rounding::same(size.y / 2.0)),
        )
        .clicked()
    })
    .inner
}
