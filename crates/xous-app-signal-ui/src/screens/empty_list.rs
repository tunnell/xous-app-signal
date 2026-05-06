//! Empty conversation list — UI.md §5.6.
//!
//! Shown after a successful link when the local store has no threads
//! yet. The hint footer collapses to one item ("Menu") since no
//! navigation is possible.

use crate::key::Key;
use crate::screen::{Screen, Transition};
use crate::screens::menu::MenuScreen;

#[derive(Debug, Clone, Default)]
pub struct EmptyListScreen;

impl EmptyListScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(10);
        out.push(String::new());
        out.push(String::new());
        out.push(String::new());
        out.push(String::from("              No conversations yet."));
        out.push(String::new());
        out.push(String::from("       Press Menu to start a new chat or"));
        out.push(String::from("       sync your contact list."));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            // Any printable letter / Home / Menu opens the app menu —
            // it's the only useful action.
            Key::Char('m') | Key::Home => Transition::Push(Screen::Menu(MenuScreen::new())),
            Key::Esc | Key::Char('q') => Transition::Quit,
            _ => Transition::None,
        }
    }
}
