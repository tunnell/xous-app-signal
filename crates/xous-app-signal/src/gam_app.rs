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
use ux_api::minigfx::*;
use ux_api::service::api::Gid;
use xous::{CID, Message};
use xous_signal_bridge::{Cmd, Event};

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
    // Always
    About,
    Quit,
}

#[derive(Clone, Debug)]
struct InboxMessage {
    sender: String,
    body: String,
    /// Server-side timestamp (millis since epoch). Captured for
    /// future ordering / dedup; not rendered in the MVP inbox
    /// (which shows order-of-arrival via deque position).
    #[allow(dead_code)]
    timestamp: u64,
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
    /// Recent messages, newest first. Capped at `INBOX_CAPACITY`.
    messages: VecDeque<InboxMessage>,
    /// Most-recent inbound sender (used as default recipient when
    /// the user picks Send before any contact is known).
    last_sender: Option<String>,
    /// One-line text rendered on transient screens (Linked,
    /// SendResult). Cleared on transition to Menu.
    last_status: String,
    quit_requested: bool,
}

impl App {
    fn menu_items(&self) -> [Option<MenuItem>; 4] {
        if self.linked {
            [
                Some(MenuItem::Inbox),
                Some(MenuItem::Send),
                Some(MenuItem::About),
                Some(MenuItem::Quit),
            ]
        } else {
            [Some(MenuItem::Link), Some(MenuItem::About), Some(MenuItem::Quit), None]
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

    fn write_inbox(&self, out: &mut String) -> Result<(), String> {
        if self.messages.is_empty() {
            write!(out, "Inbox\n\n(no messages yet)\n\nWaiting for inbound\nmessages...\n\nEnter: return")
                .map_err(|e| format!("inbox empty: {}", e))?;
            return Ok(());
        }
        write!(out, "Inbox ({})\n\n", self.messages.len())
            .map_err(|e| format!("inbox hdr: {}", e))?;
        // Render newest first; truncate sender + body so the screen
        // doesn't overflow.
        for msg in self.messages.iter() {
            let sender_short = truncate(&msg.sender, 22);
            let body_short = truncate(&msg.body, 80);
            write!(out, "from: {}\n  {}\n\n", sender_short, body_short)
                .map_err(|e| format!("inbox msg: {}", e))?;
        }
        write!(out, "\nEnter: return").map_err(|e| format!("inbox foot: {}", e))
    }
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
        messages: VecDeque::with_capacity(INBOX_CAPACITY),
        last_sender: None,
        last_status: String::new(),
        quit_requested: false,
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
                    handle_worker_event(&mut app, ev, &cmd_tx);
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
/// redraw.
fn handle_worker_event(app: &mut App, event: Event, cmd_tx: &Sender<Cmd>) {
    match event {
        Event::Message { sender, body, timestamp } => {
            log::info!(
                "xas/gam_app: inbound message from {} ({} bytes)",
                sender,
                body.len()
            );
            app.last_sender = Some(sender.clone());
            if app.messages.len() >= INBOX_CAPACITY {
                app.messages.pop_back();
            }
            app.messages.push_front(InboxMessage { sender, body, timestamp });
        }
        Event::ReceiveStarted => {
            log::info!("xas/gam_app: receive loop established");
        }
        Event::ReceiveError(msg) => {
            log::warn!("xas/gam_app: receive error: {}", msg);
            app.last_status = format!("Receive: {}", msg);
            // No screen transition — we stay where we are. The
            // status message lands in the next render of any
            // status-bearing screen (Linked, SendResult).
        }
        Event::SendComplete { timestamp } => {
            app.screen = Screen::SendResult {
                ok: true,
                status: format!("server timestamp = {}", timestamp),
            };
        }
        Event::SendError(msg) => {
            app.screen = Screen::SendResult { ok: false, status: msg };
        }
        Event::ShuttingDown => {
            log::info!("xas/gam_app: worker is shutting down");
            // We don't auto-quit; the user is expected to hit
            // Quit themselves. Just record status.
            app.last_status = "worker shutdown".to_string();
        }
        // The Link* events are handled in drive_link's own
        // loop (which blocks on event_rx directly during the
        // link sequence). We shouldn't see them here unless
        // forwarder timing got them first; just drop quietly.
        Event::LinkUrl(_) | Event::LinkComplete { .. } | Event::LinkError(_) => {
            log::debug!("xas/gam_app: stray link event after drive_link returned; ignoring");
        }
        Event::Pong | Event::Whoami(_) => {}
    }

    // If we just transitioned to "Linked Success", auto-fire
    // Cmd::StartReceive and slide the user into the Inbox.
    if matches!(app.screen, Screen::Linked { kind: LinkedKind::Success }) {
        // Note: StartReceive is idempotent; bridge layer drops
        // duplicates.
        let _ = cmd_tx.send_blocking(Cmd::StartReceive);
    }
}

fn drive_link(
    app: &mut App,
    cmd_tx: &Sender<Cmd>,
    event_rx: &Receiver<Event>,
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

    let device_name = match modals
        .alert_builder("Device name?")
        .field(Some("xas".to_string()), None)
        .build()
    {
        Ok(payloads) => {
            let trimmed = payloads.first().as_str().trim().to_string();
            if trimmed.is_empty() { "xas".to_string() } else { trimmed }
        }
        Err(e) => {
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = format!("device name modal:\n{:?}", e);
            return;
        }
    };

    app.screen = Screen::Linking;
    app.render().ok();

    if let Err(e) = cmd_tx.send_blocking(Cmd::LinkDevice { device_name }) {
        app.screen = Screen::Linked { kind: LinkedKind::Failure };
        app.last_status = format!("Cmd::LinkDevice send:\n{:?}", e);
        return;
    }

    let mut url_shown = false;
    loop {
        let event = match event_rx.recv_blocking() {
            Ok(ev) => ev,
            Err(e) => {
                app.screen = Screen::Linked { kind: LinkedKind::Failure };
                app.last_status = format!("event_rx closed:\n{:?}", e);
                return;
            }
        };
        match event {
            Event::LinkUrl(url) => {
                log::info!("xas/gam_app: link URL = {}", url);
                if !url_shown {
                    url_shown = true;
                    let _ = modals.show_notification(
                        "Scan with the Signal phone app, then press any key.",
                        Some(&url),
                    );
                }
            }
            Event::LinkComplete { device_name, aci, phone } => {
                log::info!(
                    "xas/gam_app: LinkComplete device={} aci={} phone={}",
                    device_name, aci, phone
                );
                app.linked = true;
                app.screen = Screen::Linked { kind: LinkedKind::Success };
                app.last_status =
                    format!("device:{}\naci:{}\nphone:{}", device_name, aci, phone);
                // Auto-fire StartReceive so post-link the inbox
                // begins accumulating.
                let _ = cmd_tx.send_blocking(Cmd::StartReceive);
                return;
            }
            Event::LinkError(msg) => {
                log::warn!("xas/gam_app: LinkError: {}", msg);
                app.screen = Screen::Linked { kind: LinkedKind::Failure };
                app.last_status = msg;
                return;
            }
            other => {
                // The forwarder is also running and will queue
                // these events for later. We don't process them
                // here; just let the link loop continue.
                log::debug!("xas/gam_app: drive_link saw non-link event {:?}; not forwarding", other);
            }
        }
    }
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
    if let Err(e) = cmd_tx.send_blocking(Cmd::SendMessage { recipient, body }) {
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
