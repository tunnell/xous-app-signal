# xous-app-signal

The xas binary itself (binary name: `xas`). Boots into a Xous
environment, spawns the signal worker thread, and runs the
GAM-rendered UI loop. This is the only crate in the workspace
that produces an executable.

## What's here

- **`src/main.rs`** — entry point. Constructs `PddbStore`, spawns
  `xous-signal-worker`'s thread, then either boots into the
  GAM-rendered UI (`gam_app::run`) or falls back to the stdin UI
  (`stdin_ui::Ui`) when no Xous server is reachable.
- **`src/gam_app.rs`** (~2.4 kLoC) — the primary UI: the GAM
  event loop, screen state machine, and key handlers that run
  on real hardware and inside hosted-mode Xous emulation.
- **`src/stdin_ui/`** — a stdin-driven fallback UI used only
  when `gam_app::run()` errors out (e.g., bare `cargo run`
  outside any Xous environment for sanity-checking the main
  loop). Was a separate crate (`xous-app-signal-ui`) until the
  P3 refactor; the boundary didn't earn its keep.
- **`src/dialogue.rs`** — pure-data conversation summary
  modeling (per-thread last-message, unread counts, ellipsis,
  brief-relative timestamps). Unit-tested.

## Why this crate exists separately

It's the binary. Everything else is a library that this crate
consumes. The binary boundary is also where:

- The `__getrandom_v03_custom` extern is defined (rv32-xous
  needs an in-tree provider for `getrandom 0.3`'s custom
  backend; see `.cargo/config.toml`).
- Cargo's per-binary feature unification happens — the
  `precursor` / `hosted` features cascade out from this crate
  to gam, blitstr2, ux-api, graphics-server, utralib, modals,
  locales.
- `catch_unwind` would live if it ever moved out of the worker
  (today it's inside `xous-signal-worker`).

## Who depends on this crate

Only Cargo and the OS — it's the executable. Nothing in the
workspace or vendored trees should ever `use xous_app_signal::*`.
