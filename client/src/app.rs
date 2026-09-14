//! `ost app` — the on-device window a child (or parent) opens from the app grid.
//!
//! GNOME shows no usable system tray (no StatusNotifierItem host without an
//! extension), so a tray icon is invisible there — yet a device this quiet
//! about what it's doing is exactly the complaint. This window is the answer:
//! a real, launchable app that says, in plain words, how much time is left,
//! whether the device is connected, and — the honest part — what OpenScreenTime
//! can and cannot see. One button: ask a parent for more.
//!
//! It reads the very same status file the background companion (`ost tray`)
//! watches, written by the root agent, and refreshes live. It runs as the
//! desktop user (no root), and the only thing it can *do* is drop a
//! "please, more time" marker in the user's own runtime dir — the same
//! spoof-proof channel the tray uses.
//!
//! Built with `--features gui`. Launched as `ost app` (a `.desktop` entry the
//! installer drops into the app grid).

use eframe::egui;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Status snapshot (a read-only mirror of runner::write_status_file; kept
// independent of the tray module so the window builds on `gui` alone).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
struct Status {
    #[serde(default)]
    connection: String,
    #[serde(default)]
    device_locked: bool,
    #[serde(default)]
    offline_hard_lockdown: bool,
    #[serde(default)]
    tamper_lockdown: bool,
    #[serde(default)]
    users: Vec<UserStatus>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
struct UserStatus {
    name: String,
    #[serde(default)]
    used_minutes: u64,
    /// `None` = no daily limit configured.
    #[serde(default)]
    remaining_minutes: Option<i64>,
    #[serde(default)]
    frozen: bool,
    /// Countdown to an imminent session freeze, if one is pending.
    #[serde(default)]
    freeze_in_secs: Option<u64>,
}

/// This user's snapshot: the private per-user file if present, else the shared
/// device-wide one (a non-managed user still sees lock/connection state).
fn read_status(username: &str) -> Option<Status> {
    let per_user = crate::paths::run_str(&format!("status.{username}.json"));
    let raw = std::fs::read_to_string(&per_user)
        .or_else(|_| std::fs::read_to_string(crate::paths::run_str("status.json")))
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// Drop an on-demand "more time" marker in this user's own runtime dir for the
/// root agent to turn into an earn-request. `/run/user/<uid>` is the user's own
/// 0700 dir, so the channel is spoof-proof. Returns whether it was written.
fn request_more_time() -> bool {
    let uid = users::get_current_uid();
    let dir = std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime"));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    std::fs::write(dir.join("earn_request"), b"1").is_ok()
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

// OpenScreenTime brand — warm light, matching the console, the lock screen and
// the first-run intro.
const BG: (u8, u8, u8) = (0xf5, 0xf5, 0xf4); // warm off-white
const FG: (u8, u8, u8) = (0x1a, 0x1a, 0x1a); // ink
const FAINT: (u8, u8, u8) = (0x76, 0x76, 0x76);
const LINE: (u8, u8, u8) = (0xcc, 0xcc, 0xcb); // ring track / hairlines
const ACCENT: (u8, u8, u8) = (0xb3, 0x15, 0x1c); // the stop, used sparingly
const GOOD: (u8, u8, u8) = (0x2e, 0x7d, 0x46); // the ring green

struct AppView {
    username: String,
    status: Option<Status>,
    /// Set once the user asks for more; shown until the request clears.
    asked: bool,
}

impl AppView {
    fn me(&self) -> Option<&UserStatus> {
        self.status
            .as_ref()
            .and_then(|s| s.users.iter().find(|u| u.name == self.username))
    }

    /// The one honest headline for the current user's time.
    fn time_headline(&self) -> (String, (u8, u8, u8)) {
        match self.me() {
            Some(u) if u.frozen => ("Paused".to_string(), ACCENT),
            Some(u) => match u.remaining_minutes {
                Some(m) if m <= 0 => ("Time's up for today".to_string(), ACCENT),
                Some(m) => (fmt_left(m), if m <= 15 { ACCENT } else { FG }),
                None => ("No limit today".to_string(), FG),
            },
            // Not a managed user on this device (e.g. a parent's own login).
            None => ("This device is managed".to_string(), FG),
        }
    }

    /// A short line under the headline: used-so-far, or the wind-down warning.
    fn time_detail(&self) -> Option<String> {
        let u = self.me()?;
        if let Some(secs) = u.freeze_in_secs {
            return Some(format!("Saving your work — pausing in {secs}s"));
        }
        if u.frozen {
            return Some("A parent can lift this, or ask for more.".to_string());
        }
        match u.remaining_minutes {
            Some(m) if m <= 0 => Some("Ask a parent, or earn more.".to_string()),
            _ => Some(format!("{} used today", fmt_left(u.used_minutes as i64))),
        }
    }

    fn connection(&self) -> (&'static str, (u8, u8, u8)) {
        match self.status.as_ref().map(|s| s.connection.as_str()) {
            Some("online") => ("Connected", GOOD),
            Some("offline_fail_closed") => ("Offline — locked", ACCENT),
            Some(_) => ("Offline — catching up when it's back", FAINT),
            None => ("Not running", FAINT),
        }
    }

    /// A device-level restriction worth a red banner, independent of the user's
    /// own time.
    fn device_banner(&self) -> Option<&'static str> {
        let s = self.status.as_ref()?;
        if s.tamper_lockdown {
            Some("Locked down — OpenScreenTime was tampered with.")
        } else if s.offline_hard_lockdown {
            Some("Locked down — offline too long.")
        } else if s.device_locked {
            Some("A parent paused this device.")
        } else {
            None
        }
    }
}

impl eframe::App for AppView {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [
            BG.0 as f32 / 255.0,
            BG.1 as f32 / 255.0,
            BG.2 as f32 / 255.0,
            1.0,
        ]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Re-read the live snapshot each frame; the file is tiny.
        let next = read_status(&self.username);
        // Once the agent has turned our ask into a live request, the marker is
        // gone — clear the local "asked" flag so the button is usable again.
        if self.asked {
            let uid = users::get_current_uid();
            let marker =
                std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime/earn_request"));
            if !marker.exists() {
                self.asked = false;
            }
        }
        self.status = next;

        let fg = col(FG);
        let faint = col(FAINT);

        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(egui::Margin::same(28.0)))
            .show(ctx, |ui| {
                // Wordmark.
                ui.horizontal(|ui| {
                    ring(ui, 18.0);
                    ui.add_space(8.0);
                    ui.colored_label(
                        fg,
                        egui::RichText::new("OpenScreenTime").size(15.0).strong(),
                    );
                });
                ui.add_space(26.0);

                // Device-level lockdown takes the top line when present.
                if let Some(banner) = self.device_banner() {
                    ui.colored_label(col(ACCENT), egui::RichText::new(banner).size(18.0).strong());
                    ui.add_space(18.0);
                }

                // The headline: time left (or paused / time's up).
                let (head, head_col) = self.time_headline();
                ui.colored_label(col(head_col), egui::RichText::new(head).size(40.0).strong());
                if let Some(detail) = self.time_detail() {
                    ui.add_space(6.0);
                    ui.colored_label(faint, egui::RichText::new(detail).size(15.0));
                }

                ui.add_space(22.0);

                // Connection chip.
                let (conn, conn_col) = self.connection();
                ui.horizontal(|ui| {
                    dot(ui, conn_col);
                    ui.add_space(7.0);
                    ui.colored_label(col(conn_col), egui::RichText::new(conn).size(14.0));
                });

                ui.add_space(28.0);

                // Ask for more — the one thing this window can do.
                let can_ask = self.status.is_some();
                ui.add_enabled_ui(can_ask && !self.asked, |ui| {
                    let label = if self.asked {
                        "Asked — waiting for a parent"
                    } else {
                        "Ask for more time"
                    };
                    if ui
                        .add_sized(
                            [ui.available_width().min(320.0), 44.0],
                            egui::Button::new(
                                egui::RichText::new(label)
                                    .size(16.0)
                                    .strong()
                                    .color(col(BG)),
                            )
                            .fill(col(FG))
                            .rounding(10.0),
                        )
                        .clicked()
                    {
                        self.asked = request_more_time();
                    }
                });
                if self.asked {
                    ui.add_space(8.0);
                    ui.colored_label(
                        col(GOOD),
                        egui::RichText::new("Sent — a parent can say yes.").size(13.0),
                    );
                }

                // The honest footer — the same promises as the first-run intro
                // and TRANSPARENCY.md, always in view.
                ui.add_space(30.0);
                ui.separator();
                ui.add_space(12.0);
                ui.colored_label(
                    faint,
                    egui::RichText::new(
                        "OpenScreenTime counts screen time and filters the network. \
                         It cannot see your screen, your messages, what you type, \
                         or your browsing history.",
                    )
                    .size(12.5),
                );
            });

        // Live, but idle: a couple of seconds is plenty for a clock that ticks
        // in minutes, and it keeps a backgrounded window off the CPU.
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }
}

fn col(c: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

/// A small filled status dot.
fn dot(ui: &mut egui::Ui, c: (u8, u8, u8)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, col(c));
}

/// The activity-ring marque (a small arc of "used" on a faint track), painted
/// to match the favicon.
fn ring(ui: &mut egui::Ui, r: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(r * 2.0, r * 2.0), egui::Sense::hover());
    let c = rect.center();
    let stroke_track = egui::Stroke::new(r * 0.32, col(LINE));
    ui.painter().circle_stroke(c, r * 0.78, stroke_track);
    // A short "used" arc from the top, clockwise ~100°.
    let mut pts = Vec::new();
    let start = -std::f32::consts::FRAC_PI_2;
    let sweep = std::f32::consts::PI * 0.56;
    for i in 0..=24 {
        let a = start + sweep * (i as f32 / 24.0);
        pts.push(c + egui::vec2(a.cos(), a.sin()) * r * 0.78);
    }
    ui.painter().add(egui::Shape::line(
        pts,
        egui::Stroke::new(r * 0.32, col(GOOD)),
    ));
}

/// "45 min" / "1 h 05 min".
fn fmt_left(mins: i64) -> String {
    let m = mins.max(0);
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

/// Entry point for `ost app`: open the window and block until it closes.
pub fn run() -> anyhow::Result<()> {
    let username = std::env::var("USER")
        .ok()
        .or_else(|| users::get_current_username().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 520.0])
            .with_min_inner_size([360.0, 440.0])
            .with_title("OpenScreenTime"),
        ..Default::default()
    };
    let initial = read_status(&username);
    if let Err(e) = eframe::run_native(
        "OPENSCREENTIME",
        native,
        Box::new(move |cc| {
            // Light egui chrome to match the warm brand (egui defaults to dark).
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(AppView {
                username,
                status: initial,
                asked: false,
            }))
        }),
    ) {
        anyhow::bail!("could not open the OpenScreenTime window: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(users: Vec<UserStatus>, conn: &str) -> AppView {
        AppView {
            username: "kid".into(),
            status: Some(Status {
                connection: conn.into(),
                users,
                ..Default::default()
            }),
            asked: false,
        }
    }

    fn kid(remaining: Option<i64>, frozen: bool) -> UserStatus {
        UserStatus {
            name: "kid".into(),
            used_minutes: 20,
            remaining_minutes: remaining,
            frozen,
            freeze_in_secs: None,
        }
    }

    #[test]
    fn time_formatting() {
        assert_eq!(fmt_left(0), "0 min");
        assert_eq!(fmt_left(45), "45 min");
        assert_eq!(fmt_left(60), "1 h 00 min");
        assert_eq!(fmt_left(125), "2 h 05 min");
        // Never renders a negative number.
        assert_eq!(fmt_left(-10), "0 min");
    }

    #[test]
    fn headline_reflects_state() {
        // Plenty of time: white, minutes shown.
        let (h, c) = view(vec![kid(Some(90), false)], "online").time_headline();
        assert_eq!(h, "1 h 30 min");
        assert_eq!(c, FG);

        // Almost out: red.
        let (_h, c) = view(vec![kid(Some(10), false)], "online").time_headline();
        assert_eq!(c, ACCENT);

        // Out.
        let (h, c) = view(vec![kid(Some(0), false)], "online").time_headline();
        assert_eq!(h, "Time's up for today");
        assert_eq!(c, ACCENT);

        // Paused wins over any remaining count.
        let (h, _c) = view(vec![kid(Some(90), true)], "online").time_headline();
        assert_eq!(h, "Paused");

        // No limit configured.
        let (h, _c) = view(vec![kid(None, false)], "online").time_headline();
        assert_eq!(h, "No limit today");

        // Not a managed user on this device.
        let (h, _c) = view(vec![], "online").time_headline();
        assert_eq!(h, "This device is managed");
    }

    #[test]
    fn connection_words() {
        assert_eq!(view(vec![], "online").connection().0, "Connected");
        assert_eq!(
            view(vec![], "offline_fail_closed").connection().0,
            "Offline — locked"
        );
        let v = AppView {
            username: "kid".into(),
            status: None,
            asked: false,
        };
        assert_eq!(v.connection().0, "Not running");
    }
}
