//! Compose screen — UI.md §5.9.
//!
//! One per send. Holds the recipient (set on construction from the
//! most-recent received message's sender, or could be entered
//! manually in a future stage), an editable body, and a
//! `SendState` that the driver flips on `Event::SendComplete` /
//! `Event::SendError`.
//!
//! Hosted-mode input model: instead of per-char keystrokes (which
//! would need raw-mode terminal handling we deliberately avoid —
//! see `docs/UI.md` §9), the driver hands the screen the WHOLE
//! input line as `handle_line`. Type the message + Enter, the
//! line becomes the body and a `Cmd::SendMessage` fires. On-device
//! On-device GAM mode will switch to the per-char `handle_key` path
//! with backspace/cursor; the `SendState` and event handling stay
//! the same.

use crate::key::Key;
use crate::screen::Transition;

#[derive(Debug, Clone)]
pub struct ComposeScreen {
    /// Recipient `service_id_string()` — typically the most-recent
    /// sender's UUID, populated by ConversationList when pushing
    /// this screen.
    pub recipient: String,
    /// What the screen is currently doing. Determines what input
    /// the user can give and what the screen renders.
    pub state: SendState,
    /// The body the user typed, captured the moment they hit Enter.
    /// Only populated once per screen lifetime; a second Enter is a
    /// no-op until the SendComplete/Error event arrives.
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

    /// Mark that the worker is processing the send. Returns
    /// `(recipient, body)` so the driver can emit the actual
    /// `Cmd::SendMessage` with the captured payload.
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

    /// Per-key handler. Hosted mode mostly uses `handle_line`;
    /// `handle_key` only catches Esc / Left for back-out, and for
    /// the SendState::Error retry path on the first character.
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
