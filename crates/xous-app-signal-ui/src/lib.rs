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
//! Implements the four MVP screens (Splash, Menu, About, EmptyList).
//! Subsequent work fills in `Screen::Link*` / `Conversation` /
//! `Compose` placeholders.

pub mod key;
pub mod render;
pub mod screen;
pub mod screens;

pub use key::Key;
pub use screen::{Screen, Transition};

use std::io::{self, BufRead, Write};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

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

    /// Hosted-mode loop. The main thread renders + handles events;
    /// a background thread reads stdin and forwards each line via
    /// `std::sync::mpsc`. The main loop polls both channels with a
    /// short timeout, redrawing whenever a worker `Event` arrives so
    /// the linking flow's state changes are visible without forcing
    /// the user to press a key.
    ///
    /// Earlier screens were synchronous on stdin alone; the
    /// two-channel poll matters once the linking flow lands.
    /// It is intentionally minimal: no termios, no escape-code
    /// parsing, no select primitive. A full keyboard binding
    /// (arrow-key escape sequences, printable chars in real-time)
    /// lives in the GAM renderer the on-device build uses; the
    /// hosted-mode loop just needs enough to drive integration tests.
    pub fn run(mut self) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        let (line_tx, line_rx) = std_mpsc::channel::<String>();

        // Stdin reader. Forwards lines until stdin EOFs, then drops
        // its sender; the main loop notices via Disconnected.
        thread::Builder::new()
            .name("xas-ui-stdin".into())
            .spawn(move || {
                let stdin = io::stdin();
                let lock = stdin.lock();
                for line in lock.lines() {
                    let Ok(line) = line else { return };
                    if line_tx.send(line).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| io::Error::other(format!("stdin reader thread spawn: {e}")))?;

        // Render once before the first poll so the user sees the
        // splash without having to press a key.
        let mut needs_render = true;

        loop {
            if self.stack.is_empty() {
                break;
            }

            if needs_render {
                let top = self.stack.last().expect("non-empty");
                let chips = "[OFF]"; // real conn-state will replace this.
                let body = top.render();
                let hint = top.hint();
                render::render_frame(&mut stdout, chips, &body, hint)?;
                needs_render = false;
            }

            // Drain any pending worker events. Each one may transition
            // the stack and forces a re-render before we go back to
            // waiting for input.
            let mut got_event = false;
            while let Ok(evt) = self.event_rx.try_recv() {
                writeln!(stdout, "[event] {evt:?}")?;
                self.handle_event(evt);
                got_event = true;
            }
            if got_event {
                needs_render = true;
                continue;
            }

            // Wait for stdin or an event with a short timeout.
            // 50ms is fast enough that worker events appear without
            // jitter, slow enough that we don't busy-spin.
            match line_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    // When the Compose screen is on top, the entire
                    // line of input is the message body (handed off
                    // via `dispatch_line`). Otherwise we treat the
                    // first char as a Key.
                    if matches!(self.stack.last(), Some(Screen::Compose(_))) {
                        // Strip CR/LF and dispatch as a body.
                        let body = line.trim_end_matches(['\r', '\n']).to_string();
                        // Esc / "esc" / "/cancel" return to ConversationList.
                        if body == "esc" || body == "/cancel" {
                            self.dispatch(Key::Esc);
                        } else {
                            self.dispatch_line(body);
                        }
                    } else {
                        let key = parse_key(&line);
                        self.dispatch(key);
                    }
                    needs_render = true;
                }
                Err(std_mpsc::RecvTimeoutError::Timeout) => {
                    // Loop and re-poll the event channel.
                }
                Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    // stdin EOF — graceful quit.
                    break;
                }
            }
        }

        // Final shutdown to the worker. Best-effort; if the channel
        // is closed the worker is already gone.
        let _ = self.cmd_tx.send_blocking(Cmd::Shutdown);
        Ok(())
    }

    /// Apply a worker `Event` to the screen stack. Acts on the
    /// link-flow events; other events are logged but ignored (they
    /// were emitted by side channels — `Event::Pong` etc. — and
    /// don't affect the user-visible screen).
    fn handle_event(&mut self, evt: Event) {
        use crate::screens::conversation_list::{MessageSummary, ReceiveStatus};
        use crate::screens::link::{LinkDoneScreen, LinkErrorScreen, LinkShowUrlScreen};
        match evt {
            Event::LinkUrl(url) => {
                if matches!(self.stack.last(), Some(Screen::LinkStarting(_))) {
                    self.apply(Transition::Replace(Screen::LinkShowUrl(
                        LinkShowUrlScreen::new(url),
                    )));
                } else {
                    // User navigated away. The link future runs to
                    // completion in the worker; we ignore the URL.
                }
            }
            Event::LinkComplete {
                device_name,
                aci,
                phone,
            } => {
                if matches!(
                    self.stack.last(),
                    Some(Screen::LinkStarting(_))
                        | Some(Screen::LinkShowUrl(_))
                        | Some(Screen::LinkConfirming(_))
                ) {
                    self.apply(Transition::Replace(Screen::LinkDone(LinkDoneScreen::new(
                        device_name,
                        aci,
                        phone,
                    ))));
                }
            }
            Event::LinkError(reason) => {
                if matches!(
                    self.stack.last(),
                    Some(Screen::LinkStarting(_))
                        | Some(Screen::LinkShowUrl(_))
                        | Some(Screen::LinkConfirming(_))
                ) {
                    self.apply(Transition::Replace(Screen::LinkError(
                        LinkErrorScreen::new(reason),
                    )));
                }
            }
            Event::ReceiveStarted => {
                if let Some(Screen::ConversationList(s)) = self.stack.last_mut() {
                    s.set_status(ReceiveStatus::Listening);
                }
            }
            Event::Message {
                sender,
                sender_phone: _,
                sender_name: _,
                body,
                timestamp,
            } => {
                if let Some(Screen::ConversationList(s)) = self.stack.last_mut() {
                    s.push_message(MessageSummary {
                        sender,
                        body,
                        timestamp,
                    });
                }
                // If the user is on a different screen (e.g. menu),
                // the message is dropped on the floor for MVP. The
                // PDDB store has already absorbed it through
                // presage's internals; a future iteration can
                // populate the screen from PDDB on re-entry.
            }
            Event::ReceiveError(reason) => {
                if let Some(Screen::ConversationList(s)) = self.stack.last_mut() {
                    s.set_status(ReceiveStatus::Error(reason));
                }
            }
            Event::SendComplete { timestamp } => {
                if let Some(Screen::Compose(c)) = self.stack.last_mut() {
                    c.on_send_complete(timestamp);
                }
            }
            Event::SendError(reason) => {
                if let Some(Screen::Compose(c)) = self.stack.last_mut() {
                    c.on_send_error(reason);
                }
            }

            // Pong / Whoami / ShuttingDown — no UI effect.
            _ => {}
        }
    }

    /// Visible-for-tests entry point. Apply a single key event to the
    /// current screen and return how many screens are on the stack
    /// after. Tests use this to assert on transitions without
    /// touching stdin/stdout. Side-effects (e.g. sending
    /// `Cmd::LinkDevice` when the user enters the LinkStarting screen)
    /// are dispatched here too so test invocations exercise the same
    /// path the live driver takes.
    pub fn dispatch(&mut self, key: Key) -> usize {
        let prev_top_id = top_id(self.stack.last());
        let transition = match self.stack.last_mut() {
            Some(top) => top.handle_key(key),
            None => return self.stack.len(),
        };
        self.apply(transition);
        if prev_top_id != top_id(self.stack.last()) {
            self.on_screen_entered();
        }
        self.stack.len()
    }

    /// Entry point for whole-line input. Currently used by
    /// the Compose screen — typing a message and pressing Enter
    /// sends the line as the message body. If the top screen isn't
    /// Compose, the line is silently dropped.
    pub fn dispatch_line(&mut self, line: String) -> usize {
        if let Some(Screen::Compose(c)) = self.stack.last_mut() {
            if let Some((recipient, body)) = c.submit(line) {
                let _ = self
                    .cmd_tx
                    .send_blocking(Cmd::SendMessage { recipient, body });
            }
        }
        self.stack.len()
    }

    /// Called by `dispatch` when the top screen changes. If the new
    /// top is a screen with a one-shot side-effect (currently:
    /// `LinkStarting` triggers `Cmd::LinkDevice`), emit the Cmd here.
    /// Audit story: every screen→Cmd binding lives in this one
    /// function.
    fn on_screen_entered(&self) {
        match self.stack.last() {
            Some(Screen::LinkStarting(_)) => {
                let _ = self.cmd_tx.send_blocking(Cmd::LinkDevice {
                    // Hosted mode hardcodes the device name.
                    // A future flow lets the user customise it
                    // via a text-input screen.
                    device_name: "Precursor".to_string(),
                });
            }
            Some(Screen::ConversationList(_)) => {
                // Starting receive is a side-effect of landing on
                // the conversation list. The worker moves the
                // linked Manager into a long-running task.
                let _ = self.cmd_tx.send_blocking(Cmd::StartReceive);
            }
            _ => {}
        }
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

/// Discriminant id for a `Screen`. Used by `dispatch` to detect
/// "top changed" without comparing whole `Screen` values (which
/// would require `PartialEq` on every payload).
fn top_id(s: Option<&Screen>) -> u8 {
    match s {
        None => 0,
        Some(Screen::Splash(_)) => 1,
        Some(Screen::Menu(_)) => 2,
        Some(Screen::About(_)) => 3,
        Some(Screen::EmptyList(_)) => 4,
        Some(Screen::LinkStarting(_)) => 5,
        Some(Screen::LinkShowUrl(_)) => 6,
        Some(Screen::LinkConfirming(_)) => 7,
        Some(Screen::LinkDone(_)) => 8,
        Some(Screen::LinkError(_)) => 9,
        Some(Screen::ConversationList(_)) => 10,
        Some(Screen::Compose(_)) => 11,
        Some(Screen::Conversation) => 12,
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

    // ----- link flow -----

    #[test]
    fn splash_link_pushes_link_starting_and_emits_cmd() {
        let (cmd_tx, cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        // Splash: Home selects "Link this device" (focus=0).
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::LinkStarting(_))));

        // Side-effect: Cmd::LinkDevice should be in the queue.
        match cmd_rx.try_recv() {
            Ok(Cmd::LinkDevice { device_name }) => assert_eq!(device_name, "Precursor"),
            other => panic!("expected LinkDevice cmd, got {other:?}"),
        }
    }

    #[test]
    fn link_url_event_replaces_link_starting() {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        // Push LinkStarting.
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::LinkStarting(_))));

        // Worker emits LinkUrl.
        ui.handle_event(Event::LinkUrl(
            "tsdevice://?uuid=deadbeef&pubkey=cafebabe".to_string(),
        ));
        match ui.top() {
            Some(Screen::LinkShowUrl(s)) => {
                assert!(s.url.starts_with("tsdevice://"));
            }
            other => panic!("expected LinkShowUrl, got {other:?}"),
        }
    }

    #[test]
    fn link_complete_event_replaces_with_done() {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        ui.dispatch(Key::Home);
        ui.handle_event(Event::LinkUrl("tsdevice://test".to_string()));
        ui.handle_event(Event::LinkComplete {
            device_name: "Precursor".to_string(),
            aci: "00000000-0000-4000-8000-000000000001".to_string(),
            phone: "+15555550100".to_string(),
        });
        match ui.top() {
            Some(Screen::LinkDone(s)) => {
                assert_eq!(s.device_name, "Precursor");
                assert!(s.phone.starts_with("+1"));
            }
            other => panic!("expected LinkDone, got {other:?}"),
        }
    }

    #[test]
    fn link_error_event_replaces_with_error() {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        ui.dispatch(Key::Home);
        ui.handle_event(Event::LinkError("network unreachable".to_string()));
        match ui.top() {
            Some(Screen::LinkError(s)) => assert_eq!(s.reason, "network unreachable"),
            other => panic!("expected LinkError, got {other:?}"),
        }
    }

    #[test]
    fn link_done_home_transitions_to_conversation_list() {
        let (cmd_tx, cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        // Run through the link flow so we land on LinkDone.
        ui.dispatch(Key::Home); // Splash -> LinkStarting (cmd: LinkDevice)
        ui.handle_event(Event::LinkComplete {
            device_name: "P".into(),
            aci: "x".into(),
            phone: "+1".into(),
        });
        // Drain the LinkDevice cmd from the queue.
        let _ = cmd_rx.try_recv();

        ui.dispatch(Key::Home); // LinkDone -> ConversationList (cmd: StartReceive)
        assert!(matches!(ui.top(), Some(Screen::ConversationList(_))));

        // Side-effect: Cmd::StartReceive should be in the queue.
        match cmd_rx.try_recv() {
            Ok(Cmd::StartReceive) => {}
            other => panic!("expected StartReceive cmd, got {other:?}"),
        }
    }

    #[test]
    fn link_url_event_ignored_if_user_navigated_away() {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        // Enter link flow, then back out (user pressed Cancel).
        ui.dispatch(Key::Home);
        assert!(matches!(ui.top(), Some(Screen::LinkStarting(_))));
        ui.dispatch(Key::Left);
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));

        // Stale event arrives. Should be a no-op — UI is on splash.
        ui.handle_event(Event::LinkUrl("tsdevice://stale".to_string()));
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));
    }

    // ----- ConversationList -----

    use crate::screens::conversation_list::{
        ConversationListScreen, MessageSummary, ReceiveStatus,
    };

    /// Helper: build a UI parked on the ConversationList screen with
    /// the LinkStarting/StartReceive cmds already drained.
    fn ui_on_conversation_list() -> (Ui, async_channel::Receiver<Cmd>) {
        let (cmd_tx, cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);
        ui.dispatch(Key::Home);
        ui.handle_event(Event::LinkComplete {
            device_name: "P".into(),
            aci: "x".into(),
            phone: "+1".into(),
        });
        ui.dispatch(Key::Home); // -> ConversationList (sends StartReceive)
        // Drain LinkDevice + StartReceive.
        while cmd_rx.try_recv().is_ok() {}
        (ui, cmd_rx)
    }

    #[test]
    fn conversation_list_starts_in_starting_status() {
        let (ui, _cmd_rx) = ui_on_conversation_list();
        match ui.top() {
            Some(Screen::ConversationList(s)) => {
                assert!(matches!(s.status, ReceiveStatus::Starting));
                assert!(s.messages.is_empty());
            }
            other => panic!("expected ConversationList, got {other:?}"),
        }
    }

    #[test]
    fn receive_started_event_sets_listening_status() {
        let (mut ui, _cmd_rx) = ui_on_conversation_list();
        ui.handle_event(Event::ReceiveStarted);
        match ui.top() {
            Some(Screen::ConversationList(s)) => {
                assert!(matches!(s.status, ReceiveStatus::Listening));
            }
            other => panic!("expected ConversationList, got {other:?}"),
        }
    }

    #[test]
    fn message_event_appends_to_list() {
        let (mut ui, _cmd_rx) = ui_on_conversation_list();
        ui.handle_event(Event::Message {
            sender_phone: None,
            sender_name: None,
            sender: "alice".into(),
            body: "hi from alice".into(),
            timestamp: 1000,
        });
        ui.handle_event(Event::Message {
            sender_phone: None,
            sender_name: None,
            sender: "bob".into(),
            body: "hi from bob".into(),
            timestamp: 1100,
        });
        match ui.top() {
            Some(Screen::ConversationList(s)) => {
                assert_eq!(s.messages.len(), 2);
                assert_eq!(s.messages[0].sender, "alice");
                assert_eq!(s.messages[1].body, "hi from bob");
            }
            other => panic!("expected ConversationList, got {other:?}"),
        }
    }

    #[test]
    fn receive_error_event_sets_error_status() {
        let (mut ui, _cmd_rx) = ui_on_conversation_list();
        ui.handle_event(Event::ReceiveError("WS closed".into()));
        match ui.top() {
            Some(Screen::ConversationList(s)) => match &s.status {
                ReceiveStatus::Error(reason) => assert_eq!(reason, "WS closed"),
                other => panic!("expected Error, got {other:?}"),
            },
            other => panic!("expected ConversationList, got {other:?}"),
        }
    }

    #[test]
    fn message_event_ignored_when_not_on_conversation_list() {
        let (cmd_tx, _cmd_rx) = bounded::<Cmd>(4);
        let (_event_tx, event_rx) = bounded::<Event>(4);
        let mut ui = Ui::new(cmd_tx, event_rx);

        // Still on Splash. Message arrives — should be dropped.
        ui.handle_event(Event::Message {
            sender_phone: None,
            sender_name: None,
            sender: "carol".into(),
            body: "noise".into(),
            timestamp: 0,
        });
        assert!(matches!(ui.top(), Some(Screen::Splash(_))));
    }

    #[test]
    fn message_list_caps_at_max_visible_times_two() {
        let mut s = ConversationListScreen::new();
        for i in 0..50 {
            s.push_message(MessageSummary {
                sender: format!("u{i}"),
                body: format!("m{i}"),
                timestamp: i,
            });
        }
        // MAX_VISIBLE = 8; cap = 16. Latest 16 messages retained;
        // the oldest 34 dropped.
        assert_eq!(s.messages.len(), 16);
        assert_eq!(s.messages.first().unwrap().sender, "u34");
        assert_eq!(s.messages.last().unwrap().sender, "u49");
    }

    // ----- Compose -----

    use crate::screens::compose::SendState;

    /// Helper: park UI on a populated ConversationList, then push
    /// Compose. Drains all queued cmds so caller sees only
    /// post-helper traffic.
    fn ui_on_compose() -> (Ui, async_channel::Receiver<Cmd>) {
        let (mut ui, cmd_rx) = ui_on_conversation_list();
        // Inject one received message so 'c' has a recipient.
        ui.handle_event(Event::Message {
            sender_phone: None,
            sender_name: None,
            sender: "00000000-0000-4000-8000-000000000abc".into(),
            body: "hi".into(),
            timestamp: 1000,
        });
        ui.dispatch(Key::Char('c'));
        // Drain.
        while cmd_rx.try_recv().is_ok() {}
        (ui, cmd_rx)
    }

    #[test]
    fn conversation_list_c_with_no_messages_is_noop() {
        let (mut ui, _cmd_rx) = ui_on_conversation_list();
        ui.dispatch(Key::Char('c'));
        // Still on ConversationList — no Compose pushed.
        assert!(matches!(ui.top(), Some(Screen::ConversationList(_))));
    }

    #[test]
    fn conversation_list_c_with_messages_pushes_compose() {
        let (ui, _cmd_rx) = ui_on_compose();
        match ui.top() {
            Some(Screen::Compose(c)) => {
                assert_eq!(c.recipient, "00000000-0000-4000-8000-000000000abc");
                assert!(matches!(c.state, SendState::Editing));
            }
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_line_emits_send_message_cmd() {
        let (mut ui, cmd_rx) = ui_on_compose();
        ui.dispatch_line("hello world".to_string());
        match cmd_rx.try_recv() {
            Ok(Cmd::SendMessage { recipient, body }) => {
                assert_eq!(recipient, "00000000-0000-4000-8000-000000000abc");
                assert_eq!(body, "hello world");
            }
            other => panic!("expected SendMessage, got {other:?}"),
        }
        // State should have advanced to Sending.
        match ui.top() {
            Some(Screen::Compose(c)) => assert!(matches!(c.state, SendState::Sending)),
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn empty_dispatch_line_does_not_send() {
        let (mut ui, cmd_rx) = ui_on_compose();
        ui.dispatch_line(String::new());
        // No cmd emitted.
        assert!(cmd_rx.try_recv().is_err());
        // State unchanged.
        match ui.top() {
            Some(Screen::Compose(c)) => assert!(matches!(c.state, SendState::Editing)),
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn send_complete_event_advances_compose() {
        let (mut ui, _cmd_rx) = ui_on_compose();
        ui.dispatch_line("hi".to_string());
        ui.handle_event(Event::SendComplete { timestamp: 1234 });
        match ui.top() {
            Some(Screen::Compose(c)) => match &c.state {
                SendState::Sent { timestamp } => assert_eq!(*timestamp, 1234),
                other => panic!("expected Sent, got {other:?}"),
            },
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn send_error_event_advances_compose_to_error() {
        let (mut ui, _cmd_rx) = ui_on_compose();
        ui.dispatch_line("hi".to_string());
        ui.handle_event(Event::SendError("recipient unknown".to_string()));
        match ui.top() {
            Some(Screen::Compose(c)) => match &c.state {
                SendState::Error(reason) => assert_eq!(reason, "recipient unknown"),
                other => panic!("expected Error, got {other:?}"),
            },
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn compose_esc_pops_to_conversation_list() {
        let (mut ui, _cmd_rx) = ui_on_compose();
        ui.dispatch(Key::Esc);
        assert!(matches!(ui.top(), Some(Screen::ConversationList(_))));
    }
}
