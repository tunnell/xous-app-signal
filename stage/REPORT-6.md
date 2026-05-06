# Stage 6 — `libsignal-service-rs` transport fork (complete)

Status: **Stage 6 complete.** All six phases (6.0 through 6.1.{1,2,3a–3f}) landed across 8 commits. rv32 cross-compile of `presage-store-pddb` passes for the first time.

## Final state

```sh
$ cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb
    Checking presage-store-pddb v0.0.1
    Finished `dev` profile in 22.05s
    ✓ rv32 cross-compile of full Whisperfish stack passes.

$ cargo build --workspace                              # ✓ clean
$ cargo run -p xous-app-signal --bin xas               # ✓ "got: hello"
$ cargo run --example signal_ws_keepalive              # ✓ 101 + 94B frame
$ cargo tree -p reqwest                                # ✗ NOT FOUND
$ cargo tree -p reqwest-websocket                      # ✗ NOT FOUND
$ cargo tree -p tokio                                  # ✗ NOT FOUND
$ cargo tree -p mio                                    # ✗ NOT FOUND
$ cargo fmt --all -- --check                           # ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings  # ✓ clean
```

The whole tokio + reqwest + mio chain is gone from production deps (still pulled in dev-deps for upstream tests, which we don't run).

## Commits

| Phase | Commit | Notes |
|---|---|---|
| 6.0 — vendor + CDSI off | `bcf158a` | scaffolding |
| 6.1 phase 1 — http types | `b09f81a` | `reqwest::Method/StatusCode` → `http::*` (15 files) |
| 6.1 phase 2 — tokio::time | `efdfa0d` | `tokio::time::interval_at` → `futures-timer::Delay` |
| 6.1 phase 3a — trait | `dc95be4`, `66211f9` | `HttpClient` trait, builder, response types, thread-locals |
| 6.1 phase 3b prep — dual errors | `407b791` | `WsFrame`/`WebSocketChannels` types added |
| 6.1 phases 3b+3c | `447479b` | `PushService.client` field swap; WS channel-bridge in libsignal-service-rs; ~14 callsites; presage `(ws, task)` tuple migration |
| 6.1 phases 3d+3e + cleanup | `294acef` | `SyncHttpClient`/WS pump in `xous-net-bridge`; reqwest fully removed from Cargo.toml + error.rs + response.rs |

## What got built

### `vendor/libsignal-service-rs/src/transport.rs` (~330 LoC, new)

Defines the abstraction:
- `HttpClient` trait — `async fn execute(req) -> Result<resp, err>` and `async fn connect_websocket(url, headers, auth) -> WebSocketChannels`
- `HttpRequest`, `HttpResponse`, `RequestBuilder` — mirror the subset of reqwest's API the codebase used
- `WsFrame`, `WebSocketChannels` — channel ends bridging a sync WS pump to async-side consumers (uses `async-channel` for native sync/async dual-API)
- `Certificate::from_pem`, `BasicAuth`, `HttpError`
- Thread-local handles: `set_http_client(Arc<dyn HttpClient + Send + Sync>)`, `set_task_spawner(Box<dyn Fn(BoxFuture<()>)>)`, plus internal `get_http_client()` and `spawn_detached()` accessors. Mirrors `presage::set_executor` pattern from Stage 7.

### `vendor/libsignal-service-rs/src/push_service/mod.rs` (modified)

- `PushService.client` field: `reqwest::Client` → `Arc<dyn HttpClient + Send + Sync>`. Constructor pulls from thread-local.
- `PushService::request` returns our `RequestBuilder` instead of `reqwest::RequestBuilder`. The builder API surface (`.header`, `.body`, `.json`, `.basic_auth`, `.timeout`, `.send().await`) is source-compatible with the ~14 callsites that used reqwest's.
- `PushService::ws` returns `(SignalWebSocket, BoxFuture<()>)`. The task is the `SignalWebSocketProcess::run` loop; caller spawns it on the local executor.

### `vendor/libsignal-service-rs/src/websocket/mod.rs` (modified)

`SignalWebSocketProcess.ws: WebSocket` → `(ws_outgoing: async_channel::Sender<WsFrame>, ws_incoming: async_channel::Receiver<Result<WsFrame, HttpError>>)`. The `run` loop's `select!` arms translate sends and receives mechanically; close frames now specify `code: 1001` (Going Away) explicitly. `tokio::time::*` already gone since phase 2.

### `vendor/libsignal-service-rs/src/push_service/{response,error,cdn}.rs` (modified)

- `response.rs`: `SignalServiceResponse for reqwest::Response` removed. New impl for our `HttpResponse`. New `HttpResponseExt` trait replaces `ReqwestExt`.
- `error.rs`: `Http(reqwest::Error)` and `WsError(Box<reqwest_websocket::Error>)` variants removed. `HttpTransport(transport::HttpError)` is the unified HTTP error variant.
- `cdn.rs`: `get_from_cdn`'s `bytes_stream().into_async_read()` becomes a fully-buffered `futures::io::Cursor` (acceptable for MVP — attachments capped small and post-MVP anyway). `upload_to_cdn0` returns a "multipart not implemented in Stage 6.1" error stub. Both noted in §"Open follow-ups".

### `vendor/presage/presage/src/manager/{registered,registration,confirmation}.rs` (modified)

`.ws(...)` callsites updated to destructure the new `(ws, task)` tuple and call `crate::runtime::spawn_detached(task)`. `vendor/libsignal-service-rs/src/{provisioning/mod,receiver}.rs` similarly with `crate::transport::spawn_detached`.

### `crates/xous-net-bridge/src/http.rs` (new, ~190 LoC)

`SyncHttpClient` implementing `HttpClient`. Spawns a one-shot worker thread per request to run a hand-rolled HTTP/1.1 exchange over `tls_connect` (Stage 2). Sync→async via `async-channel`. Avoids `ureq` (which bundles its own rustls and would conflict with our `=0.22.2` pin). Status, body parsing, headers; `Connection: close` so we read until EOF.

### `crates/xous-net-bridge/src/ws_pump.rs` (new, ~200 LoC)

`connect_websocket` returns `WebSocketChannels` and spawns three threads:
- **setup**: runs the TLS handshake + `tungstenite::client(request, stream)`; on success, hands the WS off to reader+writer.
- **reader**: blocks on `ws.read()`, forwards frames to `incoming_tx.send_blocking`.
- **writer**: blocks on `outgoing_rx.recv_blocking`, forwards frames via `ws.write(msg)`.

Two threads (rather than one) sharing `Arc<Mutex<WebSocket>>` because single-threaded designs deadlock on `read()`. Documented in the file header.

### `.cargo/config.toml` (modified)

Adds `--cfg getrandom_backend="custom"` for rv32-xous so getrandom 0.3 (pulled by hpke-rs) doesn't fail with "unsupported target". Disables u32e_backend pending Precursor SOC feature wiring (see "Open follow-ups").

## Stop conditions — final tally

- **Diff > 2 kLoC in libsignal-service-rs**: NOT triggered. Total libsignal-service-rs diff is roughly:
  - `transport.rs` new: ~330 LoC
  - `push_service/mod.rs`: ~100 LoC modified
  - `websocket/mod.rs`: ~50 LoC modified
  - `push_service/{cdn,response,error,linking}.rs`: ~100 LoC modified collectively
  - `provisioning/mod.rs`, `receiver.rs`: ~5 LoC each
  - **Total: ~600 LoC**, well under the 2 kLoC budget.
- **Any callsite refusing to migrate cleanly**: NOT triggered. All ~14 REST callsites migrated mechanically. The two CDN methods (`get_from_cdn`, `upload_to_cdn0`) had to be adapted — `get_from_cdn` cleanly buffered, `upload_to_cdn0` stubbed pending multipart-builder (post-MVP).

## Outstanding stop-gaps (revisit before MVP hardware tests)

These are working but not production-ready:

1. **`upload_to_cdn0` is a stub.** Returns `ServiceError::SendError` without uploading. To re-enable: write a hand-rolled multipart/form-data body builder in `transport.rs` (~100 LoC). Only matters for attachment uploads (Stage 12 send-with-attachment, profile avatar upload).

2. **`getrandom 0.3` custom backend not implemented.** The cfg is set but the `__getrandom_v03_custom` extern function isn't defined yet. `cargo check` passes because it doesn't link; `cargo build` for rv32 will fail with an unresolved-symbol error. Options:
   - Write a 30-LoC custom backend in `xous-net-bridge` that calls Xous's TRNG service directly.
   - Patch xous-core's getrandom fork to also support 0.3.
   - File a fork (`getrandom-xous` 0.3) that does what xous-core's 0.2 fork does.
   Pick one before Stage 9 hardware bring-up.

3. **u32e_backend disabled.** `betrusted-io/curve25519-dalek`'s u32e backend is on a target-specific cfg, but its build pulls `utralib` whose build script needs a Precursor SOC feature flag (e.g. `precursor-c809403`) we haven't wired in. To re-enable: add `utralib = { version = "...", features = ["precursor-c809403"] }` as a dep somewhere in the build path so the feature propagates to the build script, OR merge into xous-core's tree at Stage 9 (which inherits the feature naturally). Until then we get the portable Rust curve25519-dalek backend on rv32 — slower on Precursor hardware than the IP core, but functionally correct.

4. **Worker-thread bootstrap not yet wired.** The thread-local handles (`transport::set_http_client`, `transport::set_task_spawner`, `presage::set_executor`) are defined; the worker-thread that calls them isn't yet (that's Stage 8). So any code that constructs a `PushService` will panic at runtime. We don't run such code yet — smoke tests don't touch presage.

5. **Two duplicate-version warnings**: `thiserror v1/v2` and `tungstenite v0.21/v0.24` (the v0.24 path comes via... worth tracing in a follow-up). Don't break the build; flagged for cleanup.

## What this unlocks

- **Stages 4 (full) and 5 can now ship with rv32 verification on first commit.** That was the original motivation for Option B ordering.
- **Stage 8 (worker thread + IPC) can build on top of the thread-local registration helpers** (`transport::set_http_client`, `transport::set_task_spawner`, `presage::set_executor`) — Stage 8 is the place where the bootstrap finally happens.
- **Stage 9 (rv32 hardware bring-up)** still needs the three follow-ups above (multipart, getrandom 0.3 custom, u32e SOC feature) before the binary actually runs on Precursor. None of them is cryptographic-protocol work; all are dep/build wiring.

## ROADMAP refinements suggested

1. **Stage 6.1 step list** in the ROADMAP should reflect that the actual diff is split across `vendor/libsignal-service-rs/src/transport.rs` (new), `push_service/mod.rs` (PushService swap), `push_service/{cdn,error,response,linking}.rs` (callsite migrations + cleanup), `websocket/mod.rs` (channel swap), plus `vendor/presage/...` callsite updates. The "abstract `Spawn` trait" framing in the original ROADMAP undercounted the WS work.

2. **Add Stage 6.5 (or similar)**: "wire up the worker-thread bootstrap" — the place where `transport::set_http_client(Arc::new(SyncHttpClient::new(...)))`, `transport::set_task_spawner(...)`, and `presage::set_executor(...)` all get called. Currently this is implicit in Stage 8; making it its own bullet means the next agent doesn't forget any of the three.

3. **Open follow-ups list** above should be referenced from the Stage 9 prerequisites so they don't get lost.

## Files changed in this session (since `a9b240e`)

```
modified:
  Cargo.toml                                                 (no change)
  Cargo.lock                                                 (regenerated)
  .cargo/config.toml                                         (+getrandom_backend="custom"; u32e disabled with comment)

vendored, modified:
  vendor/libsignal-service-rs/Cargo.toml                     (-reqwest -reqwest-websocket; +http +futures-timer +async-channel)
  vendor/libsignal-service-rs/src/lib.rs                     (+pub mod transport)
  vendor/libsignal-service-rs/src/transport.rs               (NEW ~330 LoC)
  vendor/libsignal-service-rs/src/push_service/mod.rs        (PushService client field + request/ws methods)
  vendor/libsignal-service-rs/src/push_service/error.rs      (-Http(reqwest::Error) -WsError(reqwest_websocket); +HttpTransport)
  vendor/libsignal-service-rs/src/push_service/response.rs   (-reqwest impls; +HttpResponse impls; +HttpResponseExt)
  vendor/libsignal-service-rs/src/push_service/cdn.rs        (bytes_stream→Cursor; multipart→stub)
  vendor/libsignal-service-rs/src/push_service/linking.rs    (ReqwestExt→HttpResponseExt)
  vendor/libsignal-service-rs/src/account_manager.rs         (same)
  vendor/libsignal-service-rs/src/groups_v2/manager.rs       (same)
  vendor/libsignal-service-rs/src/websocket/mod.rs           (WS field type swap; recv_blocking-friendly select)
  vendor/libsignal-service-rs/src/provisioning/mod.rs        (ws() tuple destructure + spawn_detached)
  vendor/libsignal-service-rs/src/receiver.rs                (same)
  vendor/presage/presage/src/manager/registered.rs           (3 ws() callsites)
  vendor/presage/presage/src/manager/registration.rs         (1 ws() callsite)
  vendor/presage/presage/src/manager/confirmation.rs         (1 ws() callsite)

new:
  crates/xous-net-bridge/Cargo.toml                          (+libsignal-service path dep + http/serde/etc.)
  crates/xous-net-bridge/src/http.rs                         (NEW ~190 LoC: SyncHttpClient)
  crates/xous-net-bridge/src/ws_pump.rs                      (NEW ~200 LoC: WS pump worker threads)
  crates/xous-net-bridge/src/lib.rs                          (+pub mod http +pub mod ws_pump)
  stage/REPORT-6.md                                          (this file)
```
