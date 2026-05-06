# Stage 4 — partial (Cargo wiring + curve25519-dalek vendoring) complete

Status: **Stage 4 step 1 complete; Stage 4 main work (StateStore impl + tests) deferred until after Stages 6 + 7 land.**

This stage was originally going to be a single end-to-end pass: add presage as a dep, implement `StateStore` on a mock backend, write tests, verify hosted + rv32. Two issues showed up that the design doc had anticipated but hadn't yet been resolved in code; both are now resolved. The actual `StateStore` impl is deferred to keep rv32 verification unbroken (see "Decision: ordering" below).

## What landed in this stage

### 1. `presage` is now a workspace dep

`crates/presage-store-pddb/Cargo.toml` declares `presage = { git = "https://github.com/whisperfish/presage", rev = "600c4ed" }`. The full Whisperfish stack (libsignal v0.91.0 by tag, libsignal-service-rs HEAD by rev, presage HEAD by rev) is now resolvable. Hosted-mode `cargo build -p presage-store-pddb` succeeds.

### 2. curve25519-dalek strategy: vendored `betrusted-io/curve25519-dalek` (Precursor HW-accelerated) + version bump + lizard port

The Precursor curve25519 IP core is **Precursor-only** (per bunnie, 2026-05) — not on the Bao1x tape-out, which has a different PKE engine. We're targeting **Precursor first**; Bao1x is a future swap (different backend would need to be written).

The vendored copy at `vendor/curve25519-dalek/` is `betrusted-io/curve25519-dalek` (carries the u32e IP-core driver at `curve25519-dalek/src/backend/serial/u32e/`), with three small modifications:

1. Manifest version bumped `4.1.2` → `4.1.3` so the `[patch.crates-io]` redirect matches what libsignal's zkgroup declares (`curve25519-dalek = "4.1.3"`).
2. The `src/lizard/` module ported verbatim from `signalapp/curve25519-dalek` (`signal-curve25519-4.1.3` tag): 4 `RistrettoPoint` methods used by zkgroup (`lizard_encode<H>`, `lizard_decode<H>`, `from_uniform_bytes_single_elligator`, `decode_253_bits`). Additive vs the betrusted-io fork — no API conflicts.
3. One `pub mod lizard;` line in `src/lib.rs`.

That's the entire delta over upstream betrusted-io.

**HW acceleration activation.** The u32e backend is selected at compile time by `--cfg curve25519_dalek_backend="u32e_backend"`. We auto-set this for rv32-xous via `.cargo/config.toml`:

```toml
[target.riscv32imac-unknown-xous-elf]
rustflags = ["--cfg", "curve25519_dalek_backend=\"u32e_backend\""]
```

On hosted Linux the same code falls back to the portable Rust backend, so tests/CI run unaffected. On Precursor hardware, ECC operations route through the IP core.

Workspace `[patch.crates-io]`:

```toml
[patch.crates-io.curve25519-dalek]
path = "vendor/curve25519-dalek/curve25519-dalek"

[patch.crates-io.curve25519-dalek-derive]
path = "vendor/curve25519-dalek/curve25519-dalek-derive"

# libsignal also imports curve25519-dalek directly via the git URL alias
# `curve25519-dalek-signal = { git = "...signalapp/...", package = "curve25519-dalek" }`
# at libsignal/Cargo.toml:90. [patch.crates-io] doesn't redirect git sources,
# so we additionally patch the git URL.
[patch."https://github.com/signalapp/curve25519-dalek"]
curve25519-dalek = { path = "vendor/curve25519-dalek/curve25519-dalek" }
curve25519-dalek-derive = { path = "vendor/curve25519-dalek/curve25519-dalek-derive" }
```

**Future-target story.** The choice is target-scoped via `.cargo/config.toml`, not workspace-scoped. Adding Bao1x support later means writing a new backend module (e.g. `src/backend/serial/bao1x_pke/`) and adding another `[target.…]` block to `.cargo/config.toml`. The Precursor decision doesn't lock us out.

`docs/REPORT.md` §Decision 6 and Risk #3 have been rewritten to document this strategy.

### History note (for the curious)

The plan went through three iterations during Stage 4 as new information came in:

1. **Original** (pre-input): vendor `betrusted-io/curve25519-dalek` for HW acceleration.
2. **After bunnie's "profile first" guidance**: swap to upstream `dalek-cryptography/curve25519-dalek` 4.1.3 + lizard port, software-only, with a plug-in seam for HW acceleration later.
3. **Current** (after user's "Precursor-only, get something going" call): swap back to `betrusted-io/curve25519-dalek` + lizard port + version bump, auto-activated for rv32-xous via `.cargo/config.toml`. Get HW acceleration on the target we care about now; defer Bao1x.

The lizard module port and the `[patch.crates-io]` redirect mechanics are identical across iterations 2 and 3; only the underlying base changed.

## What did NOT land in this stage (and why)

- `StateStore` impl on `PddbStore`.
- `ContentsStore::profile` / `save_profile`.
- Mock backend.
- Unit tests.

These are deferred to a later pass after Stages 6 + 7 have landed. Reason below.

## Decision: ordering — Option B (per user)

Stage 4's verification step requires `cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb`. The dep chain on rv32 is:

```
presage-store-pddb → presage → libsignal-service-rs → reqwest-websocket
                                                   → reqwest → hyper-util
                                                            → tokio → mio
```

`mio` doesn't support rv32-xous (its `cfg` ladder covers Unix/Windows/WASI/hermit only). So full `cargo check` on rv32 fails with 47 type errors, even though hosted-mode builds. This is the exact coupling REPORT.md Decisions 2 (tokio removal in presage) and 3 (libsignal-service-rs transport fork) are designed to break.

Per user direction: **Option B — do Stages 6 + 7 next, then come back for the actual Stage 4 implementation.** Reasoning: writing storage trait impls without per-stage rv32 verification accumulates code that's untested on the actual target. Doing 6 + 7 first restores rv32 sanity before any storage code is written.

The `ROADMAP.md` has been updated to reflect this temporal order: **0 → 1 → 2 → 3 → 6 → 7 → 4 (full) → 5 → 8 → 9 → 10 → 11 → 12.** Stage numbering is preserved for section references; ordering is governed by each stage's `Prerequisites` line.

## Verification (hosted; rv32 deferred)

```sh
$ cargo build -p presage-store-pddb
   Compiling signal-crypto, zkcredential, zkgroup, libsignal-protocol,
             libsignal-service, presage, presage-store-pddb
   Finished `dev` profile in 16s
✓ Hosted-mode full Whisperfish stack compiles.

$ cargo run -p xous-app-signal --bin xas               ✓ "got: hello"
$ cargo run --example https_get -p xous-net-bridge     ✓ "HTTP/1.1 200 OK"
$ cargo run --example signal_ws_keepalive -p xous-net-bridge
                                                       ✓ handshake 101 + 94-byte frame

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-net-bridge
✓ rv32 still passes for the network layer (rustls + ring + getrandom).

$ cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb
✗ DEFERRED — gated on Stage 7. Will pass once mio is removed from the
   transitive dep tree (Stages 6 + 7 do this).

$ cargo tree --workspace -d
⚠ Two new duplicates from adding presage:
    - thiserror v1.0.69 vs v2.0.18
    - tungstenite v0.21 vs v0.24
   These are version conflicts that will be resolved either via additional
   `[patch.crates-io]` entries or by waiting for the libsignal-service-rs
   transport fork at Stage 6 to remove the v0.24 path. Tracked as a
   follow-up; not blocking.

$ cargo fmt --all -- --check         ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
```

## ROADMAP refinements applied (alongside this stage)

The ROADMAP was edited as part of this stage's work to reflect:

1. **Recommended ordering**: 0 → 1 → 2 → 3 → 6 → 7 → 4 → 5 → 8+ (Option B). Stage section numbering preserved; temporal order from `Prerequisites` lines.
2. **Stage 4 step 1 split out** as the "Cargo wiring + curve25519-dalek vendoring" sub-step that lands early (this stage).
3. **Stage 4 step 1 documents** the betrusted-io vendoring + version bump + lizard-port pattern.
4. **Stage 4 / Stage 5 verification** explicitly notes rv32 cross-compile is gated on Stage 7.
5. **Stage 6 prerequisites** changed from "Stages 3 + 5 complete" to "Stage 3 complete + Stage 4 step 1 complete".

The ROADMAP refinements are in the ROADMAP.md commits; nothing in this stage requires re-reading them in isolation.

## Open questions for later stages

1. **Stage 9: update xous-core's `[patch.crates-io].curve25519-dalek`.** xous-core currently points at `tunnell/curve25519-dalek`; should be updated to `betrusted-io/curve25519-dalek` (or the publishable form of our vendored copy with the lizard module ported in). Coordinate with xous-core maintainers.

2. **`getrandom 0.3` may resurface** when libsignal-service-rs's transport fork removes the tokio-stack but leaves modern `rand`/`getrandom` paths in. xous-core has only a `getrandom 0.2` fork. Surfacing for Stage 6.

3. **`thiserror` and `tungstenite` duplicates**. Two versions of each are now in the dep graph. Likely resolves automatically when libsignal-service-rs's transport is forked (Stage 6) — that removes the v0.24 tungstenite path. If not, `[patch.crates-io]` redirects can force convergence.

## Files changed (since Stage 3 commit)

```
modified:
  Cargo.toml                                                   (+vendored curve25519-dalek patch; +signalapp git-URL patch)
  Cargo.lock                                                   (regenerated; full Whisperfish stack)
  crates/presage-store-pddb/Cargo.toml                         (+presage as git dep)

new (vendored):
  vendor/curve25519-dalek/                                     (betrusted-io fork, 4.1.2 → 4.1.3 bump)
  vendor/curve25519-dalek/curve25519-dalek/src/lizard/         (ported from signalapp fork; 6 files)

modified (vendored):
  vendor/curve25519-dalek/curve25519-dalek/Cargo.toml          (version "4.1.2" → "4.1.3")
  vendor/curve25519-dalek/curve25519-dalek/src/lib.rs          (+pub mod lizard;)

new (docs):
  stage/REPORT-4.md                                            (this file)

modified (docs):
  docs/REPORT.md                                               (Decision 6 + Risk #3 rewritten for betrusted-io strategy)
  docs/ROADMAP.md                                              (Option B ordering; Stage 4 step 1 split out; rv32-gated notes)
```
