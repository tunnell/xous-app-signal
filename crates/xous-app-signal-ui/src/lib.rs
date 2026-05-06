//! UI layer for `xas`.
//!
//! Holds the screen stack, runs the input loop, and (in hosted mode)
//! renders to stdout. The `Ui` struct's API is just `new` + `run`; all
//! state lives behind it. Screens never see the cmd/event channels
//! directly — the driver decides when a `Cmd::Hello` is sent and when
//! an `Event::Pong` updates which screen is on top.
//!
//! Audit-friendly invariants:
//!
//! - One screen on top at a time. The stack only grows via
//!   `Transition::Push` and only shrinks via `Transition::Pop` /
//!   `Replace`.
//! - All input goes through `Screen::handle_key` (one method, one
//!   match). No screen has a side channel into the worker — only the
//!   driver does.
//! - Render is `Vec<String>`-shaped. The hosted renderer wraps it in
//!   a box; a future GAM renderer wraps it in `TextView`s. No screen
//!   knows which.
//!
//! Stage 9c implements the four MVP screens (Splash, Menu, About,
//! EmptyList). Stages 10-12 fill in `Screen::Link*` / `Conversation` /
//! `Compose` placeholders.

pub mod key;
pub mod render;
pub mod screen;
pub mod screens;

pub use key::Key;
pub use screen::{Screen, Transition};

use std::io::{self, BufRead, Write};

use async_channel::{Receiver, Sender};
use xous_signal_bridge::{Cmd, Event};

/// Driver. Owns the screen stack and routes input/events.
#[derive(Debug)]
pub struct Ui {
    stack: Vec<Screen>,
    cmd_tx: Sender<Cmd>,
    event_rx: Receiver<Event>,
}

impl Ui {
    /// Build the driver with the splash screen on top.
    pub fn new(cmd_tx: Sender<Cmd>, event_rx: Receiver<Event>) -> Self {
        Self {
            stack: vec![Screen::Splash(screens::splash::SplashScreen::new())],
            cmd_tx,
            event_rx,
        }
    }

    /// Hosted-mode loop. Reads stdin a line at a time; treats the
    /// first character of each line as the key, with bare empty
    /// lines mapping to `Home`. Exits on `Transition::Quit`.
    ///
    /// This is intentionally minimal — no termios, no escape-code
    /// parsing. A full keyboard binding (arrow-key escape sequences,
    /// printable chars in real-time) lives in the GAM renderer the
    /// on-device build uses; the hosted-mode loop just needs enough
    /// to drive integration tests and human-readable smoke runs.
    pub fn run(mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut stdin_lock = stdin.lock();
        let mut stdout = io::stdout().lock();

        while let Some(top) = self.stack.last() {
            let chips = "[OFF]"; // Stage 10+ wires real conn-state here.
            let body = top.render();
            let hint = top.hint();
            render::render_frame(&mut stdout, chips, &body, hint)?;

            // Read one line of input. EOF (Ctrl-D / closed pipe) =
            // graceful quit, same shape as Transition::Quit.
            let mut line = String::new();
            match stdin_lock.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(e),
            }

            // Drain any pending events from the worker before handling
            // input. Stage 9c doesn't act on them — they're just
            // logged so the human running the binary can see the
            // worker is alive.
            while let Ok(evt) = self.event_rx.try_recv() {
                writeln!(stdout, "[event] {evt:?}")?;
            }

            let key = parse_key(&line);
            self.dispatch(key);
        }

        // Final shutdown to the worker. Best-effort; if the channel
        // is closed the worker is already gone.
        let _ = self.cmd_tx.send_blocking(Cmd::Shutdown);
        Ok(())
    }

    /// Visible-for-tests entry point. Apply a single key event to the
    /// current screen and return how many screens are on the stack
    /// after. Tests use this to assert on transitions without
    /// touching stdin/stdout.
    pub fn dispatch(&mut self, key: Key) -> usize {
        if let Some(top) = self.stack.last_mut() {
            let transition = top.handle_key(key);
            self.apply(transition);
        }
        self.stack.len()
    }

    /// Visible-for-tests: peek the top screen's discriminant.
    pub fn top(&self) -> Option<&Screen> {
        self.stack.last()
    }

    fn apply(&mut self, transition: Transition) {
        match transition {
            Transition::None => {}
            Transition::Push(s) => self.stack.push(s),
            Transition::Pop => {
                self.stack.pop();
                if self.stack.is_empty() {
                    // Always keep the splash at the bottom; popping
                    // past it shouldn't quit the app.
                    self.stack
                        .push(Screen::Splash(screens::splash::SplashScreen::new()));
                }
            }
            Transition::Replace(s) => {
                self.stack.pop();
                self.stack.push(s);
            }
            Transition::Quit => self.stack.clear(),
        }
    }
}

/// Translate a stdin-line into a `Key`. Conventions: bare `\n` =
/// Home; first char of any other input is the key; `j`/`k`/arrow-
/// names spelled out (`up`/`down`/`left`/`right`) work too for
/// scriptability.
fn parse_key(line: &str) -> Key {
    let trimmed = line.trim_end_matches('\n');
    if trimmed.is_empty() {
        return Key::Home;
    }
    match trimmed {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "esc" => Key::Esc,
        "home" => Key::Home,
        _ => match trimmed.chars().next().unwrap_or(' ') {
            'j' => Key::Down,
            'k' => Key::Up,
            'h' => Key::Left,
            'l' => Key::Right,
            c => Key::Char(c),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_channel::bounded;

    fn fresh_ui() -> Ui {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(1);
        let (_event_tx, event_rx) = bounded::<Event>(1);
        Ui::new(cmd_tx, event_rx)
    }

    #[test]
    fn starts_on_splash() {
        let ui = fresh_ui();
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));
    }

    #[test]
    fn splash_down_then_select_about() {
        let mut ui = fresh_ui();
        ui.dispatch(Key::Down); // Register (greyed)
        ui.dispatch(Key::Down); // About
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::About(_))));
    }

    #[test]
    fn about_back_pops_to_splash() {
        let mut ui = fresh_ui();
        ui.dispatch(Key::Down);
        ui.dispatch(Key::Down);
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::About(_))));
        ui.dispatch(Key::Left);
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));
    }

    #[test]
    fn splash_q_quits() {
        let mut ui = fresh_ui();
        let depth = ui.dispatch(Key::Char('q'));
        // Stack cleared — no top screen anymore.
        assert_eq!(depth, 0);
        assert!(ui.top().is_none());
    }

    #[test]
    fn menu_about_replaces_top() {
        let mut ui = fresh_ui();
        ui.dispatch(Key::Char('m')); // open menu from splash
        assert!(matches!(ui.top(), Some(Screen::Menu(_))));
        // Navigate to About: NewChat → MarkAllRead → LinkAnother →
        // Settings → About (4 Downs from index 0).
        for _ in 0..4 {
            ui.dispatch(Key::Down);
        }
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::About(_))));
    }

    #[test]
    fn menu_left_pops() {
        let mut ui = fresh_ui();
        ui.dispatch(Key::Char('m'));
        assert!(matches!(ui.top(), Some(Screen::Menu(_))));
        ui.dispatch(Key::Left);
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));
    }

    #[test]
    fn empty_list_menu_key_pushes_menu() {
        let mut ui = fresh_ui();
        // Replace the splash with the empty-list screen for the test.
        ui.stack.push(Screen::EmptyList(
            screens::empty_list::EmptyListScreen::new(),
        ));
        ui.dispatch(Key::Char('m'));
        assert!(matches!(ui.top(), Some(Screen::Menu(_))));
    }

    #[test]
    fn parse_key_basics() {
        assert_eq!(parse_key("\n"), Key::Home);
        assert_eq!(parse_key("up\n"), Key::Up);
        assert_eq!(parse_key("j\n"), Key::Down);
        assert_eq!(parse_key("k\n"), Key::Up);
        assert_eq!(parse_key("q\n"), Key::Char('q'));
    }
}
