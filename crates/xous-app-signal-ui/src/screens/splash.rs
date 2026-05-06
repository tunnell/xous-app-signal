//! Splash / first-run screen — UI.md §5.1.
//!
//! Four menu items: Link this device / Register a phone number (greyed)
//! / About / Quit. Up/Down moves focus, Home selects.

use crate::key::Key;
use crate::screen::{Screen, Transition};
use crate::screens::{about::AboutScreen, menu::MenuScreen};

/// 0 = Link, 1 = Register (deferred), 2 = About, 3 = Quit. `Default`
/// starts focused on Link, which is what users hit first on a fresh
/// install.
#[derive(Debug, Clone, Default)]
pub struct SplashScreen {
    focus: usize,
}

impl SplashScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&self) -> Vec<String> {
        const ITEMS: [(&str, bool); 4] = [
            ("Link this device", true),
            ("Register a phone number", false), // greyed; Stage 13+
            ("About", true),
            ("Quit", true),
        ];

        let mut out = vec![
            String::new(),
            String::new(),
            centered("xas", 50),
            String::new(),
            centered("Signal client for Precursor", 50),
            String::new(),
            centered("Not yet linked.", 50),
            String::new(),
        ];
        for (i, (label, enabled)) in ITEMS.iter().enumerate() {
            let prefix = if i == self.focus { ">" } else { " " };
            let line = if *enabled {
                format!("        {prefix} {label}")
            } else {
                format!("        {prefix} [{label}]")
            };
            out.push(line);
        }
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
                if self.focus < 3 {
                    self.focus += 1;
                }
                Transition::None
            }
            Key::Home | Key::Right => match self.focus {
                // Stage 10: Push LinkStarting; the driver issues
                // `Cmd::LinkDevice` and replaces the screen with
                // LinkShowUrl when the worker emits `Event::LinkUrl`.
                0 => Transition::Push(Screen::LinkStarting(
                    crate::screens::link::LinkStartingScreen::new(),
                )),
                1 => Transition::None, // Register: greyed, no-op
                2 => Transition::Push(Screen::About(AboutScreen::new())),
                3 => Transition::Quit,
                _ => Transition::None,
            },
            Key::Char('q') => Transition::Quit,
            // Pressing the menu key from the splash is unusual but
            // useful — it routes to the same global menu the
            // populated-list screen uses.
            Key::Char('m') => Transition::Push(Screen::Menu(MenuScreen::new())),
            _ => Transition::None,
        }
    }
}

fn centered(s: &str, width: usize) -> String {
    if s.len() >= width {
        return s.to_string();
    }
    let pad = (width - s.len()) / 2;
    format!("{}{s}", " ".repeat(pad))
}
