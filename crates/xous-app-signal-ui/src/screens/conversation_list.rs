//! Conversation list — UI.md §5.6 (empty) and §5.7 (populated).
//!
//! Stage 11 minimum: replaces the Stage 9c `EmptyListScreen`. On
//! entry the driver sends `Cmd::StartReceive`. The screen's status
//! tracks whether the receive loop is active; received messages are
//! appended to a flat `Vec<MessageSummary>` (Stage 11+ may group by
//! thread per UI.md §5.7's pinned/unpinned layout, but for MVP a
//! single chronological list is enough).

use crate::key::Key;
use crate::screen::{Screen, Transition};
use crate::screens::menu::MenuScreen;

/// One received message, flattened for display. Mirrors the
/// `Event::Message` payload from `xous-signal-bridge`.
#[derive(Debug, Clone)]
pub struct MessageSummary {
    pub sender: String,
    pub body: String,
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
/// the visible area; for Stage 11 MVP we don't need pagination.
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
    /// past the visible area without unbounded growth. Stage 11+
    /// will move this to PDDB-backed persistence and drop the
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
            // Empty state — UI.md §5.6.
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
            // Stage 12: 'c' opens the Compose screen, prefilled with
            // the most-recent sender as recipient. If there are no
            // messages yet (`messages.is_empty()`), Compose can't
            // resolve a recipient — drop the keypress on the floor.
            // Future stage may surface a contact-picker instead.
            Key::Char('c') => {
                if let Some(latest) = self.messages.last() {
                    Transition::Push(Screen::Compose(
                        crate::screens::compose::ComposeScreen::new(latest.sender.clone()),
                    ))
                } else {
                    Transition::None
                }
            }
            // Press M (or Home with no message focus) to open the
            // app menu — gives access to "Test worker", quit, etc.
            Key::Char('m') | Key::Home => Transition::Push(Screen::Menu(MenuScreen::new())),
            // Esc / q quits — same shape as the Stage 9c empty list.
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
