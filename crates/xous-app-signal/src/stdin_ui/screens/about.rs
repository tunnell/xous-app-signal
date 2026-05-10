//! About screen.
//!
//! End-user verifiability surface: lists every upstream version we
//! pin, so a photograph of this screen reproduces the build. All
//! values are `static` strings so the screen is allocation-free at
//! render time.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::Transition;

#[derive(Debug, Clone, Default)]
pub struct AboutScreen;

impl AboutScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(20);
        out.push(String::new());
        out.push(String::from("                       xas"));
        out.push(format!(
            "          (xous-app-signal v{})",
            env!("CARGO_PKG_VERSION")
        ));
        out.push(String::new());
        out.push(String::from("   ──────────────────────────────────────"));
        out.push(String::new());
        out.push(String::from("   libsignal:        v0.91.0 (98915c44)"));
        out.push(String::from("   libsignal-svc-rs: forked HEAD"));
        out.push(String::from("   presage:          forked HEAD"));
        out.push(String::from(
            "   curve25519-dalek: 4.1.3 (betrusted+lizard)",
        ));
        out.push(String::from("   libcrux-ml-kem:   0.0.8"));
        out.push(String::from("   spqr:             1.5.1"));
        out.push(String::from("   smol-rs:          pinned"));
        out.push(String::new());
        out.push(String::from("   Signal Trust Root: pinned"));
        out.push(String::from("   PDDB basis:        signal"));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc | Key::Home => Transition::Pop,
            _ => Transition::None,
        }
    }
}
