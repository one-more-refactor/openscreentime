//! `ost app` — the clock on the desk (brand board 05c).
//!
//! Time left and the ring, today's three rules in one glance, one verb ("Ask
//! for more time"), and the honest footer — true for whoever is reading it
//! (`glance.rs`). The first time it opens, the same window shows a few short
//! cards first: what this is, who sees what, what it can't see.
//!
//! One window per person: a second `ost app` (the launcher, a notification's
//! "Open OpenScreenTime", the agent opening it for a sign-in code) asks the
//! first to come forward over a socket in the person's own runtime dir, and
//! leaves. It is not started at every login — the companion (`ost tray`) is
//! the always-on piece; it opens this window once, on first run.
//!
//! It reads the status file the root agent writes and refreshes live. It runs
//! as the desktop user; the only thing it can *do* is drop a "more time,
//! please" marker in the user's own runtime dir — the channel the companion
//! uses too. A sign-in code (logincode.rs) shows as a card at the top.
//!
//! Built with `--features gui`.

use crate::glance::{self, Sees, Today};
use crate::icons::Icon;
use crate::ui::{self, col, Button, Kind, RingState, W};
use eframe::egui;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// The verdict: time left, the stop, a parent's override.
    #[serde(flatten)]
    clock: glance::Clock,
    /// Countdown to an imminent stop, if one is running.
    #[serde(default)]
    freeze_in_secs: Option<u64>,
    #[serde(default)]
    self_managed: bool,
    /// May ask a parent for more (absent from older agents: yes, if managed).
    #[serde(default = "yes")]
    can_ask: bool,
    #[serde(default)]
    sees: Sees,
    #[serde(default)]
    shared_sites: bool,
    #[serde(default)]
    today: Option<Today>,
    /// A request for more time is waiting on a parent (`None`: an agent that
    /// doesn't say — the window remembers its own click).
    #[serde(default)]
    ask_pending: Option<bool>,
}

fn yes() -> bool {
    true
}

fn read_status(username: &str) -> Option<Status> {
    let per_user = crate::paths::run_str(&format!("status.{username}.json"));
    let raw = std::fs::read_to_string(&per_user)
        .or_else(|_| std::fs::read_to_string(crate::paths::run_str("status.json")))
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// This person's own runtime dir (`$XDG_RUNTIME_DIR/openscreentime`).
fn runtime_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|d| Path::new(d).is_dir())
        .unwrap_or_else(|| format!("/run/user/{}", users::get_current_uid()));
    PathBuf::from(base).join("openscreentime")
}

fn request_more_time() -> bool {
    let dir = PathBuf::from(format!(
        "/run/user/{}/openscreentime",
        users::get_current_uid()
    ));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    std::fs::write(dir.join("earn_request"), b"1").is_ok()
}

// ---------------------------------------------------------------------------
// First run
// ---------------------------------------------------------------------------

/// `~/.config/openscreentime/intro_seen` — presence means "already shown".
pub fn seen_marker() -> Option<PathBuf> {
    crate::parent::config_path().and_then(|p| p.parent().map(|d| d.join("intro_seen")))
}

/// Whether the first-run cards have been shown to this person.
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
pub fn intro_seen() -> bool {
    seen_marker().is_some_and(|p| p.exists())
}

fn mark_intro_seen() {
    if let Some(path) = seen_marker() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, b"1");
    }
}

/// The first-run cards, true for this person (board 06 "First run").
fn intro_cards(sees: Sees, shared_sites: bool, can_ask: bool) -> Vec<(Icon, String, String)> {
    let own = sees == Sees::Own;
    let mut cards = vec![if own {
        (
            Icon::Clock,
            "This computer keeps the time you set".to_string(),
            "It counts your time against the limits you chose. Here's the honest version — \
             read on, or skip."
                .to_string(),
        )
    } else {
        (
            Icon::Clock,
            "This computer keeps track of screen time".to_string(),
            "It counts your time and blocks a few things online. Here's the honest version — \
             read on, or skip."
                .to_string(),
        )
    }];
    let shared = if shared_sites {
        " This computer is shared with someone younger, so a parent sees the sites it looks up."
    } else {
        ""
    };
    cards.push(match sees {
        Sees::TimeAppsSites => (
            Icon::Info,
            "What a parent sees".into(),
            "How long you used this computer today, which apps were open and which sites it \
             looked up — the same picture you get on your own page. And whether someone \
             changed OpenScreenTime."
                .into(),
        ),
        Sees::TimeApps => (
            Icon::Info,
            "What a parent sees".into(),
            format!(
                "How long you used this computer today and which apps were open — not the \
                 sites you visit.{shared}"
            ),
        ),
        Sees::Own => (
            Icon::Info,
            "Who sees what".into(),
            format!("Your apps and sites are yours: no one else sees them.{shared}"),
        ),
    });
    cards.push((
        Icon::EyeOff,
        "What it can't see".into(),
        "Not your screen, not your messages, not what you type. It keeps time — it doesn't \
         watch you."
            .into(),
    ));
    cards.push((
        Icon::Bell,
        "Before it stops".into(),
        "A heads-up 15, 5 and 1 minute before your time ends, so you can save your work. When \
         it stops, your apps are paused, not closed."
            .into(),
    ));
    if own {
        cards.push((
            Icon::Clock,
            "Need a few more minutes?".into(),
            "When your time runs out you can give yourself 15 more minutes, up to three times \
             a day."
                .into(),
        ));
    } else if can_ask {
        cards.push((
            Icon::Ask,
            "Need more time?".into(),
            "Open OpenScreenTime and choose Ask for more time. A parent gets it and can say \
             yes."
                .into(),
        ));
    }
    cards
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// A line under the ring: words, their colour, the card behind them.
type Notice = (String, (u8, u8, u8), (u8, u8, u8));

/// What the ring and its centre show.
struct Hero {
    ring: RingState,
    big: String,
    label: String,
    /// Paused: the pause icon goes in the middle.
    paused: bool,
}

/// The ring for the current state (the ring is time used today, nothing else).
/// "Time left" is the verdict's — the same number the warnings, the tray and
/// the console count down — so a parent's override shows as the time it
/// gives, never as a red zero.
fn hero(
    status: Option<&Status>,
    me: Option<&UserStatus>,
    now: chrono::DateTime<chrono::Local>,
) -> Hero {
    let blank = |label: &str, ring: RingState| Hero {
        ring,
        big: String::new(),
        label: label.into(),
        paused: false,
    };
    let Some(s) = status else {
        return blank("not running yet", RingState::Track);
    };
    if s.device_locked {
        return Hero {
            ring: RingState::Paused,
            big: String::new(),
            label: "paused".into(),
            paused: true,
        };
    }
    let spent = || Hero {
        ring: RingState::Full { color: ui::STOP },
        big: "0".into(),
        label: "min left".into(),
        paused: false,
    };
    if s.tamper_lockdown || s.offline_hard_lockdown {
        return spent();
    }
    let Some(u) = me else {
        return blank("no limit here", RingState::Track);
    };
    match u.clock.left(now) {
        glance::Left::NoLimit => Hero {
            ring: RingState::Track,
            big: u.used_minutes.to_string(),
            label: "min today".into(),
            paused: false,
        },
        glance::Left::Stopped => spent(),
        glance::Left::Minutes { minutes: m, .. } => {
            let total = u.used_minutes as f32 + m as f32;
            let frac = if total > 0.0 {
                u.used_minutes as f32 / total
            } else {
                0.0
            };
            Hero {
                ring: RingState::Fill {
                    frac,
                    color: if m <= 15 { ui::WARN } else { ui::BRAND },
                },
                big: left_words(m),
                label: "min left".into(),
                paused: false,
            }
        }
    }
}

/// "27", or "1 h 36" past an hour (board: `1 h 48` over "min left").
fn left_words(m: i64) -> String {
    if m < 60 {
        m.to_string()
    } else {
        format!("{} h {:02}", m / 60, m % 60)
    }
}

struct AppView {
    username: String,
    status: Option<Status>,
    read_at: Option<Instant>,
    asked: bool,
    /// The newest code this window has already come forward for.
    shown_code: Option<String>,
    /// First-run card on screen, or `None` for the normal view.
    intro: Option<usize>,
    /// Another `ost app` asked this window to come forward.
    raise: Arc<AtomicBool>,
}

impl AppView {
    fn me(&self) -> Option<&UserStatus> {
        self.status
            .as_ref()
            .and_then(|s| s.users.iter().find(|u| u.name == self.username))
    }

    fn come_forward(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
    }

    /// A line under the ring when something is off, in the board's words.
    fn notice(&self) -> Option<Notice> {
        let s = self.status.as_ref()?;
        if s.tamper_lockdown {
            return Some((
                "Stopped until a parent checks this computer.".into(),
                ui::STOP,
                ui::STOP_TINT,
            ));
        }
        if s.offline_hard_lockdown {
            return Some((
                "Stopped until this computer reaches the family server.".into(),
                ui::STOP,
                ui::STOP_TINT,
            ));
        }
        if s.device_locked {
            return Some((
                "Paused. A parent paused this computer — it comes back when they lift it.".into(),
                ui::INK_2,
                ui::SURFACE_2,
            ));
        }
        if let Some(secs) = self.me().and_then(|u| u.freeze_in_secs) {
            return Some((
                format!("Save your work — the screen stops in {secs} s."),
                ui::WARN,
                ui::WARN_TINT,
            ));
        }
        if s.connection != "online" && !s.connection.is_empty() {
            return Some((
                "Offline — it keeps today's rules and catches up when it's back.".into(),
                ui::WARN,
                ui::WARN_TINT,
            ));
        }
        if let Some(line) = self
            .me()
            .and_then(|u| unlocked_line(&u.clock, chrono::Local::now()))
        {
            return Some((line, ui::BRAND_INK, ui::BRAND_TINT));
        }
        None
    }

    /// A request is waiting on a parent: this window's click until the agent
    /// has picked it up, then the agent's word (a grant or a "not now" clears
    /// it, and the button comes back).
    fn asked(&self) -> bool {
        self.asked || self.me().and_then(|u| u.ask_pending) == Some(true)
    }
}

/// "Unlocked until 00:27." — when a parent's override (or their own snooze)
/// is what keeps the screen on, said plainly.
fn unlocked_line(c: &glance::Clock, now: chrono::DateTime<chrono::Local>) -> Option<String> {
    match c.left(now) {
        glance::Left::Minutes {
            unlocked_until: Some(t),
            ..
        } => Some(format!("Unlocked until {}.", t.format("%H:%M"))),
        _ => None,
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
        if self
            .read_at
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(900))
        {
            self.status = read_status(&self.username);
            self.read_at = Some(Instant::now());
            // The agent has the ask now: from here on, its word counts.
            if self.me().and_then(|u| u.ask_pending) == Some(true) {
                self.asked = false;
            }
        }
        if self.raise.swap(false, Ordering::SeqCst) {
            Self::come_forward(ctx);
        }

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
        #[cfg(feature = "tray")]
        crate::logincode::close_stale(&codes);
        if let Some(c) = codes.last() {
            if self.shown_code.as_deref() != Some(c.id.as_str()) {
                self.shown_code = Some(c.id.clone());
                Self::come_forward(ctx);
                #[cfg(feature = "tray")]
                if crate::logincode::first_sighting(c) {
                    crate::logincode::notify(c);
                }
            }
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(col(ui::BG))
                    .inner_margin(egui::Margin {
                        left: 28.0,
                        right: 28.0,
                        top: 24.0,
                        bottom: 22.0,
                    }),
            )
            .show(ctx, |uic| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(uic, |uic| {
                        ui::lockup(uic, 15.0, false);
                        // A sign-in code comes first while it lasts: it's why
                        // the window is open.
                        for c in &codes {
                            uic.add_space(16.0);
                            code_card(uic, c, now);
                        }
                        match self.intro {
                            Some(i) => self.intro_view(uic, i),
                            None => self.clock_view(uic),
                        }
                    });
            });

        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

impl AppView {
    /// Board 05c: the ring, today's rules, one verb, the honest footer.
    fn clock_view(&mut self, uic: &mut egui::Ui) {
        let me = self.me().cloned();
        let h = hero(self.status.as_ref(), me.as_ref(), chrono::Local::now());
        uic.vertical_centered(|uic| {
            uic.add_space(24.0);
            let d = 168.0;
            let (rect, _) = uic.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
            let c = rect.center();
            let p = uic.painter();
            ui::ring(p, c, d, &h.ring, false);
            if h.paused {
                ui::paint_icon(
                    p,
                    egui::Rect::from_center_size(c - egui::vec2(0.0, 10.0), egui::vec2(36.0, 36.0)),
                    Icon::Pause,
                    col(ui::INK_2),
                );
                ui::ring_center(
                    p,
                    c + egui::vec2(0.0, 22.0),
                    "",
                    0.0,
                    col(ui::INK),
                    &h.label,
                    13.0,
                    0.0,
                );
            } else {
                let size = if h.big.chars().count() > 3 {
                    40.0
                } else {
                    56.0
                };
                ui::ring_center(p, c, &h.big, size, col(ui::INK), &h.label, 13.0, 4.0);
            }
        });
        uic.add_space(22.0);

        if let Some((line, fg, bg)) = self.notice() {
            egui::Frame::default()
                .fill(col(bg))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 11.0))
                .show(uic, |uic| {
                    uic.set_width(uic.available_width());
                    uic.add(
                        egui::Label::new(
                            egui::RichText::new(line)
                                .font(ui::font(14.0, W::Medium))
                                .color(col(fg)),
                        )
                        .wrap(),
                    );
                });
            uic.add_space(16.0);
        }

        // Today's three rules, in one glance.
        if let Some(t) = me.as_ref().and_then(|u| u.today.clone()) {
            let mut rows: Vec<(Icon, &str, String)> = vec![(
                Icon::Clock,
                "Every day",
                t.limit_minutes
                    .map(glance::duration)
                    .unwrap_or_else(|| "No limit".into()),
            )];
            if let Some(w) = &t.screens_on {
                rows.push((Icon::AllowedHours, "Screens on", w.clone()));
            }
            if let Some(b) = &t.bedtime {
                rows.push((Icon::Moon, "Bedtime", b.clone()));
            }
            rules_card(uic, &rows);
            uic.add_space(22.0);
        }

        // The one verb.
        if let Some(u) = &me {
            if u.can_ask && !u.self_managed {
                let w = uic.available_width();
                if self.asked() {
                    Button::new("Asked — waiting for a parent", Kind::Primary)
                        .icon(Icon::Check)
                        .width(w)
                        .enabled(false)
                        .show(uic);
                    uic.add_space(8.0);
                    uic.vertical_centered(|uic| {
                        ui::text(
                            uic,
                            "Sent — a parent can say yes.",
                            13.0,
                            W::Medium,
                            ui::BRAND_INK,
                        );
                    });
                } else if Button::new("Ask for more time", Kind::Primary)
                    .icon(Icon::Ask)
                    .width(w)
                    .show(uic)
                    .clicked()
                {
                    self.asked = request_more_time();
                }
            }
        }

        // The honest footer, always in view: the product's thesis, not fine print.
        uic.add_space(16.0);
        let (rect, _) =
            uic.allocate_exact_size(egui::vec2(uic.available_width(), 1.0), egui::Sense::hover());
        uic.painter().line_segment(
            [rect.left_center(), rect.right_center()],
            egui::Stroke::new(1.0, col(ui::LINE)),
        );
        uic.add_space(14.0);
        let footer = match &me {
            Some(u) => glance::footer(u.sees, u.shared_sites),
            None => "It doesn't keep time for you here; sites are counted for the whole \
                     computer. It can't see your screen, your messages or what you type."
                .to_string(),
        };
        uic.add(
            egui::Label::new(
                egui::RichText::new(footer)
                    .font(ui::font(12.5, W::Regular))
                    .color(col(ui::INK_3)),
            )
            .wrap(),
        );
    }

    /// The first-run cards, one at a time.
    fn intro_view(&mut self, uic: &mut egui::Ui, i: usize) {
        let me = self.me().cloned().unwrap_or_default();
        let sees = if self.me().is_some() {
            me.sees
        } else {
            Sees::TimeAppsSites
        };
        let cards = intro_cards(sees, me.shared_sites, me.can_ask || self.me().is_none());
        let i = i.min(cards.len() - 1);
        let (icon, title, body) = &cards[i];
        uic.add_space(40.0);
        egui::Frame::default()
            .fill(col(ui::SURFACE))
            .stroke(egui::Stroke::new(1.0, col(ui::LINE)))
            .rounding(egui::Rounding::same(16.0))
            .inner_margin(egui::Margin::same(24.0))
            .show(uic, |uic| {
                uic.set_width(uic.available_width());
                let (r, _) = uic.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                uic.painter()
                    .circle_filled(r.center(), 22.0, col(ui::BRAND_TINT));
                ui::paint_icon(
                    uic.painter(),
                    egui::Rect::from_center_size(r.center(), egui::vec2(24.0, 24.0)),
                    *icon,
                    col(ui::BRAND_INK),
                );
                uic.add_space(18.0);
                uic.add(
                    egui::Label::new(egui::widget_text::WidgetText::LayoutJob(ui::job(
                        title,
                        ui::font(24.0, W::Bold),
                        col(ui::INK),
                        -0.02,
                    )))
                    .wrap(),
                );
                uic.add_space(10.0);
                uic.add(
                    egui::Label::new(
                        egui::RichText::new(body.as_str())
                            .font(ui::font(16.0, W::Regular))
                            .color(col(ui::INK_2)),
                    )
                    .wrap(),
                );
                uic.add_space(8.0);
            });
        uic.add_space(22.0);
        // Where you are: quiet dots (the ring only ever means time used today).
        uic.horizontal(|uic| {
            let n = cards.len();
            let (r, _) =
                uic.allocate_exact_size(egui::vec2(n as f32 * 14.0, 20.0), egui::Sense::hover());
            for k in 0..n {
                let c = egui::pos2(r.left() + 5.0 + k as f32 * 14.0, r.center().y);
                let colr = if k == i { ui::BRAND } else { ui::LINE_2 };
                uic.painter().circle_filled(c, 4.0, col(colr));
            }
            uic.with_layout(egui::Layout::right_to_left(egui::Align::Center), |uic| {
                let last = i + 1 >= cards.len();
                let next = Button::new(if last { "Done" } else { "Next" }, Kind::Primary);
                let next = if last {
                    next
                } else {
                    next.icon(Icon::ArrowRight)
                };
                if next.show(uic).clicked() {
                    if last {
                        mark_intro_seen();
                        self.intro = None;
                    } else {
                        self.intro = Some(i + 1);
                    }
                }
                if !last && Button::new("Skip", Kind::Quiet).show(uic).clicked() {
                    mark_intro_seen();
                    self.intro = None;
                }
            });
        });
    }
}

/// Today's rules as a card of rows (board `.rules`).
fn rules_card(uic: &mut egui::Ui, rows: &[(Icon, &str, String)]) {
    egui::Frame::default()
        .fill(col(ui::SURFACE))
        .stroke(egui::Stroke::new(1.0, col(ui::LINE)))
        .rounding(egui::Rounding::same(10.0))
        .show(uic, |uic| {
            uic.set_width(uic.available_width());
            uic.spacing_mut().item_spacing.y = 0.0;
            for (k, (icon, label, value)) in rows.iter().enumerate() {
                if k > 0 {
                    let (r, _) = uic.allocate_exact_size(
                        egui::vec2(uic.available_width(), 1.0),
                        egui::Sense::hover(),
                    );
                    uic.painter().line_segment(
                        [r.left_center(), r.right_center()],
                        egui::Stroke::new(1.0, col(ui::LINE)),
                    );
                }
                let (r, _) = uic.allocate_exact_size(
                    egui::vec2(uic.available_width(), 42.0),
                    egui::Sense::hover(),
                );
                let p = uic.painter();
                let ir = egui::Rect::from_center_size(
                    egui::pos2(r.left() + 14.0 + 10.0, r.center().y),
                    egui::vec2(20.0, 20.0),
                );
                ui::paint_icon(p, ir, *icon, col(ui::INK_2));
                let l = p.layout_job(ui::job(
                    label,
                    ui::font(14.0, W::Regular),
                    col(ui::INK),
                    0.0,
                ));
                p.galley(
                    egui::pos2(ir.right() + 12.0, r.center().y - l.size().y / 2.0),
                    l,
                    col(ui::INK),
                );
                let v = p.layout_job(ui::job(
                    value,
                    ui::font(14.0, W::SemiBold),
                    col(ui::INK),
                    0.0,
                ));
                p.galley(
                    egui::pos2(
                        r.right() - 14.0 - v.size().x,
                        r.center().y - v.size().y / 2.0,
                    ),
                    v,
                    col(ui::INK),
                );
            }
        });
}

/// The sign-in code card: the code, big and spaced, and one plain line on
/// where to type it.
fn code_card(
    uic: &mut egui::Ui,
    c: &crate::logincode::LoginCode,
    now: chrono::DateTime<chrono::Utc>,
) {
    egui::Frame::default()
        .fill(col(ui::BRAND_TINT))
        .rounding(egui::Rounding::same(16.0))
        .inner_margin(egui::Margin::same(18.0))
        .show(uic, |uic| {
            uic.set_width(uic.available_width());
            ui::text(uic, c.headline(), 13.0, W::SemiBold, ui::BRAND_INK);
            uic.add_space(4.0);
            uic.label(egui::widget_text::WidgetText::LayoutJob(ui::job(
                &c.spaced(),
                ui::mono(40.0),
                col(ui::BRAND_INK),
                0.06,
            )));
            uic.add_space(4.0);
            uic.add(
                egui::Label::new(
                    egui::RichText::new(c.instructions())
                        .font(ui::font(14.0, W::Regular))
                        .color(col(ui::INK)),
                )
                .wrap(),
            );
            let m = c.minutes_left(now);
            uic.add_space(4.0);
            ui::text(
                uic,
                &format!(
                    "Works for {m} more minute{}.",
                    if m == 1 { "" } else { "s" }
                ),
                12.5,
                W::Regular,
                ui::INK_2,
            );
        });
}

// ---------------------------------------------------------------------------
// One window per person
// ---------------------------------------------------------------------------

enum Instance {
    /// This process holds the window: keep the lock, listen for "show".
    Primary {
        _lock: std::fs::File,
        listener: std::os::unix::net::UnixListener,
    },
    /// Another window was already open and has been asked to come forward.
    Signalled,
}

/// Claim the one window in `dir`, or ask the window that has it to come
/// forward. The lock (flock, dies with the process) decides; the socket is
/// only the doorbell.
fn claim(dir: &Path) -> std::io::Result<Instance> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("app.lock"))?;
    let sock = dir.join("app.sock");
    // SAFETY: flock on a descriptor we own; no memory is shared.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock)?;
        return Ok(Instance::Primary { _lock: f, listener });
    }
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&sock) {
        let _ = s.write_all(b"show\n");
    }
    Ok(Instance::Signalled)
}

/// Ring the doorbell: every connection to the socket means "come forward".
fn listen(listener: std::os::unix::net::UnixListener, raise: Arc<AtomicBool>, ctx: egui::Context) {
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            if conn.is_ok() {
                raise.store(true, Ordering::SeqCst);
                ctx.request_repaint();
            }
        }
    });
}

/// Entry point for `ost app`: open the window (or bring the open one
/// forward) and block until it closes.
pub fn run() -> anyhow::Result<()> {
    let username = std::env::var("USER")
        .ok()
        .or_else(|| users::get_current_username().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();

    let listener = match claim(&runtime_dir())? {
        Instance::Signalled => return Ok(()),
        Instance::Primary { _lock, listener } => (listener, _lock),
    };

    let native = eframe::NativeOptions {
        viewport: ui::identity(egui::ViewportBuilder::default())
            .with_inner_size([420.0, 640.0])
            .with_min_inner_size([380.0, 520.0]),
        ..Default::default()
    };
    let initial = read_status(&username);
    let intro = (!intro_seen()).then_some(0);
    let raise = Arc::new(AtomicBool::new(false));
    if let Err(e) = eframe::run_native(
        "openscreentime",
        native,
        Box::new(move |cc| {
            ui::install(&cc.egui_ctx);
            let (l, _keep) = listener;
            listen(l, raise.clone(), cc.egui_ctx.clone());
            std::mem::forget(_keep); // held until the process exits
            Ok(Box::new(AppView {
                username,
                status: initial,
                read_at: Some(Instant::now()),
                asked: false,
                shown_code: None,
                intro,
                raise,
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

    fn status(users: Vec<UserStatus>) -> Status {
        Status {
            connection: "online".into(),
            users,
            ..Default::default()
        }
    }
    fn now() -> chrono::DateTime<chrono::Local> {
        chrono::Local::now()
    }
    /// A status as the agent writes it: `remaining` is the budget, the
    /// verdict's stop is that far ahead (limit, counting).
    fn kid(remaining: Option<i64>, frozen: bool) -> UserStatus {
        let n = now();
        UserStatus {
            name: "kid".into(),
            used_minutes: 48,
            clock: glance::Clock {
                remaining_minutes: remaining,
                allowed: Some(!frozen && remaining.is_none_or(|m| m > 0)),
                reason: remaining.map(|_| "limit".into()),
                minutes_left: remaining.map(|m| m.max(0) as u32),
                stop_at: remaining.map(|m| (n + chrono::Duration::minutes(m.max(0))).to_rfc3339()),
                counting: true,
                frozen,
                ..Default::default()
            },
            can_ask: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_ring_is_time_used_today() {
        let hero = |s: Option<&Status>, u: Option<&UserStatus>| hero(s, u, now());
        // 48 used, 27 left: 64 % of the ring, green, "27 min left".
        let s = status(vec![kid(Some(27), false)]);
        let h = hero(Some(&s), s.users.first());
        assert_eq!((h.big.as_str(), h.label.as_str()), ("27", "min left"));
        match h.ring {
            RingState::Fill { frac, color } => {
                assert!((frac - 0.64).abs() < 0.001);
                assert_eq!(color, ui::BRAND);
            }
            _ => panic!("a partial ring"),
        }
        // Fifteen minutes or less: amber, the one transition.
        let s = status(vec![kid(Some(10), false)]);
        assert!(matches!(
            hero(Some(&s), s.users.first()).ring,
            RingState::Fill {
                color: ui::WARN,
                ..
            }
        ));
        // Past an hour, hours and minutes.
        assert_eq!(left_words(108), "1 h 48");
        // Used up: the completed red ring, "0 min left".
        let s = status(vec![kid(Some(0), false)]);
        let h = hero(Some(&s), s.users.first());
        assert!(matches!(h.ring, RingState::Full { color: ui::STOP }));
        assert_eq!(h.big, "0");
        // No limit: the track, and the time used — never a missing glyph.
        let s = status(vec![kid(None, false)]);
        let h = hero(Some(&s), s.users.first());
        assert!(matches!(h.ring, RingState::Track));
        assert_eq!((h.big.as_str(), h.label.as_str()), ("48", "min today"));
        // A parent's pause: the neutral dashed ring, not red.
        let mut s = status(vec![kid(Some(20), false)]);
        s.device_locked = true;
        let h = hero(Some(&s), s.users.first());
        assert!(h.paused && matches!(h.ring, RingState::Paused));
        // The agent isn't running: a calm track, no number.
        let h = hero(None, None);
        assert!(h.big.is_empty() && matches!(h.ring, RingState::Track));
    }

    /// Acceptance, step 5: after the unlock code Mia had 30 minutes, but the
    /// window showed a red "0 min left" (it read the spent budget). The
    /// window says what the rules say: 29 left, unlocked until then.
    #[test]
    fn an_unlock_code_shows_its_time_not_a_red_zero() {
        let n = now();
        let until = n + chrono::Duration::minutes(29);
        let mia = UserStatus {
            name: "mia".into(),
            used_minutes: 6,
            clock: glance::Clock {
                remaining_minutes: Some(-1),
                allowed: Some(true),
                reason: Some("limit".into()),
                minutes_left: Some(29),
                stop_at: Some(until.to_rfc3339()),
                override_until: Some(until.to_rfc3339()),
                counting: true,
                frozen: false,
            },
            ..Default::default()
        };
        let s = status(vec![mia]);
        let h = hero(Some(&s), s.users.first(), n);
        assert_eq!((h.big.as_str(), h.label.as_str()), ("29", "min left"));
        assert!(
            !matches!(h.ring, RingState::Full { color: ui::STOP }),
            "never a red zero while unlocked"
        );
        let line = unlocked_line(&s.users[0].clock, n).expect("says it is unlocked");
        assert_eq!(line, format!("Unlocked until {}.", until.format("%H:%M")));
    }

    /// Acceptance, step 6a: after "Give 15" on a day already over the limit
    /// the window said 9 and hit 0 at 00:22 while the lock came at 00:27. It
    /// counts down to the one stop the warnings announce.
    #[test]
    fn a_grant_counts_down_to_the_announced_stop() {
        let n = now();
        let stop = n + chrono::Duration::minutes(15);
        let mia = UserStatus {
            name: "mia".into(),
            used_minutes: 10,
            clock: glance::Clock {
                remaining_minutes: Some(10), // limit 5 + earned 15 − used 10
                allowed: Some(true),
                reason: Some("limit".into()),
                minutes_left: Some(15),
                stop_at: Some(stop.to_rfc3339()),
                override_until: Some(stop.to_rfc3339()),
                counting: true,
                frozen: false,
            },
            ..Default::default()
        };
        let s = status(vec![mia]);
        assert_eq!(hero(Some(&s), s.users.first(), n).big, "15");
        // Ten minutes on: 5 left, not 0 — the lock comes when the ring says.
        let later = n + chrono::Duration::minutes(10);
        assert_eq!(hero(Some(&s), s.users.first(), later).big, "5");
    }

    #[test]
    fn first_run_cards_are_true_for_the_reader() {
        let kid = intro_cards(Sees::TimeAppsSites, false, true);
        assert_eq!(kid.len(), 5);
        assert!(kid[1].2.contains("which sites"));
        assert!(kid[4].2.contains("Ask for more time"));
        let teen = intro_cards(Sees::TimeApps, false, true);
        assert!(teen[1].2.contains("not the sites"));
        let adult = intro_cards(Sees::Own, false, false);
        assert!(adult[0].1.contains("the time you set"));
        assert!(adult[4].2.contains("15 more minutes"));
        assert!(adult.iter().all(|c| !c.2.contains("Ask for more time")));
        // Someone little can't ask: no card promising it.
        let little = intro_cards(Sees::TimeAppsSites, false, false);
        assert_eq!(little.len(), 4);
        for (_, t, b) in kid.iter().chain(&teen).chain(&adult) {
            assert_ne!(t, &t.to_uppercase(), "sentence case");
            assert!(!b.is_empty());
        }
    }

    #[test]
    fn a_second_window_rings_the_first() {
        let dir = std::env::temp_dir().join(format!(
            "ost-app-instance-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let first = claim(&dir).unwrap();
        let Instance::Primary { listener, _lock } = first else {
            panic!("the first claim holds the window");
        };
        listener.set_nonblocking(false).unwrap();
        let second = claim(&dir).unwrap();
        assert!(
            matches!(second, Instance::Signalled),
            "a second is turned away"
        );
        let (mut conn, _) = listener.accept().expect("…after ringing the first");
        let mut msg = String::new();
        use std::io::Read;
        conn.read_to_string(&mut msg).unwrap();
        assert_eq!(msg, "show\n");
        // Once the first is gone, the next launch holds the window again.
        drop(_lock);
        drop(listener);
        assert!(matches!(claim(&dir).unwrap(), Instance::Primary { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_from_an_older_agent_still_reads() {
        let s: Status = serde_json::from_str(
            r#"{"connection":"online","users":[{"name":"kid","used_minutes":5,"remaining_minutes":40}]}"#,
        )
        .unwrap();
        let u = &s.users[0];
        assert!(u.can_ask, "an older agent's managed user may ask");
        assert_eq!(u.sees, Sees::TimeAppsSites);
        assert!(u.today.is_none());
    }
}
