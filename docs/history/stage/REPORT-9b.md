# Stage 9b — xtask bundling + Renode test scaffolding

**Date.** 2026-05-06
**Scope landed.** xtask crate, Renode boot/test scripts, hosted log
facade. Hardware-side wiring (real PDDB, real getrandom backend, u32e
cfg flip) is split into a follow-up *Stage 9b-deploy* described at the
bottom of this report.
**Status.** Scaffolding green. End-to-end Renode boot is not yet
exercised (requires a Xous image, see deferred section).

---

## 1. What this stage delivered

### 1.1 xtask

New crate at `xtask/`, listed in the workspace `[workspace] members`,
with no Cargo deps (so it has zero impact on the rv32 dep graph).
Three subcommands:

| subcommand           | what it does                                                  |
|----------------------|---------------------------------------------------------------|
| `cargo xtask build-rv32` | `cargo build --release --target=riscv32imac-unknown-xous-elf -p xous-app-signal` |
| `cargo xtask dist`       | runs `build-rv32`, then copies the ELF to `$XAS_DIST_DIR/xas` (default `dist/xas-rv32/xas`). Renode's `LoadELF` reads from this path. |
| `cargo xtask renode-test`| `$RENODE tests/renode/xas-smoke.robot` — invokes `renode-test` (Robot Framework runner). |

Cargo alias in `.cargo/config.toml`:

```toml
[alias]
xtask = "run --package xtask --"
```

so the canonical invocation is `cargo xtask <subcmd>` from anywhere
in the workspace.

### 1.2 Renode test files

Three new files under `tests/renode/`:

- **`xas-smoke.resc`** — boot script. Sets `$xous_core_root`,
  `$xous_image`, `$xas_elf` from environment, then `include`s
  `~/precursor-signal/repos/xous-core/emulation/betrusted.resc` to
  set up the Precursor machine model. The Plan-A (image-bundled xas)
  branch is the active one; the Plan-B (`sysbus.LoadELF $xas_elf`)
  fallback is commented in but inert until the image-side work lands.
- **`xas-smoke.robot`** — Robot test:
  ```robot
  *** Test Cases ***
  Should Boot And Print Splash
      Create Xas Machine
      Create Terminal Tester    sysbus.uart    timeout=30
      Start Emulation
      Wait For Line On Uart    xas: starting
      Wait For Line On Uart    xas: worker started
  ```
  The two `Wait For Line On Uart` strings are exactly the
  `log::info!` lines added in §1.3 — the test asserts the binary's
  boot messages reach Renode's emulated UART.
- **`run-renode-tests.sh`** — wrapper. Runs `cargo xtask dist` then
  `renode-test xas-smoke.robot`. Also surfaces the
  Renode-1.16+/rv32-rust-std/Xous-image prerequisite list at the top
  of the file.

### 1.3 Hosted log facade

Replaces ad-hoc `println!` boot messages with `log::info!` so the
Renode UART tester can match on them via Robot Framework rather than
brittle stdout grepping.

In `crates/xous-app-signal/Cargo.toml`:

```toml
log = "0.4"

[target.'cfg(not(target_os = "xous"))'.dependencies]
env_logger = { version = "0.11", default-features = false }
```

In `crates/xous-app-signal/src/main.rs`:

```rust
fn main() -> std::io::Result<()> {
    init_logger();
    log::info!("xas: starting");
    // ... worker spawn ...
    log::info!("xas: worker started");
    // ... UI run + worker join ...
    log::info!("xas: exiting");
    Ok(())
}

#[cfg(not(target_os = "xous"))]
fn init_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

#[cfg(target_os = "xous")]
fn init_logger() {
    // TODO Stage 9b-deploy: wire xous-api-log here.
}
```

`env_logger` is a target-cfg-gated dep — it does not enter the rv32
graph. The rv32 stub is a no-op for now; `log::info!` calls under
xous still go to whatever Xous binds as default-stdio (typically the
UART), which is what the Robot test will pick up once the rv32 path
is exercised.

### 1.4 Verification

All checks green on hosted Linux:

```
cargo build -p xous-app-signal                       → ok
cargo check --target=riscv32imac-unknown-xous-elf    → ok
                            -p xous-app-signal
cargo test -p xous-app-signal-ui                     → 31 passed
cargo test -p xous-signal-bridge                     → 3 passed
cargo test -p presage-store-pddb                     → 22 passed
cargo clippy --workspace --all-targets -- -D warnings → clean
cargo fmt --all -- --check                            → clean
cargo xtask help                                      → prints subcommand list
```

Hosted smoke run captures the new boot lines on stderr:

```
$ env RUST_LOG=info target/debug/xas
[INFO  xas] xas: starting
[INFO  xas] xas: worker started
... [splash screen frame on stdout] ...
```

End-to-end Renode boot is **not** exercised yet — see §3.

---

## 2. What this stage *did not* do

Five items from the ROADMAP §9b deliverable list were intentionally
deferred to Stage 9b-deploy (see §3). Each one is gated on an
integration question that's out of scope for the test-scaffolding
work:

| ROADMAP item | deferred because |
|--------------|------------------|
| Real `backend_pddb.rs` (path-dep on `xous-core/services/pddb/`) | Path-dep pulls `[patch.crates-io].aes` from xous-core's workspace, which is the same `Aes256Enc` blocker that killed the workspace-merge attempt. Resolving this requires the path-B (LoadELF + image-bundled core services) integration, which lands in 9b-deploy. |
| Real `__getrandom_v03_custom` body (calls `trng::Trng`) | Same path-dep blocker as PDDB. |
| u32e backend cfg re-enable | Same. |
| `xas-smoke.resc` Plan-A image-bundled boot | Requires xous-core image regeneration with our binary registered as an app — also a 9b-deploy step. The current `.resc` keeps Plan-A as a placeholder branch and Plan-B (LoadELF) as a commented fallback. |
| End-to-end Renode test pass | Gated on the four items above. Robot test file is committed; running it requires a Xous image. |

The split isolates *test scaffolding* (this stage, hosted-verifiable)
from *hardware integration* (next stage, requires the path-dep
reshuffle). It also keeps the standalone workspace's invariant intact:
nothing in `crates/` or `xtask/` pulls a path-dep into
`~/precursor-signal/repos/xous-core/`, so the rv32 dep graph is
identical to Stage 9a.

---

## 3. Stage 9b-deploy — follow-up scope

What needs to happen before the Renode smoke test goes green:

1. **Path-dep wiring with `[patch.crates-io]` overrides.** Take
   path-deps on `xous-core/services/{pddb,trng}` and
   `xous-core/api/xous-names/`. To dodge the `Aes256Enc` blocker,
   replicate xous-core's workspace `[patch.crates-io]` table inside
   *our* root `Cargo.toml`, but *exclude* the `aes` redirect (we want
   the upstream `aes` crate's `Aes256Enc`). Verify libsignal-zkgroup
   still resolves; verify xous-core's `services/aes` consumers don't
   leak into our binary's call graph.
2. **Real `backend_pddb.rs`.** Behind the `pddb-backend` feature
   (already declared at Stage 4). Single `Arc<Mutex<Pddb>>` per
   `PddbStore`; basis = `"signal"`; trait shape unchanged.
3. **Real `__getrandom_v03_custom` body.** Call `trng::Trng::fill_buf`
   (the bulk variant; the per-call `get_u64` would be wasteful for
   ML-KEM-1024's keygen).
4. **u32e cfg flip.** Uncomment `--cfg curve25519_dalek_backend="u32e_backend"`
   in `.cargo/config.toml`'s `[target.riscv32imac-unknown-xous-elf]`
   block. Verify `cargo xtask build-rv32` still passes.
5. **Image regeneration.** Run xous-core's xtask to produce an image
   that includes our `xas` binary as an app, *or* (Plan-B) load the
   ELF directly via Renode's `sysbus.LoadELF` against an
   apps-stripped baseline image. Plan-B is preferred for the smoke
   test — less coupling, faster turnaround.
6. **Renode end-to-end run.** `./tests/renode/run-renode-tests.sh`
   should produce `1 test passed` with both `xas: starting` and
   `xas: worker started` matched on the UART.
7. **Logger init for rv32.** Replace the `init_logger()` no-op stub
   with a `log_server::init_wait()` call (path-dep on
   `xous-core/services/xous-log/`). Optional — Xous's default UART
   binding will surface the messages without it, but routing through
   `xous-api-log` matches the rest of the platform's pattern.

---

## 4. Files touched

```
A  xtask/Cargo.toml
A  xtask/src/main.rs
A  tests/renode/xas-smoke.resc
A  tests/renode/xas-smoke.robot
A  tests/renode/run-renode-tests.sh         (chmod +x)
A  stage/REPORT-9b.md                       (this file)
M  Cargo.toml                               (workspace member added)
M  .cargo/config.toml                       (xtask alias)
M  crates/xous-app-signal/Cargo.toml        (log + env_logger deps)
M  crates/xous-app-signal/src/main.rs       (init_logger + log::info! calls)
```

No source changes outside `crates/xous-app-signal` — the bridge / UI
/ store crates are unchanged.

---

## 5. Risk notes

- **Plan-A vs Plan-B.** The current `.resc` favors Plan-A (image-bundled
  xas), which will require xous-core integration we haven't done.
  If 9b-deploy's `[patch.crates-io]` reshuffle in step 1 above is
  intractable, fall back to Plan-B by uncommenting the `LoadELF` line
  and skipping the image-bundling work. Plan-B is also faster for
  iterating on Renode-only flow tests in 9b-deploy.
- **Dist directory churn.** `XAS_DIST_DIR` defaults to `dist/xas-rv32/`
  inside the workspace. That directory is gitignored. If a stale
  artifact from a previous build survives, it's used; `cargo xtask
  dist` always rebuilds first, so this is mostly cosmetic, but
  external consumers of the artifact path should treat
  `dist/xas-rv32/xas` as ephemeral.
- **`log` facade choice.** Picked `log` (the `log::Log` facade) over
  `tracing` to keep the rv32 surface minimal. The bridge already uses
  `tracing` internally for span-shaped diagnostics; that stays. Only
  the binary entry point and the Robot-test-asserted boot lines go
  through `log`.
