# xous-modals-ipc

Hand-rolled Modals IPC client. **rv32-xous-only.** Same
rationale as `xous-pddb-ipc` (bypass the upstream service-crate
dep cascade), scaled down to just the one operation we need.

## Why this exists

The upstream `xous-core/services/modals` crate pulls in the
full GAM + ux-api + locales tree as path-deps (modals are
GAM-rendered UI elements). For xas's auto-link path we only
need to call `Modals::show_notification()` with optional QR-code
overlay, so re-implementing the wire protocol for that one
opcode is cheaper than pulling in the cascade.

The interactive UI (which uses many more modals operations:
TextEntry, ListPicker, Sliders, etc.) uses the upstream
`services/modals` crate directly via `xous-app-signal`'s
unconditional dep — see `crates/xous-app-signal/Cargo.toml`.
This crate is just for the auto-link's startup code, where
keeping the dep cascade out of the workspace matters more.

## What's here

- **`src/lib.rs`** — `ManagedNotification` message struct (3
  fields: token, message text, optional QR text) + a tiny send
  function. ~155 LoC total.

## Who depends on this crate

- `xous-app-signal` — only inside `#[cfg(feature = "auto-link", target_os = "xous")]`
  paths in `main.rs::auto_link`.

## Naming note

`xous-modals-ipc` is the IPC client name; the operation it
exposes is `show_notification` (with optional QR). Don't
mistake this for a wire-protocol client to GAM itself — it's
just to modals-the-service.
