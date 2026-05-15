//! [`Screen`] and [`Transition`] enums.
//!
//! Every screen state is one variant of [`Screen`]. Most variants
//! are populated; the `Conversation` placeholder compiles but
//! renders a "not yet implemented" notice — kept in the enum so
//! the state graph is visible at one glance.
//!
//! [`Transition`] is what `Screen::handle_key` returns. It is the
//! only way to mutate the UI's screen stack — screens never see
//! the stack directly. This keeps the audit story clean: a screen
//! reads its own state and returns a transition; the [`crate::stdin_ui::Ui`]
//! driver applies it.

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
///
/// # Security
///
/// Several variants carry plaintext that crossed the libsignal
/// decrypt boundary: [`Self::ConversationList`] holds received
/// message bodies, [`Self::LinkShowUrl`] holds the provisioning
/// URL, [`Self::LinkDone`] holds the registration tuple, and
/// [`Self::Compose`] holds the in-flight outbound body. The
/// derived `Debug` impl renders these verbatim — avoid
/// `tracing::debug!(?screen)` on any value of this type.
#[derive(Debug, Clone)]
pub enum Screen {
    Splash(SplashScreen),
    Menu(MenuScreen),
    About(AboutScreen),

    /// Stub kept around for tests and as the bottom of the
    /// post-link stack before any messages arrive. The
    /// `LinkDone → ConversationList` transition replaces this so
    /// the receive loop can populate it.
    EmptyList(EmptyListScreen),

    LinkStarting(LinkStartingScreen),
    LinkShowUrl(LinkShowUrlScreen),
    LinkConfirming(LinkConfirmingScreen),
    LinkDone(LinkDoneScreen),
    LinkError(LinkErrorScreen),

    /// Renders the receive-status indicator and a flat list of
    /// received messages, latest-first.
    ConversationList(ConversationListScreen),

    /// The compose-and-send screen.
    Compose(ComposeScreen),

    /// Placeholder single-thread conversation view. Currently
    /// renders the "not yet implemented" notice; the active Compose
    /// flow is reachable directly from `ConversationList` via 'c'.
    Conversation,
}

/// What `Screen::handle_key` asks the driver to do next.
///
/// - [`Self::None`] keeps the current screen on top (possibly with
///   mutated state).
/// - [`Self::Push`] adds a new screen above the current one.
/// - [`Self::Pop`] removes the current screen, revealing whatever
///   was beneath. Popping past the splash re-inserts the splash so
///   the user is never stuck on an empty stack.
/// - [`Self::Replace`] swaps the top screen for a new one
///   (`Pop` + `Push` as one atomic transition).
/// - [`Self::Quit`] clears the stack and ends the UI loop.
#[derive(Debug, Clone)]
pub enum Transition {
    None,
    Push(Screen),
    Pop,
    Replace(Screen),
    Quit,
}

impl Screen {
    /// Render the screen as ASCII. Each line is a separate
    /// `String` so the renderer can deal with overflow uniformly.
    /// Width is approximately 50 chars; height is up to 22 lines
    /// including the status bar and hint footer the driver
    /// overlays.
    ///
    /// # Security
    ///
    /// For [`Screen::ConversationList`] / [`Screen::LinkShowUrl`] /
    /// [`Screen::Compose`] / [`Screen::LinkDone`] the returned
    /// strings contain plaintext PII or higher; the renderer in
    /// [`crate::stdin_ui::render`] writes them straight to stdout.
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

    /// Hint footer text. The driver renders this in a uniform
    /// band along the bottom; each screen knows what its own
    /// active keys are. Returning a single short line keeps the
    /// audit story for "what does this key do" at one read.
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

    /// Handle a key event and return a [`Transition`].
    ///
    /// Mutates `self` for in-screen state changes (moving menu
    /// focus, advancing the compose buffer); returns
    /// `Push`/`Pop`/`Replace`/`Quit` for stack changes. Screens may
    /// not send `Cmd`s directly — they signal intent through the
    /// returned transition, and the driver's
    /// [`crate::stdin_ui::Ui::on_screen_entered`] is the one place
    /// any screen → Cmd binding lives.
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
