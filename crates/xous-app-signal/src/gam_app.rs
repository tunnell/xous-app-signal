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
//! - **(SendResult / Inbox screens were removed in the conversation-list redesign;
//!   their roles are folded into Home + Thread.)
//!   round-trip. Enter returns to Menu.
//!
//! Worker integration: events from the signal worker (`event_rx`)
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
use xous_signal_worker::{Cmd, Event};

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
    /// Pre-link app menu (Link / About / Quit). Post-link landing
    /// is `Home`; the equivalent post-link surface is `Settings`.
    Menu,
    About,
    Linking,
    Linked { kind: LinkedKind },
    /// Post-link landing: conversation list. Default screen after
    /// `LinkComplete`. F4 / Esc opens `Settings`.
    Home,
    /// Per-conversation history view + compose input.
    Thread { uuid: Uuid },
    /// Post-link settings sub-menu (Profile / Help / About / Logout / Quit).
    Settings,
    /// Account info display (Name / Number / Username).
    Profile,
    /// FAQ + issue-tracker pointer.
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkedKind {
    Success,
    Failure,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuItem {
    Link,
    About,
    Help,
}

/// Items shown on `Screen::Settings`. Reachable via F4 (or Esc) from
/// `Home` once the device is linked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SettingsItem {
    Profile,
    Help,
    About,
    Logout,
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
    /// One-line text rendered on transient screens (Linked banner,
    /// optional SendError surface). Cleared on transition to Home.
    last_status: String,
    /// True between Cmd::LinkDevice send and Event::Link{Complete,Error}.
    /// While set, handle_worker_event opens the QR modal on LinkUrl
    /// and transitions on LinkComplete/LinkError. Cleared on
    /// terminal events.
    linking_in_progress: bool,
    /// Cursor on Screen::Settings.
    settings_selected: SettingsItem,
    /// Account info captured from Event::LinkComplete in this session.
    /// On a cold start where the device is already linked from a
    /// prior session, these stay None until a Cmd::GetAccountInfo
    /// path is wired (Tier 2 chore).
    account_device_name: Option<String>,
    account_aci: Option<String>,
    account_phone: Option<String>,
}

impl App {
    fn menu_items(&self) -> [Option<MenuItem>; 3] {
        if self.linked {
            // Post-link Menu is reachable from Home (via the Menu key)
            // for the few utility actions that don't have a dedicated
            // F-key. Settings (F4) is the main post-link surface.
            [Some(MenuItem::About), Some(MenuItem::Help), None]
        } else {
            // Pre-link Menu IS the landing screen.
            [Some(MenuItem::Link), Some(MenuItem::About), Some(MenuItem::Help)]
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
        // Bold on the conversation-list and thread screens for
        // legibility — those are the primary surfaces. Other transient
        // screens (About, Linked banner, Linking) keep Regular.
        tv.style = match self.screen {
            Screen::Home | Screen::Thread { .. } => GlyphStyle::Bold,
            _ => GlyphStyle::Regular,
        };

        match &self.screen {
            Screen::Menu => self.write_menu(&mut tv.text)?,
            Screen::About => write!(
                tv.text,
                "About xas\n\n\
                 Unofficial Signal client\n\
                 for Xous on Precursor.\n\n\
                 Version: {}\n\
                 Author:  @tunnell\n\n\
                 github.com/tunnell/\n\
                  xous-app-signal\n\n\
                 Security:\n\
                  - End-to-end encrypted\n\
                    via the Signal Protocol\n\
                    (same Double Ratchet +\n\
                    PQXDH used by the\n\
                    official Signal app).\n\
                  - Network connection to\n\
                    chat.signal.org is\n\
                    verified against\n\
                    Signal's own pinned\n\
                    Certificate Authority\n\
                    (no public CA bundle\n\
                    is trusted).\n\
                  - Hardware-rooted on\n\
                    Precursor: every layer\n\
                    is inspectable; keys\n\
                    are sealed on-device.\n\n\
                 Built on:\n\
                  - presage v0.8.0-dev\n\
                  - libsignal-service-rs\n\
                  - libsignal v0.91.0\n\n\
                 Limitations (alpha):\n\
                  - No group chats\n\
                  - No attachments\n\
                  - Send takes 1-4 min\n\
                    (transport refactor\n\
                    pending)\n\
                  - 2.4 GHz Wi-Fi only\n\n\
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
                 The TLS certificate is\n\
                 verified against Signal's\n\
                 pinned Certificate\n\
                 Authority (CA) — no\n\
                 public root CA is\n\
                 trusted.\n\n\
                 Linking takes a few\n\
                 minutes after you scan\n\
                 the QR. Don't\n\
                 power-cycle.\n\n\
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
            Screen::Home => self.write_home(&mut tv.text)?,
            Screen::Thread { uuid } => self.write_thread(&mut tv.text, uuid)?,
            Screen::Settings => self.write_settings(&mut tv.text)?,
            Screen::Profile => self.write_profile(&mut tv.text)?,
            Screen::Help => self.write_help(&mut tv.text)?,
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
                    MenuItem::About => "About",
                    MenuItem::Help => "Help",
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
        writeln!(out, "{}", "-".repeat(45)).map_err(|e| format!("home rule: {}", e))?;

        if self.dialogues.is_empty() {
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "      No conversations yet.")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  Wait for someone to message,")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  or press F1 to start one.")
                .map_err(|e| format!("home empty: {}", e))?;
            writeln!(out).map_err(|e| format!("home empty: {}", e))?;
            writeln!(out, "  F1 New  F2 Sync  F3 Help  F4 Settings")
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

            writeln!(out, "{}", "-".repeat(45))
                .map_err(|e| format!("home sep: {}", e))?;
        }
        writeln!(out).map_err(|e| format!("home foot: {}", e))?;
        write!(out, "  ↑↓ Sel  Enter Open\n  F1 New  F2 Sync  F3 Help  F4 Settings")
            .map_err(|e| format!("home hint: {}", e))
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
        writeln!(out, "{}", "-".repeat(45)).map_err(|e| format!("thread rule: {}", e))?;

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
        writeln!(out, "{}", "-".repeat(45)).map_err(|e| format!("thread foot rule: {}", e))?;
        // Compose input. Cursor is `_` at the end of the buffer for
        // Phase A — no horizontal scroll if the buffer is wider than
        // the visible width, just shows the trailing chars.
        writeln!(out, "> {}_", crate::dialogue::ellipsize(&self.compose_buffer, 30))
            .map_err(|e| format!("thread compose: {}", e))?;
        write!(out, "Enter Send  Esc Back  F4 Settings")
            .map_err(|e| format!("thread hint: {}", e))?;
        Ok(())
    }

    fn settings_items(&self) -> [SettingsItem; 4] {
        [
            SettingsItem::Profile,
            SettingsItem::Help,
            SettingsItem::About,
            SettingsItem::Logout,
        ]
    }

    fn settings_move(&mut self, delta: isize) {
        let items = self.settings_items();
        let idx = items
            .iter()
            .position(|s| *s == self.settings_selected)
            .unwrap_or(0);
        let len = items.len() as isize;
        let new = (idx as isize + delta).rem_euclid(len);
        self.settings_selected = items[new as usize];
    }

    fn write_settings(&self, out: &mut String) -> Result<(), String> {
        write!(out, "Settings\n\n").map_err(|e| format!("set hdr: {}", e))?;
        for item in self.settings_items() {
            let mark = if item == self.settings_selected { ">" } else { " " };
            let label = match item {
                SettingsItem::Profile => "Profile",
                SettingsItem::Help => "Help",
                SettingsItem::About => "About",
                SettingsItem::Logout => "Logout",
            };
            writeln!(out, "{} {}", mark, label).map_err(|e| format!("set item: {}", e))?;
        }
        write!(out, "\n↑↓ Sel  Enter Open  Esc Back")
            .map_err(|e| format!("set foot: {}", e))
    }

    fn write_profile(&self, out: &mut String) -> Result<(), String> {
        let none = "(not loaded)";
        let device = self
            .account_device_name
            .as_deref()
            .unwrap_or(none);
        let phone = self.account_phone.as_deref().unwrap_or(none);
        let aci = self.account_aci.as_deref().unwrap_or(none);
        write!(
            out,
            "Profile\n\n\
             Name:     {}\n\
             Number:   {}\n\
             Username: {}\n\n\
             ACI:      {}\n\n\
             (Loaded only after a fresh\n\
              link this session. Persist\n\
              across sessions is a TODO.)\n\n\
             Press Enter to return.",
            device,
            phone,
            "(not synced)",
            aci,
        )
        .map_err(|e| format!("profile: {}", e))
    }

    fn write_help(&self, out: &mut String) -> Result<(), String> {
        write!(
            out,
            "Help\n\n\
             xas — Signal client for\n\
             Precursor (Xous OS).\n\
             Status: alpha.\n\n\
             Wi-Fi (do this first, in\n\
             shellchat, before opening\n\
             xas):\n\
               wlan off\n\
               wlan on\n\
               ssid scan\n\
               wlan status   (until\n\
                 'Connected')\n\
               net ping 1.1.1.1\n\
             Use one SSID only — no\n\
             roaming support yet.\n\n\
             Known limits:\n\
              - Send may fail (WS keepalive)\n\
              - No group chats\n\
              - No images / attachments\n\
              - No history scroll / search\n\n\
             File a bug:\n\
             github.com/tunnell/\n\
              xous-app-signal/issues\n\n\
             Full FAQ: see FAQ.md in repo.\n\n\
             Press Enter to return."
        )
        .map_err(|e| format!("help: {}", e))
    }

}

/// Whether a character is acceptable in the Thread compose buffer.
/// Phase A: alphanumeric + space + ASCII punctuation. Non-ASCII
/// (emoji, accented chars, CJK) is silently dropped; Phase B
/// widens to anything the GAM font can render.
fn is_compose_char(c: char) -> bool {
    c.is_alphanumeric() || c == ' ' || c.is_ascii_punctuation()
}

/// If `XAS_MOCK_MESSAGES=1` is set in the environment, seed `App` with
/// a small fake conversation history so hosted-mode UI iteration
/// shows a populated Home + Thread without needing signal-cli traffic.
/// Linked state is unaffected; messages are RAM-only and disappear
/// across runs unless `pddb dump` is invoked.
///
/// Hosted-only: `std::env::var` is harmless on rv32 too (no env there)
/// but the path simply does nothing without the variable set.
fn seed_mock_messages_if_requested(app: &mut App) {
    if std::env::var("XAS_MOCK_MESSAGES").ok().as_deref() != Some("1") {
        return;
    }
    log::info!("xas/gam_app: XAS_MOCK_MESSAGES=1 — seeding mock history");

    let now_ms = unix_now_ms();
    let alice = Uuid::from_u128(0x0a11ce_aaaaaa_bbbbbb_cccccc_dddddd_001);
    let bob = Uuid::from_u128(0x0b0b_bbbbbb_cccccc_dddddd_eeeeee_002);
    let dad = Uuid::from_u128(0xdad0_cccccc_dddddd_eeeeee_ffffff_003);
    let unknown = Uuid::from_u128(0x9999_dddddd_eeeeee_ffffff_111111_004);

    let mocks: &[(Uuid, &str, &str, u64, bool, SendStatus)] = &[
        (alice, "Alice Nguyen", "sure, meet at 6", 2 * 60_000, false, SendStatus::Sent),
        (alice, "Alice Nguyen", "I'll bring drinks", 1 * 60_000, false, SendStatus::Sent),
        (alice, "Alice Nguyen", "actually make it 6:30", 30_000, false, SendStatus::Sent),
        (bob, "Bob Kowalski", "did you get the file?", 12 * 60_000, false, SendStatus::Sent),
        (bob, "You", "yes, on my way", 11 * 60_000, true, SendStatus::Delivered),
        (bob, "Bob Kowalski", "thanks!", 5 * 60_000, false, SendStatus::Sent),
        (dad, "Dad", "lunch sunday?", 25 * 60 * 60_000, false, SendStatus::Sent),
        (dad, "You", "On my way", 24 * 60 * 60_000, true, SendStatus::Delivered),
        (
            unknown,
            "+14155550199",
            "Your Uber has arrived",
            3 * 60 * 60_000,
            false,
            SendStatus::Sent,
        ),
        (
            unknown,
            "+14155550199",
            "Your driver is waiting",
            2 * 60 * 60_000,
            false,
            SendStatus::Sent,
        ),
    ];

    for (uuid, label, body, age_ms, outgoing, status) in mocks {
        app.messages.push(ThreadMessage {
            uuid: *uuid,
            author_label: label.to_string(),
            body: body.to_string(),
            timestamp: now_ms.saturating_sub(*age_ms),
            outgoing: *outgoing,
            status: *status,
            read: *outgoing, // outgoing always read; incoming starts unread
        });
    }
    app.dialogues = rebuild_summaries(&app.messages);
    // Mark linked + jump straight into Home so the demo lands on the
    // populated conversation list rather than the pre-link Welcome.
    app.linked = true;
    app.screen = Screen::Home;
    app.home_focus = 0;
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

/// Mark every message belonging to `uuid` as read, then rebuild
/// the dialogue summaries so unread counts reflect the new state.
fn mark_thread_read(app: &mut App, uuid: Uuid) {
    let mut changed = false;
    for m in app.messages.iter_mut() {
        if m.uuid == uuid && !m.read {
            m.read = true;
            changed = true;
        }
    }
    if changed {
        app.dialogues = rebuild_summaries(&app.messages);
    }
}

/// True if `s` matches the Signal "username" shape: a non-empty
/// nickname (3–32 chars, ASCII alnum or `_`) followed by `.` and
/// 2–9 digits, e.g. `alice.42`. Used only for classifying user input
/// in the New chat modal so the rejection message can be specific.
fn looks_like_signal_username(s: &str) -> bool {
    let (nick, rest) = match s.split_once('.') {
        Some(parts) => parts,
        None => return false,
    };
    if !(3..=32).contains(&nick.len()) {
        return false;
    }
    if !nick.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    if !(2..=9).contains(&rest.len()) {
        return false;
    }
    rest.chars().all(|c| c.is_ascii_digit())
}

/// Settings → Logout. Stub: the actual flow needs to wipe the
/// presage account state in PDDB and stop the worker. Tracked as a
/// Tier-2 chore. For now show a notification with the manual path.
fn drive_logout(modals_xns: &xous_names::XousNames) {
    if let Ok(modals) = modals::Modals::new(modals_xns) {
        let _ = modals.show_notification(
            "Logout not yet implemented.\n\
             To re-link, wipe the PDDB\n\
             (or `pddb wipe` in shellchat)\n\
             then reflash and link again.",
            None,
        );
    }
}

/// F1 on Home: prompt for a UUID (or +e164) and open an empty
/// thread for it. The compose box becomes the entry point; the
/// thread becomes a real conversation as soon as a message goes
/// out or comes back.
fn drive_new_chat(app: &mut App, modals_xns: &xous_names::XousNames) {
    let modals = match modals::Modals::new(modals_xns) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("xas/gam_app: F1 new chat — Modals::new err {:?}", e);
            return;
        }
    };
    let raw = match modals
        .alert_builder("New chat — UUID, +E.164, or name.000")
        .field(None, None)
        .build()
    {
        Ok(payloads) => payloads.first().as_str().trim().to_string(),
        Err(e) => {
            log::info!("xas/gam_app: F1 new chat cancelled / err: {:?}", e);
            return;
        }
    };
    if raw.is_empty() {
        return;
    }
    // Phone-number and username (Signal "name.000" form) input is
    // best-effort: both need a server round-trip we don't have yet.
    // UUID input is the working path. The error path classifies the
    // input so the user knows which lookup is missing.
    let uuid = match Uuid::parse_str(&raw) {
        Ok(u) => u,
        Err(_) => {
            let kind = if raw.starts_with('+') && raw.len() > 1 && raw[1..].chars().all(|c| c.is_ascii_digit()) {
                "Phone-number"
            } else if looks_like_signal_username(&raw) {
                "Username"
            } else {
                "Contact"
            };
            log::info!(
                "xas/gam_app: F1 new chat — {} input {:?}; lookup not yet supported",
                kind, raw,
            );
            let _ = modals.show_notification(
                &format!(
                    "{} lookup not\nsupported yet. Enter a UUID\nfor now.",
                    kind
                ),
                None,
            );
            return;
        }
    };
    app.screen = Screen::Thread { uuid };
    app.compose_buffer.clear();
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
        last_status: String::new(),
        linking_in_progress: false,
        settings_selected: SettingsItem::Profile,
        account_device_name: None,
        account_aci: None,
        account_phone: None,
    };
    seed_mock_messages_if_requested(&mut app);
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
                // Quit was removed from the menus (it didn't actually
                // exit anything — just hid the app and reset state);
                // the app stays foregrounded for its lifetime.
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
                MenuItem::About => app.screen = Screen::About,
                MenuItem::Help => app.screen = Screen::Help,
            },
            // Esc on the post-link Menu returns to Home (the landing).
            // Pre-link, Menu IS the landing — Esc is a no-op.
            (Screen::Menu, '\u{1b}') if app.linked => {
                app.screen = Screen::Home;
                app.selected = MenuItem::About;
            }
            (Screen::About, '∴') | (Screen::About, '\u{d}') => {
                app.screen = if app.linked { Screen::Home } else { Screen::Menu };
            }
            (Screen::Linked { .. }, '∴') | (Screen::Linked { .. }, '\u{d}') => {
                if app.linked {
                    app.screen = Screen::Home;
                    app.home_focus = 0;
                } else {
                    app.screen = Screen::Menu;
                    app.selected = MenuItem::Link;
                }
                app.last_status.clear();
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
                // do nothing — user can still press the Menu key
                // to access About / Quit.
                if let Some(d) = app.dialogues.get(app.home_focus) {
                    let opened = d.uuid;
                    mark_thread_read(app, opened);
                    app.screen = Screen::Thread { uuid: opened };
                }
            }
            // Menu / Esc on Home opens Settings.
            (Screen::Home, '☰') | (Screen::Home, '\u{1b}') => {
                app.screen = Screen::Settings;
                app.settings_selected = SettingsItem::Profile;
            }
            // F1 (Precursor sends 0x11): "New chat" — prompt for UUID
            // or +e164, then open the empty Thread.
            (Screen::Home, '\u{11}') => {
                drive_new_chat(app, modals_xns);
            }
            // F2 (0x12): Sync — placeholder. Needs a worker-side
            // Cmd::SyncContacts plus a manager_task handler.
            // Tier-2 chore. For now show a notification.
            (Screen::Home, '\u{12}') => {
                log::info!("xas/gam_app: F2 sync requested (not yet implemented)");
                if let Ok(modals) = modals::Modals::new(modals_xns) {
                    let _ = modals.show_notification(
                        "Sync not yet implemented.\nSee Help for status.",
                        None,
                    );
                }
            }
            // F3 (0x13): Help / FAQ.
            (Screen::Home, '\u{13}') => {
                app.screen = Screen::Help;
            }
            // F4 (0x14): Settings.
            (Screen::Home, '\u{14}') => {
                app.screen = Screen::Settings;
                app.settings_selected = SettingsItem::Profile;
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
                        read: true,
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
            // F1 on Thread: same as Enter when buffer non-empty (send).
            // No-op on empty buffer (don't surprise-back-out via F1).
            (Screen::Thread { uuid }, '\u{11}') => {
                if !app.compose_buffer.is_empty() {
                    let body = std::mem::take(&mut app.compose_buffer);
                    let recipient_uuid = *uuid;
                    let send_ts = unix_now_ms();
                    let recipient_str = recipient_uuid.to_string();
                    log::info!(
                        "xas/gam_app: F1 send to={} ({} bytes) ts={}",
                        recipient_str,
                        body.len(),
                        send_ts,
                    );
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
                        read: true,
                    });
                    app.dialogues = rebuild_summaries(&app.messages);
                    if let Err(e) = cmd_tx.send_blocking(Cmd::SendMessage {
                        recipient: recipient_str,
                        body,
                        timestamp: send_ts,
                    }) {
                        log::warn!("xas/gam_app: Cmd::SendMessage send err: {:?}", e);
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
            // F4 on Thread: open Settings.
            (Screen::Thread { .. }, '\u{14}') => {
                app.screen = Screen::Settings;
                app.settings_selected = SettingsItem::Profile;
            }
            // F3 on Thread: Help.
            (Screen::Thread { .. }, '\u{13}') => {
                app.screen = Screen::Help;
            }
            // Settings sub-menu navigation + selection.
            (Screen::Settings, '↑') => app.settings_move(-1),
            (Screen::Settings, '↓') => app.settings_move(1),
            (Screen::Settings, '∴') | (Screen::Settings, '\u{d}') => {
                match app.settings_selected {
                    SettingsItem::Profile => {
                        // If Profile fields aren't loaded yet (cold
                        // start with linked-from-PDDB but no fresh
                        // LinkComplete), fire Cmd::GetAccountInfo
                        // so the worker can read registration_data
                        // and emit Event::AccountInfo. UI updates
                        // when Event arrives.
                        if app.account_aci.is_none() {
                            log::info!("xas/gam_app: Profile entry, account info not loaded — sending Cmd::GetAccountInfo");
                            if let Err(e) = cmd_tx.send_blocking(Cmd::GetAccountInfo) {
                                log::warn!("xas/gam_app: Cmd::GetAccountInfo send err: {:?}", e);
                            }
                        }
                        app.screen = Screen::Profile;
                    }
                    SettingsItem::Help => app.screen = Screen::Help,
                    SettingsItem::About => app.screen = Screen::About,
                    SettingsItem::Logout => drive_logout(modals_xns),
                }
            }
            (Screen::Settings, '\u{1b}') | (Screen::Settings, '☰') | (Screen::Settings, '\u{11}') => {
                app.screen = Screen::Home;
            }
            // Profile / Help / About: Enter, Esc, or F1 returns to
            // the screen we came from. For Help we always go to
            // Home (F3 is the global Help shortcut from Home).
            // F1 acts as a "back" affordance because Esc isn't
            // present on the Precursor key set.
            (Screen::Profile, '∴')
            | (Screen::Profile, '\u{d}')
            | (Screen::Profile, '\u{1b}')
            | (Screen::Profile, '\u{11}') => {
                app.screen = Screen::Settings;
            }
            (Screen::Help, '∴')
            | (Screen::Help, '\u{d}')
            | (Screen::Help, '\u{1b}')
            | (Screen::Help, '\u{11}') => {
                // Help is reachable from Home (F3), pre-link Menu,
                // and Settings. Always return to Home for
                // simplicity — user can re-open Settings via F4.
                app.screen = Screen::Home;
            }
            // F1 on About also returns to the previous surface
            // (Home if linked, Menu otherwise).
            (Screen::About, '\u{11}') | (Screen::About, '\u{1b}') => {
                app.screen = if app.linked { Screen::Home } else { Screen::Menu };
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
            app.account_device_name = Some(device_name);
            app.account_aci = Some(aci);
            app.account_phone = Some(phone);
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
                read: false,
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
            // Otherwise (no match), the worker emitted an event for
            // a send we don't have an optimistic row for. Log + ignore.
            let matched = app
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.outgoing && m.timestamp == timestamp);
            if let Some(m) = matched {
                m.status = SendStatus::Delivered;
                app.dialogues = rebuild_summaries(&app.messages);
            } else {
                log::info!(
                    "xas/gam_app: SendComplete ts={} with no matching pending row",
                    timestamp
                );
            }
        }
        Event::SendError { reason, timestamp } => {
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
                log::warn!(
                    "xas/gam_app: SendError reason={} ts={:?} with no matching row",
                    reason,
                    timestamp
                );
                app.last_status = format!("Send: {}", reason);
            }
        }
        Event::ShuttingDown => {
            log::info!("xas/gam_app: worker is shutting down");
            app.last_status = "worker shutdown".to_string();
        }
        Event::AccountInfo(Ok(info)) => {
            log::info!(
                "xas/gam_app: AccountInfo OK device={} aci={} phone={}",
                info.device_name, info.aci, info.phone,
            );
            app.account_device_name = Some(info.device_name);
            app.account_aci = Some(info.aci);
            app.account_phone = Some(info.phone);
            // If we're currently rendering the Profile screen,
            // refresh so the placeholder "(not loaded)" flips to
            // the real values immediately.
            if matches!(app.screen, Screen::Profile) {
                let _ = app.render();
            }
        }
        Event::AccountInfo(Err(reason)) => {
            log::warn!("xas/gam_app: AccountInfo Err: {}", reason);
            // Leave account_* fields as-is. Profile screen will
            // continue to show "(not loaded)" placeholders. Not
            // worth showing a popup since this is a passive lookup.
        }
        Event::Pong | Event::Whoami(_) => {}
    }
}

/// Hosted-mode helper: read `$HOME/.xas-link-attempts`, increment
/// it, return the device name to default to. Each link attempt
/// gets a fresh `xasN` so the user can correlate this run's QR
/// with the entry that lands in their phone's Linked Devices list.
///
/// File semantics: missing → create with `0`, use `0`, write `1`.
/// Existing → read N, use N, write N+1. If `$HOME` isn't set or
/// the read/write fails, fall back to the bare `"xas"` name.
///
/// Hosted-only: on rv32 there's no general filesystem, so this
/// quietly falls back to "xas".
#[cfg(not(target_os = "xous"))]
fn next_attempt_device_name() -> String {
    let Ok(home) = std::env::var("HOME") else {
        return "xas".to_string();
    };
    let path = std::path::PathBuf::from(home).join(".xas-link-attempts");
    let n: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = std::fs::write(&path, format!("{}", n + 1));
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

