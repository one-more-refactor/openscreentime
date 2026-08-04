//! The one-time, skippable first-run intro shown on the kid's device.
//!
//! This is the child-facing documentation: instead of a docs site, a few short,
//! honest cards the first time the companion runs — what Sentinel does, what a
//! parent can and can't see, and how to ask for more time. Everything is
//! skippable and it never shows again once seen (a marker in the user's config
//! dir). Compiled with `--features gui`; the tray spawns it as a detached
//! `__intro` subprocess on first run.

use std::path::PathBuf;

/// `~/.config/sentinel/intro_seen` — presence means "already shown".
pub fn seen_marker() -> Option<PathBuf> {
    // Same resolution as the parent config, minus the trailing filename.
    crate::parent::config_path().and_then(|p| p.parent().map(|d| d.join("intro_seen")))
}

/// Whether the intro has already been shown for this user. Called by the tray
/// to decide whether to spawn the intro (the subprocess itself always shows).
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
pub fn already_seen() -> bool {
    seen_marker().is_some_and(|p| p.exists())
}

/// Record that the intro has been shown (best-effort).
pub fn mark_seen() {
    if let Some(path) = seen_marker() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, b"1");
    }
}

/// The cards. Short, honest, kid-first — the same promises as TRANSPARENCY.md.
const SLIDES: &[(&str, &str)] = &[
    (
        "THIS COMPUTER HAS SENTINEL",
        "It manages screen time and blocks some things on the network. Here's the honest version — swipe through, or skip.",
    ),
    (
        "WHAT A PARENT CAN SEE",
        "How much screen time you've used, on which device, and if someone tampers with Sentinel. That's it.",
    ),
    (
        "WHAT THEY CAN'T SEE",
        "Not your screen. Not what you type. Not your messages, and not your browsing history. Sentinel doesn't watch you — it counts time and filters the network.",
    ),
    (
        "SCREEN TIME",
        "You get a daily limit. When it's nearly up you'll get a heads-up, and there's a 60-second save-your-work warning before the screen pauses.",
    ),
    (
        "NEED MORE TIME?",
        "Click the Sentinel tray icon and choose REQUEST MORE TIME. A parent gets the request and can say yes.",
    ),
    (
        "ONE MORE THING",
        "There is no remote shell, no camera, no message reading. Sentinel only enforces time and network rules — and everything it does shows up right here. That's the deal.",
    ),
];

/// Entry point of the `__intro` subprocess: show the cards, mark as seen.
pub fn run() -> anyhow::Result<()> {
    show();
    mark_seen();
    Ok(())
}

fn show() {
    use eframe::egui;

    // Nothing-style palette (matches the lockout overlay).
    const ACCENT: (u8, u8, u8) = (0xd7, 0x19, 0x21);
    const BG: (u8, u8, u8) = (0x0a, 0x0a, 0x0a);
    const FG: (u8, u8, u8) = (0xfa, 0xfa, 0xfa);

    struct IntroApp {
        slide: usize,
    }

    impl eframe::App for IntroApp {
        fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
            [
                BG.0 as f32 / 255.0,
                BG.1 as f32 / 255.0,
                BG.2 as f32 / 255.0,
                1.0,
            ]
        }
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            let fg = egui::Color32::from_rgb(FG.0, FG.1, FG.2);
            let faint = egui::Color32::from_rgb(0x8a, 0x8a, 0x8a);
            let accent = egui::Color32::from_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
            let (title, body) = SLIDES[self.slide.min(SLIDES.len() - 1)];
            let last = self.slide + 1 >= SLIDES.len();

            egui::TopBottomPanel::top("intro_top").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        faint,
                        egui::RichText::new(format!("{}/{}", self.slide + 1, SLIDES.len()))
                            .monospace(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new("SKIP").monospace()).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(48.0);
                ui.colored_label(fg, egui::RichText::new(title).size(34.0).monospace());
                ui.add_space(20.0);
                ui.colored_label(fg, egui::RichText::new(body).size(18.0).monospace());
                ui.add_space(40.0);
                let label = if last { "DONE" } else { "NEXT" };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(label).size(18.0).monospace())
                            .fill(accent),
                    )
                    .clicked()
                {
                    if last {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.slide += 1;
                    }
                }
            });
        }
    }

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 380.0])
            .with_title("Sentinel"),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "SENTINEL",
        native,
        Box::new(|_cc| Ok(Box::new(IntroApp { slide: 0 }))),
    ) {
        tracing::warn!("intro window unavailable: {e}");
    }
}
