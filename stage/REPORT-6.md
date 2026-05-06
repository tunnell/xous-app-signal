# Stage 6 — checkpoint: vendoring infrastructure landed; transport swap is its own session

Status: **Stage 6.0 (vendoring + patch redirects + CDSI off) complete.** Stage 6.1 (the actual reqwest → ureq + reqwest-websocket → tungstenite swap) is the next sub-stage and should be its own focused session given the scope.

## What landed in this checkpoint

### 1. `libsignal-service-rs` vendored at presage's pinned rev

`vendor/libsignal-service-rs/` is `whisperfish/libsignal-service-rs` at rev `782c0d6bf0c4a6ab52f98d7b6d950a13f28f3020` — the rev presage 600c4ed pins. (An earlier attempt vendored HEAD instead, which broke presage's `groups_v2::Group` field expectations.)

### 2. CDSI feature off by default

In `vendor/libsignal-service-rs/Cargo.toml`:

```toml
[features]
default = []   # was: ["cdsi"]
```

Reason: CDSI transitively pulls `boring-sys` (Signal's BoringSSL fork) which doesn't target rv32-xous and needs `libclang` at host build time. Per `REPORT.md` Decision 5.

### 3. Patch redirect from presage's git URL

Workspace `Cargo.toml`:

```toml
[patch."https://github.com/whisperfish/libsignal-service-rs"]
libsignal-service = { path = "vendor/libsignal-service-rs" }
```

`[patch.crates-io]` doesn't redirect git sources, and presage pulls libsignal-service-rs via git, so we need a git-URL patch entry for the redirect to take effect.

### 4. Vendored fork's own `[patch.crates-io].curve25519-dalek` removed

The vendored libsignal-service-rs Cargo.toml originally pinned `curve25519-dalek` to `signalapp/curve25519-dalek` at signal-curve25519-4.1.3. Removed that line because the workspace-level `[patch.crates-io].curve25519-dalek` already redirects everything to our vendored `betrusted-io/curve25519-dalek` + lizard port. Two competing patches for the same crate would have undefined behavior; one wins.

## Hosted-mode verification (passes)

```sh
$ cargo build -p libsignal-service          # ✓ vendored fork compiles
$ cargo build -p presage-store-pddb         # ✓ presage pulls our vendored copy
$ cargo run -p xous-app-signal --bin xas    # ✓ "got: hello"
$ cargo run --example https_get             # ✓ HTTP/1.1 200 OK
$ cargo run --example signal_ws_keepalive   # ✓ 101 + 93-byte frame
$ cargo fmt --all -- --check                # ✓ clean (our crates)
$ cargo clippy --workspace --all-targets -- -D warnings   # ✓ clean
```

`cargo fmt` emits two warnings about rustfmt unstable features when checking the vendored libsignal-service-rs (its own `rustfmt.toml` requests nightly-only options); these don't fail the check. Our crates pass.

## What Stage 6.1 (next session) actually has to do

The transport swap is the real work. Concrete touchpoint count from this codebase:

- **44 `reqwest::` / `reqwest_websocket::` references** across `vendor/libsignal-service-rs/src/`.
- **~3 kLoC** in the push_service + websocket modules combined.

The shape of the swap:

### A. HTTP request layer (`reqwest::Client` → `HttpClient` trait + `ureq` impl)

`PushService` at [`src/push_service/mod.rs:80`](https://github.com/whisperfish/libsignal-service-rs/blob/main/src/push_service/mod.rs#L80) holds a `reqwest::Client`. Every HTTP method (`PushService::request`, `PushService::get_attachment`, etc.) constructs a `reqwest::RequestBuilder` and `.send().await`s it. The patch:

1. Define a thin `HttpClient` trait in libsignal-service-rs:
   ```rust
   pub trait HttpClient: Send + Sync + 'static {
       async fn execute(&self, method: Method, url: Url, headers: HeaderMap, body: Body, auth: Option<HttpAuth>)
           -> Result<HttpResponse, ServiceError>;
   }
   ```
2. Replace `reqwest::Client` field with `Arc<dyn HttpClient>`.
3. Rewrite each request method to construct an `HttpClient::execute()` call.
4. Provide `UreqHttpClient` impl in this workspace (in `crates/xous-net-bridge/`) that calls `ureq` synchronously, wrapped in `blocking::unblock` or a worker thread + channel for the async-side caller.
5. Provide `MockHttpClient` for tests.

### B. WebSocket layer (`reqwest_websocket::WebSocket` → `Stream<Frame> + Sink<Frame>` over `async-channel`)

`SignalWebSocket` at [`src/websocket/mod.rs:79`](https://github.com/whisperfish/libsignal-service-rs/blob/main/src/websocket/mod.rs#L79) wraps `reqwest_websocket::WebSocket`. `SignalWebSocketProcess::run()` (line 222) drives the WS via `futures::select!`. The patch:

1. Replace `WebSocket` field with `Sender<Frame>` (outgoing) + `Receiver<Frame>` (incoming) — `async-channel` types.
2. Replace `tokio::time::interval_at` with a `futures-timer::Delay`-driven loop in the keepalive arm.
3. Replace `tokio::time::Instant` with `std::time::Instant`.
4. Provide a sync `tungstenite` worker thread in `crates/xous-net-bridge/` (using our existing `ws_connect`) that pumps frames between the channel ends and the real WS stream.

### C. The `tokio::task::spawn(task)` at `push_service/mod.rs:184`

```rust
let (ws, task) = SignalWebSocket::new(ws, keepalive_path.to_owned(), unidentified_push_service);
let task = task.instrument(span);
tokio::task::spawn(task);
```

Refactor: `SignalWebSocket::new()` returns the task to the caller; the caller spawns it on `LocalExecutor`. The few callers inside libsignal-service-rs that use `PushService::ws(...)` need to be updated to thread the task through.

## Why this is its own session

Each of A, B, C is a multi-hour patch. The patches are also coupled — A's `HttpClient` trait and B's channel-Stream interact at the `SignalWebSocket::new` constructor. Doing them piecemeal across sessions risks half-states where neither hosted-mode tests nor rv32 cross-compile pass.

**Recommendation:** the next session does Stage 6.1 (transport swap A+B+C as a coordinated patch series, target ~2 kLoC diff), tests hosted-mode, then immediately Stage 7 (presage tokio-removal — much smaller, ~30 lines per `REPORT.md` Decision 2). Then verify `cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb` passes.

## Files changed (since `f6792b6`)

```
modified:
  Cargo.toml                                          (+[patch.<git>] for libsignal-service-rs; +workspace member; +exclude)
  Cargo.lock                                          (regenerated)

new (vendored):
  vendor/libsignal-service-rs/                        (whole tree at rev 782c0d6, our fork)

modified (vendored):
  vendor/libsignal-service-rs/Cargo.toml              (default = [] instead of ["cdsi"]; removed redundant curve25519-dalek patch)

new:
  stage/REPORT-6.md                                   (this file)
```

## Open questions / things to revisit

1. **The 2 kLoC diff target may be optimistic.** Original REPORT.md Decision 3 estimated "≤2 kLoC". Actual count of reqwest references is 44 across 3.1 kLoC of push_service+websocket. Once Stage 6.1 starts, recount and adjust if needed; ROADMAP `Stop conditions` say to surface if the diff exceeds 3 kLoC.

2. **`tungstenite 0.21` (Stage 3) vs new tungstenite usage in libsignal-service-rs.** Stage 3 picked tungstenite 0.21 to avoid `getrandom 0.3`. The transport swap will use the same dep — confirm rv32 still builds after the swap.

3. **rustfmt warnings from the vendored fork** are noise only. Add a `.gitignore` or rustfmt.toml override to suppress them if they become a recurring annoyance.
