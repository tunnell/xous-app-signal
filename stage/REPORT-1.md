# Stage 1 — Vendor smol primitives, write a `LocalExecutor` smoke test

Status: **complete**.

## What was done

1. Added the seven smol-rs crates as git deps with pinned revs in `[workspace.dependencies]`. Pins use the HEAD of each cloned reference at `~/precursor-signal/repos/<crate>` as of 2026-05:

   | Crate | Rev |
   |---|---|
   | `async-task` | `f98b30b` |
   | `async-executor` | `543403e` |
   | `async-channel` | `35a63c4` |
   | `async-lock` | `29f0ff9` |
   | `event-listener` | `2e9db21` |
   | `futures-lite` | `3dc587f` |
   | `futures-timer` | `07dbe53` |

2. **Mirrored those same pins into `[patch.crates-io]`** so transitive uses
   (e.g., `futures-lite` pulled by `async-executor`'s own `Cargo.toml`)
   converge on our pinned versions instead of producing duplicate copies.
   Without this, `cargo tree -d` flagged `futures-lite v2.6.1` appearing
   twice — once from crates.io (transitively) and once from our git pin.
   The `[patch.crates-io]` redirect deduplicates.

3. `crates/xous-app-signal/Cargo.toml` declares deps on `async-executor`,
   `async-channel`, `futures-lite`, `futures-timer` (the four needed for
   the smoke test).

4. `crates/xous-app-signal/src/main.rs` implements the smoke test:
   - `LocalExecutor::new()` (single-threaded, !Send-compatible).
   - `async-channel::bounded::<&'static str>(1)`.
   - Producer task: `Delay::new(100ms).await; tx.send("hello").await`.
   - Consumer (in `block_on(executor.run(...))`): `rx.recv().await; println!("got: {msg}")`.
   - Awaits the producer task at the end so a forgotten `.detach()` would
     have been caught.

5. `rust-toolchain.toml` updated: `channel = "stable"` (was `1.95.0`).
   Same rustc version on this dev box, but the
   `riscv32imac-unknown-xous-elf` target on this machine is registered
   under `stable`, not under `1.95.0`. Also added
   `targets = ["riscv32imac-unknown-xous-elf"]` so rustup will offer to
   install if needed.

## Verification

All verification commands from the (refined) ROADMAP Stage 1 spec passed.

```sh
$ cargo run -p xous-app-signal --bin xas
got: hello

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
    Checking parking, fastrand, futures-io, async-task, slab, futures-timer,
             futures-lite, concurrent-queue, event-listener, async-executor,
             event-listener-strategy, async-channel, xous-app-signal v0.0.1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.75s

$ cargo build --workspace --release      # clean
$ cargo build --workspace --profile=release-small   # clean
$ cargo tree --workspace -d              # "nothing to print" — no duplicates
$ cargo fmt --all -- --check             # clean
$ cargo clippy --workspace --all-targets -- -D warnings   # clean
```

The `cargo check --target=riscv32imac-unknown-xous-elf` step is the
significant new verification: it confirms the entire smol stack
(13 transitive crates) builds for Xous's RISC-V 32-bit target. This is
the correctness signal the dev wanted — every stage verifies the
rv32 cross-compile dep tree without needing a Renode boot.

The two harmless build warnings remain: `sha2` and `ring` patches "not
used in the crate graph" (they activate later when presage / rustls come
in transitively, Stage 2+).

## Binary sizes

| Profile | Stage 0 (empty) | Stage 1 (smol stack) | Δ |
|---|---|---|---|
| `dev` | 4.34 MB | 6.61 MB | +2.27 MB |
| `release` | 1.95 MB | 3.12 MB | +1.17 MB |
| `release-small` | 287 KB | **373 KB** | **+86 KB** |

Release-small is the meaningful number for size budget tracking. **+86 KB
to add the entire smol async runtime + 8 transitive deps** is reasonable.
For comparison, Tokio's release-small footprint per the dev community
benchmarks tends to be in the low MBs. We're getting an executor for
~5% of that.

`cargo bloat --release-small --crates -p xous-app-signal` would give a
finer breakdown but `cargo bloat` isn't installed on this box; deferred
to Stage 9 where we'd want it on the rv32 binary anyway.

## rv32 dep tree (verified by `cargo tree --target=riscv32imac-unknown-xous-elf`)

```
xous-app-signal
├── async-channel (git)
│   ├── concurrent-queue → crossbeam-utils
│   ├── event-listener-strategy → event-listener (git) → parking, pin-project-lite
│   ├── futures-core
│   └── pin-project-lite
├── async-executor (git)
│   ├── async-task (git)
│   ├── concurrent-queue (*)
│   ├── fastrand
│   ├── futures-lite (git) → fastrand, futures-core, futures-io, parking, pin-project-lite
│   ├── pin-project-lite
│   └── slab
├── futures-lite (git) (*)
└── futures-timer (git)
```

All pulled-in transitive deps (`concurrent-queue`, `crossbeam-utils`,
`parking`, `pin-project-lite`, `fastrand`, `slab`, `futures-core`,
`futures-io`) are small no_std-friendly smol-rs ecosystem crates. None
introduces a surprise dep.

## Deviations from the ROADMAP

1. **`rust-toolchain.toml` channel changed.** ROADMAP said `1.95.0` (or
   `stable`); the standalone-version pin didn't match where this dev box
   has the rv32-xous target installed (`stable` channel). Switched to
   `channel = "stable"`. Resolves to the same compiler today but uses a
   channel where the target is registered.

2. **`[patch.crates-io]` extended.** ROADMAP said "Add
   `[workspace.dependencies]` entries". Reality: that's necessary but
   not sufficient — without `[patch.crates-io]` for the same crates,
   transitive uses from crates.io produce duplicate copies that
   `cargo tree -d` flags. Both layers are needed.

## Suggested ROADMAP refinements

1. **Stage 1 Step 1 should explicitly mention `[patch.crates-io]`.**
   Suggested rewrite:

   > 1. Add `[workspace.dependencies]` entries for the seven smol
   > primitives (pinned to specific git revs). Then add the same crates
   > to `[patch.crates-io]` so transitive uses converge on our pinned
   > versions. Without the patch entries, `cargo tree -d` will flag
   > duplicate copies of `futures-lite` etc. (one from crates.io
   > transitively, one from our git pin).

2. **`rust-toolchain.toml` flexibility.** The Stage 0 spec said
   `channel = "1.95.0"` (or unset, falling back to stable). On dev
   boxes where the rv32-xous target is registered under `stable` rather
   than under a specific version, the precise version pin breaks the
   cross-compile. Suggested rewrite of Stage 0 step 7:

   > 7. Add `rust-toolchain.toml` with `channel = "stable"` and
   > `targets = ["riscv32imac-unknown-xous-elf"]`. Pinning a specific
   > version like `1.95.0` is brittle on machines where the rv32-xous
   > target is installed against the `stable` channel; if a future
   > stable release breaks something, you can pin then.

## Open questions / things to revisit

1. **`betrusted-io/rust` fork integration.** The Xous toolchain is
   normally installed via `betrusted-io/rust`'s release artifacts (or
   `xtask install-toolkit` from xous-core). On this dev box the
   `riscv32imac-unknown-xous-elf` target is already registered with
   rustup as part of the `stable` channel, suggesting it was installed
   via that path. For other developers / fresh installs, the toolchain
   bootstrap deserves a short doc — defer to Stage 9 (the first stage
   where rv32 bring-up is the primary goal) or a one-page
   `docs/TOOLCHAIN.md`. Surface for now.

2. **`cargo bloat` not installed.** Useful for size profiling on the
   rv32 binary at Stage 9. Add to the Stage 9 prerequisites.

3. **No surprises in the smol crate set.** Every transitive dep is
   small, no_std-friendly, and well-known. If we found an unexpected
   transitive dep, we'd flag it; we didn't. Nothing to do.

## Files changed (since Stage 0 commit)

```
modified:
  Cargo.toml                              (workspace deps + patch entries)
  crates/xous-app-signal/Cargo.toml       (added 4 deps)
  crates/xous-app-signal/src/main.rs      (smoke test impl)
  rust-toolchain.toml                     (channel "stable", targets)

new:
  Cargo.lock                              (regenerated)
  stage/REPORT-1.md                       (this file)
```
