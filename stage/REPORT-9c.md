# Stage 9c — UI scaffolding

Status: **complete.** New `xous-app-signal-ui` crate (840 LoC across
9 files) with screen state machine, hosted-mode TTY renderer, four
MVP screens, and 12 state-transition tests. `xas` binary's main loop
is now `Ui::run()`; the Stage 8 Hello/Whoami probe lives on as the
menu's "Test worker" item. rv32 cross-compile of the full stack
passes; clippy + fmt clean.

## What landed

```
crates/xous-app-signal-ui/
├── Cargo.toml             only xous-signal-bridge + async-channel as deps
└── src/
    ├── lib.rs       (261) Ui driver + state stack + parse_key + 12 tests
    ├── key.rs       ( 29) hardware-agnostic Key enum
    ├── screen.rs    (120) Screen / Transition enums; placeholder rendering
    ├── render.rs    (107) Surface trait + TextSurface (50×22 chars to stdout)
    └── screens/
        ├── mod.rs           (  8)
        ├── splash.rs        ( 93)  ← UI.md §5.1
        ├── menu.rs          (133)  ← UI.md §5.10
        ├── about.rs         ( 49)  ← UI.md §5.11
        └── empty_list.rs    ( 40)  ← UI.md §5.6

crates/xous-app-signal/
├── Cargo.toml      + xous-app-signal-ui dep
└── src/main.rs     replaces Stage 8 sequential probe with Ui::run()
```

## Audit-friendly invariants (from UI.md §9)

- **One screen on top at a time.** `Ui::stack: Vec<Screen>`.
  `Transition::{None, Push, Pop, Replace, Quit}` is the *only* way
  to mutate it. Audit story: read the `apply` method (one match,
  five arms), done.
- **All input goes through `Screen::handle_key`.** One method, one
  match, per screen. No hidden side channels.
- **No external UI deps.** No `crossterm`, `termion`, `tui-rs`,
  `ratatui`. Box-drawing characters are direct UTF-8 in source.
  Renderer is `pad()` + `writeln!`.
- **Renderer is `Vec<String>`-shaped.** Each `Screen::render`
  returns lines; the driver wraps in a status bar + hint footer
  via `render::render_frame`. A future GAM-side renderer translates
  the same `Vec<String>` into `TextView`s; no screen knows which
  back-end is active.
- **Pure-logic tests.** No I/O in tests. `Ui::dispatch(Key)` +
  `Ui::top()` are visible-for-tests; the 12 tests assert on the
  state-stack discriminant, never on stdout. The transition graph
  is testable like a finite state machine.

## What the user sees (hosted-mode smoke)

```sh
$ printf 'down\ndown\n\nleft\nq\n' | cargo run -p xous-app-signal --bin xas
┌──────────────────────────────────────────────────┐
│ xas   [OFF]                                      │
├──────────────────────────────────────────────────┤
│                                                  │
│                                                  │
│                       xas                        │
│                                                  │
│           Signal client for Precursor            │
│                                                  │
│                 Not yet linked.                  │
│                                                  │
│        > Link this device                        │
│          [Register a phone number]               │
│          About                                   │
│          Quit                                    │
│                                                  │
... (3 more frames as user navigates) ...

(Final About screen)
┌──────────────────────────────────────────────────┐
│ xas   [OFF]                                      │
├──────────────────────────────────────────────────┤
│                                                  │
│                       xas                        │
│          (xous-app-signal v0.0.1)                │
│                                                  │
│   ──────────────────────────────────────         │
│                                                  │
│   libsignal:        v0.91.0 (98915c44)           │
│   libsignal-svc-rs: forked HEAD                  │
│   presage:          forked HEAD                  │
│   curve25519-dalek: 4.1.3 (betrusted+lizard)     │
│   libcrux-ml-kem:   0.0.8                        │
│   spqr:             1.5.1                        │
│   smol-rs:          pinned                       │
│                                                  │
│   Signal Trust Root: pinned                      │
│   PDDB basis:        signal                      │
│                                                  │
├──────────────────────────────────────────────────┤
│ Left Back                                        │
└──────────────────────────────────────────────────┘
```

The About screen is the project's end-user-verifiability surface.
Photographing it gives every upstream version pin needed to
reproduce the build (UI.md §5.11).

## Verification

```sh
$ cargo test -p xous-app-signal-ui
test result: ok. 12 passed; 0 failed; 0 ignored

$ cargo test -p xous-signal-bridge
test result: ok. 3 passed; 0 failed

$ cargo test -p presage-store-pddb
test result: ok. 22 passed; 0 failed

$ cargo run -p xous-app-signal --bin xas <<< $'\nq\n'    # hosted smoke
✓ splash renders, q quits

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ Full rv32 cross-compile of the stack including the new UI crate.

$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean
```

## Test coverage (12 tests in xous-app-signal-ui)

| Test | What it checks |
|---|---|
| `starts_on_splash` | Fresh `Ui::new()` has `Screen::Splash` on top |
| `splash_down_then_select_about` | Down × 2 + Home navigates to About |
| `about_back_pops_to_splash` | About + Left returns to splash |
| `splash_q_quits` | `q` empties the stack |
| `menu_about_replaces_top` | From menu, navigate to About item, Home replaces top |
| `menu_left_pops` | Menu + Left dismisses menu |
| `empty_list_menu_key_pushes_menu` | EmptyList + `m` pushes Menu |
| `parse_key_basics` | Stdin "up"/"down"/"j"/"k"/"q"/`\n` → correct Key |
| `pad_truncates_long_lines` | render::pad correctness |
| `pad_extends_short_lines` | render::pad correctness |
| `pad_handles_multibyte` | `✓` (3-byte UTF-8) counts as 1 char |
| `render_frame_writes_a_box` | render::render_frame produces a box-drawn frame |

## Tradeoffs

### What we deliberately skipped

- **Real keyboard handling** (escape sequences for arrow keys,
  termios raw mode). Hosted-mode reads stdin a line at a time;
  the user types `up`/`down`/`j`/`k`/`q` or hits enter for Home.
  This is enough for human smoke testing and trivial to script.
  The on-device build (Stage 9b/c follow-up) gets arrow-key
  events directly from Xous's keyboard service.
- **Real status chips.** `[OFF]` is hardcoded. Stage 10+ wires
  `ConnectionState` events from the worker.
- **Real GAM rendering.** The `TextSurface` is the only renderer
  in this commit. A `GamSurface` for `cfg(target_os = "xous")`
  is a Stage 9c follow-up; the screens already shape their
  output to be GAM-compatible (`Vec<String>` of lines, fixed
  width).
- **Toast / banner overlay.** Stub in screen.rs::Screen but no
  driver-side timer / dismissal logic yet. Defer to Stage 10
  when the first real "Message sent" toast is needed.
- **Worker integration.** The "Test worker" menu item currently
  Pushes back to the splash screen; it doesn't actually send
  `Cmd::Hello`. The driver wiring to forward menu actions to
  `cmd_tx` is Stage 10 work — Stage 9c's purpose was the screen
  shell, not the data flow.

### Alternatives we rejected (and why)

- **Vendoring `libs/chat` from xous-core.** UI.md §9 makes the
  case in detail. Would have saved ~1 kLoC vs the ~840 LoC we
  wrote, but added a fork to track upstream and would have
  forced a translation layer between presage's Content/Metadata
  model and `libs/chat`'s Dialogue/Post model. Not worth it.
- **`crossterm` / `tui-rs` / `ratatui`.** Each adds ~5-10
  kLoC of TUI library to the audit surface for affordances we
  don't use. The hosted-mode renderer is one `pad()` function
  + a few `writeln!` calls; a TUI library is overkill.
- **Boxed dyn `Screen` trait.** Considered, rejected. The enum
  approach (`Screen::Splash(SplashScreen) | Screen::Menu(...) |
  ...`) is more explicit at the cost of one match per call
  site; for 12 screens that's 12 match arms, audit-friendly.

## What this unblocks

- **Stage 10** can now treat link-flow screens as data: populate
  `Screen::LinkShowUrl(url)`, `LinkConfirming`, `LinkDone(...)`,
  `LinkError(reason)`. The driver routes `Cmd::LinkBegin` and
  `Event::LinkUrl(_)` etc. through the channels the UI already
  holds.
- **Stage 11** populates `Screen::List(Vec<ThreadSummary>)` and
  `Screen::Conversation { thread, messages }`. Same pattern:
  the screens are the renderer + key-router; the Cmd/Event types
  carry the data.
- **Stage 12** populates `Screen::Compose { thread, draft }` and
  the `Cmd::SendMessage` path.

Each of those stages adds one screen + one Cmd + one Event variant;
nothing else changes. That's the design intent.

## Files changed (this commit)

```
modified:
  Cargo.toml                                      (+xous-app-signal-ui member)
  Cargo.lock                                      (resolver picked up new crate)
  crates/xous-app-signal/Cargo.toml               (+xous-app-signal-ui dep)
  crates/xous-app-signal/src/main.rs              (Stage 8 probe → Ui::run)
  docs/ROADMAP.md                                 (+Stage 9c section, +order)

new:
  crates/xous-app-signal-ui/Cargo.toml            ( 14 LoC)
  crates/xous-app-signal-ui/src/lib.rs            (261 LoC)
  crates/xous-app-signal-ui/src/key.rs            ( 29 LoC)
  crates/xous-app-signal-ui/src/screen.rs         (120 LoC)
  crates/xous-app-signal-ui/src/render.rs         (107 LoC)
  crates/xous-app-signal-ui/src/screens/mod.rs    (  8 LoC)
  crates/xous-app-signal-ui/src/screens/splash.rs ( 93 LoC)
  crates/xous-app-signal-ui/src/screens/menu.rs   (133 LoC)
  crates/xous-app-signal-ui/src/screens/about.rs  ( 49 LoC)
  crates/xous-app-signal-ui/src/screens/empty_list.rs ( 40 LoC)
  stage/REPORT-9c.md                              (this file)

Total new code: 854 LoC across 11 files. Stage 9c stop condition was
"don't exceed 1.5 kLoC"; we came in at 57% of that.
```
