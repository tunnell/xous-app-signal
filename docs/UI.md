# UI

How the xas user interface actually works as of 2026-05-10.
For the runtime + worker architecture, see
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## Where the UI code lives

```
crates/xous-app-signal/src/
├── gam_app.rs       (~1.9 kLoC)  ← the UI: GAM-rendered, hardware + hosted-Xous
├── store.rs         (~0.4 kLoC)  ← MessageStore: message/thread state mutation funnel
└── dialogue.rs      (~0.4 kLoC)  ← pure-data conversation summarization
```

`gam_app.rs` renders into a single `gam::TextView` inside the
GAM's bounded canvas, dispatches keys via a `match (Screen, char)`
table, and reacts to `Event`s from the worker via a forwarder
thread that pushes into a `Mutex<VecDeque<Event>>`. All message
and thread state mutation goes through `store.rs`'s
`MessageStore`, which is unit-testable without a GAM. Running the
UI requires a Xous environment (hosted emulation via
`cargo xtask run`, or hardware); there is no standalone fallback.

## Design principles (still load-bearing from V1)

- **End-user verifiability.** Every pixel rendered should be
  defensible from a few hundred lines of UI code we own; no
  opaque widget toolkit. Single `TextView` rendered inside a GAM
  Canvas; no third-party UI deps.
- **Keyboard-only.** No touch, no scroll wheel. Arrow keys + Home
  + Esc + printable ASCII + F1-F4 (Precursor sends 0x11-0x14 for
  the F-keys).
- **Monochrome.** 336 × 536 1-bit Sharp Memory LCD. Information
  density encoded in shape, weight, position, and inversion only.
- **Single-thread, sync.** UI runs in the GAM event loop;
  `Event`s arrive from `xous-signal-worker` over an
  `async-channel` consumed by a forwarder thread. No locks in
  the render path; no `Send` plumbing in the screen code.

## Screen inventory

The current `Screen` enum is in
[`crates/xous-app-signal/src/gam_app.rs`](../crates/xous-app-signal/src/gam_app.rs)
near line 68. Nine variants:

| Variant | When | What |
|---|---|---|
| `Menu` | Pre-link landing; reachable post-link via Menu key from Home | Two states: pre-link shows Link / About / Help; post-link shows About / Help (Settings is the main post-link surface, reached via F4) |
| `About` | Reachable from Menu and Settings | Static text: version, author, security overview, build deps, alpha limitations |
| `Linking` | Active during `Cmd::LinkDevice` flow | Shows "connecting / waiting for QR scan" copy. The QR code is rendered by `modals::Modals::show_notification` overlay, not by the screen itself |
| `Linked { kind }` | Brief screen after link result | Success: brief banner, auto-fires `Cmd::StartReceive`, transitions to Home. Failure: error text, Enter returns to Menu |
| `Home` | Post-link landing | The conversation list. Empty state shows "No conversations yet. F1 to start one." Populated state shows up to `INBOX_CAPACITY` recent threads |
| `Thread { uuid }` | After opening a conversation from Home | Per-conversation history view + compose input. Auto-marks thread read on open |
| `Settings` | F4 from Home or Thread | Sub-menu: Profile / Help / About / Logout |
| `Profile` | Settings → Profile | Account info: device name, ACI, phone number. May show "(not loaded)" on cold-start without a fresh `LinkComplete` (Tier-2 chore: fire `Cmd::GetAccountInfo`) |
| `Help` | F3 from Home, or Settings → Help | In-app FAQ — Wi-Fi recipe, send-latency note, file-a-bug pointer |

Five screens were never built (the V1 design deferred them to
"later"):
group chats, attachments, search-in-list, archive view, typing
indicators.

## Keyboard map

Precursor sends the F-keys as `\u{11}`, `\u{12}`, `\u{13}`,
`\u{14}` (= 0x11–0x14). The Menu key sends `'☰'`. Enter sends
`'∴'` or `\u{d}`. Backspace sends `\u{8}`. Esc sends `\u{1b}`.

| Screen | Key | Action |
|---|---|---|
| Menu | `↑`/`↓` | Move cursor |
| Menu | Enter / `∴` | Select item |
| Menu | Esc (post-link only) | Back to Home |
| Home | `↑`/`↓` | Move focus across conversations |
| Home | Enter / `∴` | Open focused thread; mark as read on open |
| Home | Menu (`☰`) / Esc | Open Settings |
| Home | F1 (`\u{11}`) | New chat (modal prompts for UUID; +E.164 / username rejected — Tier-2 lookup chore) |
| Home | F2 (`\u{12}`) | Sync — placeholder (notification only) |
| Home | F3 (`\u{13}`) | Open Help |
| Home | F4 (`\u{14}`) | Open Settings |
| Thread | Enter / `∴` | If compose-buffer non-empty: send. If empty: back to Home |
| Thread | Backspace (`\u{8}`) | Pop char from compose buffer |
| Thread | F1 (`\u{11}`) | Send (no-op on empty buffer) |
| Thread | F3 / F4 | Help / Settings |
| Thread | printable ASCII | Append to compose buffer |
| Settings | `↑`/`↓` | Move cursor |
| Settings | Enter / `∴` | Open selected sub-screen |
| Settings | Esc | Back to Home |
| About / Help / Profile | Enter / Esc | Back |
| Linking | Esc / Backspace | Cancel the in-flight link, return to Menu |
| Linking | (other keys — auto-transitions on worker events) | |
| Linked | Enter | Continue (Home if Success, Menu if Failure) |

The full table is the `match (&app.screen, k)` block in
[`gam_app.rs::handle_keys`](../crates/xous-app-signal/src/gam_app.rs)
near line 910 — that's the source of truth.

Not yet implemented (the V1 design listed these; xas doesn't
bind them):
`Shift+↑`/`Shift+↓` for page nav, `n` (next unread), `u` (toggle
unread), `p` (toggle pin), digit shortcuts `1`-`9`/`0` for
direct conversation jump.

## Render path

Single `TextView` inside the GAM canvas. The render function
(`App::render` in `gam_app.rs` near line 183) clears the
canvas, sets a glyph style (Bold for Home/Thread, Regular
elsewhere), and calls a per-screen `write_*` method that
appends formatted text. There's no widget tree, no layout
engine — just bytes into a `String`.

This means screens are easy to add: define a new `Screen`
enum variant, write a `write_X` method, add the dispatch arm
in `handle_keys`. ~30 LoC for a basic screen.

It also means screens are limited: no overlapping regions, no
non-text graphics inside the content area (the QR code uses
`modals::Modals` as an overlay window). For richer UI we'd
need to stop using a single TextView. None of the deferred
screens (group chats etc.) require that.

## Worker integration

A forwarder thread (spawned from `gam_app::run`) does
`event_rx.recv_blocking()` in a loop, pushes the result into a
shared `Mutex<VecDeque<Event>>`, and sends a `XasOp::WorkerEvent`
scalar IPC to our own SID. The GAM main loop wakes on that
opcode, drains the deque, and calls `handle_worker_event` for
each event. This pattern keeps the UI single-threaded and
sync while still being responsive to events from the worker.

The actual `Cmd` and `Event` enums (8 + 13 variants) live in
[`crates/xous-signal-worker/src/cmd.rs`](../crates/xous-signal-worker/src/cmd.rs).
For a layer-by-layer trace of how a `Cmd::SendMessage` goes
from key-press to TLS bytes, see the layer walkthrough in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## What's deferred

UI items still on the roadmap:

- **F1 New chat — Username + E.164 lookup.** Today only UUIDs
  work; the modal rejects username/E.164 with "lookup not yet
  supported."
- **F2 Sync — real implementation.** Today F2 just shows a
  notification.
- **Logout (Settings → Logout).** Stub — tells the user to
  wipe PDDB manually.
- **Profile cold-start population.** Account info shows "(not
  loaded)" on a cold-start that didn't go through a fresh
  `LinkComplete`. Fix: fire `Cmd::GetAccountInfo` on app boot
  when `linked == true`.
- **icontray IME plugin.** The GAM's bottom soft-key tray
  currently shows leaked shellchat predictor entries; a custom
  icontray plugin would put pencil/refresh/?/gear glyphs under
  F1-F4 instead.
- **No-internet preflight on Home** when xas opens with no
  Wi-Fi joined.

## Memory

The V1 design budgeted ~10 KiB UI working set + ~32 KiB during
conversation view. The current implementation hits both budgets
comfortably — `messages: Vec<ThreadMessage>` is capped at
`INBOX_CAPACITY` (5) and `dialogues` is computed on demand. The
4 MiB app budget is still mostly libsignal + zkgroup + ML-KEM-1024
+ TLS state.

Persistence of message history across sessions is
queued for a future release ("Persistence: store message history in
PDDB"); today, on app restart, `messages` is empty until new
messages arrive.
