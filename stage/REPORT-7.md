# Stage 7 — presage tokio-removal patch

Status: **complete**.

## What landed

`vendor/presage/` is `whisperfish/presage` at rev `600c4ed`. Workspace `Cargo.toml` redirects `presage`'s git URL to it via `[patch."https://github.com/whisperfish/presage"]`.

Six concrete patch sites per `REPORT.md` Decision 2 + a thread-local executor handle:

### 1. Cargo.toml

`vendor/presage/presage/Cargo.toml` — drop `tokio = { version = "1.48", features = ["rt", "sync", "time"] }`. Add `async-executor`, `async-lock`, `futures-timer` (git pins matching the workspace).

### 2. `errors.rs:65`

```rust
// before
Timeout(#[from] tokio::time::error::Elapsed),
// after
Timeout,
```

The `#[from]` conversion is gone (no longer an Elapsed source). Callers that wanted to map a timeout error now do so explicitly. This is a public-API change.

### 3. `manager/registered.rs:47`

```rust
use async_lock::Mutex;   // was: tokio::sync::Mutex
```

Drop-in. `async_lock::Mutex::lock()` returns a future that resolves to a `MutexGuard<'_, T>` with the same `Deref + DerefMut` semantics presage uses. The few call sites using `lock().await` work unchanged.

### 4. Spawn sites — five `tokio::task::spawn_local` / `tokio::spawn` → `crate::runtime::spawn_detached(...)`

- `registered.rs:698` (was line 696) — sync-message Contacts reply
- `registered.rs:729` (was 727) — sync-message Keys reply
- `registered.rs:748` (was 746) — sync-message Blocked reply
- `registered.rs:818` (was 816) — sticker-pack download
- `registered.rs:1709` (was 1707) — upsert_contact_from_profile

All five are fire-and-forget (caller doesn't capture the JoinHandle). `LocalExecutor::spawn(...).detach()` matches that semantic.

### 5. `registered.rs:1257` — `spawn_blocking` → inline

```rust
// before
if ciphertext.len() > DECRYPT_IN_THREAD_THRESHOLD {
    ciphertext = tokio::task::spawn_blocking(move || {
        decrypt_in_place(key, &mut ciphertext).map(|_| ciphertext)
    }).await.expect("decryption in another thread")?;
} else {
    decrypt_in_place(key, &mut ciphertext)?;
}

// after
decrypt_in_place(key, &mut ciphertext)?;
```

On a single-threaded LocalExecutor `spawn_blocking` would need a separate worker thread + channel for the same effect — overkill for a CPU-bound op. The threshold check (100 KB attachment size) was a fairness optimization for a multi-threaded runtime; we don't have one.

### 6. New: `presage::runtime` — thread-local executor handle

Defined in `vendor/presage/presage/src/lib.rs`:

```rust
pub mod runtime {
    thread_local! {
        static PRESAGE_EXECUTOR: RefCell<Option<&'static LocalExecutor<'static>>> =
            const { RefCell::new(None) };
    }

    pub fn set_executor(exec: &'static LocalExecutor<'static>) { ... }
    pub fn spawn_detached<F: Future<Output = ()> + 'static>(future: F) { ... }
}
pub use runtime::{set_executor, spawn_detached};
```

The worker thread that hosts `Manager` calls `presage::set_executor(exec)` once at startup, where `exec` is a `&'static LocalExecutor<'static>` (typically `Box::leak(Box::new(LocalExecutor::new()))`). After that, the spawn sites above just call `crate::runtime::spawn_detached(future)` without needing an executor handle threaded through every method.

This avoids the more invasive alternative of changing `Manager::link_secondary_device`, `Manager::register`, `Manager::confirm_verification_code`, `Manager::load_registered`'s public signatures to take an `executor: &'static LocalExecutor` parameter. The thread-local approach keeps Manager's API source-compatible with upstream, modulo the new requirement to call `set_executor` before any Manager method runs on a thread.

`spawn_detached` panics if `set_executor` wasn't called on the current thread. That's a programmer error — failing fast catches it during development; a Manager method calling it on the wrong thread is always a bug.

## Verification

```sh
$ cargo build -p presage-store-pddb
    Compiling presage v0.8.0-dev (/home/tunnell/precursor-signal/xous-app-signal/vendor/presage/presage)
    Finished `dev` profile in 20.41s
✓ Vendored presage compiles with the patch.

$ cargo tree -p presage --depth=1 | grep -E "tokio|async-executor|async-lock|futures-timer"
├── async-executor v1.14.0 (git smol-rs)
├── async-lock v3.4.2 (git smol-rs)
├── futures-timer v3.0.3 (git async-rs)
✓ Tokio gone from presage's direct deps.

$ cargo run -p xous-app-signal --bin xas              # ✓ "got: hello"
$ cargo run --example https_get -p xous-net-bridge    # ✓ HTTP/1.1 200 OK
$ cargo run --example signal_ws_keepalive -p xous-net-bridge   # ✓ 101 + 94B frame

$ cargo fmt --all -- --check       # ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings   # ✓ clean
```

`cargo fmt` and `cargo build` emit two warnings about the vendored libsignal-service-rs's `rustfmt.toml` requesting nightly-only options; harmless, not specific to Stage 7.

## What Stage 7 doesn't do

**rv32 cross-compile of `presage-store-pddb` is still blocked.** Reason: tokio is still pulled transitively via `libsignal-service v0.1.0 → reqwest v0.12 → tokio v1.52 → mio v1.2`. This is exactly what Stage 6 (libsignal-service-rs transport fork) is for. After Stage 6 lands, `cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb` should pass for the first time.

## Diff size

About 30 lines of patch against `vendor/presage/`:
- Cargo.toml: 1 line removed (tokio dep), 4 lines added (3 new git deps + comment block).
- errors.rs: 2-line change.
- registered.rs:47: 1-line change.
- registered.rs spawn sites: 5 × 1-line changes.
- registered.rs:1257: ~10 lines collapsed to ~3.
- lib.rs: ~30 lines of new `runtime` module.

Total around the predicted 30 lines if you don't count the new `runtime` module (which is a one-time addition that didn't exist upstream). Counting it, ~60 lines. Either way, well within the budget set by REPORT.md Decision 2.

## API surface change for callers

One new requirement: any thread that hosts `Manager` must call `presage::set_executor(...)` before any Manager method runs. We'll wire this up at Stage 8 (worker thread + IPC scaffolding):

```rust
let executor: &'static LocalExecutor<'static> = Box::leak(Box::new(LocalExecutor::new()));
presage::set_executor(executor);
// ... later, on the same thread:
let manager = Manager::load_registered(store).await?;
```

`Manager::load_registered`, `link_secondary_device`, `register`, `confirm_verification_code` all keep their original signatures — no `executor` parameter added. The thread-local handle is the indirection point.

## Suggested ROADMAP refinements

1. **Stage 7 patch table** in `ROADMAP.md` step 1 should mention the thread-local `presage::runtime` module as a new addition (not a swap), in addition to the listed swaps. Otherwise an agent reading just the patch table would miss this and try to thread an executor parameter through every constructor.

2. **Stage 8 (worker thread setup)** should include the `presage::set_executor(...)` call as part of the worker-thread boilerplate.

## Files changed (since `bcf158a`)

```
modified:
  Cargo.toml                                              (+[patch.<git>] for presage)
  Cargo.lock                                              (regenerated)

new (vendored):
  vendor/presage/                                         (whole tree at rev 600c4ed)

modified (vendored):
  vendor/presage/presage/Cargo.toml                       (-tokio, +async-executor, +async-lock, +futures-timer)
  vendor/presage/presage/src/lib.rs                       (+runtime module ~30 lines)
  vendor/presage/presage/src/errors.rs                    (Timeout variant: -from-impl, unit variant)
  vendor/presage/presage/src/manager/registered.rs        (5 spawn sites + Mutex import + spawn_blocking inline)

new:
  stage/REPORT-7.md                                       (this file)
```
