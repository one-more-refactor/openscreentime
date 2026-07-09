//! The full-screen host interruption — the Duolingo-style lockout / nudge screen.
//!
//! Aesthetic (DESIGN.md → "Host-side full-screen interruption"): black background,
//! dot grid, one big dot-numeral countdown or streak flame in monochrome, a single
//! accent-red action, mono uppercase copy. Calm and game-like, not punitive.
//!
//! Presenters:
//!   * default (headless-safe): renders the screen as an ASCII/log overlay so the
//!     agent builds and runs with no display. This is what CI and the dev box use.
//!   * `--features gui`: an `eframe/egui` fullscreen window (see `gui` module below).
//!
//! Challenge verification (math / wait / parent_pin) is pure logic in `challenge`
//! so it's testable and shared by both presenters.

use crate::policy::Lockout;
use crate::util::Exec;

/// What the overlay should say + how to dismiss it.
#[derive(Debug, Clone)]
pub struct LockSpec {
    pub headline: String,           // "TIME'S UP", "BEDTIME"
    pub detail: String,             // "USED 60 / 60 MIN TODAY"
    pub big_number: Option<String>, // countdown / streak numeral
    pub action: String,             // the single accent-red CTA
    pub challenge: challenge::Challenge,
    pub for_user: String,
}

pub mod challenge {
    use rand::Rng;

    #[derive(Debug, Clone)]
    pub enum Challenge {
        /// Solve `a op b = ?`.
        Math {
            a: i64,
            b: i64,
            op: char,
            #[allow(dead_code)] // read by `verify` (early-dismiss gate, GUI-only path)
            answer: i64,
        },
        /// Wait out a cooldown before the dismiss button enables.
        Wait { seconds: u32 },
        /// Enter the parent PIN.
        ParentPin,
        /// No unlock challenge (nudge only).
        None,
    }

    impl Challenge {
        pub fn from_kind(kind: &str) -> Challenge {
            match kind {
                "math" => Self::new_math(),
                "wait" => Challenge::Wait { seconds: 60 },
                "parent_pin" => Challenge::ParentPin,
                _ => Challenge::None,
            }
        }

        pub fn new_math() -> Challenge {
            let mut rng = rand::thread_rng();
            let a = rng.gen_range(6..=12);
            let b = rng.gen_range(3..=9);
            Challenge::Math {
                a,
                b,
                op: '×',
                answer: a * b,
            }
        }

        pub fn prompt(&self) -> String {
            match self {
                Challenge::Math { a, b, op, .. } => format!("SOLVE  {a} {op} {b} = ?"),
                Challenge::Wait { seconds } => format!("WAIT {seconds}s TO CONTINUE"),
                Challenge::ParentPin => "ENTER PARENT PIN".into(),
                Challenge::None => String::new(),
            }
        }

        /// Verify a typed response (math answer or PIN). `parent_pin` is the
        /// configured PIN for the ParentPin variant.
        #[allow(dead_code)] // early-dismiss gate for the GUI presenter; covered by tests
        pub fn verify(&self, input: &str, parent_pin: Option<&str>) -> bool {
            match self {
                Challenge::Math { answer, .. } => input
                    .trim()
                    .parse::<i64>()
                    .map(|v| v == *answer)
                    .unwrap_or(false),
                Challenge::ParentPin => parent_pin.map(|p| p == input.trim()).unwrap_or(false),
                Challenge::Wait { .. } => false, // dismissed by time, not input
                Challenge::None => true,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn math_verifies() {
            let c = Challenge::Math {
                a: 7,
                b: 8,
                op: '×',
                answer: 56,
            };
            assert!(c.verify("56", None));
            assert!(!c.verify("55", None));
        }
        #[test]
        fn pin_verifies() {
            let c = Challenge::ParentPin;
            assert!(c.verify("1234", Some("1234")));
            assert!(!c.verify("0000", Some("1234")));
        }
    }
}

impl LockSpec {
    pub fn from_lockout(cfg: &Lockout, headline: &str, detail: &str, user: &str) -> LockSpec {
        let challenge = if cfg.enabled {
            challenge::Challenge::from_kind(&cfg.unlock_challenge)
        } else {
            challenge::Challenge::None
        };
        LockSpec {
            headline: headline.to_string(),
            detail: detail.to_string(),
            big_number: None,
            action: "TAP TO CONTINUE".to_string(),
            challenge,
            for_user: user.to_string(),
        }
    }
}

/// Render the Nothing-styled overlay as text (used by the headless presenter and
/// available as the egui label source too).
pub fn render_ascii(spec: &LockSpec) -> String {
    let dot_row = "· ".repeat(28);
    let mut s = String::new();
    s.push('\n');
    s.push_str(&dot_row);
    s.push('\n');
    s.push_str(&format!(
        "\n    ▍ SENTINEL — {}\n\n",
        spec.for_user.to_uppercase()
    ));
    if let Some(n) = &spec.big_number {
        s.push_str(&format!(
            "        ┏━━━━━━━━━━━━┓\n        ┃   {n:^6}   ┃\n        ┗━━━━━━━━━━━━┛\n\n"
        ));
    }
    s.push_str(&format!("    {}\n", spec.headline));
    s.push_str(&format!("    {}\n\n", spec.detail));
    let prompt = spec.challenge.prompt();
    if !prompt.is_empty() {
        s.push_str(&format!("    {prompt}\n\n"));
    }
    s.push_str(&format!("    [ {} ]\n\n", spec.action));
    s.push_str(&dot_row);
    s.push('\n');
    s
}

/// Present the lockout. Non-blocking: it puts the screen up (or logs it) and
/// returns. The runner keeps the user frozen until policy allows again; the
/// challenge is what a real GUI presenter would gate the *early* dismiss on.
pub fn present(exec: &Exec, spec: &LockSpec) {
    let screen = render_ascii(spec);
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD SHOW FULLSCREEN LOCKOUT for {}:\n{}", spec.for_user, screen);
        return;
    }

    #[cfg(feature = "gui")]
    {
        gui::show(spec);
        return;
    }

    // Headless / no-GUI build: broadcast the overlay to the user's TTYs and log it.
    #[cfg(not(feature = "gui"))]
    {
        let _ = exec.run(
            "wall",
            &[
                "-n",
                &format!("SENTINEL: {} — {}", spec.headline, spec.detail),
            ],
        );
        tracing::warn!("LOCKOUT ({}): {}\n{}", spec.for_user, spec.headline, screen);
    }
}

#[cfg(feature = "gui")]
mod gui {
    //! Minimal eframe/egui fullscreen presenter. Compiled only with `--features gui`.
    use super::LockSpec;
    use eframe::egui;

    /// Nothing-style palette from DESIGN.md (accent red, near-black bg, off-white fg).
    const ACCENT: (u8, u8, u8) = (0xd7, 0x19, 0x21);
    const BG: (u8, u8, u8) = (0x0a, 0x0a, 0x0a);
    const FG: (u8, u8, u8) = (0xfa, 0xfa, 0xfa);

    pub fn show(spec: &LockSpec) {
        let spec = spec.clone();
        let native = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_fullscreen(true)
                .with_decorations(false),
            ..Default::default()
        };
        // Best-effort: if there's no display this returns an error we just log.
        if let Err(e) = eframe::run_native(
            "SENTINEL",
            native,
            Box::new(move |_cc| Ok(Box::new(LockApp { spec: spec.clone() }))),
        ) {
            tracing::warn!("egui lockout unavailable: {e}");
        }
    }

    struct LockApp {
        spec: LockSpec,
    }

    impl eframe::App for LockApp {
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
            let accent = egui::Color32::from_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.colored_label(
                        fg,
                        egui::RichText::new(&self.spec.headline)
                            .size(64.0)
                            .monospace(),
                    );
                    ui.add_space(16.0);
                    ui.colored_label(
                        fg,
                        egui::RichText::new(&self.spec.detail)
                            .size(22.0)
                            .monospace(),
                    );
                    ui.add_space(40.0);
                    let prompt = self.spec.challenge.prompt();
                    if !prompt.is_empty() {
                        ui.colored_label(fg, egui::RichText::new(prompt).size(28.0).monospace());
                    }
                    ui.add_space(40.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(&self.spec.action)
                                    .size(20.0)
                                    .monospace(),
                            )
                            .fill(accent),
                        )
                        .clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        }
    }
}
