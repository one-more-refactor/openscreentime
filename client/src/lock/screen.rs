//! The graphical lock: an egui window inside `cage`, running as `ost-lock` in
//! `openscreentime-lock@<vt>.service`. Compiled only with `--features gui`.
//!
//! It holds nothing worth stealing: it shows the face the agent publishes and
//! passes what is typed to the agent over the lock socket, which does all the
//! checking. The look is brand board 05a: the day's ring completed, with "0 min
//! left" inside, one plain sentence, a code field that already has the
//! keyboard, and — separately — the person's own way forward.

use super::socket::{self, Outcome, Request};
use super::text::group;
use super::{AskState, CodeState, Face, Look, Snooze};
use crate::icons::Icon;
use crate::ui::{self, col, Button, Kind, RingState, Size, W};
use eframe::egui;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a first cage gets before a quick exit counts as "this GPU path
/// doesn't work here" and the software renderer is tried.
const QUICK_EXIT: Duration = Duration::from_secs(6);

/// The environment for a cage that renders on the CPU: wlroots' pixman
/// renderer, and Mesa's llvmpipe for the lock window's GL (EGL on Wayland
/// falls back to wl_shm buffers, which is all a pixman compositor takes).
pub const SOFTWARE_ENV: [(&str, &str); 3] = [
    ("WLR_RENDERER", "pixman"),
    ("LIBGL_ALWAYS_SOFTWARE", "1"),
    ("WLR_NO_HARDWARE_CURSORS", "1"),
];

/// `ost __lock-session`: the lock unit's entry point. Runs `cage` (no `-s`:
/// the keyboard can't switch VTs) hosting `ost __lockscreen`. If cage (or the
/// window in it) gives up at once — a GPU wlroots can't drive, like QEMU's
/// standard VGA ("PRIME import not supported") — it is tried once more on the
/// CPU. If that fails too the unit fails, and the agent's text lock takes the
/// VT instead.
pub fn run_session() -> anyhow::Result<()> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|d| std::path::Path::new(d).is_dir())
        .unwrap_or_else(|| "/run/openscreentime-lock".to_string());
    let cage = ["/usr/bin/cage", "/usr/local/bin/cage", "/bin/cage"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| anyhow::anyhow!("cage is not installed"))?;
    let exe = std::env::current_exe()?;
    let run = |software: bool| -> anyhow::Result<(std::process::ExitStatus, Duration)> {
        let mut cmd = std::process::Command::new(cage);
        cmd.arg("-d")
            .arg("--")
            .arg(&exe)
            .arg("__lockscreen")
            .env("XDG_RUNTIME_DIR", &runtime)
            // Mesa / fontconfig caches: somewhere of ours, never a real home.
            .env("HOME", &runtime)
            .env("XDG_CACHE_HOME", format!("{runtime}/cache"));
        if software {
            cmd.envs(SOFTWARE_ENV);
        }
        let started = Instant::now();
        let status = cmd
            .status()
            .map_err(|e| anyhow::anyhow!("could not start cage: {e}"))?;
        Ok((status, started.elapsed()))
    };
    let (status, took) = run(false)?;
    if took >= QUICK_EXIT {
        // It ran; it ended (the agent stopped the unit, or the window closed).
        return exit_with(status);
    }
    eprintln!(
        "cage exited after {:.1} s ({status}); trying the software renderer",
        took.as_secs_f32()
    );
    let (status, _) = run(true)?;
    exit_with(status)
}

fn exit_with(status: std::process::ExitStatus) -> anyhow::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("cage exited: {status}"))
    }
}

/// What the window knows, kept current by a worker thread that talks to the
/// agent so the UI never blocks on the socket.
#[derive(Default)]
struct View {
    face: Option<Face>,
    /// When `face` arrived (the snooze countdown runs from it).
    face_at: Option<Instant>,
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
        viewport: ui::identity(egui::ViewportBuilder::default())
            .with_fullscreen(true)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "openscreentime",
        native,
        Box::new(move |cc| {
            ui::install(&cc.egui_ctx);
            spawn_worker(view.clone(), rx, cc.egui_ctx.clone());
            Ok(Box::new(LockWindow {
                view,
                tx,
                typed: String::new(),
                content_h: 0.0,
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
        // Say hello at once: the agent waits for the first word before it
        // freezes anyone, and falls back to the text lock if none comes.
        let mut first = true;
        loop {
            let req = if std::mem::take(&mut first) {
                Request::Face
            } else {
                match rx.recv_timeout(Duration::from_millis(1000)) {
                    Ok(r) => r,
                    Err(mpsc::RecvTimeoutError::Timeout) => Request::Face,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
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
                            v.face_at = Some(Instant::now());
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
    /// Height of the centred block last frame (to centre it on this one).
    content_h: f32,
}

/// The column everything below the heading lines up in (board: 380 px).
const COLUMN: f32 = 380.0;

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
        let (face, face_at, busy, message, released, flashing, connected) = {
            let v = lock(&self.view);
            (
                v.face.clone().unwrap_or_else(Face::waiting),
                v.face_at,
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

        // How a parent gets you out, pinned to the bottom (board `.help`).
        egui::TopBottomPanel::bottom("help")
            .frame(
                egui::Frame::default()
                    .fill(col(ui::BG))
                    .inner_margin(egui::Margin::symmetric(24.0, 24.0)),
            )
            .show_separator_line(false)
            .show(ctx, |uic| {
                uic.vertical_centered(|uic| {
                    let help = if released {
                        ""
                    } else if connected {
                        face.help.as_str()
                    } else {
                        "Waiting for OpenScreenTime to answer…"
                    };
                    ui::text(uic, help, 13.0, W::Regular, ui::INK_3);
                    // A shared computer: someone else can still sign in.
                    if face.switch_user && connected && !released {
                        uic.add_space(8.0);
                        if Button::new(super::SWITCH_USER, Kind::Quiet)
                            .size(Size::Sm)
                            .enabled(!busy)
                            .show(uic)
                            .clicked()
                        {
                            let _ = self.tx.send(Request::SwitchUser);
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(col(ui::BG))
                    .inner_margin(egui::Margin {
                        left: 24.0,
                        right: 24.0,
                        top: 28.0,
                        bottom: 0.0,
                    }),
            )
            .show(ctx, |uic| {
                uic.vertical_centered(|uic| {
                    ui::lockup(uic, 16.0, true);
                    // Centre the rest in what's left (measured last frame).
                    let spare = uic.available_height() - self.content_h;
                    uic.add_space((spare / 2.0).clamp(24.0, 160.0));
                    let top = uic.cursor().top();
                    self.body(uic, &face, face_at, busy, &message, released, flashing);
                    self.content_h = uic.cursor().top() - top;
                });
            });
    }
}

impl LockWindow {
    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        uic: &mut egui::Ui,
        face: &Face,
        face_at: Option<Instant>,
        busy: bool,
        message: &Option<Outcome>,
        released: bool,
        flashing: bool,
    ) {
        // The day's ring, completed — or the neutral dashed ring of a pause.
        let d = 220.0;
        let (rect, _) = uic.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
        let c = rect.center();
        let p = uic.painter();
        if released {
            ui::ring(p, c, d, &RingState::Track, false);
            ui::paint_icon(
                p,
                egui::Rect::from_center_size(c, egui::vec2(56.0, 56.0)),
                Icon::Check,
                col(ui::BRAND),
            );
        } else {
            let state = match (flashing, face.look) {
                (true, _) | (false, Look::Wall | Look::Night) => {
                    RingState::Full { color: ui::STOP }
                }
                (false, Look::Paused) => RingState::Paused,
            };
            ui::ring(p, c, d, &state, false);
            if face.look == Look::Paused {
                ui::paint_icon(
                    p,
                    egui::Rect::from_center_size(c - egui::vec2(0.0, 12.0), egui::vec2(44.0, 44.0)),
                    Icon::Pause,
                    col(ui::INK_2),
                );
                ui::ring_center(
                    p,
                    c + egui::vec2(0.0, 26.0),
                    "",
                    0.0,
                    col(ui::INK),
                    "paused",
                    14.0,
                    0.0,
                );
            } else {
                ui::ring_center(p, c, "0", 64.0, col(ui::INK), "min left", 14.0, 6.0);
            }
        }
        uic.add_space(24.0);

        // One sentence: why.
        let title = if released {
            "You're back"
        } else {
            face.title.as_str()
        };
        uic.label(egui::widget_text::WidgetText::LayoutJob(ui::job(
            title,
            ui::font(34.0, W::Bold),
            col(ui::INK),
            -0.02,
        )));
        if !face.detail.is_empty() && !released {
            uic.add_space(8.0);
            ui::para(uic, &face.detail, 18.0, W::Regular, ui::INK_2, 560.0);
        }
        if released {
            return;
        }
        uic.add_space(28.0);

        // The code: a parent standing there just types.
        let width = COLUMN.min(uic.available_width());
        uic.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |uic| {
                uic.set_width(width);
                self.code_block(uic, face, busy, message);
            },
        );

        // …and, separately, the person's own way forward.
        let ask_shown = face.ask != AskState::Hidden;
        let snooze_shown = face.snooze != Snooze::Hidden;
        if ask_shown || snooze_shown {
            let code_shown = face.code != CodeState::Unavailable;
            if code_shown {
                or_rule(uic, width);
            } else {
                uic.add_space(20.0);
            }
        }
        match face.ask {
            AskState::Ready => {
                if Button::new("Ask for more time", Kind::Primary)
                    .icon(Icon::Ask)
                    .size(Size::Lg)
                    .enabled(!busy)
                    .show(uic)
                    .clicked()
                {
                    let _ = self.tx.send(Request::Ask);
                }
            }
            AskState::Sent => {
                Button::new("Asked — a parent will see it", Kind::Secondary)
                    .icon(Icon::Check)
                    .size(Size::Lg)
                    .enabled(false)
                    .show(uic);
            }
            AskState::Hidden => {}
        }
        self.snooze(uic, &face.snooze, face_at, busy);
    }

    /// Label, field + Unlock, hint (board `.code`).
    fn code_block(
        &mut self,
        uic: &mut egui::Ui,
        face: &Face,
        busy: bool,
        message: &Option<Outcome>,
    ) {
        match face.code {
            CodeState::Unavailable => return,
            CodeState::Wait { secs } => {
                ui::text(uic, "Unlock code", 13.0, W::SemiBold, ui::INK_2);
                uic.add_space(6.0);
                ui::text(
                    uic,
                    &format!("Too many tries — wait {} to try again.", human_secs(secs)),
                    15.0,
                    W::Medium,
                    ui::INK_2,
                );
                return;
            }
            CodeState::Ready { .. } => {}
        }
        ui::text(uic, "Unlock code", 13.0, W::SemiBold, ui::INK_2);
        uic.add_space(6.0);
        let mut submit = false;
        uic.horizontal(|uic| {
            uic.spacing_mut().item_spacing.x = 10.0;
            let unlock_w = 96.0;
            let field_w = uic.available_width() - unlock_w - 10.0;
            submit |= self.code_field(uic, field_w, busy);
            submit |= Button::new("Unlock", Kind::Secondary)
                .width(unlock_w)
                .enabled(!busy && !self.typed.is_empty())
                .show(uic)
                .clicked();
        });
        if submit && !busy && !self.typed.is_empty() {
            let code = std::mem::take(&mut self.typed);
            let _ = self.tx.send(Request::Code { code });
        }
        uic.add_space(6.0);
        let (line, color) = match (busy, message) {
            (true, _) => ("Checking…".to_string(), ui::INK_3),
            (false, Some(m)) if !m.ok => {
                let tries = match face.code {
                    CodeState::Ready { tries_left } if tries_left <= 3 => format!(
                        " {tries_left} {} left.",
                        if tries_left == 1 { "try" } else { "tries" }
                    ),
                    _ => String::new(),
                };
                (format!("{}{tries}", m.message), ui::STOP)
            }
            (false, Some(m)) => (m.message.clone(), ui::BRAND_INK),
            (false, None) => (face.code_hint.clone(), ui::INK_3),
        };
        uic.add(
            egui::Label::new(
                egui::RichText::new(line)
                    .font(ui::font(12.5, W::Regular))
                    .color(col(color)),
            )
            .wrap(),
        );
    }

    /// Digits only, grouped as printed, focused from the first frame, Enter
    /// submits. It keeps the keyboard: focus is taken back every frame.
    /// Returns whether Enter was pressed.
    fn code_field(&mut self, uic: &mut egui::Ui, width: f32, busy: bool) -> bool {
        let h = 44.0;
        let (rect, _) = uic.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
        let id = egui::Id::new("ost-unlock-code");
        let focused = uic.memory(|m| m.has_focus(id));
        {
            // Field (board `.field.focus`): white, a green edge and a soft glow.
            let p = uic.painter();
            if focused {
                p.rect_filled(
                    rect.expand(3.0),
                    egui::Rounding::same(13.0),
                    egui::Color32::from_rgba_unmultiplied(46, 125, 70, 56),
                );
            }
            p.rect(
                rect,
                egui::Rounding::same(10.0),
                col(ui::SURFACE),
                egui::Stroke::new(1.0, col(if focused { ui::BRAND } else { ui::LINE_2 })),
            );
        }
        let mut buf = group(&self.typed);
        let mut layouter = |ui: &egui::Ui, text: &str, _wrap: f32| {
            ui.fonts(|f| f.layout_job(ui::job(text, ui::mono(20.0), col(ui::INK), 0.06)))
        };
        let inner = rect.shrink2(egui::vec2(14.0, 0.0));
        // A child Ui inside the field: the row's own layout (field, gap,
        // Unlock) stays as allocated.
        let mut field = uic.child_ui(
            inner,
            egui::Layout::left_to_right(egui::Align::Center),
            None,
        );
        let out = field.add(
            egui::TextEdit::singleline(&mut buf)
                .id(id)
                .frame(false)
                .desired_width(inner.width())
                .vertical_align(egui::Align::Center)
                .layouter(&mut layouter)
                .interactive(!busy),
        );
        if out.changed() {
            self.typed = buf.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
            let shown = group(&self.typed);
            if let Some(mut st) = egui::TextEdit::load_state(uic.ctx(), out.id) {
                let end = egui::text::CCursor::new(shown.chars().count());
                st.cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(end)));
                st.store(uic.ctx(), out.id);
            }
        }
        if !focused && !busy {
            out.request_focus();
        }
        focused && uic.input(|i| i.key_pressed(egui::Key::Enter))
    }

    /// The self-set escape hatch (someone who set their own limits).
    fn snooze(&mut self, uic: &mut egui::Ui, s: &Snooze, face_at: Option<Instant>, busy: bool) {
        match s {
            Snooze::Hidden => {}
            Snooze::Wait { secs, opens_at_ms } => {
                let left = match opens_at_ms {
                    Some(_) => s.wait_left(super::now_ms()).unwrap_or(0),
                    None => {
                        let gone = face_at.map_or(0, |t| t.elapsed().as_secs());
                        secs.saturating_sub(gone)
                    }
                };
                if left == 0 {
                    // The agent agrees now; its next face will say so.
                    if Button::new("Give me 15 more minutes", Kind::Secondary)
                        .size(Size::Lg)
                        .enabled(!busy)
                        .show(uic)
                        .clicked()
                    {
                        let _ = self.tx.send(Request::Snooze);
                    }
                    uic.ctx().request_repaint_after(Duration::from_millis(250));
                    return;
                }
                Button::new("Give me 15 more minutes", Kind::Secondary)
                    .size(Size::Lg)
                    .enabled(false)
                    .show(uic);
                uic.add_space(8.0);
                ui::text(
                    uic,
                    &format!("You can use it in {}:{:02}.", left / 60, left % 60),
                    13.0,
                    W::Regular,
                    ui::INK_3,
                );
                uic.ctx().request_repaint_after(Duration::from_millis(250));
            }
            Snooze::Ready { left } => {
                if Button::new("Give me 15 more minutes", Kind::Secondary)
                    .size(Size::Lg)
                    .enabled(!busy)
                    .show(uic)
                    .clicked()
                {
                    let _ = self.tx.send(Request::Snooze);
                }
                uic.add_space(8.0);
                let more = match left {
                    0 => "The last one today.".to_string(),
                    1 => "One more after this today.".to_string(),
                    n => format!("{n} more after this today."),
                };
                ui::text(uic, &more, 13.0, W::Regular, ui::INK_3);
            }
            Snooze::UsedUp { back } => {
                let line = match back {
                    Some(b) => format!("That's today's extra time — back {b}."),
                    None => "That's today's extra time.".to_string(),
                };
                ui::text(uic, &line, 15.0, W::Medium, ui::INK_2);
            }
        }
    }
}

/// The "or" between the code and the person's own way (board `.or`).
fn or_rule(uic: &mut egui::Ui, width: f32) {
    uic.add_space(18.0);
    let (rect, _) = uic.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::hover());
    let p = uic.painter();
    let g = p.layout_job(ui::job(
        "or",
        ui::font(12.5, W::Regular),
        col(ui::INK_3),
        0.0,
    ));
    let (cx, cy) = (rect.center().x, rect.center().y);
    let half = g.size().x / 2.0 + 12.0;
    let line = egui::Stroke::new(1.0, col(ui::LINE_2));
    p.line_segment(
        [egui::pos2(rect.left(), cy), egui::pos2(cx - half, cy)],
        line,
    );
    p.line_segment(
        [egui::pos2(cx + half, cy), egui::pos2(rect.right(), cy)],
        line,
    );
    p.galley(
        egui::pos2(cx - g.size().x / 2.0, cy - g.size().y / 2.0),
        g,
        col(ui::INK_3),
    );
    uic.add_space(14.0);
}

fn human_secs(s: u64) -> String {
    if s >= 90 {
        format!("{} minutes", s.div_ceil(60))
    } else {
        format!("{s} seconds")
    }
}
