//! Compose screen.
//!
//! One per send. Holds the recipient (set on construction from
//! the most recent received message's sender), an editable body,
//! and a [`SendState`] that the driver flips when the worker emits
//! `Event::SendComplete` or `Event::SendError`.
//!
//! # Trust boundary
//!
//! The `body` field is the outbound message plaintext — the user
//! has typed it but the worker has not yet handed it to
//! libsignal's encrypt path. The `recipient` field carries the
//! recipient's ACI (UUID) or e164 fallback; account-identifying
//! PII. Treat both fields as confidential when adding consumers.
//!
//! # Hosted-mode input model
//!
//! Instead of per-char keystrokes (which would need raw-mode
//! terminal handling the hosted UI deliberately avoids), the
//! driver hands the screen the entire input line via
//! [`crate::stdin_ui::Ui::dispatch_line`]. The user types the
//! message and presses Enter; the line becomes the body and a
//! `Cmd::SendMessage` fires. The on-device GAM build uses the
//! per-char path inside `gam_app.rs::handle_keys`; the
//! [`SendState`] transitions are the same.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::Transition;

#[derive(Debug, Clone)]
pub struct ComposeScreen {
    /// Recipient `service_id_string()` — typically the most
    /// recent sender's UUID, populated by `ConversationList` when
    /// pushing this screen.
    ///
    /// # Security
    ///
    /// Account-identifying. Renders on screen so the user can
    /// confirm who they are sending to; do not extend logging to
    /// include this field.
    pub recipient: String,
    /// What the screen is currently doing. Determines what input
    /// the user can give and what the screen renders.
    pub state: SendState,
    /// The body the user typed, captured the moment they pressed
    /// Enter. Only populated once per screen lifetime; a second
    /// Enter is a no-op until the matching `SendComplete` or
    /// `SendError` event arrives.
    ///
    /// # Security
    ///
    /// Plaintext message body. Do not log; do not include in
    /// `Debug` output that may reach UART.
    pub body: String,
}

#[derive(Debug, Clone)]
pub enum SendState {
    /// Initial state. Screen is waiting for a `handle_line` call.
    Editing,
    /// `Cmd::SendMessage` has been emitted; waiting for
    /// `Event::SendComplete` or `Event::SendError`.
    Sending,
    /// Worker reported success.
    Sent { timestamp: u64 },
    /// Worker reported failure. UI lets the user choose to retry
    /// (re-enters Editing) or back out via Esc.
    Error(String),
}

impl ComposeScreen {
    pub fn new(recipient: String) -> Self {
        Self {
            recipient,
            state: SendState::Editing,
            body: String::new(),
        }
    }

    /// Capture the user's typed message and transition to
    /// [`SendState::Sending`]. Returns `(recipient, body)` so the
    /// driver can emit the actual `Cmd::SendMessage` with the
    /// captured payload.
    ///
    /// Returns `None` when:
    /// - The screen is not in [`SendState::Editing`] (a previous
    ///   send is still in flight).
    /// - `body` is empty — treated as "cancel this attempt"; the
    ///   screen stays in `Editing`.
    ///
    /// # Security
    ///
    /// Receives the plaintext message body. The returned tuple is
    /// handed straight to `Cmd::SendMessage` by the driver; do not
    /// add intermediate logging or `Debug` printing.
    pub fn submit(&mut self, body: String) -> Option<(String, String)> {
        if !matches!(self.state, SendState::Editing) {
            return None;
        }
        if body.is_empty() {
            // Enter on an empty line — treat as cancel.
            return None;
        }
        self.body = body.clone();
        self.state = SendState::Sending;
        Some((self.recipient.clone(), body))
    }

    pub fn on_send_complete(&mut self, timestamp: u64) {
        if matches!(self.state, SendState::Sending) {
            self.state = SendState::Sent { timestamp };
        }
    }

    pub fn on_send_error(&mut self, reason: String) {
        if matches!(self.state, SendState::Sending) {
            self.state = SendState::Error(reason);
        }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = vec![
            String::new(),
            String::from("              Compose"),
            String::new(),
            format!("   To: {}", short(&self.recipient, 40)),
            String::new(),
        ];
        match &self.state {
            SendState::Editing => {
                out.push(String::from("   Type a message and press Enter to send."));
                out.push(String::from("   Empty line / Esc to cancel."));
            }
            SendState::Sending => {
                out.push(String::from("   Sending..."));
                out.push(String::new());
                out.push(format!("   > {}", short(&self.body, 40)));
            }
            SendState::Sent { timestamp } => {
                out.push(format!("   Sent at ts={}.", timestamp));
                out.push(String::new());
                out.push(format!("   > {}", short(&self.body, 40)));
                out.push(String::new());
                out.push(String::from("   Press Left to return."));
            }
            SendState::Error(reason) => {
                out.push(format!("   Send failed: {}", short(reason, 40)));
                out.push(String::new());
                out.push(format!("   > {}", short(&self.body, 40)));
                out.push(String::new());
                out.push(String::from("   Enter to retry, Esc to cancel."));
            }
        }
        out
    }

    /// Per-key handler. Hosted mode mostly uses the whole-line
    /// path via [`crate::stdin_ui::Ui::dispatch_line`]; this method
    /// only catches Esc / Left for back-out.
    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Esc => Transition::Pop,
            Key::Left => {
                // After a successful send / on the error screen,
                // Left returns to ConversationList. While editing,
                // Left is treated as cancel too (no per-char
                // editing in hosted mode).
                Transition::Pop
            }
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
