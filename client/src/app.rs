//! `ost app` — the on-device window a child (or parent) opens from the app grid.
//!
//! GNOME shows no usable system tray, so this window is how the device shows
//! itself: in plain words, how much time is left (the ring is the hero), whether
//! it's connected, and — the honest part — what OpenScreenTime can and cannot
//! see. One thing it can do: ask a parent for more.
//!
//! It reads the same status file the background companion watches, written by
//! the root agent, and refreshes live. It runs as the desktop user (no root),
//! and the only thing it can *do* is drop a "please, more time" marker in the
//! user's own runtime dir — the spoof-proof channel the tray uses.
//!
//! It is also where a **sign-in code** shows up (logincode.rs): a card at the
//! top, and the window comes forward. The agent opens the window for a code if
//! it isn't open; one copy runs per user.
//!
//! Built with `--features gui`. Launched as `ost app`. Design: DESIGN-CLIENT.md §2.

use crate::ui::{self, RingState};
use eframe::egui;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Status snapshot (a read-only mirror of runner::write_status_file).
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
    /// Sign-in / confirm codes for this user (logincode.rs).
    #[serde(default)]
    login_codes: Vec<crate::logincode::LoginCode>,
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

fn read_status(username: &str) -> Option<Status> {
    let per_user = crate::paths::run_str(&format!("status.{username}.json"));
    let raw = std::fs::read_to_string(&per_user)
        .or_else(|_| std::fs::read_to_string(crate::paths::run_str("status.json")))
        .ok()?;
    serde_json::from_str(&raw).ok()
}

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

/// What to paint in and around the hero ring for the current state.
struct Hero {
    ring: RingState,
    /// The big centred glyph/number inside the disc.
    big: String,
    /// The small label under it.
    label: String,
    /// Colour of the big number.
    num: (u8, u8, u8),
}

struct AppView {
    username: String,
    status: Option<Status>,
    asked: bool,
    /// The newest code this window has already come forward for.
    shown_code: Option<String>,
}

impl AppView {
    fn me(&self) -> Option<&UserStatus> {
        self.status
            .as_ref()
            .and_then(|s| s.users.iter().find(|u| u.name == self.username))
    }

    /// The ring + its centre, from the current state (DESIGN-CLIENT.md §1/§2).
    fn hero(&self) -> Hero {
        // Agent not running at all: a calm waiting state, track only.
        if self.status.is_none() {
            return Hero {
                ring: RingState::Track,
                big: "·".into(),
                label: "not running yet".into(),
                num: ui::INK_3,
            };
        }
        match self.me() {
            Some(u) if u.frozen => Hero {
                ring: RingState::Paused,
                big: "‖".into(),
                label: "Paused".into(),
                num: ui::INK_3,
            },
            Some(u) => match u.remaining_minutes {
                None => Hero {
                    ring: RingState::Track,
                    big: "✓".into(),
                    label: "no limit today".into(),
                    num: ui::BRAND,
                },
                Some(m) if m <= 0 => Hero {
                    ring: RingState::Full { color: ui::STOP },
                    big: "0".into(),
                    label: "time's up today".into(),
                    num: ui::STOP,
                },
                Some(m) => {
                    let total = u.used_minutes as i64 + m;
                    let frac = if total > 0 {
                        (u.used_minutes as f32) / total as f32
                    } else {
                        0.0
                    };
                    let color = if m <= 15 { ui::WARN } else { ui::BRAND };
                    let (big, label) = ring_center(m);
                    Hero {
                        ring: RingState::Fill { frac, color },
                        big,
                        label,
                        num: color,
                    }
                }
            },
            // Not a managed user on this device (e.g. a parent's own login).
            None => Hero {
                ring: RingState::Track,
                big: "·".into(),
                label: "managed device".into(),
                num: ui::INK_3,
            },
        }
    }

    /// The wind-down line, when a freeze is imminent (drawn under the ring).
    fn winddown(&self) -> Option<String> {
        let secs = self.me()?.freeze_in_secs?;
        Some(format!("Save your work — the screen pauses in {secs}s."))
    }

    fn connection(&self) -> (&'static str, (u8, u8, u8)) {
        match self.status.as_ref().map(|s| s.connection.as_str()) {
            Some("online") => ("Connected", ui::BRAND),
            Some("offline_fail_closed") => ("Offline — locked", ui::STOP),
            Some(_) => ("Offline — catching up when it's back", ui::INK_3),
            None => ("Not running", ui::INK_3),
        }
    }

    /// A device-level restriction, mirrored in miniature at the top of the window
    /// so an open window is never out of sync with a locked session.
    fn device_banner(&self) -> Option<&'static str> {
        let s = self.status.as_ref()?;
        if s.tamper_lockdown {
            Some("This computer is locked — OpenScreenTime was tampered with.")
        } else if s.offline_hard_lockdown {
            Some("This computer is locked — it's been offline too long.")
        } else if s.device_locked {
            Some("A parent paused this computer.")
        } else {
            None
        }
    }
}

impl eframe::App for AppView {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        let c = ui::BG;
        [
            c.0 as f32 / 255.0,
            c.1 as f32 / 255.0,
            c.2 as f32 / 255.0,
            1.0,
        ]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let next = read_status(&self.username);
        if self.asked {
            let uid = users::get_current_uid();
            let marker =
                std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime/earn_request"));
            if !marker.exists() {
                self.asked = false;
            }
        }
        self.status = next;

        // A new sign-in code: come forward (and ring once, if the tray hasn't).
        let now = chrono::Utc::now();
        let codes: Vec<crate::logincode::LoginCode> = self
            .status
            .as_ref()
            .map(|s| {
                s.login_codes
                    .iter()
                    .filter(|c| c.is_live(now))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(c) = codes.last() {
            if self.shown_code.as_deref() != Some(c.id.as_str()) {
                self.shown_code = Some(c.id.clone());
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Critical,
                ));
                #[cfg(feature = "tray")]
                if crate::logincode::first_sighting(c) {
                    crate::logincode::notify(c);
                }
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(ui::col(ui::BG)).inner_margin(egui::Margin::same(28.0)))
            .show(ctx, |ui_| {
                // Wordmark, top-left (stays put).
                ui_.horizontal(|ui_| {
                    let (rect, _) = ui_.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    ui::ring(ui_.painter(), rect.center(), 20.0, &RingState::Fill { frac: 0.4, color: ui::BRAND });
                    ui_.add_space(9.0);
                    ui_.label(egui::RichText::new("OpenScreenTime").font(ui::font(15.0)).strong().color(ui::col(ui::INK)));
                });

                // A sign-in code, above everything else while it lasts.
                for c in &codes {
                    ui_.add_space(14.0);
                    code_card(ui_, c, now);
                }

                // Device-level lock mirrored at the top.
                if let Some(banner) = self.device_banner() {
                    ui_.add_space(14.0);
                    banner_card(ui_, banner);
                }

                ui_.vertical_centered(|ui_| {
                    ui_.add_space((ui_.available_height() * 0.06).min(28.0));

                    // The hero ring, 168px, with the number + label inside.
                    let hero = self.hero();
                    let d = 168.0;
                    let (rect, _) = ui_.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
                    let center = rect.center();
                    ui::ring(ui_.painter(), center, d, &hero.ring);
                    ui_.painter().text(
                        center - egui::vec2(0.0, 8.0),
                        egui::Align2::CENTER_CENTER,
                        &hero.big,
                        ui::font(if hero.big.chars().count() > 2 { 44.0 } else { 60.0 }),
                        ui::col(hero.num),
                    );
                    ui_.painter().text(
                        center + egui::vec2(0.0, 34.0),
                        egui::Align2::CENTER_CENTER,
                        &hero.label,
                        ui::font(13.0),
                        ui::col(ui::INK_2),
                    );

                    // Wind-down line, if a freeze is imminent.
                    if let Some(w) = self.winddown() {
                        ui_.add_space(14.0);
                        ui_.label(egui::RichText::new(w).font(ui::font(14.0)).color(ui::col(ui::WARN)));
                    }

                    ui_.add_space(20.0);

                    // Connection chip.
                    let (conn, conn_col) = self.connection();
                    ui_.horizontal(|ui_| {
                        ui_.add_space((ui_.available_width() - text_w(ui_, conn) - 20.0).max(0.0) / 2.0);
                        let (dot, _) = ui_.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                        ui_.painter().circle_filled(dot.center(), 4.5, ui::col(conn_col));
                        ui_.add_space(7.0);
                        ui_.label(egui::RichText::new(conn).font(ui::font(13.0)).color(ui::col(conn_col)));
                    });

                    ui_.add_space(26.0);

                    // Ask for more — the one thing this window can do.
                    let can_ask = self.me().is_some() && !self.asked;
                    let label = if self.asked { "Asked — waiting for a parent" } else { "Ask for more time" };
                    let w = ui_.available_width().min(300.0);
                    if ui::primary_button(ui_, label, egui::vec2(w, 44.0), can_ask) {
                        self.asked = request_more_time();
                    }
                    if self.asked {
                        ui_.add_space(8.0);
                        ui_.label(egui::RichText::new("Sent — a parent can say yes.").font(ui::font(13.0)).color(ui::col(ui::BRAND_INK)));
                    }
                });

                // The honest promise, always in view — a sunken footer card.
                let avail = ui_.available_height();
                if avail > 96.0 {
                    ui_.add_space(avail - 96.0);
                }
                egui::Frame::default()
                    .fill(ui::col(ui::SUNKEN))
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(egui::Margin::same(14.0))
                    .show(ui_, |ui_| {
                        ui_.set_width(ui_.available_width());
                        ui_.label(
                            egui::RichText::new(
                                "OpenScreenTime counts screen time and filters the network. It can't \
                                 see your screen, your messages, what you type, or your browsing history.",
                            )
                            .font(ui::font(12.5))
                            .color(ui::col(ui::INK_3)),
                        );
                    });
            });

        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }
}

/// The sign-in code card: the code, big and spaced, and one plain line on
/// where to type it.
fn code_card(
    ui_: &mut egui::Ui,
    c: &crate::logincode::LoginCode,
    now: chrono::DateTime<chrono::Utc>,
) {
    egui::Frame::default()
        .fill(ui::col(ui::BRAND_TINT))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(14.0))
        .show(ui_, |ui_| {
            ui_.set_width(ui_.available_width());
            ui_.label(
                egui::RichText::new(c.headline())
                    .font(ui::font(13.0))
                    .color(ui::col(ui::INK_2)),
            );
            ui_.label(
                egui::RichText::new(c.spaced())
                    .font(ui::mono(40.0))
                    .color(ui::col(ui::BRAND_INK)),
            );
            ui_.label(
                egui::RichText::new(c.instructions())
                    .font(ui::font(13.0))
                    .color(ui::col(ui::INK_2)),
            );
            let m = c.minutes_left(now);
            ui_.label(
                egui::RichText::new(format!(
                    "Works for {m} more minute{}.",
                    if m == 1 { "" } else { "s" }
                ))
                .font(ui::font(12.5))
                .color(ui::col(ui::INK_3)),
            );
        });
}

/// One window per user: a second `ost app` (the agent opens one when a code
/// arrives) leaves as soon as it sees the first holding this lock. The lock
/// lives in the user's own 0700 runtime dir and dies with the process.
fn single_instance() -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let dir = std::path::PathBuf::from(format!(
        "/run/user/{}/openscreentime",
        users::get_current_uid()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("app.lock"))
        .ok()?;
    // SAFETY: flock on a descriptor we own; no memory is shared.
    let got = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    got.then_some(f)
}

/// A small stop-tinted device-lock card at the top of the window.
fn banner_card(ui_: &mut egui::Ui, text: &str) {
    egui::Frame::default()
        .fill(ui::col(ui::STOP_TINT))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui_, |ui_| {
            ui_.set_width(ui_.available_width());
            ui_.label(
                egui::RichText::new(text)
                    .font(ui::font(14.0))
                    .strong()
                    .color(ui::col(ui::STOP)),
            );
        });
}

fn text_w(ui_: &egui::Ui, s: &str) -> f32 {
    ui_.fonts(|f| {
        s.chars()
            .map(|c| f.glyph_width(&ui::font(13.0), c))
            .sum::<f32>()
    })
}

/// The number and label inside the ring: minutes, or "h:mm" past an hour.
fn ring_center(m: i64) -> (String, String) {
    if m < 60 {
        (m.to_string(), "minutes left".into())
    } else {
        (format!("{}:{:02}", m / 60, m % 60), "left today".into())
    }
}

/// Entry point for `ost app`: open the window and block until it closes.
pub fn run() -> anyhow::Result<()> {
    let username = std::env::var("USER")
        .ok()
        .or_else(|| users::get_current_username().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();

    // Already open (the agent opens one per code): the running window has
    // seen the code and come forward; nothing to do here.
    let Some(_lock) = single_instance() else {
        return Ok(());
    };

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
            ui::install(cc);
            Ok(Box::new(AppView {
                username,
                status: initial,
                asked: false,
                shown_code: None,
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
            shown_code: None,
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
    fn ring_center_formats() {
        assert_eq!(ring_center(45), ("45".into(), "minutes left".into()));
        assert_eq!(ring_center(90), ("1:30".into(), "left today".into()));
        assert_eq!(ring_center(60), ("1:00".into(), "left today".into()));
    }

    #[test]
    fn hero_reflects_state() {
        // Plenty of time: green fill, minutes shown.
        let h = view(vec![kid(Some(90), false)], "online").hero();
        assert_eq!(h.big, "1:30");
        assert_eq!(h.num, ui::BRAND);
        assert!(matches!(h.ring, RingState::Fill { .. }));

        // Almost out: amber.
        assert_eq!(
            view(vec![kid(Some(10), false)], "online").hero().num,
            ui::WARN
        );

        // Out: red, full ring.
        let h = view(vec![kid(Some(0), false)], "online").hero();
        assert_eq!(h.num, ui::STOP);
        assert!(matches!(h.ring, RingState::Full { .. }));

        // Paused wins.
        assert!(matches!(
            view(vec![kid(Some(90), true)], "online").hero().ring,
            RingState::Paused
        ));

        // No limit: a check on a plain track.
        let h = view(vec![kid(None, false)], "online").hero();
        assert_eq!(h.big, "✓");
        assert!(matches!(h.ring, RingState::Track));
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
            shown_code: None,
        };
        assert_eq!(v.connection().0, "Not running");
    }
}
