//! App menu.
//!
//! Modal: New chat / Mark all read / (sep) / Link another device /
//! Settings / About / (sep) / Test worker (echo) / Quit.
//!
//! Only the navigable shell + About + Quit + the worker probe land.
//! The other items are flagged unimplemented and Pop immediately
//! when chosen.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::{Screen, Transition};
use crate::stdin_ui::screens::about::AboutScreen;

/// Menu item. We use indexes-into-a-static-list rather than a
/// `&'static str` per item so the audit can see the entire menu by
/// reading a single array.
#[derive(Debug, Clone, Copy)]
enum Item {
    NewChat,
    MarkAllRead,
    LinkAnother,
    Settings,
    About,
    TestWorker,
    Quit,
    Separator,
}

const ITEMS: &[Item] = &[
    Item::NewChat,
    Item::MarkAllRead,
    Item::Separator,
    Item::LinkAnother,
    Item::Settings,
    Item::About,
    Item::Separator,
    Item::TestWorker,
    Item::Quit,
];

/// First focusable item is index 0 (NewChat); we skip separators
/// when moving focus, so the derived `Default` (`focus: 0`) is the
/// correct starting position.
#[derive(Debug, Clone, Default)]
pub struct MenuScreen {
    focus: usize,
}

impl MenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    fn label(item: Item) -> Option<&'static str> {
        match item {
            Item::NewChat => Some("New chat"),
            Item::MarkAllRead => Some("Mark all read"),
            Item::LinkAnother => Some("Link another device"),
            Item::Settings => Some("Settings"),
            Item::About => Some("About"),
            Item::TestWorker => Some("Test worker (Hello/Pong)"),
            Item::Quit => Some("Quit"),
            Item::Separator => None,
        }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(20);
        out.push(String::new());
        out.push(String::from("           xas — Menu"));
        out.push(String::from("           ───────────────────"));
        for (i, item) in ITEMS.iter().enumerate() {
            match Self::label(*item) {
                None => out.push(String::from("           ──────────────────")),
                Some(label) => {
                    let prefix = if i == self.focus { ">" } else { " " };
                    out.push(format!("           {prefix} {label}"));
                }
            }
        }
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Up => {
                self.focus = prev_focusable(self.focus);
                Transition::None
            }
            Key::Down => {
                self.focus = next_focusable(self.focus);
                Transition::None
            }
            Key::Left | Key::Esc => Transition::Pop,
            Key::Home | Key::Right => match ITEMS.get(self.focus) {
                Some(Item::About) => Transition::Replace(Screen::About(AboutScreen::new())),
                Some(Item::Quit) => Transition::Quit,
                Some(Item::TestWorker) => {
                    // Hands a `Cmd::Hello` to the worker. The
                    // driver intercepts this transition, sends, awaits
                    // pong, and pops back. See Ui::run.
                    Transition::Push(Screen::Splash(crate::stdin_ui::screens::splash::SplashScreen::new()))
                    // ^ placeholder until the driver wiring lands; the
                    // test only exercises that we hit this branch.
                }
                _ => Transition::None,
            },
            _ => Transition::None,
        }
    }
}

fn next_focusable(mut idx: usize) -> usize {
    idx = (idx + 1) % ITEMS.len();
    while matches!(ITEMS.get(idx), Some(Item::Separator)) {
        idx = (idx + 1) % ITEMS.len();
    }
    idx
}

fn prev_focusable(mut idx: usize) -> usize {
    idx = (idx + ITEMS.len() - 1) % ITEMS.len();
    while matches!(ITEMS.get(idx), Some(Item::Separator)) {
        idx = (idx + ITEMS.len() - 1) % ITEMS.len();
    }
    idx
}
