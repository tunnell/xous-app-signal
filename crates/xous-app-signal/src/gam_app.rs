//! GAM-rendered xas app: full MVP UI for link / receive / send.
//!
//! State machine:
//!
//! - **Menu**: pre-link shows Link / About / Quit; post-link
//!   becomes Inbox / Send / About / Quit. The cursor is on the
//!   first item by default; Up/Down navigates; Enter (or `'∴'`)
//!   selects.
//!
//! - **About**: hardcoded version string, author handle, credits.
//!   Enter returns to Menu.
//!
//! - **Linking**: transient screen shown after Link is selected
//!   while we wait for the worker's `Event::LinkUrl`. The QR
//!   modal opens on top of this screen; once user presses any key
//!   on the modal, we wait for `Event::LinkComplete` /
//!   `Event::LinkError` and transition to `Linked { Success } →
//!   Inbox` or `Linked { Failure }`.
//!
//! - **Linked { Success }**: brief confirmation. Auto-fires
//!   `Cmd::StartReceive` and transitions to Inbox.
//!
//! - **Linked { Failure }**: error message. Enter returns to Menu.
//!
//! - **Inbox**: list of received messages (sender + body +
//!   timestamp). Enter returns to Menu.
//!
//! - **SendResult { ok, status }**: outcome of a `Cmd::SendMessage`
//!   round-trip. Enter returns to Menu.
//!
//! Worker integration: events from the bridge worker (`event_rx`)
//! reach the GAM main loop via a forwarder thread that pushes
//! into a shared `Mutex<VecDeque<Event>>` and wakes us via a
//! `XasOp::WorkerEvent` scalar IPC to our own SID.

use core::fmt::Write;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_channel::{Receiver, Sender};
use blitstr2::GlyphStyle;
use num_traits::*;
use uuid::Uuid;
use ux_api::minigfx::*;
use ux_api::service::api::Gid;
use xous::{CID, Message};
use xous_signal_bridge::{Cmd, Event};

use crate::dialogue::{DialogueSummary, SendStatus, ThreadMessage, rebuild_summaries};

const SERVER_NAME_XAS: &str = "_xas_";

/// How many recent messages we keep in the Inbox view. MVP bound
/// — keeps render cheap and avoids any thread / sync work for
/// scrolling.
const INBOX_CAPACITY: usize = 5;

#[derive(Debug, num_derive::FromPrimitive, num_derive::ToPrimitive)]
enum XasOp {
    Redraw = 0,
    Rawkeys = 1,
    FocusChange = 2,
    /// Forwarder thread woke us; drain the pending-events deque.
    WorkerEvent = 3,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Screen {
    Menu,
    About,
    Linking,
    Linked { kind: LinkedKind },
    Inbox,
    /// Conversation list — Phase A scaffolding. Renders one row per
    /// UUID from `App.dialogues`, with `App.home_focus` driving the
    /// `>` cursor. Phase A keeps the existing menu intact and routes
    /// here only via `MenuItem::Home`; later phases replace the menu
    /// landing with this screen.
    Home,
    /// Per-conversation history view, read-only in Phase A. Shows
    /// the messages from `App.messages` filtered by `uuid`, oldest
    /// at top. A later step adds a compose input at the bottom.
    Thread { uuid: Uuid },
    SendResult { ok: bool, status: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkedKind {
    Success,
    Failure,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuItem {
    // Pre-link items
    Link,
    // Post-link items
    Inbox,
    Send,
    /// Conversation-list view (Phase A scaffolding). Sits alongside
    /// Inbox/Send for now; later phases promote it to the default
    /// post-link landing screen and drop Inbox/Send.
    Home,
    // Always
    About,
    Quit,
}

struct App {
    gam: gam::Gam,
    content: Gid,
    bounds: Point,
    screen: Screen,
    selected: MenuItem,
    /// Pre-link → false; post-Link `Cmd::LinkDevice` success → true.
    /// Drives which menu items are visible + selectable.
    linked: bool,
    /// All inbound + outbound messages held in RAM for the lifetime
    /// of this app run. Capped at `INBOX_CAPACITY` (oldest dropped
    /// when full). Order is insertion-order (push_back); render
    /// passes filter / sort / aggregate as needed.
    messages: Vec<ThreadMessage>,
    /// Aggregated per-conversation view derived from `messages`.
    /// Re-built on every message arrival; rendered by `write_home`.
    dialogues: Vec<DialogueSummary>,
    /// Index of the focused row when Screen::Home is active.
    home_focus: usize,
    /// Active compose buffer when Screen::Thread is on top. Cleared
    /// when the user sends or backs out. Phase A allows alphanumeric
    /// + space + ASCII punctuation; non-ASCII (emoji, accented chars)
    /// is silently dropped — Phase B widens this to anything the
    /// GAM font can render.
    compose_buffer: String,
    /// Most-recent inbound sender (used as default recipient when
    /// the user picks Send before any contact is known).
    last_sender: Option<String>,
    /// One-line text rendered on transient screens (Linked,
    /// SendResult). Cleared on transition to Menu.
    last_status: String,
    quit_requested: bool,
    /// True between Cmd::LinkDevice send and Event::Link{Complete,Error}.
    /// While set, handle_worker_event opens the QR modal on LinkUrl
    /// and transitions on LinkComplete/LinkError. Cleared on
    /// terminal events.
    linking_in_progress: bool,
}

impl App {
    fn menu_items(&self) -> [Option<MenuItem>; 5] {
        if self.linked {
            [
                Some(MenuItem::Home),
                Some(MenuItem::Inbox),
                Some(MenuItem::Send),
                Some(MenuItem::About),
                Some(MenuItem::Quit),
            ]
        } else {
            [
                Some(MenuItem::Link),
                Some(MenuItem::About),
                Some(MenuItem::Quit),
                None,
                None,
            ]
        }
    }

    /// Move the cursor by `delta` (+1 = down, -1 = up) wrapping
    /// around the visible items.
    fn move_cursor(&mut self, delta: isize) {
        let items = self.menu_items();
        let visible: Vec<MenuItem> = items.iter().flatten().copied().collect();
        if visible.is_empty() {
            return;
        }
        let idx = visible.iter().position(|m| *m == self.selected).unwrap_or(0);
        let len = visible.len() as isize;
        let new = (idx as isize + delta).rem_euclid(len);
        self.selected = visible[new as usize];
    }

    fn render(&self) -> Result<(), String> {
        self.gam
            .draw_rectangle(
                self.content,
                Rectangle::new_with_style(
                    Point::new(0, 0),
                    self.bounds,
                    DrawStyle {
                        fill_color: Some(PixelColor::Light),
                        stroke_color: None,
                        stroke_width: 0,
                    },
                ),
            )
            .map_err(|e| format!("draw_rectangle: {:?}", e))?;

        let mut tv = TextView::new(
            self.content,
            TextBounds::BoundingBox(Rectangle::new(
                Point::new(8, 8),
                Point::new(self.bounds.x - 8, self.bounds.y - 8),
            )),
        );
        tv.border_width = 1;
        tv.draw_border = true;
        tv.clear_area = true;
        tv.rounded_border = Some(3);
        tv.style = GlyphStyle::Regular;

        match &self.screen {
            Screen::Menu => self.write_menu(&mut tv.text)?,
            Screen::About => write!(
                tv.text,
                "About xas\n\n\
                 Unofficial Signal client\n\
                 for Xous on Precursor.\n\n\
                 Version: {}\n\
                 Author:  @tunnell\n\n\
                 Built on:\n\
                  - presage v0.8.0-dev\n\
                  - libsignal-service-rs\n\
                  - libsignal v0.91.0\n\n\
                 Press Enter to return.",
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(|e| format!("write About: {}", e))?,
            Screen::Linking => write!(
                tv.text,
                "Linking device...\n\n\
                 Connecting to Signal\n\
                 servers and requesting\n\
                 a provisioning URL.\n\n\
                 Cert is verified against\n\
                 Signal's pinned CA.\n\n\
                 (Please wait.)"
            )
            .map_err(|e| format!("write Linking: {}", e))?,
            Screen::Linked { kind } => {
                let title = match kind {
                    LinkedKind::Success => "Link succeeded",
                    LinkedKind::Failure => "Link failed",
                };
                write!(
                    tv.text,
                    "{}\n\n{}\n\nPress Enter to continue.",
                    title, self.last_status
                )
                .map_err(|e| format!("write Linked: {}", e))?
            }
            Screen::Inbox => self.write_inbox(&mut tv.text)?,
            Screen::Home => self.write_home(&mut tv.text)?,
            Screen::Thread { uuid } => self.write_thread(&mut tv.text, uuid)?,
            Screen::SendResult { ok, status } => {
                let title = if *ok { "Sent" } else { "Send failed" };
                write!(
                    tv.text,
                    "{}\n\n{}\n\nPress Enter to return.",
                    title, status
                )
                .map_err(|e| format!("write SendResult: {}", e))?
            }
        }

        self.gam
            .post_textview(&mut tv)
            .map_err(|e| format!("post_textview: {:?}", e))?;
        self.gam.redraw().map_err(|e| format!("redraw: {:?}", e))?;
        Ok(())
    }

    fn write_menu(&self, out: &mut String) -> Result<(), String> {
        let header = if self.linked { "Signal — linked" } else { "xas — Signal client" };
        write!(out, "{}\n\n", header).map_err(|e| format!("hdr: {}", e))?;
        for maybe in self.menu_items() {
            if let Some(item) = maybe {
                let mark = if item == self.selected { ">" } else { " " };
                let label = match item {
                    MenuItem::Link => "Link device",
                    MenuItem::Home => "Home",
                    MenuItem::Inbox => "Inbox",
                    MenuItem::Send => "Send message",
                    MenuItem::About => "About",
                    MenuItem::Quit => "Quit",
                };
                write!(out, "{} {}\n", mark, label).map_err(|e| format!("item: {}", e))?;
            }
        }
        write!(out, "\nUp/Down: navigate\nEnter: select")
            .map_err(|e| format!("foot: {}", e))
    }

    /// Render the conversation-list screen.
    ///
    /// Layout per row (single-TextView text mode for Phase A):
    /// ```
    ///   Bob Kowalski                    12m
    /// * did you get the file?           (1)
    /// ───────────────────────────────────────
    /// > Carol                            1h
    ///   Thanks!
    /// ```
    /// `>` marks the focused row, `*` marks unread (Phase A doesn't
    /// have per-row bold; that comes with multi-TextView render in a
    /// later phase). Right-aligned timestamp, trailing unread count
    /// in `(N)` form, second-line preview with optional outgoing
    /// status glyph.
    fn write_home(&self, out: &mut String) -> Result<(), String> {
        let total_unread: u32 = self.dialogues.iter().map(|d| d.unread_count).sum();
        if total_unread > 0 {
            writeln!(out, "xas                       {} unread", total_unread)
                .map_err(|e| format!("home hdr: {}", e))?;
        } else {
            writeln!(out, "xas").map_err(|e| format!("home hdr: {}", e))?;
        }
        writeln!(out, "{}", "-".repeat(37)).map_err(|e| format!("home rule: {}", e))?;

        if self.dialogues.is_empty() {
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "      No conversations yet.")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  Wait for someone to message,")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  or use Send from the menu.")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  Enter: return to menu")
                .map_err(|e| format!("home empty: {}", e))?;
            return Ok(());
        }

        let now_ms = unix_now_ms();
        // Clamp focus index in case the list shrank.
        let focus = self.home_focus.min(self.dialogues.len().saturating_sub(1));

        for (i, d) in self.dialogues.iter().enumerate() {
            let focus_marker = if i == focus { '>' } else { ' ' };
            let unread_marker = if d.unread_count > 0 { '*' } else { ' ' };
            let timestamp = crate::dialogue::brief_relative(d.last_msg_ts, now_ms);

            let name_short = crate::dialogue::ellipsize(&d.display_name, 24);
            // First line: focus, name, right-padded timestamp.
            writeln!(out, "{} {:<24} {:>6}", focus_marker, name_short, timestamp)
                .map_err(|e| format!("home row name: {}", e))?;

            // Second line: unread marker, status glyph, snippet, count.
            let status_glyph = match (d.last_msg_outgoing, d.last_msg_status) {
                (true, SendStatus::Pending) => "..",
                (true, SendStatus::Sent) => "v ",
                (true, SendStatus::Delivered) => "vv",
                (true, SendStatus::Failed) => "! ",
                _ => "  ",
            };
            let snippet_short = crate::dialogue::ellipsize(&d.last_msg_snippet, 26);
            let badge = if d.unread_count > 0 {
                let n = d.unread_count.min(99);
                format!("({})", n)
            } else {
                String::new()
            };
            writeln!(
                out,
                "{} {} {:<26} {:>4}",
                unread_marker, status_glyph, snippet_short, badge
            )
            .map_err(|e| format!("home row body: {}", e))?;

            writeln!(out, "{}", "-".repeat(37))
                .map_err(|e| format!("home sep: {}", e))?;
        }
        writeln!(out).map_err(|e| format!("home foot: {}", e))?;
        write!(out, "  ↑↓ select   Enter open").map_err(|e| format!("home hint: {}", e))
    }

    /// Render the per-conversation history view (read-only in Phase A).
    ///
    /// Layout:
    /// ```
    /// Bob Kowalski
    /// -------------------------------------
    /// Bob 12m
    ///   did you get the file?
    ///
    /// You 11m  vv
    ///   yes, on my way
    ///
    /// Bob 5m
    ///   thanks!
    /// -------------------------------------
    ///   Enter: back to Home
    /// ```
    /// Messages oldest at top, newest just above the footer (mirrors
    /// every chat UI). Outgoing messages prefixed with their send-
    /// status glyph; incoming messages have no prefix.
    fn write_thread(&self, out: &mut String, uuid: &Uuid) -> Result<(), String> {
        let header = self
            .dialogues
            .iter()
            .find(|d| d.uuid == *uuid)
            .map(|d| d.display_name.clone())
            .unwrap_or_else(|| format!("uuid:{:.8}", uuid.simple().to_string()));
        writeln!(out, "{}", header).map_err(|e| format!("thread hdr: {}", e))?;
        writeln!(out, "{}", "-".repeat(37)).map_err(|e| format!("thread rule: {}", e))?;

        let now_ms = unix_now_ms();
        let thread_msgs: Vec<&ThreadMessage> =
            self.messages.iter().filter(|m| m.uuid == *uuid).collect();

        if thread_msgs.is_empty() {
            writeln!(out).map_err(|e| format!("thread empty: {}", e))?;
            writeln!(out, "  (no messages in this thread)")
                .map_err(|e| format!("thread empty: {}", e))?;
        } else {
            for m in &thread_msgs {
                let ts = crate::dialogue::brief_relative(m.timestamp, now_ms);
                let header_line = if m.outgoing {
                    let glyph = match m.status {
                        SendStatus::Pending => "..",
                        SendStatus::Sent => "v ",
                        SendStatus::Delivered => "vv",
                        SendStatus::Failed => "! ",
                    };
                    format!("You {}  {}", ts, glyph)
                } else {
                    format!("{} {}", crate::dialogue::ellipsize(&m.author_label, 22), ts)
                };
                writeln!(out, "{}", header_line)
                    .map_err(|e| format!("thread row hdr: {}", e))?;
                writeln!(out, "  {}", crate::dialogue::ellipsize(&m.body, 70))
                    .map_err(|e| format!("thread row body: {}", e))?;
                writeln!(out).map_err(|e| format!("thread row sep: {}", e))?;
            }
        }
        writeln!(out, "{}", "-".repeat(37)).map_err(|e| format!("thread foot rule: {}", e))?;
        // Compose input. Cursor is `_` at the end of the buffer for
        // Phase A — no horizontal scroll if the buffer is wider than
        // the visible width, just shows the trailing chars.
        write!(out, "> {}_", crate::dialogue::ellipsize(&self.compose_buffer, 30))
            .map_err(|e| format!("thread compose: {}", e))?;
        Ok(())
    }

    fn write_inbox(&self, out: &mut String) -> Result<(), String> {
        if self.messages.is_empty() {
            write!(out, "Inbox\n\n(no messages yet)\n\nWaiting for inbound\nmessages...\n\nEnter: return")
                .map_err(|e| format!("inbox empty: {}", e))?;
            return Ok(());
        }
        write!(out, "Inbox ({})\n\n", self.messages.len())
            .map_err(|e| format!("inbox hdr: {}", e))?;
        // Render newest first; truncate sender + body so the screen
        // doesn't overflow. `messages` is push-back-ordered so we
        // iterate in reverse to surface the latest at the top.
        for msg in self.messages.iter().rev() {
            let sender_short = truncate(&msg.author_label, 22);
            let body_short = truncate(&msg.body, 80);
            write!(out, "from: {}\n  {}\n\n", sender_short, body_short)
                .map_err(|e| format!("inbox msg: {}", e))?;
        }
        write!(out, "\nEnter: return").map_err(|e| format!("inbox foot: {}", e))
    }
}

/// Whether a character is acceptable in the Thread compose buffer.
/// Phase A: alphanumeric + space + ASCII punctuation. Non-ASCII
/// (emoji, accented chars, CJK) is silently dropped; Phase B
/// widens to anything the GAM font can render.
fn is_compose_char(c: char) -> bool {
    c.is_alphanumeric() || c == ' ' || c.is_ascii_punctuation()
}

/// Unix milliseconds, or 0 if the system clock is somehow before
/// the epoch. Used by the conversation-list renderer to compute
/// relative timestamps.
fn unix_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut r: String = s.chars().take(max.saturating_sub(1)).collect();
        r.push('…');
        r
    }
}

pub fn run(cmd_tx: Sender<Cmd>, event_rx: Receiver<Event>) -> Result<(), String> {
    log::info!("xas/gam_app: starting GAM-rendered loop");

    let xns = xous_names::XousNames::new().map_err(|e| format!("XousNames::new: {:?}", e))?;
    let sid = xns
        .register_name(SERVER_NAME_XAS, None)
        .map_err(|e| format!("register_name: {:?}", e))?;

    let gam = gam::Gam::new(&xns).map_err(|e| format!("Gam::new: {:?}", e))?;
    let token = gam
        .register_ux(gam::UxRegistration {
            app_name: String::from(gam::APP_NAME_XAS),
            ux_type: gam::UxType::Chat,
            predictor: None,
            listener: sid.to_array(),
            redraw_id: XasOp::Redraw.to_u32().unwrap(),
            gotinput_id: None,
            audioframe_id: None,
            rawkeys_id: Some(XasOp::Rawkeys.to_u32().unwrap()),
            focuschange_id: Some(XasOp::FocusChange.to_u32().unwrap()),
        })
        .map_err(|e| format!("register_ux: {:?}", e))?
        .ok_or_else(|| "register_ux returned None token".to_string())?;
    log::info!("xas/gam_app: GAM token = {:x?}", token);

    let content = gam.request_content_canvas(token).map_err(|e| format!("canvas: {:?}", e))?;
    let bounds = gam.get_canvas_bounds(content).map_err(|e| format!("bounds: {:?}", e))?;
    log::info!("xas/gam_app: canvas {:?}, bounds {:?}", content, bounds);

    let _ = gam.allow_mainmenu();

    // === Worker-event forwarder ===
    //
    // gam_app's main loop blocks on `xous::receive_message(sid)`,
    // but worker events arrive on `event_rx`. A dedicated thread
    // bridges the two: it blocks on `event_rx.recv_blocking`,
    // pushes each event onto a shared deque, and pokes our SID via
    // a `XasOp::WorkerEvent` scalar so our main loop wakes and
    // drains the deque.
    let pending_events: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::new()));
    let self_cid: CID = xous::connect(sid).map_err(|e| format!("self-connect: {:?}", e))?;
    {
        let pending = pending_events.clone();
        let event_rx = event_rx.clone();
        std::thread::Builder::new()
            .name("xas-event-forwarder".into())
            .spawn(move || {
                while let Ok(event) = event_rx.recv_blocking() {
                    pending.lock().unwrap().push_back(event);
                    let _ = xous::send_message(
                        self_cid,
                        Message::new_scalar(
                            XasOp::WorkerEvent.to_usize().unwrap(),
                            0,
                            0,
                            0,
                            0,
                        ),
                    );
                }
                log::warn!("xas/gam_app: event forwarder exited (event_rx closed)");
            })
            .map_err(|e| format!("spawn forwarder: {}", e))?;
    }

    let modals_xns = xous_names::XousNames::new()
        .map_err(|e| format!("XousNames for modals: {:?}", e))?;

    let mut app = App {
        gam,
        content,
        bounds,
        screen: Screen::Menu,
        selected: MenuItem::Link,
        linked: false,
        messages: Vec::with_capacity(INBOX_CAPACITY),
        dialogues: Vec::new(),
        home_focus: 0,
        compose_buffer: String::new(),
        last_sender: None,
        last_status: String::new(),
        quit_requested: false,
        linking_in_progress: false,
    };
    app.render().ok();

    loop {
        let msg = xous::receive_message(sid).map_err(|e| format!("receive: {:?}", e))?;
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(XasOp::Redraw) => {
                app.render().ok();
            }
            Some(XasOp::Rawkeys) => {
                xous::msg_scalar_unpack!(msg, k1, k2, k3, k4, {
                    let keys = [
                        char::from_u32(k1 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k2 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k3 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k4 as u32).unwrap_or('\u{0}'),
                    ];
                    log::info!("xas/gam_app: keys {:?}", keys);
                    handle_keys(&mut app, keys, &cmd_tx, &event_rx, &modals_xns);
                });
                if app.quit_requested {
                    let _ = app.gam.switch_to_app(gam::APP_NAME_SHELLCHAT, token);
                    log::info!("xas/gam_app: hidden via Quit; staying alive");
                    app.screen = Screen::Menu;
                    app.selected = if app.linked { MenuItem::Inbox } else { MenuItem::Link };
                    app.last_status.clear();
                    app.quit_requested = false;
                }
            }
            Some(XasOp::FocusChange) => {
                xous::msg_scalar_unpack!(msg, new_state_code, _, _, _, {
                    let new_state = gam::FocusState::convert_focus_change(new_state_code);
                    log::info!("xas/gam_app: focus change -> {:?}", new_state);
                    if matches!(new_state, gam::FocusState::Foreground) {
                        if let Err(e) = app.render() {
                            log::warn!("xas/gam_app: render after focus: {}", e);
                        }
                    }
                });
            }
            Some(XasOp::WorkerEvent) => {
                let drained: Vec<Event> = {
                    let mut q = pending_events.lock().unwrap();
                    q.drain(..).collect()
                };
                for ev in drained {
                    handle_worker_event(&mut app, ev, &cmd_tx, &modals_xns);
                }
                if let Err(e) = app.render() {
                    log::warn!("xas/gam_app: render after WorkerEvent: {}", e);
                }
            }
            _ => {
                log::debug!("xas/gam_app: unknown msg id={}", msg.body.id());
            }
        }
    }
}

fn handle_keys(
    app: &mut App,
    keys: [char; 4],
    cmd_tx: &Sender<Cmd>,
    event_rx: &Receiver<Event>,
    modals_xns: &xous_names::XousNames,
) {
    for &k in keys.iter() {
        if k == '\u{0}' {
            continue;
        }
        match (&app.screen, k) {
            (Screen::Menu, '↑') => app.move_cursor(-1),
            (Screen::Menu, '↓') => app.move_cursor(1),
            (Screen::Menu, '∴') | (Screen::Menu, '\u{d}') => match app.selected {
                MenuItem::Link => drive_link(app, cmd_tx, event_rx, modals_xns),
                MenuItem::Home => {
                    app.home_focus = 0;
                    app.screen = Screen::Home;
                }
                MenuItem::Inbox => app.screen = Screen::Inbox,
                MenuItem::Send => drive_send(app, cmd_tx, modals_xns),
                MenuItem::About => app.screen = Screen::About,
                MenuItem::Quit => app.quit_requested = true,
            },
            (Screen::About, '∴') | (Screen::About, '\u{d}') => app.screen = Screen::Menu,
            (Screen::Linked { .. }, '∴') | (Screen::Linked { .. }, '\u{d}') => {
                app.screen = Screen::Menu;
                app.selected = if app.linked { MenuItem::Inbox } else { MenuItem::Link };
                app.last_status.clear();
            }
            (Screen::Inbox, '∴') | (Screen::Inbox, '\u{d}') => {
                app.screen = Screen::Menu;
            }
            (Screen::Home, '↑') => {
                app.home_focus = app.home_focus.saturating_sub(1);
            }
            (Screen::Home, '↓') => {
                let max = app.dialogues.len().saturating_sub(1);
                if app.home_focus < max {
                    app.home_focus += 1;
                }
            }
            (Screen::Home, '∴') | (Screen::Home, '\u{d}') => {
                // Open the focused thread. If the list is empty,
                // fall back to the menu.
                if let Some(d) = app.dialogues.get(app.home_focus) {
                    app.screen = Screen::Thread { uuid: d.uuid };
                } else {
                    app.screen = Screen::Menu;
                }
            }
            (Screen::Thread { uuid }, '∴') | (Screen::Thread { uuid }, '\u{d}') => {
                // Enter behavior depends on compose buffer state:
                // - empty → back to Home (BlackBerry/Nokia convention)
                // - non-empty → send the message
                if app.compose_buffer.is_empty() {
                    app.screen = Screen::Home;
                } else {
                    let body = std::mem::take(&mut app.compose_buffer);
                    let recipient_uuid = *uuid;
                    let send_ts = unix_now_ms();
                    let recipient_str = recipient_uuid.to_string();
                    log::info!(
                        "xas/gam_app: send to={} ({} body bytes) ts={}",
                        recipient_str,
                        body.len(),
                        send_ts,
                    );
                    // Optimistic-append the outgoing message so it
                    // shows up in the Thread render immediately;
                    // status updates from the worker arrive later.
                    if app.messages.len() >= INBOX_CAPACITY {
                        app.messages.remove(0);
                    }
                    app.messages.push(ThreadMessage {
                        uuid: recipient_uuid,
                        author_label: "You".to_string(),
                        body: body.clone(),
                        timestamp: send_ts,
                        outgoing: true,
                        status: SendStatus::Pending,
                    });
                    app.dialogues = rebuild_summaries(&app.messages);
                    if let Err(e) = cmd_tx.send_blocking(Cmd::SendMessage {
                        recipient: recipient_str,
                        body,
                        timestamp: send_ts,
                    }) {
                        log::warn!("xas/gam_app: Cmd::SendMessage send err: {:?}", e);
                        // Mark the optimistic row Failed in place.
                        if let Some(m) = app
                            .messages
                            .iter_mut()
                            .rev()
                            .find(|m| m.outgoing && m.timestamp == send_ts)
                        {
                            m.status = SendStatus::Failed;
                        }
                        app.dialogues = rebuild_summaries(&app.messages);
                    }
                }
            }
            (Screen::Thread { .. }, '\u{8}') => {
                // Backspace: pop a char from the compose buffer.
                app.compose_buffer.pop();
            }
            (Screen::Thread { .. }, c) if is_compose_char(c) => {
                app.compose_buffer.push(c);
            }
            (Screen::SendResult { .. }, '∴') | (Screen::SendResult { .. }, '\u{d}') => {
                app.screen = Screen::Menu;
                app.selected = MenuItem::Send;
            }
            // Linking is transient (waiting on worker); rawkeys are
            // ignored.
            _ => {}
        }
    }
    if let Err(e) = app.render() {
        log::warn!("render: {}", e);
    }
}

/// Process a worker event delivered via the forwarder thread.
/// Mutates `app` state; the WorkerEvent handler in `run()` does
/// the render after we return so multiple events batch into one
/// redraw. **The forwarder is the only consumer of `event_rx`.**
/// All Event variants — including Link{Url, Complete, Error} —
/// land here.
fn handle_worker_event(
    app: &mut App,
    event: Event,
    cmd_tx: &Sender<Cmd>,
    modals_xns: &xous_names::XousNames,
) {
    match event {
        Event::LinkUrl(url) => {
            log::info!("xas/gam_app: link URL = {}", url);
            // Open the QR modal. show_notification blocks until the
            // user dismisses it — meanwhile the worker keeps the
            // provisioning WS alive waiting for the encrypted
            // envelope. After the user scans + dismisses, we keep
            // looping; LinkComplete or LinkError will arrive next.
            if app.linking_in_progress {
                if let Ok(modals) = modals::Modals::new(modals_xns) {
                    let _ = modals.show_notification(
                        "Signal on phone.\n\
                         Scan QR, then press any key.\n\
                         Don't transfer old messages.",
                        Some(&url),
                    );
                }
            }
        }
        Event::LinkComplete { device_name, aci, phone } => {
            log::info!(
                "xas/gam_app: LinkComplete device={} aci={} phone={}",
                device_name, aci, phone
            );
            app.linked = true;
            app.linking_in_progress = false;
            app.screen = Screen::Linked { kind: LinkedKind::Success };
            app.last_status =
                format!("device: {}\naci:    {}\nphone:  {}", device_name, aci, phone);
            // Auto-fire StartReceive so the inbox begins
            // accumulating. Bridge dedupes; calling again later is
            // harmless.
            log::info!("xas/gam_app: sending Cmd::StartReceive");
            match cmd_tx.send_blocking(Cmd::StartReceive) {
                Ok(()) => log::info!("xas/gam_app: Cmd::StartReceive sent ok"),
                Err(e) => log::warn!("xas/gam_app: Cmd::StartReceive send err: {:?}", e),
            }
        }
        Event::LinkError(msg) => {
            log::warn!("xas/gam_app: LinkError: {}", msg);
            app.linking_in_progress = false;
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = msg;
        }
        Event::Message { sender, sender_phone, sender_name, body, timestamp } => {
            // Pretty label preference: name → phone → UUID. The
            // contacts store typically has both name and phone for
            // peers who've been synced from the linked phone; only
            // first-sight peers fall through to UUID.
            let author_label = sender_name
                .clone()
                .or_else(|| sender_phone.clone())
                .unwrap_or_else(|| sender.clone());
            log::info!(
                "xas/gam_app: inbound message from {} ({} bytes)",
                author_label,
                body.len()
            );
            // ACI from the worker is a canonical UUID string.
            // Fall back to the nil UUID if parse fails (defensive
            // — practically shouldn't happen since the worker only
            // surfaces senders it recognized).
            let uuid = Uuid::parse_str(&sender).unwrap_or_else(|_| {
                log::warn!("xas/gam_app: sender {:?} doesn't parse as UUID; using nil", sender);
                Uuid::nil()
            });
            app.last_sender = Some(sender);
            // Drop oldest first if we've hit the cap, then append.
            if app.messages.len() >= INBOX_CAPACITY {
                app.messages.remove(0);
            }
            app.messages.push(ThreadMessage {
                uuid,
                author_label,
                body,
                timestamp,
                outgoing: false,
                status: SendStatus::Sent,
            });
            app.dialogues = rebuild_summaries(&app.messages);
        }
        Event::ReceiveStarted => {
            log::info!("xas/gam_app: receive loop established");
        }
        Event::ReceiveError(msg) => {
            log::warn!("xas/gam_app: receive error: {}", msg);
            app.last_status = format!("Receive: {}", msg);
        }
        Event::SendComplete { timestamp } => {
            // If the send originated from a Thread compose, there's a
            // pending optimistic-rendered message in `messages` with
            // this timestamp. Update its status in place; rebuild
            // dialogues so any cached snippet/status reflects the new
            // state. No screen change — the Thread is already showing
            // the message.
            //
            // Otherwise (no match), the send was menu-initiated via
            // `drive_send`; show the legacy SendResult screen.
            let matched = app
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.outgoing && m.timestamp == timestamp);
            if let Some(m) = matched {
                m.status = SendStatus::Delivered;
                app.dialogues = rebuild_summaries(&app.messages);
            } else {
                app.screen = Screen::SendResult {
                    ok: true,
                    status: format!("server timestamp = {}", timestamp),
                };
            }
        }
        Event::SendError { reason, timestamp } => {
            // Same pattern as SendComplete: if a matching pending
            // Thread message exists, mark it Failed in place;
            // otherwise treat as a menu-Send result and pop the
            // SendResult screen.
            let matched = timestamp.and_then(|ts| {
                app.messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.outgoing && m.timestamp == ts)
            });
            if let Some(m) = matched {
                m.status = SendStatus::Failed;
                app.dialogues = rebuild_summaries(&app.messages);
                app.last_status = format!("Send: {}", reason);
            } else {
                app.screen = Screen::SendResult { ok: false, status: reason };
            }
        }
        Event::ShuttingDown => {
            log::info!("xas/gam_app: worker is shutting down");
            app.last_status = "worker shutdown".to_string();
        }
        Event::Pong | Event::Whoami(_) => {}
    }
}

/// Hosted-mode helper: read `~/precursor-signal/.link_attempts`,
/// increment it, return the device name to default to. Each link
/// attempt gets a fresh `xasN` so the user can correlate this run's
/// QR with the entry that lands in their phone's Linked Devices list.
///
/// File semantics: missing → create with `0`, use `0`, write `1`.
/// Existing → read N, use N, write N+1.
///
/// Hosted-only: on rv32 there's no general filesystem, so this
/// quietly falls back to "xas".
#[cfg(not(target_os = "xous"))]
fn next_attempt_device_name() -> String {
    use std::path::Path;
    let path = Path::new("/home/tunnell/precursor-signal/.link_attempts");
    let n: u32 = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = std::fs::write(path, format!("{}", n + 1));
    format!("xas{}", n)
}

#[cfg(target_os = "xous")]
fn next_attempt_device_name() -> String {
    "xas".to_string()
}

/// Kick off the link flow. Synchronous part only: prompts for a
/// device name, sends `Cmd::LinkDevice`, sets the Linking screen,
/// and returns. All async results (`LinkUrl`, `LinkComplete`,
/// `LinkError`) flow through the forwarder thread and land in
/// `handle_worker_event`.
///
/// The forwarder is the *only* consumer of `event_rx` — drive_link
/// no longer races for events directly. (Earlier bug: both
/// drive_link and the forwarder called `event_rx.recv_blocking`,
/// so individual events were delivered to one or the other
/// non-deterministically. The QR modal sometimes never opened
/// because Event::LinkUrl was grabbed by the forwarder before
/// drive_link saw it.)
fn drive_link(
    app: &mut App,
    cmd_tx: &Sender<Cmd>,
    _event_rx: &Receiver<Event>,
    modals_xns: &xous_names::XousNames,
) {
    let modals = match modals::Modals::new(modals_xns) {
        Ok(m) => m,
        Err(e) => {
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = format!("Modals init failed:\n{:?}", e);
            return;
        }
    };

    let default_name = next_attempt_device_name();
    let device_name = match modals
        .alert_builder("Device name?")
        .field(Some(default_name.clone()), None)
        .build()
    {
        Ok(payloads) => {
            let trimmed = payloads.first().as_str().trim().to_string();
            if trimmed.is_empty() { default_name } else { trimmed }
        }
        Err(e) => {
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = format!("device name modal:\n{:?}", e);
            return;
        }
    };

    app.screen = Screen::Linking;
    app.linking_in_progress = true;
    app.render().ok();

    if let Err(e) = cmd_tx.send_blocking(Cmd::LinkDevice { device_name }) {
        app.screen = Screen::Linked { kind: LinkedKind::Failure };
        app.last_status = format!("Cmd::LinkDevice send:\n{:?}", e);
        app.linking_in_progress = false;
        return;
    }
    // Return now; events arrive via the forwarder.
}

/// Drive the send flow. Two TextEntry modals (recipient + body)
/// then a `Cmd::SendMessage` whose result lands as a worker event
/// processed by `handle_worker_event` (which sets the
/// `Screen::SendResult` view).
fn drive_send(app: &mut App, cmd_tx: &Sender<Cmd>, modals_xns: &xous_names::XousNames) {
    let modals = match modals::Modals::new(modals_xns) {
        Ok(m) => m,
        Err(e) => {
            app.screen = Screen::SendResult {
                ok: false,
                status: format!("modals init: {:?}", e),
            };
            return;
        }
    };

    // Step 1: recipient. Default to last received sender if any.
    let default_recipient = app.last_sender.clone().unwrap_or_default();
    let recipient = match modals
        .alert_builder("Recipient (ACI/phone):")
        .field(Some(default_recipient), None)
        .build()
    {
        Ok(payloads) => payloads.first().as_str().trim().to_string(),
        Err(e) => {
            app.screen = Screen::SendResult {
                ok: false,
                status: format!("recipient modal: {:?}", e),
            };
            return;
        }
    };
    if recipient.is_empty() {
        app.screen = Screen::SendResult {
            ok: false,
            status: "no recipient".to_string(),
        };
        return;
    }

    // Step 2: body.
    let body = match modals
        .alert_builder("Message body:")
        .field(Some(String::new()), None)
        .build()
    {
        Ok(payloads) => payloads.first().as_str().to_string(),
        Err(e) => {
            app.screen = Screen::SendResult {
                ok: false,
                status: format!("body modal: {:?}", e),
            };
            return;
        }
    };
    if body.is_empty() {
        app.screen = Screen::SendResult {
            ok: false,
            status: "empty body".to_string(),
        };
        return;
    }

    log::info!(
        "xas/gam_app: send to={} ({} body bytes)",
        recipient,
        body.len()
    );
    let send_ts = unix_now_ms();
    if let Err(e) =
        cmd_tx.send_blocking(Cmd::SendMessage { recipient, body, timestamp: send_ts })
    {
        app.screen = Screen::SendResult {
            ok: false,
            status: format!("Cmd::SendMessage send: {:?}", e),
        };
        return;
    }

    // The result comes back asynchronously via the forwarder
    // thread → WorkerEvent → handle_worker_event → SendResult.
    // Stay on Menu until that fires; render a placeholder.
    app.screen = Screen::SendResult {
        ok: true,
        status: "sending...".to_string(),
    };
}
