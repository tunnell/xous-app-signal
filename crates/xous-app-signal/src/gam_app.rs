//! GAM-rendered xas app, modeled on `apps/hello/src/main.rs` and
//! `apps/ball/src/ball.rs` for the rawkeys handling pattern.
//!
//! Three menu actions, all wired to real behavior on hosted-Xous
//! and rv32:
//!
//! - **Link device**: prompts for a device name (Modals TextEntry),
//!   then drives the worker's `Cmd::LinkDevice`. On
//!   `Event::LinkUrl`, opens a Modals notification with the
//!   `tsdevice://` URL rendered as a QR code (server-side
//!   `set_qrcode`). User scans with the Signal phone app, presses
//!   any key to dismiss the modal, and the worker continues
//!   processing the provisioning envelope. On
//!   `Event::LinkComplete`, the menu shows "Linked!" with the new
//!   ACI/phone. On `Event::LinkError`, the failure cause is shown.
//!
//! - **About**: project name, version (from `CARGO_PKG_VERSION`),
//!   author, and one-line description.
//!
//! - **Quit**: switches focus back to shellchat (so the launcher
//!   menu is reachable again) and returns from `run()`. The
//!   caller (`main.rs`) joins the worker thread and exits.

use core::fmt::Write;

use async_channel::{Receiver, Sender};
use blitstr2::GlyphStyle;
use num_traits::*;
use ux_api::minigfx::*;
use ux_api::service::api::Gid;
use xous_signal_bridge::{Cmd, Event};

const SERVER_NAME_XAS: &str = "_xas_";

#[derive(Debug, num_derive::FromPrimitive, num_derive::ToPrimitive)]
enum XasOp {
    Redraw = 0,
    Rawkeys = 1,
    FocusChange = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Menu,
    About,
    Linking,    // transient: waiting for LinkUrl from worker
    Linked {
        kind: LinkedKind,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkedKind {
    Success,
    Failure,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuItem {
    Link,
    About,
    Quit,
}

impl MenuItem {
    fn next(self) -> Self {
        match self {
            Self::Link => Self::About,
            Self::About => Self::Quit,
            Self::Quit => Self::Link,
        }
    }
    fn prev(self) -> Self {
        match self {
            Self::Link => Self::Quit,
            Self::About => Self::Link,
            Self::Quit => Self::About,
        }
    }
}

struct App {
    gam: gam::Gam,
    content: Gid,
    bounds: Point,
    screen: Screen,
    selected: MenuItem,
    /// Last event status text, shown on the Linked screen.
    last_status: String,
    /// Asks the run loop to break.
    quit_requested: bool,
}

impl App {
    fn render(&self) -> Result<(), String> {
        self.gam
            .draw_rectangle(
                self.content,
                Rectangle::new_with_style(
                    Point::new(0, 0),
                    self.bounds,
                    DrawStyle {
                        fill_color: Some(PixelColor::Light),
                        stroke_color: None,
                        stroke_width: 0,
                    },
                ),
            )
            .map_err(|e| format!("draw_rectangle: {:?}", e))?;

        let mut tv = TextView::new(
            self.content,
            TextBounds::BoundingBox(Rectangle::new(
                Point::new(8, 8),
                Point::new(self.bounds.x - 8, self.bounds.y - 8),
            )),
        );
        tv.border_width = 1;
        tv.draw_border = true;
        tv.clear_area = true;
        tv.rounded_border = Some(3);
        tv.style = GlyphStyle::Regular;

        match self.screen {
            Screen::Menu => {
                let mark = |item: MenuItem| if item == self.selected { ">" } else { " " };
                write!(
                    tv.text,
                    "xas — Signal client\n\n\
                     {} Link device\n\
                     {} About\n\
                     {} Quit\n\n\
                     Up/Down: navigate\n\
                     Home or Enter: select",
                    mark(MenuItem::Link),
                    mark(MenuItem::About),
                    mark(MenuItem::Quit),
                )
            }
            Screen::About => write!(
                tv.text,
                "About xas\n\n\
                 Unofficial Signal client\n\
                 for Xous on Precursor.\n\n\
                 Version: {}\n\
                 Author:  @tunnell\n\n\
                 Built on:\n\
                  - presage v0.8.0-dev\n\
                  - libsignal-service-rs\n\
                  - libsignal v0.91.0\n\n\
                 Press Home/Enter to\n\
                 return to menu.",
                env!("CARGO_PKG_VERSION"),
            ),
            Screen::Linking => write!(
                tv.text,
                "Linking device...\n\n\
                 Connecting to Signal\n\
                 servers and requesting\n\
                 a provisioning URL.\n\n\
                 Cert is verified against\n\
                 Signal's pinned CA.\n\n\
                 (Please wait.)"
            ),
            Screen::Linked { kind } => {
                let title = match kind {
                    LinkedKind::Success => "Link succeeded",
                    LinkedKind::Failure => "Link failed",
                };
                write!(
                    tv.text,
                    "{}\n\n{}\n\nPress Home/Enter to\nreturn to menu.",
                    title, self.last_status
                )
            }
        }
        .map_err(|e| format!("write text: {}", e))?;

        self.gam.post_textview(&mut tv).map_err(|e| format!("post_textview: {:?}", e))?;
        self.gam.redraw().map_err(|e| format!("redraw: {:?}", e))?;
        Ok(())
    }
}

pub fn run(cmd_tx: Sender<Cmd>, event_rx: Receiver<Event>) -> Result<(), String> {
    log::info!("xas/gam_app: starting GAM-rendered loop");

    let xns = xous_names::XousNames::new().map_err(|e| format!("XousNames::new: {:?}", e))?;
    let sid = xns
        .register_name(SERVER_NAME_XAS, None)
        .map_err(|e| format!("register_name: {:?}", e))?;

    let gam = gam::Gam::new(&xns).map_err(|e| format!("Gam::new: {:?}", e))?;
    let token = gam
        .register_ux(gam::UxRegistration {
            app_name: String::from(gam::APP_NAME_XAS),
            ux_type: gam::UxType::Chat,
            predictor: None,
            listener: sid.to_array(),
            redraw_id: XasOp::Redraw.to_u32().unwrap(),
            gotinput_id: None,
            audioframe_id: None,
            rawkeys_id: Some(XasOp::Rawkeys.to_u32().unwrap()),
            focuschange_id: Some(XasOp::FocusChange.to_u32().unwrap()),
        })
        .map_err(|e| format!("register_ux: {:?}", e))?
        .ok_or_else(|| "register_ux returned None token".to_string())?;
    log::info!("xas/gam_app: GAM token = {:x?}", token);

    let content = gam.request_content_canvas(token).map_err(|e| format!("canvas: {:?}", e))?;
    let bounds = gam.get_canvas_bounds(content).map_err(|e| format!("bounds: {:?}", e))?;
    log::info!("xas/gam_app: canvas {:?}, bounds {:?}", content, bounds);

    let _ = gam.allow_mainmenu();

    // The Modals client we use for the QR notification + device-name
    // prompt. Built lazily-on-demand so each link attempt gets a
    // clean token; cheap to construct.
    let modals_xns = xous_names::XousNames::new()
        .map_err(|e| format!("XousNames for modals: {:?}", e))?;

    let mut app = App {
        gam,
        content,
        bounds,
        screen: Screen::Menu,
        selected: MenuItem::Link,
        last_status: String::new(),
        quit_requested: false,
    };
    app.render().ok();

    loop {
        let msg = xous::receive_message(sid).map_err(|e| format!("receive: {:?}", e))?;
        match FromPrimitive::from_usize(msg.body.id()) {
            Some(XasOp::Redraw) => {
                app.render().ok();
            }
            Some(XasOp::Rawkeys) => {
                xous::msg_scalar_unpack!(msg, k1, k2, k3, k4, {
                    let keys = [
                        char::from_u32(k1 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k2 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k3 as u32).unwrap_or('\u{0}'),
                        char::from_u32(k4 as u32).unwrap_or('\u{0}'),
                    ];
                    log::info!("xas/gam_app: keys {:?}", keys);
                    handle_keys(&mut app, keys, &cmd_tx, &event_rx, &modals_xns);
                });
                if app.quit_requested {
                    // Quit on Xous means "hide and go back to
                    // shellchat" — the process stays alive so the
                    // launcher can re-raise us. (Xous doesn't
                    // auto-relaunch terminated app processes; if we
                    // exit here, clicking Signal again from the
                    // launcher silently does nothing because our
                    // server is gone.) Reset menu state so the next
                    // time the user comes back they see a clean
                    // menu, then keep looping.
                    let _ = app.gam.switch_to_app(gam::APP_NAME_SHELLCHAT, token);
                    log::info!("xas/gam_app: hidden via Quit; staying alive");
                    app.screen = Screen::Menu;
                    app.selected = MenuItem::Link;
                    app.last_status.clear();
                    app.quit_requested = false;
                }
            }
            Some(XasOp::FocusChange) => {
                // GAM sends FocusChange when our context moves to
                // Foreground (e.g. user re-launched us from the
                // launcher after we hid via Quit) or Background
                // (we just lost focus to something else). Foreground
                // requires us to redraw — GAM doesn't follow up
                // with a Redraw on its own. Background is a no-op
                // for us currently.
                xous::msg_scalar_unpack!(msg, new_state_code, _, _, _, {
                    let new_state = gam::FocusState::convert_focus_change(new_state_code);
                    log::info!("xas/gam_app: focus change -> {:?}", new_state);
                    if matches!(new_state, gam::FocusState::Foreground) {
                        if let Err(e) = app.render() {
                            log::warn!("xas/gam_app: render after focus: {}", e);
                        }
                    }
                });
            }
            _ => {
                log::debug!("xas/gam_app: unknown msg id={}", msg.body.id());
            }
        }
    }
}

fn handle_keys(
    app: &mut App,
    keys: [char; 4],
    cmd_tx: &Sender<Cmd>,
    event_rx: &Receiver<Event>,
    modals_xns: &xous_names::XousNames,
) {
    for &k in keys.iter() {
        if k == '\u{0}' {
            continue;
        }
        match (app.screen, k) {
            (Screen::Menu, '↑') => app.selected = app.selected.prev(),
            (Screen::Menu, '↓') => app.selected = app.selected.next(),
            (Screen::Menu, '∴') | (Screen::Menu, '\u{d}') => match app.selected {
                MenuItem::Link => drive_link(app, cmd_tx, event_rx, modals_xns),
                MenuItem::About => app.screen = Screen::About,
                MenuItem::Quit => app.quit_requested = true,
            },
            (Screen::About, '∴') | (Screen::About, '\u{d}') => app.screen = Screen::Menu,
            (Screen::Linked { .. }, '∴') | (Screen::Linked { .. }, '\u{d}') => {
                app.screen = Screen::Menu;
                app.last_status.clear();
            }
            // Linking is a transient screen; rawkeys are ignored
            // while we're waiting on the worker. (The modal handles
            // user input separately.)
            _ => {}
        }
    }
    if let Err(e) = app.render() {
        log::warn!("render: {}", e);
    }
}

/// Drive the link flow: prompt for device name, send `Cmd::LinkDevice`
/// to the worker, route `Event::LinkUrl` into a QR modal, wait for
/// `LinkComplete` or `LinkError`. Updates `app.screen` /
/// `app.last_status` for the post-link menu render.
fn drive_link(
    app: &mut App,
    cmd_tx: &Sender<Cmd>,
    event_rx: &Receiver<Event>,
    modals_xns: &xous_names::XousNames,
) {
    let modals = match modals::Modals::new(modals_xns) {
        Ok(m) => m,
        Err(e) => {
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = format!("Modals init failed:\n{:?}", e);
            return;
        }
    };

    // Step 1: device name. TextEntry modal with "xas" pre-filled.
    let device_name = match modals
        .alert_builder("Device name?")
        .field(Some("xas".to_string()), None)
        .build()
    {
        Ok(payloads) => {
            // TextEntryPayloads::first returns a TextEntryPayload by
            // value (single field, since we registered just one).
            // Its `to_string()` impl is what we want; trim and
            // sanity-check.
            // TextEntryPayload::as_str returns the entered text; we
            // copy out and trim.
            let first = payloads.first();
            let trimmed = first.as_str().trim();
            if trimmed.is_empty() { "xas".to_string() } else { trimmed.to_string() }
        }
        Err(e) => {
            app.screen = Screen::Linked { kind: LinkedKind::Failure };
            app.last_status = format!("device name modal:\n{:?}", e);
            return;
        }
    };

    // Step 2: switch to "linking..." screen, send Cmd::LinkDevice.
    app.screen = Screen::Linking;
    app.render().ok();

    if let Err(e) = cmd_tx.send_blocking(Cmd::LinkDevice { device_name }) {
        app.screen = Screen::Linked { kind: LinkedKind::Failure };
        app.last_status = format!("Cmd::LinkDevice send:\n{:?}", e);
        return;
    }

    // Step 3: drain events. First we expect LinkUrl, then
    // LinkComplete or LinkError. Any other Event variants are
    // logged + ignored (they'd be spurious during link).
    let mut url_shown = false;
    loop {
        let event = match event_rx.recv_blocking() {
            Ok(ev) => ev,
            Err(e) => {
                app.screen = Screen::Linked { kind: LinkedKind::Failure };
                app.last_status = format!("event_rx closed:\n{:?}", e);
                return;
            }
        };
        match event {
            Event::LinkUrl(url) => {
                log::info!("xas/gam_app: link URL = {}", url);
                if !url_shown {
                    url_shown = true;
                    let _ = modals.show_notification(
                        "Scan with the Signal phone app, then press any key.",
                        Some(&url),
                    );
                }
            }
            Event::LinkComplete { device_name, aci, phone } => {
                log::info!(
                    "xas/gam_app: LinkComplete device={} aci={} phone={}",
                    device_name, aci, phone
                );
                app.screen = Screen::Linked { kind: LinkedKind::Success };
                app.last_status =
                    format!("device:{}\naci:{}\nphone:{}", device_name, aci, phone);
                return;
            }
            Event::LinkError(msg) => {
                log::warn!("xas/gam_app: LinkError: {}", msg);
                app.screen = Screen::Linked { kind: LinkedKind::Failure };
                app.last_status = msg;
                return;
            }
            other => {
                log::debug!("xas/gam_app: ignoring event during link: {:?}", other);
            }
        }
    }
}
