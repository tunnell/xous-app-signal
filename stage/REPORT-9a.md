# Stage 9a — Workspace-internal preparation for the xous-core fork

Status: **complete.** Three workspace-internal pieces of Stage 9 land
in this commit; the rest waits for `tunnell/xous-core-for-xas` to
exist on GitHub.

This is the first half of the Stage 9 split the ROADMAP now
documents. Everything in this report is doable without the fork
because none of it requires `path = "../../services/pddb"`-style
links into xous-core's internal crates.

## What landed

### 1. ROADMAP refinement: Stage 9 split into 9a / 9b

`docs/ROADMAP.md` now distinguishes:

- **Stage 9a** — workspace-internal: getrandom 0.3 extern, PDDB
  feature flag, INTEGRATION.md.
- **Stage 9b** — fork integration: copy our crates into the
  `tunnell/xous-core-for-xas` clone as `apps/xas/`, wire xous-core's
  `cargo xtask` into our build, write Renode tests, run
  `renode-test`.

The integration choice (merge into a xous-core fork as `apps/xas/`)
is now committed in the ROADMAP rather than deferred. Same shape
`apps/sigchat`, `apps/mtxchat`, `apps/vault` already use in xous-core.

### 2. `__getrandom_v03_custom` extern (Stage 6.1 phase 3f follow-up)

`.cargo/config.toml` sets `--cfg getrandom_backend="custom"` for
rv32-xous, which makes `getrandom 0.3` expect an extern function in
the user binary. Without it, the rv32 release build fails with
`__getrandom_v03_custom: undefined symbol`.

Stage 9a adds the symbol to `crates/xous-app-signal/src/main.rs`,
gated on `#[cfg(target_os = "xous")]`, with a `panic!` body. The
panic is intentional: we want any code path that actually consumes
randomness to surface clearly (Stage 9b's Renode boot test will
exercise this) rather than silently produce zero-bytes-back.

The signature mirrors `getrandom-0.3.4/src/backends/custom.rs:10` —
`unsafe extern "Rust" fn __getrandom_v03_custom(*mut u8, usize) ->
Result<(), getrandom::Error>`. `getrandom = "0.3"` is a new
target-scoped dep on `xous-app-signal`'s `Cargo.toml`:

```toml
[target.'cfg(target_os = "xous")'.dependencies]
getrandom = { version = "0.3", default-features = false }
```

The hosted build is unaffected — the cfg gate keeps the symbol out
of non-xous builds (which use `getrandom`'s standard backends).

### 3. PDDB backend feature-flag scaffolding

`crates/presage-store-pddb/Cargo.toml` now has:

```toml
[features]
default = []
mock-backend = []
pddb-backend = []
```

A new module `src/backend_pddb.rs` is gated on
`#![cfg(all(feature = "pddb-backend", target_os = "xous"))]`. Body
is a `_test_compile_stub` placeholder; Stage 9b fills in the real
`pddb::Pddb` call-throughs. The cfg-gating means hosted builds —
where `pddb` isn't path-resolvable — never see the module.

### 4. `docs/INTEGRATION.md` — Stage 9b's mechanical recipe

Spells out the merge steps so Stage 9b can be executed without
re-deciding anything:

- One-time prerequisites (fork creation, local clone)
- Crate layout after merge (`apps/xas/{crates,vendor,docs,tests}/`)
- Cargo.toml workspace integration (which `[patch.crates-io]` entries
  need adding to xous-core's; which are inherited)
- App registration (`apps/manifest.json` entry)
- Logging shim (`println!` → `log::info!` via `xous-api-log`)
- Real `__getrandom_v03_custom` body (calls `trng::Trng::get_u64`)
- PDDB backend inner-trait impl
- u32e backend re-enable in `.cargo/config.toml`
- Renode test files (xas-smoke.resc, xas-smoke.robot, run-renode-tests.sh)

The "why merge instead of bundle" rationale is captured in the
final section. Two reasons: xous-core's `[patch.crates-io]`
inheritance, and `cargo xtask renode-image` reuse.

## Verification

```sh
$ cargo run -p xous-app-signal --bin xas
xas: starting
xas: worker started
xas: pong
xas: whoami err (expected): this client is not yet registered, please register or link as a secondary device
xas: worker shut down
xas: exiting
✓ Hosted-mode behaviour unchanged from Stage 8.

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ rv32 cross-compile passes. (cargo build for rv32 release would
  also exercise the linker — but we don't have rust-std-rv32 on
  the dev host, so the strict rv32 build link-test happens at
  Stage 9b inside the xous-core-for-xas tree, where xous-core's
  toolchain bootstrap is in place.)

$ cargo test -p presage-store-pddb        ✓ 22 passed
$ cargo test -p xous-signal-bridge        ✓ 3 passed
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check              ✓ clean
```

## Why no full rv32 release build verification yet

Stage 9a's strict goal was "getrandom 0.3 extern lands so the rv32
release build can link". `cargo check` confirms the symbol is
syntactically right; a `cargo build --target=...-xous-elf --release`
would confirm linker resolution.

We don't have `rust-std` for `riscv32imac-unknown-xous-elf` on this
host (the rustup channel reports `warn: skipping unavailable
component rust-std for target riscv32imac-unknown-xous-elf`). That's
a xous-core-tree-only artifact. Stage 9b's first action inside the
fork — running `cargo build --target=... --release -p xas` — is
where the link gets exercised; if the extern signature is wrong,
that's where it surfaces.

For Stage 9a the right check is what we did: `cargo check`-level
syntax validation, plus a hand-traced match of the extern signature
to `getrandom-0.3.4/src/backends/custom.rs:10`.

## What this stage does NOT cover

- The real PDDB `KvBackend` impl. Stub only.
- The real `__getrandom_v03_custom` body (calls `trng::Trng`). Stub
  panics — Stage 9b replaces.
- u32e backend re-enable. The `.cargo/config.toml` line stays
  commented; the `utralib` SOC feature wiring lives in xous-core's
  workspace, which we're not yet inside.
- `upload_to_cdn0` multipart implementation in
  `vendor/libsignal-service-rs/src/push_service/cdn.rs`. Stage 12
  work; not blocking Stage 9.
- The Renode test files. Stage 9b.

## Stage 9b user-action to unblock

Create `tunnell/xous-core-for-xas` on GitHub by forking
`betrusted-io/xous-core`. Then:

```sh
git clone /home/tunnell/precursor-signal/repos/xous-core \
  ~/precursor-signal/repos/xous-core-for-xas
cd ~/precursor-signal/repos/xous-core-for-xas
git remote set-url origin git@github.com:tunnell/xous-core-for-xas.git
git fetch origin
```

(Or clone directly from the GitHub fork — either works; the local-
clone path is faster and keeps your local commits available.)

Once that's in place, Stage 9b is a mechanical execution of
INTEGRATION.md.

## Files changed (this commit)

```
modified:
  Cargo.lock                                    (resolver: + getrandom 0.3
                                                  as a direct dep of
                                                  xous-app-signal)
  crates/xous-app-signal/Cargo.toml             (+target-scoped getrandom
                                                  dep)
  crates/xous-app-signal/src/main.rs            (+__getrandom_v03_custom
                                                  extern, panic body)
  crates/presage-store-pddb/Cargo.toml          (+[features] block)
  crates/presage-store-pddb/src/lib.rs          (+mod backend_pddb;)
  docs/ROADMAP.md                               (Stage 9 split into
                                                  9a + 9b)

new:
  crates/presage-store-pddb/src/backend_pddb.rs (Stage 9b stub w/ cfg)
  docs/INTEGRATION.md                           (Stage 9b recipe)
  stage/REPORT-9a.md                            (this file)
```
