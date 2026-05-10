//! `Screen` and `Transition` enums.
//!
//! Every screen state is one variant of `Screen`. Some MVP variants
//! (`Splash`, `Menu`, `About`, `EmptyList`) are populated; later
//! variants are placeholders that compile but render a "not
//! implemented" notice — we keep them in the enum so the state
//! graph is visible at one glance.
//!
//! `Transition` is what `Screen::handle_key` returns. It's the only
//! way to mutate the UI's screen stack — screens never see the stack
//! directly. This keeps the audit story clean: a screen reads its own
//! state, returns a transition; the `Ui` driver applies the transition.

use crate::stdin_ui::screens::{
    about::AboutScreen,
    compose::ComposeScreen,
    conversation_list::ConversationListScreen,
    empty_list::EmptyListScreen,
    link::{
        LinkConfirmingScreen, LinkDoneScreen, LinkErrorScreen, LinkShowUrlScreen,
        LinkStartingScreen,
    },
    menu::MenuScreen,
    splash::SplashScreen,
};

/// One screen state.
#[derive(Debug, Clone)]
pub enum Screen {
    Splash(SplashScreen),
    Menu(MenuScreen),
    About(AboutScreen),

    /// Stub kept around for tests and as the bottom of the
    /// post-link stack before any messages arrive. The
    /// `LinkDone -> Home` transition replaces this with
    /// `ConversationList` so the receive loop can populate it.
    EmptyList(EmptyListScreen),

    LinkStarting(LinkStartingScreen),
    LinkShowUrl(LinkShowUrlScreen),
    LinkConfirming(LinkConfirmingScreen),
    LinkDone(LinkDoneScreen),
    LinkError(LinkErrorScreen),

    /// Renders the receive-status indicator
    /// and a flat list of received messages, latest-first.
    ConversationList(ConversationListScreen),

    /// The compose-and-send screen.
    Compose(ComposeScreen),

    /// Future single-thread conversation view. The MVP doesn't
    /// render this yet; the Compose flow is reachable directly
    /// from ConversationList via `'c'`.
    Conversation,
}

/// What `Screen::handle_key` asks the driver to do next.
///
/// `None` keeps the current screen on top, possibly with mutated state.
/// `Push` adds a new screen; `Pop` removes the current one (revealing
/// whatever was beneath); `Replace` swaps the top screen for a new one.
/// `Quit` terminates the UI loop.
#[derive(Debug, Clone)]
pub enum Transition {
    None,
    Push(Screen),
    Pop,
    Replace(Screen),
    Quit,
}

impl Screen {
    /// Render the screen as ASCII. Each line is a separate `String` so
    /// the renderer can deal with overflow uniformly. Width is
    /// approximately 50 chars; height is up to 22 lines including the
    /// status bar and hint footer (which the driver overlays).
    pub fn render(&self) -> Vec<String> {
        match self {
            Screen::Splash(s) => s.render(),
            Screen::Menu(m) => m.render(),
            Screen::About(a) => a.render(),
            Screen::EmptyList(e) => e.render(),
            Screen::LinkStarting(s) => s.render(),
            Screen::LinkShowUrl(s) => s.render(),
            Screen::LinkConfirming(s) => s.render(),
            Screen::LinkDone(s) => s.render(),
            Screen::LinkError(s) => s.render(),
            Screen::ConversationList(s) => s.render(),
            Screen::Compose(s) => s.render(),

            // Per-thread conversation view — future work.
            Screen::Conversation => placeholder("Conversation", "TBD"),
        }
    }

    /// Hint footer text. The driver renders this in a uniform 20-px
    /// band along the bottom; each screen knows what its own active
    /// keys are. Returning a single short line keeps the audit
    /// story for "what does this key do" at one read.
    pub fn hint(&self) -> &'static str {
        match self {
            Screen::Splash(_) => "Up/Down Select   Home Choose",
            Screen::Menu(_) => "Up/Down Select   Home Choose   Left Close",
            Screen::About(_) => "Left Back",
            Screen::EmptyList(_) => "Menu",
            Screen::LinkStarting(_) | Screen::LinkShowUrl(_) | Screen::LinkConfirming(_) => {
                "Left Cancel"
            }
            Screen::LinkDone(_) => "Home Continue",
            Screen::LinkError(_) => "Up/Down Select   Home Choose",
            Screen::ConversationList(_) => "c Compose   Menu   q Quit",
            Screen::Compose(_) => "Type + Enter Send   Esc Cancel",
            Screen::Conversation => "Up/Down Scroll   Left Back   Home Reply",
        }
    }

    /// Handle a key event; return a `Transition` describing what the
    /// driver should do next. Mutates `self` for in-screen state
    /// changes (e.g. moving menu focus); returns `Push`/`Pop`/etc. for
    /// stack changes.
    pub fn handle_key(&mut self, key: crate::stdin_ui::key::Key) -> Transition {
        match self {
            Screen::Splash(s) => s.handle_key(key),
            Screen::Menu(m) => m.handle_key(key),
            Screen::About(a) => a.handle_key(key),
            Screen::EmptyList(e) => e.handle_key(key),
            Screen::LinkStarting(s) => s.handle_key(key),
            Screen::LinkShowUrl(s) => s.handle_key(key),
            Screen::LinkConfirming(s) => s.handle_key(key),
            Screen::LinkDone(s) => s.handle_key(key),
            Screen::LinkError(s) => s.handle_key(key),
            Screen::ConversationList(s) => s.handle_key(key),
            Screen::Compose(s) => s.handle_key(key),

            // Placeholder: any key returns to the previous screen.
            _ => Transition::Pop,
        }
    }
}

fn placeholder(title: &str, note: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(8);
    out.push(String::new());
    out.push(format!("    {title}"));
    out.push(String::new());
    out.push(format!("    [not yet implemented — {note}]"));
    out.push(String::new());
    out.push(String::from("    Press any key to go back."));
    out
}
