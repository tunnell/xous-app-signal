//! `Screen` and `Transition` enums.
//!
//! Every screen state is one variant of `Screen`. Stage 9c implements
//! the four MVP variants (`Splash`, `Menu`, `About`, `EmptyList`); the
//! Stage 10/11/12 variants are placeholders that compile but render a
//! "not implemented" notice — we keep them in the enum so the state
//! graph is visible at one glance.
//!
//! `Transition` is what `Screen::handle_key` returns. It's the only
//! way to mutate the UI's screen stack — screens never see the stack
//! directly. This keeps the audit story clean: a screen reads its own
//! state, returns a transition; the `Ui` driver applies the transition.

use crate::screens::{
    about::AboutScreen, empty_list::EmptyListScreen, menu::MenuScreen, splash::SplashScreen,
};

/// One screen state.
#[derive(Debug, Clone)]
pub enum Screen {
    Splash(SplashScreen),
    Menu(MenuScreen),
    About(AboutScreen),
    EmptyList(EmptyListScreen),

    // --- Stage 10/11/12 placeholders ---
    // Each renders a "not yet implemented" notice when reached. Keeping
    // them in the enum makes the state graph visible. Add real fields
    // (URL string, message list, etc.) as the corresponding stages land.
    LinkShowUrl,
    LinkConfirming,
    LinkDone,
    LinkError(String),
    Conversation,
    Compose,
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

            // Placeholder screens — Stage 10/11/12 fill these in.
            Screen::LinkShowUrl => placeholder("Link — show URL", "Stage 10"),
            Screen::LinkConfirming => placeholder("Link — confirming", "Stage 10"),
            Screen::LinkDone => placeholder("Link — done", "Stage 10"),
            Screen::LinkError(msg) => placeholder("Link — error", msg),
            Screen::Conversation => placeholder("Conversation", "Stage 11"),
            Screen::Compose => placeholder("Compose", "Stage 12"),
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
            Screen::LinkShowUrl | Screen::LinkConfirming => "Home OK   Left Cancel",
            Screen::LinkDone => "Home Continue",
            Screen::LinkError(_) => "Home Retry   Left Cancel",
            Screen::Conversation => "Up/Down Scroll   Left Back   Home Reply",
            Screen::Compose => "Home Send   Left Back   Esc Discard",
        }
    }

    /// Handle a key event; return a `Transition` describing what the
    /// driver should do next. Mutates `self` for in-screen state
    /// changes (e.g. moving menu focus); returns `Push`/`Pop`/etc. for
    /// stack changes.
    pub fn handle_key(&mut self, key: crate::key::Key) -> Transition {
        match self {
            Screen::Splash(s) => s.handle_key(key),
            Screen::Menu(m) => m.handle_key(key),
            Screen::About(a) => a.handle_key(key),
            Screen::EmptyList(e) => e.handle_key(key),

            // Placeholders: any key returns to the previous screen.
            // Stage 10/11/12 replace these with proper state machines.
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
