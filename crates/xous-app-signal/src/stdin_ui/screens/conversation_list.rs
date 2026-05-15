//! Conversation list.
//!
//! On entry, the driver sends `Cmd::StartReceive`. The screen's
//! [`ReceiveStatus`] tracks whether the receive loop is active;
//! received messages are appended to a flat
//! `Vec<MessageSummary>` rendered latest-first.
//!
//! # Trust boundary
//!
//! Each [`MessageSummary`] carries plaintext that crossed the
//! libsignal decrypt boundary inside the worker. The sender field
//! is the account-identifying ACI (or e164 fallback) that the
//! worker forwarded; the body is the decrypted message text. Treat
//! everything in this screen as PII or higher when adding new
//! consumers — see workspace REFACTOR_NOTES W-W.2 on PII redaction
//! discipline and W-W.3 on wrapping decrypted bodies in zeroizing
//! containers.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::{Screen, Transition};
use crate::stdin_ui::screens::menu::MenuScreen;

/// One received message, flattened for display. Mirrors the
/// `Event::Message` payload from `xous-signal-worker`.
///
/// # Security
///
/// Both `sender` and `body` are plaintext that crossed the
/// libsignal decrypt boundary. `sender` is account-identifying;
/// `body` is the actual message text. The derived `Debug` impl
/// renders both verbatim — avoid `tracing::debug!(?summary)` and
/// equivalent on any value of this type.
#[derive(Debug, Clone)]
pub struct MessageSummary {
    /// ACI (UUID) of the message author, or e164/UUID-shaped
    /// fallback if the worker could not resolve a contact name.
    pub sender: String,
    /// Decrypted message body.
    pub body: String,
    /// Unix milliseconds. Comes from the worker; xas does not
    /// re-stamp.
    pub timestamp: u64,
}

/// Receive-loop state from the screen's perspective.
#[derive(Debug, Clone)]
pub enum ReceiveStatus {
    /// `Cmd::StartReceive` sent on entry; no `ReceiveStarted` event
    /// back yet.
    Starting,
    /// `Event::ReceiveStarted` received; receive loop is active.
    Listening,
    /// `Event::ReceiveError(reason)` received; loop is dead.
    Error(String),
}

/// Maximum number of messages to render. Older entries scroll off
/// the visible area; pagination not needed for MVP.
const MAX_VISIBLE: usize = 8;

#[derive(Debug, Clone)]
pub struct ConversationListScreen {
    pub status: ReceiveStatus,
    pub messages: Vec<MessageSummary>,
}

impl Default for ConversationListScreen {
    fn default() -> Self {
        Self {
            status: ReceiveStatus::Starting,
            messages: Vec::new(),
        }
    }
}

impl ConversationListScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a received message. Keeps the most recent
    /// `MAX_VISIBLE * 2` messages — enough to scroll a little bit
    /// past the visible area without unbounded growth. A future
    /// pass can move this to PDDB-backed persistence and drop the
    /// in-memory cap.
    pub fn push_message(&mut self, msg: MessageSummary) {
        self.messages.push(msg);
        if self.messages.len() > MAX_VISIBLE * 2 {
            let drop = self.messages.len() - MAX_VISIBLE * 2;
            self.messages.drain(..drop);
        }
    }

    pub fn set_status(&mut self, status: ReceiveStatus) {
        self.status = status;
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(MAX_VISIBLE + 6);
        out.push(String::new());

        // Status line. One short text indicator; whole line is bold
        // when receive is healthy, regular when starting/error.
        let status_line = match &self.status {
            ReceiveStatus::Starting => "   Starting receive...".to_string(),
            ReceiveStatus::Listening => "   Listening for messages.".to_string(),
            ReceiveStatus::Error(reason) => format!("   Receive error: {}", short(reason, 32)),
        };
        out.push(status_line);
        out.push(String::new());

        if self.messages.is_empty() {
            // Empty state.
            out.push(String::new());
            out.push(String::from("              No messages yet."));
            out.push(String::new());
            out.push(String::from("       Press Menu to send one,"));
            out.push(String::from("       or wait for incoming."));
        } else {
            // Latest first; show most recent at top so the user
            // sees new arrivals without scrolling.
            for msg in self.messages.iter().rev().take(MAX_VISIBLE) {
                let sender = short(&msg.sender, 16);
                let body = short(&msg.body, 28);
                out.push(format!("   {sender}"));
                out.push(format!("     {body}"));
            }
        }
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            // 'c' opens the Compose screen, prefilled with
            // the most-recent sender as recipient. If there are no
            // messages yet (`messages.is_empty()`), Compose can't
            // resolve a recipient — drop the keypress on the floor.
            // Could later surface a contact-picker instead.
            Key::Char('c') => {
                if let Some(latest) = self.messages.last() {
                    Transition::Push(Screen::Compose(
                        crate::stdin_ui::screens::compose::ComposeScreen::new(latest.sender.clone()),
                    ))
                } else {
                    Transition::None
                }
            }
            // Press M (or Home with no message focus) to open the
            // app menu — gives access to "Test worker", quit, etc.
            Key::Char('m') | Key::Home => Transition::Push(Screen::Menu(MenuScreen::new())),
            // Esc / q quits.
            Key::Esc | Key::Char('q') => Transition::Quit,
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
