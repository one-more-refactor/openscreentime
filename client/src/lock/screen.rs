//! The graphical lock: an egui window inside `cage`, running as `ost-lock` in
//! `openscreentime-lock@<vt>.service`. Compiled only with `--features gui`.
//!
//! It holds nothing worth stealing: it shows the face the agent publishes and
//! passes what is typed to the agent over the lock socket, which does all the
//! checking. The look is today's ring language (DESIGN-CLIENT.md §4, commit
//! 610e9ab): the completed ring, one plain sentence, the way back in.

use super::socket::{self, Outcome, Request};
use super::text::group;
use super::{AskState, CodeState, Face, Look};
use crate::ui::{self, col, RingState};
use eframe::egui;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// `ost __lock-session`: the lock unit's entry point. Runs `cage` (no `-s`:
/// the keyboard can't switch VTs) hosting `ost __lockscreen`. If cage can't be
/// started the unit fails, and the agent's text lock takes the VT instead.
pub fn run_session() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|d| std::path::Path::new(d).is_dir())
        .unwrap_or_else(|| "/run/openscreentime-lock".to_string());
    let cage = ["/usr/bin/cage", "/usr/local/bin/cage", "/bin/cage"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| anyhow::anyhow!("cage is not installed"))?;
    let exe = std::env::current_exe()?;
    let err = std::process::Command::new(cage)
        .arg("-d")
        .arg("--")
        .arg(exe)
        .arg("__lockscreen")
        .env("XDG_RUNTIME_DIR", &runtime)
        // Mesa / fontconfig caches: somewhere of ours, never a real home.
        .env("HOME", &runtime)
        .env("XDG_CACHE_HOME", format!("{runtime}/cache"))
        .exec();
    Err(anyhow::anyhow!("could not start cage: {err}"))
}

/// What the window knows, kept current by a worker thread that talks to the
/// agent so the UI never blocks on the socket.
#[derive(Default)]
struct View {
    face: Option<Face>,
    /// The agent answered the last poll.
    connected: bool,
    /// A code or ask is on its way.
    busy: bool,
    message: Option<Outcome>,
    /// The agent took the lock down.
    released: bool,
    /// A wrong code just came back: flash the ring once.
    flash: Option<Instant>,
}

/// `ost __lockscreen`: the window inside cage.
pub fn run_lockscreen() -> anyhow::Result<()> {
    let view = Arc::new(Mutex::new(View::default()));
    let (tx, rx) = mpsc::channel::<Request>();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OpenScreenTime")
            .with_app_id("openscreentime-lock")
            .with_fullscreen(true)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "OpenScreenTime",
        native,
        Box::new(move |cc| {
            ui::install(cc);
            spawn_worker(view.clone(), rx, cc.egui_ctx.clone());
            Ok(Box::new(LockWindow {
                view,
                tx,
                typed: String::new(),
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("lock window: {e}"))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn spawn_worker(view: Arc<Mutex<View>>, rx: mpsc::Receiver<Request>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let at = socket::path();
        loop {
            let req = match rx.recv_timeout(Duration::from_millis(1200)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => Request::Face,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let asked = req != Request::Face;
            if asked {
                lock(&view).busy = true;
                ctx.request_repaint();
            }
            let reply = socket::call(&at, &req);
            {
                let mut v = lock(&view);
                v.busy = false;
                match reply {
                    Ok(r) => {
                        v.connected = true;
                        v.released = r.face.is_none();
                        if r.face.is_some() {
                            v.face = r.face;
                        }
                        if let Some(out) = r.result {
                            if !out.ok && matches!(req, Request::Code { .. }) {
                                v.flash = Some(Instant::now());
                            }
                            v.message = Some(out);
                        }
                    }
                    Err(_) => {
                        v.connected = false;
                        if asked {
                            v.message = Some(Outcome::no(
                                "Can't check that right now — try again in a moment.",
                            ));
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    });
}

struct LockWindow {
    view: Arc<Mutex<View>>,
    tx: mpsc::Sender<Request>,
    /// Digits typed so far, shown grouped ("123 456").
    typed: String,
}

impl eframe::App for LockWindow {
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
        let (face, busy, message, released, flashing, connected) = {
            let v = lock(&self.view);
            (
                v.face.clone().unwrap_or_else(Face::waiting),
                v.busy,
                v.message.clone(),
                v.released,
                v.flash
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(240)),
                v.connected,
            )
        };
        if flashing {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(col(ui::BG))
                    .inner_margin(egui::Margin::same(24.0)),
            )
            .show(ctx, |uic| {
                uic.vertical_centered(|uic| {
                    uic.add_space((uic.available_height() * 0.08).min(70.0));
                    wordmark(uic);
                    uic.add_space(28.0);

                    // The hero ring — the gauge completed, not an alarm.
                    let d = 200.0;
                    let (rect, _) = uic.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
                    let c = rect.center();
                    if released {
                        ui::ring(uic.painter(), c, d, &RingState::Full { color: ui::BRAND });
                        ui::glyph_check(uic.painter(), c, 60.0, ui::BRAND);
                    } else {
                        let (state, gcol) = match face.look {
                            Look::Wall => (RingState::Full { color: ui::STOP }, ui::STOP),
                            Look::Night => (RingState::Full { color: ui::INK_2 }, ui::INK_2),
                            Look::Paused => (RingState::Paused, ui::INK_3),
                        };
                        let (state, gcol) = if flashing {
                            (RingState::Full { color: ui::STOP }, ui::STOP)
                        } else {
                            (state, gcol)
                        };
                        ui::ring(uic.painter(), c, d, &state);
                        match face.look {
                            Look::Wall => ui::glyph_padlock(uic.painter(), c, 58.0, gcol),
                            Look::Night => ui::glyph_moon(uic.painter(), c, 58.0, gcol),
                            Look::Paused => ui::glyph_pause(uic.painter(), c, 54.0, gcol),
                        }
                    }
                    uic.add_space(26.0);

                    uic.set_max_width(520.0);
                    let title = if released {
                        "You're back"
                    } else {
                        face.title.as_str()
                    };
                    uic.label(
                        egui::RichText::new(title)
                            .font(ui::font(32.0))
                            .strong()
                            .color(col(ui::INK)),
                    );
                    if !face.detail.is_empty() && !released {
                        uic.add_space(8.0);
                        uic.label(
                            egui::RichText::new(&face.detail)
                                .font(ui::font(18.0))
                                .color(col(ui::INK_2)),
                        );
                    }
                    uic.add_space(28.0);
                    if released {
                        return;
                    }

                    // The way back in: a code field that already has the keyboard.
                    match face.code {
                        CodeState::Ready { tries_left } => {
                            self.code_field(uic, busy);
                            uic.add_space(6.0);
                            let hint = match (&message, busy) {
                                (_, true) => "Checking…".to_string(),
                                _ => format!(
                                    "Unlock code · {tries_left} {} left",
                                    if tries_left == 1 { "try" } else { "tries" }
                                ),
                            };
                            uic.label(
                                egui::RichText::new(hint)
                                    .font(ui::font(14.0))
                                    .color(col(ui::INK_3)),
                            );
                        }
                        CodeState::Wait { secs } => {
                            uic.label(
                                egui::RichText::new(format!(
                                    "Too many tries — wait {} to try again.",
                                    human_secs(secs)
                                ))
                                .font(ui::font(18.0))
                                .color(col(ui::INK_2)),
                            );
                        }
                        CodeState::Unavailable => {
                            uic.label(
                                egui::RichText::new("There's no unlock code on this computer yet.")
                                    .font(ui::font(18.0))
                                    .color(col(ui::INK_2)),
                            );
                        }
                    }
                    if let Some(m) = &message {
                        uic.add_space(8.0);
                        uic.label(
                            egui::RichText::new(&m.message)
                                .font(ui::font(15.0))
                                .color(col(if m.ok { ui::BRAND_INK } else { ui::STOP })),
                        );
                    }
                    uic.add_space(26.0);

                    match face.ask {
                        AskState::Ready => {
                            if secondary_button(uic, "Ask for more time", !busy) {
                                let _ = self.tx.send(Request::Ask);
                            }
                        }
                        AskState::Sent => {
                            uic.label(
                                egui::RichText::new("Asked — a parent will see it.")
                                    .font(ui::font(15.0))
                                    .color(col(ui::INK_2)),
                            );
                        }
                        AskState::Hidden => {}
                    }
                    uic.add_space(22.0);
                    let help = if connected {
                        face.help.as_str()
                    } else {
                        "Waiting for OpenScreenTime to answer…"
                    };
                    uic.label(
                        egui::RichText::new(help)
                            .font(ui::font(14.0))
                            .color(col(ui::INK_3)),
                    );
                });
            });
    }
}

impl LockWindow {
    /// Digits only, grouped as printed, focused from the first frame, Enter
    /// submits. It keeps the keyboard: focus is taken back every frame.
    fn code_field(&mut self, uic: &mut egui::Ui, busy: bool) {
        let mut buf = group(&self.typed);
        let out = egui::TextEdit::singleline(&mut buf)
            .font(ui::mono(30.0))
            .hint_text("123 456")
            .desired_width(250.0)
            .horizontal_align(egui::Align::Center)
            .interactive(!busy)
            .show(uic);
        if out.response.changed() {
            self.typed = buf.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
            let shown = group(&self.typed);
            let mut st = out.state.clone();
            let end = egui::text::CCursor::new(shown.chars().count());
            st.cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            st.store(uic.ctx(), out.response.id);
        }
        if !out.response.has_focus() && !busy {
            out.response.request_focus();
        }
        let enter = uic.input(|i| i.key_pressed(egui::Key::Enter));
        if enter && !busy && !self.typed.is_empty() {
            let code = std::mem::take(&mut self.typed);
            let _ = self.tx.send(Request::Code { code });
        }
    }
}

fn wordmark(uic: &mut egui::Ui) {
    uic.horizontal(|uic| {
        uic.add_space(((uic.available_width() - 175.0) / 2.0).max(0.0));
        let (m, _) = uic.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        ui::ring(
            uic.painter(),
            m.center(),
            20.0,
            &RingState::Fill {
                frac: 0.4,
                color: ui::BRAND,
            },
        );
        uic.add_space(8.0);
        uic.label(
            egui::RichText::new("OpenScreenTime")
                .font(ui::font(15.0))
                .strong()
                .color(col(ui::INK_3)),
        );
    });
}

fn secondary_button(uic: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    uic.add_enabled_ui(enabled, |uic| {
        uic.add_sized(
            [240.0, 44.0],
            egui::Button::new(
                egui::RichText::new(text)
                    .font(ui::font(16.0))
                    .strong()
                    .color(col(ui::BRAND_INK)),
            )
            .fill(col(ui::SURFACE))
            .stroke(egui::Stroke::new(1.0, col(ui::LINE_2)))
            .rounding(egui::Rounding::same(22.0)),
        )
        .clicked()
    })
    .inner
}

fn human_secs(s: u64) -> String {
    if s >= 90 {
        format!("{} minutes", s.div_ceil(60))
    } else {
        format!("{s} seconds")
    }
}
