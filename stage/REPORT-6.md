# Stage 6 — `libsignal-service-rs` transport fork

Status: **6.0 + 6.1 phases 1, 2, 3a complete.** Phases 3b–3f (the substantive reqwest/reqwest-websocket replacement + ureq+tungstenite impls) are next session's work; foundations are in place.

## What landed

### Stage 6.0 (commit `bcf158a`) — vendoring infrastructure

- `vendor/libsignal-service-rs/` at rev `782c0d6bf0c4a6ab52f98d7b6d950a13f28f3020` (the rev presage 600c4ed pins).
- `default = []` instead of `["cdsi"]` so boring-sys (BoringSSL) is not pulled.
- Workspace `[patch."https://github.com/whisperfish/libsignal-service-rs"]` redirect.

### Stage 6.1 phase 1 (commit `b09f81a`) — type-only `reqwest::*` → `http::*`

15 files, mechanical: `reqwest::Method` and `reqwest::StatusCode` are re-exports of types from the `http` crate. Switching imports doesn't change the binary, just trims one layer of reqwest-coupling and makes the eventual `reqwest::Client` removal smaller.

### Stage 6.1 phase 2 (commit `efdfa0d`) — `tokio::time::*` removed from `websocket/mod.rs`

`tokio::time::Instant` (just timestamps) → not needed; `tokio::time::interval_at` (recurring keepalive) → `futures_timer::Delay` reset on each tick. Same external behaviour. Adds `futures-timer` git dep.

### Stage 6.1 phase 3a (commits `dc95be4`, `66211f9`) — `HttpClient` trait + thread-local handle

New file `vendor/libsignal-service-rs/src/transport.rs` (~290 LoC, `#![allow(dead_code)]` until phase 3b uses the types):

- `pub trait HttpClient` — `async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>`. `#[async_trait(?Send)]` for single-threaded executor friendliness.
- `HttpRequest`, `HttpResponse`, `BasicAuth`, `HttpError` types.
- `RequestBuilder` — fluent builder mirroring the subset of `reqwest::RequestBuilder` used (method, url, body, json, header, basic_auth, timeout, send).
- `HttpResponse` methods — status, headers, json, bytes, text, error_for_status — mirroring the subset of `reqwest::Response` used.
- `Certificate::from_pem` — replaces `reqwest::Certificate::from_pem`.
- Thread-local `HTTP_CLIENT` + `set_http_client(Arc<dyn HttpClient + Send + Sync>)` + internal `get_http_client()`. Mirrors the `presage::set_executor` pattern from Stage 7.

Module is wired into `lib.rs`; types compile but no callsites use them yet.

## What's still ahead

### Phase 3b — replace `reqwest::Client` field in `PushService` + rewrite ~14 callsites

`PushService::client: reqwest::Client` → `Arc<dyn HttpClient + Send + Sync>` (sourced from `transport::get_http_client()`). `PushService::request` returns our `RequestBuilder` instead of `reqwest::RequestBuilder`. ~14 callsites across `push_service/{cdn,linking,response}.rs` + `groups_v2/manager.rs` + `account_manager.rs` use `.send().await?.json()` etc.; those are mostly source-compatible with the new `HttpResponse` because the method names match.

Open issues at this phase:

1. **`PushService::ws()`** at `push_service/mod.rs:142-186` uses `self.client.get(url).upgrade().send().await?.into_websocket()` — that's `reqwest_websocket`'s API and depends on the `reqwest::Client` field still being present. Phase 3b ends with `ws()` broken until phase 3c lands. Options: (a) keep both fields temporarily (`client: reqwest::Client` for ws + `http: Arc<dyn HttpClient>` for REST) — defeats the dep-removal purpose; (b) defer phase 3b until phase 3c is also ready; (c) add a stub WS construction that the caller doesn't actually invoke until phase 3c. **Pick (b)**: do 3b and 3c as a coordinated patch series in one session, since they share the WS-related cleanup.

2. **`push_service::cdn` multipart**. `Form::new().part(...)` with file_name + mime is used at `cdn.rs:309` for attachment uploads. Our `RequestBuilder` doesn't have multipart support yet. Either add a `multipart` module (~150 LoC; build the multipart/form-data body manually with our own boundary), or skip attachment upload for MVP and add the Form support later.

### Phase 3c — replace `reqwest_websocket::WebSocket` with channel-backed Stream/Sink

`SignalWebSocketProcess.ws: WebSocket` (line 117) → `(Sender<Frame>, Receiver<Frame>)` from `async-channel`. The `process_frame`, `run`, etc. methods at `websocket/mod.rs:120-348` need adaptation. The actual sync `tungstenite` pump runs in `xous-net-bridge` (phase 3e); libsignal-service-rs only sees the channel ends.

Also: refactor `SignalWebSocket::new()` to *return* the run-task to caller instead of `tokio::task::spawn(task)`-ing it (line 184).

### Phase 3d — `UreqHttpClient` impl in `xous-net-bridge`

A `pub struct UreqHttpClient { ... }` with `impl HttpClient for UreqHttpClient`. Uses `ureq` (sync HTTP/1.1 + rustls native) wrapped in `blocking::unblock` or a worker thread + channel for the `?Send` future. Pinned CA via `rustls::ClientConfig::with_root_certificates(signal_production_roots())` — already have those helpers from Stage 3.

Estimated ~150 LoC.

### Phase 3e — WS pump impl in `xous-net-bridge`

A worker thread that holds a sync `tungstenite::WebSocket<rustls::StreamOwned<...>>` and pumps frames between an `async-channel` (executor side) and the WS (sync side). Uses our existing `xous_net_bridge::ws_connect` (Stage 3) for the handshake.

Estimated ~150 LoC.

### Phase 3f — verify rv32 cross-compile of `presage-store-pddb` passes

The whole point. After phases 3b–3e, `cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb` should pass for the first time — closing the loop on the original "rv32 verification per stage" goal.

## Recommendation for next session

Do phases 3b + 3c **together** as one coordinated patch series, then 3d + 3e in `xous-net-bridge`, then run 3f. Total estimated diff: ~1 kLoC libsignal-service-rs fork + ~300 LoC xous-net-bridge impls. Achievable in one focused 2–3 hour session.

Stop conditions for the next session:
- If the diff exceeds 2 kLoC in libsignal-service-rs alone, surface — abstraction is wrong.
- If a callsite refuses to migrate cleanly (e.g., stream-style chunked response handling), surface and discuss before forcing it.
- If multipart turns out to take more than ~200 LoC, stop and decide if MVP can ship without attachment upload (the user-facing flows that need it are: send 1:1 with attachment — Stage 12 — and avatar upload — post-MVP).

## Verification at this checkpoint (all pass)

```sh
$ cargo run -p xous-app-signal --bin xas              # ✓ "got: hello"
$ cargo run --example https_get -p xous-net-bridge    # ✓ HTTP/1.1 200 OK
$ cargo run --example signal_ws_keepalive -p xous-net-bridge   # ✓ 101 + 94B frame
$ cargo build -p libsignal-service                    # ✓ clean (transport module compiles)
$ cargo build -p presage-store-pddb                   # ✓ clean (full Whisperfish stack)
$ cargo tree --workspace -d                           # ✓ no duplicates
$ cargo fmt --all -- --check                          # ✓ clean for our crates
$ cargo clippy --workspace --all-targets -- -D warnings   # ✓ clean
```

rv32 still gated on phases 3b–3e — same as before this session. The work in 6.1.{1,2,3a} doesn't unblock rv32 by itself; that requires the full reqwest+reqwest-websocket removal.

## Open follow-ups (carry forward)

1. **Multipart attachment upload (`cdn.rs:309`)** — decide MVP scope before writing.
2. **The `tokio::task::spawn(task)` at `push_service/mod.rs:184`** — needs the API refactor where `SignalWebSocket::new()` returns the task. Lands during phase 3c.
3. **Two duplicate-version warnings** from earlier (`thiserror v1/v2`, `tungstenite v0.21/v0.24`) likely resolve in phase 3c when the duplicate `tungstenite` path goes away.

## Files changed (since last REPORT-6.md, commit `bcf158a`)

```
modified:
  Cargo.toml                                          (no change)
  Cargo.lock                                          (regenerated for futures-timer + http)

vendored, modified:
  vendor/libsignal-service-rs/Cargo.toml              (+http "1", +futures-timer git pin)
  vendor/libsignal-service-rs/src/proto.rs            (reqwest::StatusCode → http::StatusCode)
  vendor/libsignal-service-rs/src/account_manager.rs  (reqwest::Method → http::Method)
  vendor/libsignal-service-rs/src/groups_v2/manager.rs (same)
  vendor/libsignal-service-rs/src/push_service/{mod,linking}.rs (same)
  vendor/libsignal-service-rs/src/websocket/{mod,account,directory,keys,linking,profile,registration,request,usernames}.rs (same)
  vendor/libsignal-service-rs/src/lib.rs              (+pub mod transport)
  vendor/libsignal-service-rs/src/websocket/mod.rs    (tokio::time::* → futures_timer::Delay)

new:
  vendor/libsignal-service-rs/src/transport.rs        (~290 LoC: HttpClient trait + types + thread-local)
  stage/REPORT-6.md                                   (this file, replacing the previous 6.0-only one)
```
