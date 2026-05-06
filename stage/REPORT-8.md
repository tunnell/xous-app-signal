# Stage 8 — Manager worker thread + IPC scaffolding

Status: **complete.** First end-to-end stack glue. The `xas` binary
constructs a `PddbStore`, spawns a `presage::Manager` worker thread,
exchanges three IPC commands (Hello / GetWhoami / Shutdown) over
async-channel, and exits cleanly. 3 worker tests pass; rv32
cross-compile of the entire stack passes; fmt + clippy clean.

This is the "everything compiles together" milestone. After this
stage, the only thing missing before bringing up on Renode (Stage 9)
is the real `pddb::Pddb` backend behind `KvBackend` and the Stage 6.1
follow-ups (`getrandom 0.3`, `upload_to_cdn0`, `u32e_backend`).

## Verification (hosted)

```sh
$ cargo run -p xous-app-signal --bin xas
xas: starting
xas: worker started
xas: pong
xas: whoami err (expected): this client is not yet registered, please register or link as a secondary device
xas: worker shut down
xas: exiting

$ cargo test -p xous-signal-bridge
running 3 tests
test tests::dropping_cmd_channel_shuts_down_cleanly ... ok
test tests::hello_pong_round_trip ... ok
test tests::whoami_returns_error_on_empty_store ... ok
test result: ok. 3 passed; 0 failed; 0 ignored

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ Full rv32 cross-compile of the entire stack glue —
  presage + libsignal + libsignal-service-rs + presage-store-pddb +
  xous-signal-bridge + xous-app-signal. Strongest pre-Stage-9
  sanity check we have.

$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean
$ cargo tree --workspace -d                               ⚠ same as Stage 6;
                                                            no new dups
```

The `whoami err` line is the load-bearing test — it confirms that
`Manager::load_registered<S: Store>` accepts our `PddbStore` (so the
trait wiring is correct) and that the error path returns over the
async-channel without deadlocking the executor.

## Crate layout

```
crates/xous-signal-bridge/
├── Cargo.toml                    +5 deps (presage, presage-store-pddb,
│                                  async-executor, async-channel,
│                                  futures-lite, tracing)
└── src/
    ├── lib.rs        (190 LoC)   run_signal_worker + worker_main +
    │                              handle_whoami + 3 integration tests
    └── cmd.rs        ( 56 LoC)   Cmd enum (Hello, GetWhoami, Shutdown)
                                  Event enum (Pong, Whoami, ShuttingDown)

crates/xous-app-signal/
├── Cargo.toml                    deps swap: + xous-signal-bridge,
│                                  presage-store-pddb, async-channel
│                                  - smol primitives (moved to bridge)
└── src/
    └── main.rs       ( 75 LoC)   Construct PddbStore, spawn worker,
                                  Hello/Pong → GetWhoami/Whoami →
                                  Shutdown/ShuttingDown, join, exit
```

## Design choices

### Local executor + 4 MB worker stack

`async_executor::LocalExecutor` rather than `Executor` (multi-thread)
because presage's storage traits use `#[async_trait(?Send)]`
(`vendor/libsignal-service-rs` /
[`libsignal/protocol/src/storage/traits.rs:48`](https://github.com/signalapp/libsignal/blob/v0.91.0/rust/protocol/src/storage/traits.rs#L48)).
A multi-thread executor would force every spawned future to be
`Send`, which our store's session cache (and several presage
internals) cannot guarantee.

The executor is `Box::leak`-ed to `'static` so spawned tasks can hold
borrows for the lifetime of the worker thread. Same pattern Stage 1's
smoke test established. `LocalExecutor::new()` is `!Send` so the leak
also has the side benefit of pinning it to this thread for life.

Stack size starts at 4 MB. Stage 8 doesn't exercise zkgroup or Kyber
keygen (the heaviest compute presage triggers), so this is comfortable
headroom. If a real-flow stage finds it short the constant in
`lib.rs:35` is the single point to bump.

### Channel-shaped IPC

`Cmd { Hello, GetWhoami, Shutdown }` flows main → worker;
`Event { Pong, Whoami(Result<String, String>), ShuttingDown }`
flows worker → main. async-channel's sync/async duality
(`send_blocking`/`recv_blocking` from sync, `send`/`recv` from async)
lets the same channel pair work from both sides — main thread uses the
sync API; the worker uses the async API inside the executor.

`Whoami(Result<String, String>)` rather than `Whoami(Result<...,
presage::Error>)` for two reasons:

1. The Stage 9+ Xous IPC boundary will force stringification anyway
   (no trait-object error types over the wire).
2. The UI ultimately needs strings — pushing the conversion to the
   worker boundary keeps the main-thread / UI code free of presage
   type-knowledge.

### Two shutdown paths

Explicit `Cmd::Shutdown` → `Event::ShuttingDown` is the polite path.
Dropping `cmd_tx` is the implicit path; the worker's `recv().await`
returns `Err(RecvError)` and the loop breaks. Both join cleanly. The
`dropping_cmd_channel_shuts_down_cleanly` test exercises the implicit
path so we're confident a panicking main thread won't leave the
worker hanging.

### No transport handles set yet

`xous-net-bridge` provides `SyncHttpClient` (Stage 6.1) and would be
registered via `presage::libsignal_service::transport::set_http_client`
inside the worker thread — but Stage 8 doesn't need it. `Cmd::GetWhoami`
fails inside `Manager::load_registered` (which only touches the store)
before any transport call. Real-network commands (Stage 10+) will set
the thread-local handles in `worker_main` before the loop starts.

## What this stage does NOT cover

- Real `pddb::Pddb` backend. `PddbStore::with_mock_backend()` is the
  only constructor used so far; Stage 9 lands `PddbStore::with_pddb(...)`
  behind a `pddb-backend` feature flag.
- Any actual network I/O. Hello, GetWhoami, and Shutdown all complete
  without a single byte going onto the network.
- Renode boot test. Stage 9 is the first time the binary runs against
  a real (or emulated) Xous environment.

## Files changed (this commit)

```
modified:
  Cargo.lock                                    (resolver picked up
                                                  presage-store-pddb as a
                                                  direct workspace edge)
  crates/xous-app-signal/Cargo.toml             (deps swap)
  crates/xous-app-signal/src/main.rs            (40 → 75 LoC; Stage 1 smoke
                                                  test → Stage 8 driver)
  crates/xous-signal-bridge/Cargo.toml          (+5 deps)
  crates/xous-signal-bridge/src/lib.rs          (skeleton 9 LoC → 190 LoC)

new:
  crates/xous-signal-bridge/src/cmd.rs          (56 LoC)
  stage/REPORT-8.md                             (this file)
```
