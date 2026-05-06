//! Hardware-agnostic key event vocabulary.
//!
//! Every screen in the UI handles input through this enum. The hosted
//! TTY renderer (`render::TextSurface`) translates stdin bytes into
//! these; a future GAM-side input reader will translate Xous keyboard
//! events to the same enum. Keeping the abstraction here means the
//! screens never see platform-specific keycodes.

/// One key event. We preserve enough structure that screens can
/// disambiguate `Char(' ')` from `Home`, `Char('q')` from `Quit`, etc.
/// Naming follows what the user *types* (Up/Down/Left/Right) rather
/// than what the underlying keycode is, so a future Xous keyboard
/// remapping doesn't ripple into screen logic.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    /// Precursor's center "Home" select key. Also stdin `\n`.
    Home,
    Esc,
    Char(char),
    /// `Shift+Up` / `Shift+Down` for page-up / page-down. Hosted-mode
    /// stdin doesn't naturally produce these; reserved for the GAM-side
    /// input reader and for direct test invocations.
    ShiftUp,
    ShiftDown,
}
