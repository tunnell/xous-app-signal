//! Linking-flow screens — UI.md §5.2-5.5.
//!
//! Four sub-states: `Starting` (waiting for the URL), `ShowUrl` (URL
//! received; user scans/copies), `Confirming` (phone scanned;
//! user-confirm in flight), `Done` (linked, transitions to empty
//! list), `Error` (error string + retry/cancel choice).
//!
//! All four are passive — they show what the worker emitted via
//! `Event::LinkUrl` / `LinkComplete` / `LinkError`. The driver
//! transitions between them based on those events.

use crate::stdin_ui::key::Key;
use crate::stdin_ui::screen::Transition;

// ---------------------------------------------------------------
// LinkStarting — shown immediately after the user hits "Link this
// device". The driver will Replace this with LinkShowUrl once the
// worker's `Event::LinkUrl` arrives. `Cancel` (Left) sends
// `Cmd::LinkCancel` and Pops back to splash.
// ---------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LinkStartingScreen;

impl LinkStartingScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        vec![
            String::new(),
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Connecting to Signal…"),
            String::new(),
            String::from("   On your phone, open Signal."),
            String::from("   Settings -> Linked Devices -> Link new."),
            String::new(),
            String::from("   The QR code will appear here."),
        ]
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

// ---------------------------------------------------------------
// LinkShowUrl — UI.md §5.2. URL has arrived. Rendered as monospaced
// text in this stdin-driven UI; QR rendering happens in `gam_app.rs`
// for the GAM-driven path.
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LinkShowUrlScreen {
    pub url: String,
}

impl LinkShowUrlScreen {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = vec![
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Scan or enter this URL on your phone:"),
            String::new(),
        ];

        // Wrap the URL across multiple lines so a 50-char-wide screen
        // can show all of it. tsdevice:// URLs are typically ~120-200
        // chars; we wrap at 44 chars per line.
        for chunk in chunk_str(&self.url, 44) {
            out.push(format!("   {chunk}"));
        }

        out.push(String::new());
        out.push(String::from("   Waiting for scan..."));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

fn chunk_str(s: &str, width: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut idx = 0;
    let bytes = s.as_bytes();
    while idx < bytes.len() {
        let end = (idx + width).min(bytes.len());
        // chunk on byte boundary; `tsdevice://` URLs are ASCII so
        // this is safe.
        out.push(&s[idx..end]);
        idx = end;
    }
    out
}

// ---------------------------------------------------------------
// LinkConfirming — UI.md §5.3. Phone has scanned; user must confirm
// on phone. We can't peek into presage's internal state to know
// when this happens; the screen shows a static "confirm on phone"
// message until the worker emits LinkComplete or LinkError.
// ---------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LinkConfirmingScreen;

impl LinkConfirmingScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self) -> Vec<String> {
        vec![
            String::new(),
            String::new(),
            String::from("              Link this device"),
            String::new(),
            String::from("   Scanned. Confirm on your phone:"),
            String::new(),
            String::from("   \"Link this device as 'Precursor'?\""),
            String::new(),
            String::from("   This may take 30-60 seconds."),
        ]
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

// ---------------------------------------------------------------
// LinkDone — UI.md §5.4. Linking complete. Shows registration data
// for verifiability. Home transitions to the empty-list screen.
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LinkDoneScreen {
    pub device_name: String,
    pub aci: String,
    pub phone: String,
}

impl LinkDoneScreen {
    pub fn new(device_name: String, aci: String, phone: String) -> Self {
        Self {
            device_name,
            aci,
            phone,
        }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(String::new());
        out.push(String::new());
        out.push(String::from("                       OK"));
        out.push(String::new());
        out.push(String::from("                Linked."));
        out.push(String::new());
        out.push(format!("       Device name: {}", &self.device_name));
        out.push(format!("       ACI:         {}", short(&self.aci, 32)));
        out.push(format!("       Phone:       {}", &self.phone));
        out.push(String::new());
        out.push(String::from("       You can now receive and send."));
        out
    }

    pub fn handle_key(&mut self, key: Key) -> Transition {
        match key {
            // From LinkDone, Home transitions into the
            // ConversationList screen which on entry sends
            // `Cmd::StartReceive` and begins streaming messages.
            Key::Home | Key::Right => Transition::Replace(crate::stdin_ui::screen::Screen::ConversationList(
                crate::stdin_ui::screens::conversation_list::ConversationListScreen::new(),
            )),
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------
// LinkError — UI.md §5.5. Linking failed. Shows the error string +
// a Retry/Cancel choice.
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LinkErrorScreen {
    pub reason: String,
    /// 0 = Retry, 1 = Cancel.
    pub focus: usize,
}

impl LinkErrorScreen {
    pub fn new(reason: String) -> Self {
        Self { reason, focus: 0 }
    }

    pub fn render(&self) -> Vec<String> {
        let mut out = vec![
            String::new(),
            String::new(),
            String::from("                       X"),
            String::new(),
            String::from("              Linking failed."),
            String::new(),
            String::from("   Reason:"),
        ];
        for chunk in chunk_str(&self.reason, 44) {
            out.push(format!("     {chunk}"));
        }
        out.push(String::new());
        let retry = if self.focus == 0 { ">" } else { " " };
        let cancel = if self.focus == 1 { ">" } else { " " };
        out.push(format!("       {retry} Try again"));
        out.push(format!("       {cancel} Cancel"));
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
                if self.focus < 1 {
                    self.focus += 1;
                }
                Transition::None
            }
            Key::Home | Key::Right => match self.focus {
                0 => Transition::Replace(crate::stdin_ui::screen::Screen::LinkStarting(
                    LinkStartingScreen::new(),
                )),
                1 => Transition::Pop,
                _ => Transition::None,
            },
            Key::Left | Key::Esc => Transition::Pop,
            _ => Transition::None,
        }
    }
}
