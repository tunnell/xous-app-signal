# crates/

xas's first-party Rust crates. Vendored upstream forks
(`presage`, `libsignal-service-rs`, `curve25519-dalek`) live in
[`../vendor/`](../vendor/). For the architectural rationale
behind this split, see
[`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md).

## Why a multi-crate workspace

xas is a single binary, but the code is split across multiple
crates for three concrete reasons:

1. **Build-time isolation of the Xous IPC bypass crates.** The
   upstream `services/pddb` and `services/modals` crates pull in
   10+ Xous services as path-deps and force a full xous-core
   workspace rebuild on every change. Hand-rolled IPC clients
   (`xous-pddb-ipc`, `xous-modals-ipc`) replicate just the wire
   protocol we need, in their own crates, so iterating on them
   doesn't trigger the cascade.
2. **Trait-impl boundary for the storage layer.** presage defines
   storage traits; `presage-store-pddb` implements them against
   PDDB. Keeping it in its own crate makes the trait surface
   explicit and means the Signal-protocol-bearing code
   (`xous-signal-bridge`) doesn't reach into PDDB internals.
3. **Bridge-vs-app separation.** The `xous-signal-bridge` crate
   owns the worker thread + LocalExecutor that runs presage; the
   `xous-app-signal` binary owns the GAM-rendered UI and talks
   to the bridge over async channels. This split is what lets
   the same UI code run in both hosted Xous emulation and on
   rv32 hardware unchanged.

## Crates in this folder

| Crate | LoC* | Purpose |
|---|---|---|
| [`xous-app-signal`](xous-app-signal/) | ~2.4k | Binary entry point (binary name: `xas`). Spawns the bridge worker, renders UI via GAM, dispatches keys → `Cmd`s and `Event`s → screen updates. |
| [`xous-app-signal-ui`](xous-app-signal-ui/) | ~2.0k | Stdin-driven UI fallback used in `main.rs` when `gam_app::run()` can't reach a Xous server (e.g., bare `cargo run` standalone for unit testing). The Xous-rendered UI in `xous-app-signal/src/gam_app.rs` is the primary path. |
| [`xous-signal-bridge`](xous-signal-bridge/) | ~1.3k | Glue between `presage::Manager` (running on a worker thread inside a `LocalExecutor`) and the rest of the app. Defines the `Cmd` / `Event` enums that flow over async channels. Where `catch_unwind` lives so panics in libsignal don't kill the worker. |
| [`presage-store-pddb`](presage-store-pddb/) | ~3.0k | Implements presage's `Store` + `IdentityKeyStore` + (a dozen) other storage traits over PDDB. Has a hosted-mode `backend_mock` for unit tests and a `backend_pddb` for real use. The biggest crate by line count, mostly because the trait surface is wide. |
| [`xous-net-bridge`](xous-net-bridge/) | ~0.6k | Sync TLS + WebSocket transport, bridged to async via channels (`ws_pump`). This is what libsignal-service-rs's HTTP/WS code calls into. The keepalive race documented in the upstream PR draft #2 lives in `ws_pump.rs`. |
| [`xous-pddb-ipc`](xous-pddb-ipc/) | ~0.8k | Hand-rolled PDDB IPC client (rv32-xous only). Replicates just the wire protocol `presage-store-pddb`'s `KvBackend` needs, instead of pulling in the full `services/pddb` crate (which would drag in 10+ other services as path deps and rebuild xous-core on every iteration). |
| [`xous-modals-ipc`](xous-modals-ipc/) | ~0.2k | Hand-rolled Modals IPC client (rv32-xous only). Same rationale as `xous-pddb-ipc`: replicates the wire surface of `services/modals` for the one operation we need (`show_notification` with optional QR-code overlay), bypassing the GAM dep cascade. |

\* LoC = `wc -l src/**/*.rs` rounded; for orientation, not load-bearing.

## Map to the architecture doc

[`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) lays out the
runtime architecture (worker thread, async channel bridge,
GAM UI loop). The crates here map to its sections roughly as:

- **Section "Worker + bridge"** → `xous-signal-bridge`
- **Section "Storage"** → `presage-store-pddb` + `xous-pddb-ipc`
- **Section "Transport (ws_pump)"** → `xous-net-bridge`
- **Section "UI"** → `xous-app-signal/src/gam_app.rs`
  + `xous-modals-ipc` (for QR-code modal) + `xous-app-signal-ui`
  (fallback)

If a crate's purpose feels unclear after reading this table,
that's a signal worth recording — the architecture-review chore
in `~/code/xas/CHORES.md` ("Architecture review: revisit code
layout post-MVP") is the right place to surface it.
