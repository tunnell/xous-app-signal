# INTEGRATION.md — Merging `xous-app-signal` into `tunnell/xous-core-for-xas`

Stage 9b's mechanical recipe. This document is what Stage 9a's `do
the next stage but first audit emails` request locked in: rather than
deciding "merge vs. bundle" inside Stage 9, the user has chosen to
**fork xous-core** as `tunnell/xous-core-for-xas` and drop our four
crates into the fork as `apps/xas/`. This file is the recipe.

## One-time prerequisites

1. **Fork on GitHub.** User creates `tunnell/xous-core-for-xas` from
   `betrusted-io/xous-core` via the GitHub UI. (Manual; not
   automatable from this side.)
2. **Local clone.** Two equivalent paths:

   - From the fork directly:
     ```sh
     git clone https://github.com/tunnell/xous-core-for-xas.git \
       ~/precursor-signal/repos/xous-core-for-xas
     ```
   - From the local clone, then re-pointing `origin`:
     ```sh
     git clone /home/tunnell/precursor-signal/repos/xous-core \
       ~/precursor-signal/repos/xous-core-for-xas
     cd ~/precursor-signal/repos/xous-core-for-xas
     git remote set-url origin git@github.com:tunnell/xous-core-for-xas.git
     ```

   The second is faster but the first guarantees we start from the
   GitHub fork's HEAD (in case the local clone has drifted).

3. **Branch for the integration:** `git checkout -b xas/integration`
   inside the fork. All Stage 9b commits land on this branch.

## Crate layout after merge

The four `xous-app-signal` workspace crates move into a single
top-level directory inside the fork:

```
xous-core-for-xas/
└── apps/
    └── xas/
        ├── Cargo.toml                  ← lists the four sub-crates as
        │                                 path members; otherwise minimal
        ├── README.md                   ← copy from xous-app-signal/README.md
        ├── docs/
        │   ├── REPORT.md, ROADMAP.md, INTEGRATION.md (this file), CALL_GRAPH.md
        │   └── stage/REPORT-*.md
        ├── crates/
        │   ├── xous-app-signal/        ← the binary
        │   ├── xous-signal-bridge/
        │   ├── presage-store-pddb/
        │   └── xous-net-bridge/
        ├── vendor/
        │   ├── presage/                ← the tokio-removed fork
        │   ├── libsignal-service-rs/   ← the transport-replaced fork
        │   └── curve25519-dalek/       ← betrusted-io fork w/ lizard port
        └── tests/
            └── renode/
                ├── xas-smoke.resc
                ├── xas-smoke.robot
                └── run-renode-tests.sh
```

The vendored copies (`vendor/`) come along to keep the fork
self-contained — same shape `apps/sigchat`, `apps/mtxchat`, `apps/vault`
already use in xous-core's tree.

## Cargo.toml workspace integration

xous-core's top-level `Cargo.toml` already declares its workspace
members. Add `apps/xas/crates/*` and `apps/xas/vendor/*` paths to
that workspace:

```toml
# In xous-core-for-xas/Cargo.toml
[workspace]
members = [
    # ... existing xous-core members ...
    "apps/xas/crates/xous-app-signal",
    "apps/xas/crates/xous-signal-bridge",
    "apps/xas/crates/presage-store-pddb",
    "apps/xas/crates/xous-net-bridge",
    "apps/xas/vendor/libsignal-service-rs",
]

# Most of our `[patch.crates-io]` entries are already in xous-core's
# top-level Cargo.toml (sha2, ring, getrandom). Three are NOT and
# must be added:
[patch.crates-io.curve25519-dalek]
path = "apps/xas/vendor/curve25519-dalek/curve25519-dalek"

[patch.crates-io.curve25519-dalek-derive]
path = "apps/xas/vendor/curve25519-dalek/curve25519-dalek-derive"

[patch."https://github.com/signalapp/curve25519-dalek"]
curve25519-dalek = { path = "apps/xas/vendor/curve25519-dalek/curve25519-dalek" }
curve25519-dalek-derive = { path = "apps/xas/vendor/curve25519-dalek/curve25519-dalek-derive" }

[patch."https://github.com/whisperfish/libsignal-service-rs"]
libsignal-service = { path = "apps/xas/vendor/libsignal-service-rs" }

[patch."https://github.com/whisperfish/presage"]
presage = { path = "apps/xas/vendor/presage/presage" }
```

Drop our workspace-local `[workspace.dependencies]` smol-rs pins —
xous-core's workspace already pins those at the same revs (verify
once: `cargo tree -p async-channel`, `-p async-executor`, etc.). If
they differ, surface the discrepancy before changing either side.

## App registration

xous-core registers loadable apps in `apps/manifest.json`. Add an
entry for `xas`:

```json
{
  "name": "xas",
  "package": "xous-app-signal",
  "binary": "xas",
  "description": "Signal client",
  "category": "Communications"
}
```

Pattern matches the existing `apps/sigchat` entry.

## Logging shim (Stage 9b step)

`xous-app-signal/src/main.rs` currently uses `println!` for the
Stage 8 smoke output. The Stage 8 test harness (hosted) reads
stdout; the Renode smoke test reads UART, which xous-core wires
through `xous-api-log`. Replace each `println!` with `log::info!` and
add a one-time logger init at the top of `main`:

```rust
fn main() {
    log_server::init_wait().ok();
    log::set_max_level(log::LevelFilter::Info);
    // ... rest of main
}
```

`log_server` is a small xous-core utility (in `apps/sigchat/src/main.rs`
and other apps the pattern is visible).

## getrandom 0.3 backend

Stage 9a stubs `__getrandom_v03_custom` with a `panic!`. Stage 9b
swaps it for the real `xous-core/services/trng` client call. Kept
here as a checklist:

```rust
#[cfg(target_os = "xous")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    use std::sync::OnceLock;
    static TRNG: OnceLock<trng::Trng> = OnceLock::new();
    let trng = TRNG.get_or_init(|| {
        let xns = xous_names::XousNames::new().expect("XousNames::new()");
        trng::Trng::new(&xns).expect("Trng::new(...)")
    });
    let slice = unsafe { std::slice::from_raw_parts_mut(dest, len) };

    // Reuse `Trng::fill_bytes_via_next`'s body but call only `&self`
    // methods so the static `Trng` doesn't need Mutex. Cribbed from
    // services/trng/src/lib.rs:117-133.
    let mut left = slice;
    while left.len() >= 8 {
        let (l, r) = left.split_at_mut(8);
        left = r;
        let chunk: [u8; 8] = trng.get_u64().expect("get_u64").to_ne_bytes();
        l.copy_from_slice(&chunk);
    }
    let n = left.len();
    if n >= 4 {
        let chunk: [u8; 8] = trng.get_u64().expect("get_u64").to_ne_bytes();
        left.copy_from_slice(&chunk[..n]);
    } else if n > 0 {
        let chunk: [u8; 4] = trng.get_u32().expect("get_u32").to_ne_bytes();
        left.copy_from_slice(&chunk[..n]);
    }
    Ok(())
}
```

## PDDB backend

Stage 9b fills in `apps/xas/crates/presage-store-pddb/src/backend_pddb.rs`.
The module is already cfg-gated on
`#![cfg(all(feature = "pddb-backend", target_os = "xous"))]` so the
hosted build path stays untouched. Add `pddb` as an optional dep
inside that crate's `Cargo.toml`:

```toml
[target.'cfg(target_os = "xous")'.dependencies]
pddb = { path = "../../../../services/pddb", optional = true }
xous-names = { path = "../../../../api/xous-names", optional = true }

[features]
pddb-backend = ["dep:pddb", "dep:xous-names"]
```

Path-counts assume `apps/xas/crates/presage-store-pddb/` → `services/pddb/`
is `../../../../services/pddb` (four levels up). Verify with
`realpath` once after the move.

## u32e backend re-enable

`apps/xas/.cargo/config.toml`'s
`# "--cfg", "curve25519_dalek_backend=\"u32e_backend\"",` line was
disabled because `utralib`'s build script needed a Precursor SOC
feature (`precursor-c809403`) we couldn't propagate from a standalone
workspace. Inside xous-core-for-xas's tree the feature is wired up
already; uncomment the cfg.

Confirmation: `cargo tree -p utralib` should show the
`features = ["precursor-c809403"]` activation in the merged tree.

## Renode test files

Modeled on `~/precursor-signal/repos/xous-core/emulation/betrusted.resc`:

- `apps/xas/tests/renode/xas-smoke.resc` — boot script that creates the
  SoC + EC machines, attaches UART terminal testers, loads the image.
- `apps/xas/tests/renode/xas-smoke.robot` — Robot test waits for the
  Stage 8 lines (`xas: pong`, `xas: whoami err (expected): ...`) and
  asserts on them.
- `apps/xas/tests/renode/run-renode-tests.sh` — `cargo xtask
  renode-image` then `renode-test xas-smoke.robot`.

## Verification (Stage 9b end)

```sh
cd ~/precursor-signal/repos/xous-core-for-xas

# rv32 release build with the real PDDB backend.
cargo build --target=riscv32imac-unknown-xous-elf --release \
  -p xous-app-signal --features=pddb-backend

# Binary size sanity.
cargo bloat --target=riscv32imac-unknown-xous-elf --release \
  --crates -p xous-app-signal --features=pddb-backend | head -20

# Renode boot test.
./apps/xas/tests/renode/run-renode-tests.sh xas-smoke.robot
# Expected: 1 test passed.
```

## Why not a separate xtask / bundle the binary independently

Considered briefly. Two reasons we picked the merge path:

1. xous-core's `[patch.crates-io]` entries (sha2, ring, getrandom)
   are non-trivial to mirror outside the tree — every release we'd
   need to chase the rev pins. Inside the tree they're inherited for
   free.
2. xous-core's `cargo xtask` already knows how to build SoC + EC
   images, package the kernel, run Renode tests. Reimplementing any
   of that for a separate workspace is wasted effort.

The cost of the merge path is that our changes to vendored crates
(libsignal-service-rs, presage, curve25519-dalek) live alongside
xous-core's core-OS code. We mitigate this with the
`apps/xas/vendor/` subdirectory — clearly fenced off from the rest
of the tree. If a future maintainer rebases `tunnell/xous-core-for-xas`
onto `betrusted-io/xous-core/main`, the `apps/xas/` subtree carries
along cleanly.
