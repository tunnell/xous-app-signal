//! Linking-flow screens.
//!
//! Five sub-states cover the device-linking flow:
//!
//! - `LinkStarting` — waiting for the worker to emit the
//!   provisioning URL.
//! - `LinkShowUrl` — URL received; user scans or copies it from the
//!   linked phone.
//! - `LinkConfirming` — phone has scanned; user-confirm round trip
//!   in flight.
//! - `LinkDone` — link complete; ready to transition into the
//!   message list.
//! - `LinkError` — link failed; user picks Retry or Cancel.
//!
//! All five are passive: they render what the worker has emitted via
//! `Event::LinkUrl` / `LinkComplete` / `LinkError`, and the driver
//! transitions between them based on those events.
//!
//! # Trust boundary
//!
//! The provisioning URL ([`LinkShowUrlScreen::url`]) is the
//! high-value secret of this flow. While displayed, anyone with a
//! camera trained on the screen (or with stdout access in hosted
//! mode) who completes the QR scan before the legitimate phone does
//! ends up paired against the xas-side identity that the worker just
//! generated. The window is short (typically tens of seconds, bounded
//! by the provisioning WebSocket) but real.
//!
//! # Security
//!
//! `LinkShowUrlScreen` holds the URL as a bare [`String`]. The
//! workspace REFACTOR_NOTES (item W-W.1) tracks a planned migration
//! to a zeroizing wrapper.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::Transition;

// ---------------------------------------------------------------
// LinkStarting
// ---------------------------------------------------------------

/// Transient screen shown immediately after the user picks "Link
/// this device". The driver issues `Cmd::LinkDevice` when this
/// screen lands on top of the stack and replaces it with
/// [`LinkShowUrlScreen`] once the worker emits `Event::LinkUrl`.
///
/// `Left` / `Esc` pops back to the splash, which the driver treats
/// as a cancel.
#[derive(Debug, Clone, Default)]
pub struct LinkStartingScreen;

impl LinkStartingScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        vec![
            String::new(),
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Connecting to Signal…"),
            String::new(),
            String::from("   On your phone, open Signal."),
            String::from("   Settings -> Linked Devices -> Link new."),
            String::new(),
            String::from("   The QR code will appear here."),
        ]
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

// ---------------------------------------------------------------
// LinkShowUrl
// ---------------------------------------------------------------

/// Shown after `Event::LinkUrl` arrives. Renders the URL as
/// monospaced text in this stdin-driven UI; QR rendering happens in
/// `gam_app.rs` for the GAM-driven path.
///
/// # Security
///
/// The URL is the link credential during its window — see the
/// module-level "Trust boundary" note. Anyone who completes the QR
/// scan first ends up paired against the xas-side identity. Treat
/// stdout in hosted mode as if it were a public bulletin board.
#[derive(Debug, Clone)]
pub struct LinkShowUrlScreen {
    /// The `tsdevice://` provisioning URL emitted by libsignal.
    ///
    /// Bearer credential during the link window. Do not log this
    /// field beyond the worker's existing trace lines, do not
    /// `Debug`-print structs wrapping it, and do not persist it.
    pub url: String,
}

impl LinkShowUrlScreen {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = vec![
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Scan or enter this URL on your phone:"),
            String::new(),
        ];

        // Wrap the URL across multiple lines so a 50-char-wide screen
        // can show all of it. tsdevice:// URLs are typically ~120-200
        // chars; we wrap at 44 chars per line.
        for chunk in chunk_str(&self.url, 44) {
            out.push(format!("   {chunk}"));
        }

        out.push(String::new());
        out.push(String::from("   Waiting for scan..."));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

fn chunk_str(s: &str, width: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut idx = 0;
    let bytes = s.as_bytes();
    while idx < bytes.len() {
        let end = (idx + width).min(bytes.len());
        // chunk on byte boundary; `tsdevice://` URLs are ASCII so
        // this is safe.
        out.push(&s[idx..end]);
        idx = end;
    }
    out
}

// ---------------------------------------------------------------
// LinkConfirming
// ---------------------------------------------------------------

/// Shown after the phone has scanned the QR, while the user is
/// confirming the device-name prompt on the phone. xas cannot peek
/// into presage's internal state to know when this transition
/// happens, so the screen displays a static "confirm on phone"
/// message until the worker emits `LinkComplete` or `LinkError`.
#[derive(Debug, Clone, Default)]
pub struct LinkConfirmingScreen;

impl LinkConfirmingScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        vec![
            String::new(),
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Scanned. Confirm on your phone:"),
            String::new(),
            String::from("   \"Link this device as 'Precursor'?\""),
            String::new(),
            String::from("   This may take 30-60 seconds."),
        ]
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

// ---------------------------------------------------------------
// LinkDone
// ---------------------------------------------------------------

/// Linking has completed. Shows the registration data so the user
/// can verify which phone they paired against. `Home` / `Right`
/// transitions into the conversation list, which sends
/// `Cmd::StartReceive` to the worker.
///
/// # Security
///
/// The ACI is account-identifying — see workspace REFACTOR_NOTES
/// W-W.2 on PII-redaction across the UART log surface. The phone
/// field is the user's E.164 number. Both are displayed because the
/// user needs to confirm the link landed on the right account; do
/// not extend the display by adding a `Debug` print of the whole
/// struct to a log line.
#[derive(Debug, Clone)]
pub struct LinkDoneScreen {
    /// Device name confirmed by the linked phone.
    pub device_name: String,
    /// ACI (Signal account UUID). PII; display only, do not log.
    pub aci: String,
    /// E.164 phone number associated with the linked account. PII;
    /// display only, do not log.
    pub phone: String,
}

impl LinkDoneScreen {
    pub fn new(device_name: String, aci: String, phone: String) -> Self {
        Self {
            device_name,
            aci,
            phone,
        }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(String::new());
        out.push(String::new());
        out.push(String::from("                       OK"));
        out.push(String::new());
        out.push(String::from("                Linked."));
        out.push(String::new());
        out.push(format!("       Device name: {}", &self.device_name));
        out.push(format!("       ACI:         {}", short(&self.aci, 32)));
        out.push(format!("       Phone:       {}", &self.phone));
        out.push(String::new());
        out.push(String::from("       You can now receive and send."));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            // From LinkDone, Home transitions into the
            // ConversationList screen which on entry sends
            // `Cmd::StartReceive` and begins streaming messages.
            Key::Home | Key::Right => Transition::Replace(crate::stdin_ui::screen::Screen::ConversationList(
                crate::stdin_ui::screens::conversation_list::ConversationListScreen::new(),
            )),
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------
// LinkError
// ---------------------------------------------------------------

/// Linking failed. Shows the worker-supplied error string and a
/// Retry / Cancel choice. Retry replaces this screen with a fresh
/// [`LinkStartingScreen`], which re-issues `Cmd::LinkDevice`; Cancel
/// pops back to the splash.
#[derive(Debug, Clone)]
pub struct LinkErrorScreen {
    /// Free-form error string from the worker. Surfaces upstream
    /// transport / libsignal failures verbatim; do not assume any
    /// stable structure.
    pub reason: String,
    /// Current cursor position: 0 = Retry, 1 = Cancel.
    pub focus: usize,
}

impl LinkErrorScreen {
    pub fn new(reason: String) -> Self {
        Self { reason, focus: 0 }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = vec![
            String::new(),
            String::new(),
            String::from("                       X"),
            String::new(),
            String::from("              Linking failed."),
            String::new(),
            String::from("   Reason:"),
        ];
        for chunk in chunk_str(&self.reason, 44) {
            out.push(format!("     {chunk}"));
        }
        out.push(String::new());
        let retry = if self.focus == 0 { ">" } else { " " };
        let cancel = if self.focus == 1 { ">" } else { " " };
        out.push(format!("       {retry} Try again"));
        out.push(format!("       {cancel} Cancel"));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Up => {
                if self.focus > 0 {
                    self.focus -= 1;
                }
                Transition::None
            }
            Key::Down => {
                if self.focus < 1 {
                    self.focus += 1;
                }
                Transition::None
            }
            Key::Home | Key::Right => match self.focus {
                0 => Transition::Replace(crate::stdin_ui::screen::Screen::LinkStarting(
                    LinkStartingScreen::new(),
                )),
                1 => Transition::Pop,
                _ => Transition::None,
            },
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}
