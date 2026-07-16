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
use serde::{Deserialize, Serialize};

/// What the overlay should say + how to dismiss it.
/// Serializable so the GUI presenter can run as a detached subprocess
/// (`sentinel-agent __lockout <b64 json>`) without stalling the tick loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockSpec {
    pub headline: String,           // "TIME'S UP", "BEDTIME"
    pub detail: String,             // "USED 60 / 60 MIN TODAY"
    pub big_number: Option<String>, // countdown / streak numeral
    pub action: String,             // the single accent-red CTA
    pub challenge: challenge::Challenge,
    pub for_user: String,
    /// When set, the overlay shows a live "SCREEN PAUSES IN Ns" countdown from
    /// this many seconds (the save-your-work grace before a screen-time freeze).
    /// `None` for immediate locks that have no grace. Defaults to `None`.
    #[serde(default)]
    pub countdown_secs: Option<u32>,
    /// The user's `policy.parent_pin_hash` (argon2 PHC string), carried along so
    /// a presenter can verify a typed PIN fully offline. `None` = no PIN
    /// configured — the parent_pin path (both the `parent_pin` challenge and the
    /// master escape) is simply unavailable.
    pub parent_pin_hash: Option<String>,
}

pub mod challenge {
    use rand::Rng;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Challenge {
        /// Solve `a op b = ?`.
        Math { a: i64, b: i64, op: char },
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
            Challenge::Math { a, b, op: '×' }
        }

        pub fn prompt(&self) -> String {
            match self {
                Challenge::Math { a, b, op, .. } => format!("SOLVE  {a} {op} {b} = ?"),
                Challenge::Wait { seconds } => format!("WAIT {seconds}s TO CONTINUE"),
                Challenge::ParentPin => "ENTER PARENT PIN".into(),
                Challenge::None => String::new(),
            }
        }

        /// Verify a typed response. Shared by the GUI presenter's early-dismiss
        /// gate and the headless/CLI recovery path (`sentinel-agent unlock`),
        /// which is why this is *not* gated behind `--features gui` — the same
        /// logic must be exercised (and tested) in the default build too.
        ///
        /// The parent PIN, when configured (`parent_pin_hash: Some`), is always
        /// accepted as a master escape regardless of challenge type — a parent
        /// physically present can always get in. If no PIN is configured, that
        /// path is unavailable and behavior falls back to the challenge's own
        /// rule (math answer / wait-only / etc.).
        ///
        /// This is the GUI presenter's typed-input gate: only the `gui` feature
        /// (and tests) exercise it. The headless override path does NOT route
        /// through here — it verifies the PIN directly (see
        /// `check_and_consume_pin_override`) so that `Challenge::None`'s
        /// "no gate" semantics can never turn into an unlock.
        #[cfg(any(feature = "gui", test))]
        pub fn verify(&self, input: &str, parent_pin_hash: Option<&str>) -> bool {
            if let Some(hash) = parent_pin_hash {
                if crate::pin::verify_pin(input.trim(), hash) {
                    return true;
                }
            }
            match self {
                Challenge::Math { a, b, op, .. } => {
                    let expected = match op {
                        '×' | '*' => a * b,
                        '+' => a + b,
                        '-' => a - b,
                        _ => return false,
                    };
                    input
                        .trim()
                        .parse::<i64>()
                        .map(|v| v == expected)
                        .unwrap_or(false)
                }
                // No hash matched above (or none configured): the ParentPin
                // challenge itself has no other way to verify.
                Challenge::ParentPin => false,
                Challenge::Wait { .. } => false, // dismissed by time, not input
                Challenge::None => true,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::Argon2;

        fn hash_of(pin: &str) -> String {
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(pin.as_bytes(), &salt)
                .expect("hash")
                .to_string()
        }

        #[test]
        fn math_verifies() {
            let c = Challenge::Math {
                a: 7,
                b: 8,
                op: '×',
            };
            assert!(c.verify("56", None));
            assert!(!c.verify("55", None));
        }

        #[test]
        fn pin_verifies_parent_pin_challenge() {
            let hash = hash_of("1234");
            let c = Challenge::ParentPin;
            assert!(c.verify("1234", Some(&hash)));
            assert!(!c.verify("0000", Some(&hash)));
        }

        #[test]
        fn pin_unavailable_when_not_configured() {
            let c = Challenge::ParentPin;
            assert!(!c.verify("1234", None));
        }

        #[test]
        fn pin_is_a_master_escape_on_any_challenge() {
            let hash = hash_of("9999");
            let math = Challenge::Math {
                a: 2,
                b: 2,
                op: '×',
            };
            // Wrong math answer, but correct parent PIN still unlocks.
            assert!(math.verify("9999", Some(&hash)));
            let wait = Challenge::Wait { seconds: 60 };
            assert!(wait.verify("9999", Some(&hash)));
        }
    }
}

impl LockSpec {
    pub fn from_lockout(
        cfg: &Lockout,
        headline: &str,
        detail: &str,
        user: &str,
        parent_pin_hash: Option<String>,
    ) -> LockSpec {
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
            parent_pin_hash,
            countdown_secs: None,
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

/// Headless/no-GUI parent-PIN override: with no display to type into, an
/// attempt is dropped as plaintext at `/run/sentinel/unlock_pin.<user>` (e.g.
/// by a companion tool acting on the parent's behalf) and consumed here —
/// read once, deleted regardless of outcome (single-use). Returns `true` only
/// if a parent PIN is configured AND the attempt matches it.
///
/// This MUST fail closed: it verifies the attempt directly against
/// `parent_pin_hash` rather than delegating to `Challenge::verify`, because the
/// whole-device admin lock (and any non-gamified lockout) carries
/// `Challenge::None`, for which `Challenge::verify` returns `true`
/// unconditionally. Routing an *override* through that would let any dropped
/// file bypass an admin lock even when no PIN is set — so the direct
/// hash check is the security boundary here, not the challenge type.
pub fn check_and_consume_pin_override(exec: &Exec, spec: &LockSpec) -> bool {
    let path = format!("/run/sentinel/unlock_pin.{}", spec.for_user);
    let Ok(attempt) = std::fs::read_to_string(&path) else {
        return false;
    };
    let _ = std::fs::remove_file(&path); // single-use, win or lose
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD VERIFY dropped parent-PIN override for {}", spec.for_user);
    }
    // No configured PIN → no override, ever.
    let Some(hash) = spec.parent_pin_hash.as_deref() else {
        return false;
    };
    crate::pin::verify_pin(attempt.trim(), hash)
}

/// An unlock the overlay has ALREADY verified (parent PIN typed into the GUI,
/// or a solved challenge). The GUI presenter runs as a detached root
/// subprocess, so it hands the verdict to the runner through a root-only file:
/// `/run/sentinel/unlock_grant.<user>` containing the granted minutes.
///
/// Unlike `unlock_pin.<user>` (an *attempt*, verified by the consumer), a
/// grant is trusted at face value — which is safe only because `/run/sentinel`
/// is root-owned (0755): no managed user can write there. The verification
/// already happened in the presenter, against the same argon2 hash.
/// `kind` is `"pin"` (parent PIN — a parent present, never rate-limited) or
/// `"challenge"` (a solved self-serve challenge — the runner caps how many of
/// these it honors per day so the trivial math can't be re-solved to defeat the
/// limit). The grant file is `"<kind>:<minutes>"`.
#[cfg_attr(not(feature = "gui"), allow(dead_code))] // written by the gui presenter only
pub fn write_unlock_grant(user: &str, minutes: u32, kind: &str) {
    let dir = std::path::Path::new("/run/sentinel");
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("unlock_grant.{user}"));
    if let Err(e) = std::fs::write(&path, format!("{kind}:{minutes}")) {
        tracing::warn!("could not write unlock grant for {user}: {e}");
    }
}

/// Consume a pending unlock grant for `user` (single-use). Returns the granted
/// minutes. Checked EVERY tick for EVERY managed user — including already-
/// frozen ones — so a parent standing at the machine can always get in
/// (the old code only consulted the override on the freeze-transition tick,
/// which stranded frozen users).
/// Returns `(minutes, kind)`. `kind` is `"pin"` or `"challenge"`; legacy/plain
/// numeric content is read as `"pin"` (uncapped) so a real grant is never
/// wrongly dropped — the safe direction for an unlock is to let the user in.
pub fn take_unlock_grant(user: &str) -> Option<(u32, String)> {
    let path = format!("/run/sentinel/unlock_grant.{user}");
    let content = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let content = content.trim();
    let (kind, mins) = match content.split_once(':') {
        Some((k, m)) => (k.trim(), m.trim()),
        None => ("pin", content),
    };
    let minutes = mins.parse::<u32>().ok()?.clamp(1, 240);
    Some((minutes, kind.to_string()))
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

    // GUI build: present in a DETACHED subprocess (`sentinel-agent __lockout`)
    // so eframe's blocking event loop can never stall the enforcement tick.
    // The subprocess writes an unlock grant on verified dismissal; the runner
    // consumes it on its next tick.
    #[cfg(feature = "gui")]
    {
        if gui::spawn_detached(spec) {
            return;
        }
        // Fall through to the headless broadcast if spawning failed.
    }

    // Headless / no-GUI build: broadcast the overlay to the user's TTYs and log it.
    let _ = exec.run(
        "wall",
        &[
            "-n",
            &format!("SENTINEL: {} — {}", spec.headline, spec.detail),
        ],
    );
    tracing::warn!("LOCKOUT ({}): {}\n{}", spec.for_user, spec.headline, screen);
}

/// Show a lightweight streak nudge (bedtime wind-down, break reminder) without
/// freezing anything. Headless build broadcasts it the same way `present` does
/// for lockouts; a future GUI presenter could render it as a toast instead.
pub fn present_nudge(exec: &Exec, nudge: &crate::gamify::Nudge) {
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD NUDGE: {}", nudge.copy);
        return;
    }
    let _ = exec.run("wall", &["-n", &format!("SENTINEL: {}", nudge.copy)]);
    tracing::info!("nudge ({}): {}", nudge.kind, nudge.copy);
}

#[cfg(feature = "gui")]
pub mod gui {
    //! Minimal eframe/egui fullscreen presenter. Compiled only with `--features gui`.
    use super::challenge::Challenge;
    use super::LockSpec;
    use base64::Engine;
    use eframe::egui;

    /// Nothing-style palette from DESIGN.md (accent red, near-black bg, off-white fg).
    const ACCENT: (u8, u8, u8) = (0xd7, 0x19, 0x21);
    const BG: (u8, u8, u8) = (0x0a, 0x0a, 0x0a);
    const FG: (u8, u8, u8) = (0xfa, 0xfa, 0xfa);

    /// Minutes granted by a verified early dismiss. The parent PIN is the real
    /// escape hatch (enough to matter); a solved challenge is a short breather
    /// (Duolingo-style: effort buys a little, not the evening).
    const GRANT_PARENT_PIN_MIN: u32 = 30;
    const GRANT_CHALLENGE_MIN: u32 = 5;

    /// Launch the overlay as a detached subprocess of this same binary
    /// (`sentinel-agent __lockout <spec-file>`). Returns false if it could not
    /// be spawned (caller falls back to the headless broadcast).
    ///
    /// The spec carries `parent_pin_hash` (the argon2 PHC of the master-unlock
    /// PIN), so it MUST NOT travel on argv — `/proc/<pid>/cmdline` is
    /// world-readable, which would hand the hash to any local user (e.g. the
    /// locked-out managed user on a second VT) for offline brute-force of the
    /// low-entropy PIN. Instead the spec is staged in a root-only (0600) file
    /// under root-owned `/run/sentinel` and only its *path* — not a secret — is
    /// passed on argv; the child reads it once and unlinks it.
    pub fn spawn_detached(spec: &LockSpec) -> bool {
        let Ok(json) = serde_json::to_string(spec) else {
            return false;
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(json);
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let Some(spec_path) = stage_spec_file(&spec.for_user, b64.as_bytes()) else {
            return false;
        };
        match std::process::Command::new(exe)
            .arg("__lockout")
            .arg(&spec_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => true,
            Err(e) => {
                let _ = std::fs::remove_file(&spec_path);
                tracing::warn!("could not spawn lockout GUI subprocess: {e}");
                false
            }
        }
    }

    /// Stage the base64 lock spec in a private, root-only file and return its
    /// path. 0600 in the root-owned `/run/sentinel` means no managed user can
    /// read the `parent_pin_hash` the spec contains.
    fn stage_spec_file(user: &str, bytes: &[u8]) -> Option<std::path::PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = std::path::Path::new("/run/sentinel");
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(format!("lockspec.{user}"));
        // Drop any stale file so create_new (hence mode 0600) applies to a fresh
        // file we own, rather than reusing one with looser bits.
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| tracing::warn!("could not stage lockout spec: {e}"))
            .ok()?;
        f.write_all(bytes)
            .map_err(|e| tracing::warn!("could not write lockout spec: {e}"))
            .ok()?;
        Some(path)
    }

    /// Entry point of the `__lockout` subprocess: read the staged spec file
    /// (single-use — unlinked immediately so the hash never lingers on disk),
    /// decode it, and run the blocking egui loop. On a verified dismissal it
    /// writes an unlock grant for the runner to consume.
    pub fn run_from_spec_file(path: &str) -> anyhow::Result<()> {
        let raw = std::fs::read(path)?;
        let _ = std::fs::remove_file(path); // single-use
        let json = base64::engine::general_purpose::STANDARD.decode(raw.trim_ascii())?;
        let spec: LockSpec = serde_json::from_slice(&json)?;
        show(&spec);
        Ok(())
    }

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
            Box::new(move |_cc| {
                let deadline = spec.countdown_secs.map(|s| {
                    std::time::Instant::now() + std::time::Duration::from_secs(u64::from(s))
                });
                Ok(Box::new(LockApp {
                    spec: spec.clone(),
                    input: String::new(),
                    deadline,
                }))
            }),
        ) {
            tracing::warn!("egui lockout unavailable: {e}");
        }
    }

    struct LockApp {
        spec: LockSpec,
        /// Typed response for `Math`/`ParentPin` challenges (the early-dismiss
        /// gate — `Challenge::verify` decides whether it's correct).
        input: String,
        /// When the save-your-work grace ends (drives the live countdown line).
        deadline: Option<std::time::Instant>,
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
                    // Live save-your-work countdown, if this is a graced lockout.
                    if let Some(deadline) = self.deadline {
                        let remaining = deadline
                            .saturating_duration_since(std::time::Instant::now())
                            .as_secs();
                        ui.add_space(20.0);
                        ui.colored_label(
                            accent,
                            egui::RichText::new(format!("SCREEN PAUSES IN {remaining}S"))
                                .size(30.0)
                                .monospace(),
                        );
                        // Keep ticking even without input events.
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                    ui.add_space(40.0);
                    let prompt = self.spec.challenge.prompt();
                    if !prompt.is_empty() {
                        ui.colored_label(fg, egui::RichText::new(prompt).size(28.0).monospace());
                    }
                    // Math / parent-PIN challenges gate the early dismiss on a
                    // typed answer, verified by `Challenge::verify`. `Wait` and
                    // `None` have no typed input, so the action button alone
                    // dismisses (Wait's cooldown isn't separately timed here —
                    // the tick loop re-freezes if dismissed early and still
                    // out of policy).
                    // A parent-PIN input box is also offered on non-ParentPin
                    // challenges when a PIN is configured, since it's always a
                    // valid master escape (a parent physically present can
                    // always get in).
                    let needs_input = matches!(
                        self.spec.challenge,
                        Challenge::Math { .. } | Challenge::ParentPin
                    ) || self.spec.parent_pin_hash.is_some();
                    if needs_input {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.input)
                                .hint_text("type your answer")
                                .font(egui::TextStyle::Monospace),
                        );
                        ui.add_space(16.0);
                    }
                    ui.add_space(24.0);
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
                        // A verified dismissal must actually UNLOCK: hand the
                        // runner an unlock grant, sized by how it was earned.
                        // (Closing the window alone changes nothing — the tick
                        // loop re-freezes — which made the challenge feel
                        // rigged. Never again.)
                        let pin_ok = self
                            .spec
                            .parent_pin_hash
                            .as_deref()
                            .map(|h| crate::pin::verify_pin(self.input.trim(), h))
                            .unwrap_or(false);
                        let challenge_ok = self.spec.challenge.verify(&self.input, None);
                        if pin_ok {
                            super::write_unlock_grant(
                                &self.spec.for_user,
                                GRANT_PARENT_PIN_MIN,
                                "pin",
                            );
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else if challenge_ok {
                            // `Challenge::None` (nudge-only) verifies trivially:
                            // it grants nothing and simply closes.
                            if matches!(self.spec.challenge, Challenge::Math { .. }) {
                                super::write_unlock_grant(
                                    &self.spec.for_user,
                                    GRANT_CHALLENGE_MIN,
                                    "challenge",
                                );
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else if matches!(
                            self.spec.challenge,
                            Challenge::Wait { .. } | Challenge::None
                        ) {
                            // Wait/None: the typed box (when shown) is only the
                            // optional PIN escape — the button alone still
                            // dismisses, granting nothing.
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            self.input.clear();
                        }
                    }
                });
            });
        }
    }
}
