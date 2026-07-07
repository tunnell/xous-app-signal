# Vendored crates

This directory contains source-vendored copies of three crypto-adjacent
crates that are part of xas's trust boundary. Each was vendored at a
specific upstream commit/tag and modified for one of:

- compatibility with `riscv32imac-unknown-xous-elf` (no `tokio` /
  `mio` / `boring-sys` etc.)
- hardware acceleration on Precursor (curve25519 IP-core driver)
- transport-fork constraints (CDSI off, redirected git deps)

**Audit posture**: keep these dirs surgically minimal vs upstream —
every local line is one more thing to re-justify on each vendor
refresh and each crypto audit. Optimization and feature work MUST NOT
be added here; it belongs in the first-party `crates/` wrappers.
vendor/ carries only the compatibility patches listed below.

## What's pinned where

| Crate | Upstream | Pinned at | Local changes (high level) |
|---|---|---|---|
| `presage/` | [whisperfish/presage](https://github.com/whisperfish/presage) | rev **`600c4ed`** | tokio-removal patch (~30 lines): drop `tokio` from Cargo.toml, swap `tokio::sync::Mutex` → `async_lock::Mutex`, swap `tokio::task::spawn_*` → `crate::runtime::spawn_detached`, inline one `spawn_blocking`, plus a new `presage::runtime` module providing a thread-local `LocalExecutor` handle. See commit `6923946` "Stage 7: presage tokio-removal patch" for the full reasoning. |
| `libsignal-service-rs/` | [whisperfish/libsignal-service-rs](https://github.com/whisperfish/libsignal-service-rs) | rev **`782c0d6`** | `default = []` instead of `["cdsi"]` (CDSI pulls boring-sys → BoringSSL → no rv32 + needs libclang at host build time); removed the fork's own `[patch.crates-io].curve25519-dalek` to avoid fighting the workspace-level redirect to our betrusted-io fork. See commit `5a48219` "Stage 6.0: vendor libsignal-service-rs at rev 782c0d6; CDSI off; patch redirect" for full notes. |
| `curve25519-dalek/` | [betrusted-io/curve25519-dalek](https://github.com/betrusted-io/curve25519-dalek) | betrusted-io fork (carries the u32e IP-core driver for Precursor) | (1) Version bump 4.1.2 → 4.1.3 so libsignal's zkgroup (which declares `4.1.3`) sees the patch as version-compatible. (2) Port `src/lizard/` module from [signalapp/curve25519-dalek](https://github.com/signalapp/curve25519-dalek) tag **`signal-curve25519-4.1.3`** — adds 4 RistrettoPoint methods (`lizard_encode`, `lizard_decode`, `from_uniform_bytes_single_elligator`, `decode_253_bits`) used by zkgroup. See commits `b981ec1` "Stage 4: revert to betrusted-io curve25519-dalek + u32e activation for Precursor" and `06361bd` "Stage 4 partial v2: vendor betrusted-io curve25519-dalek; choose Option B" for the full strategy + workspace `[patch.crates-io]` wiring. |

## How to regenerate the diffs

Each `*.diff` file in this directory is the output of `diff -ruN` of
the vendored copy against its upstream pin. The intent is that an
auditor (or a CI guard) can verify the on-disk diff matches what's
checked in — anything else means the vendored copy has drifted.

Regenerate any diff yourself with:

```sh
# presage
cd /tmp
git clone https://github.com/whisperfish/presage.git presage-upstream
cd presage-upstream
git checkout 600c4ed
cd /path/to/xous-app-signal
diff -ruN -x .git -x target -x Cargo.lock \
    -x presage-cli -x presage-store-sled -x presage-store-sqlite \
    /tmp/presage-upstream/ vendor/presage/ > vendor/presage.diff
```

The `-x presage-cli -x presage-store-sled -x presage-store-sqlite`
exclusions exist because those upstream workspace members are **not
vendored**: no xas build compiles them (we consume only the `presage`
library crate), so carrying ~5.5k LoC of CLI + sled/sqlite store
source would inflate the audit surface for nothing. The vendored
workspace manifest lists `members = ["presage"]` accordingly.

```sh
# libsignal-service-rs
cd /tmp
git clone https://github.com/whisperfish/libsignal-service-rs.git libsignal-upstream
cd libsignal-upstream
git checkout 782c0d6
cd /tmp
cd /path/to/xous-app-signal
diff -ruN -x .git -x target -x Cargo.lock \
    /tmp/libsignal-upstream/ vendor/libsignal-service-rs/ \
    > vendor/libsignal-service-rs.diff
```

```sh
# curve25519-dalek (compare to betrusted-io fork at version 4.1.2 baseline,
# since the local copy is essentially that fork + 4.1.2→4.1.3 bump + lizard
# port from signalapp's signal-curve25519-4.1.3 tag)
cd /tmp
git clone https://github.com/betrusted-io/curve25519-dalek.git curve25519-upstream
cd curve25519-upstream
# Baseline: betrusted-io fork `main` @ 16e087a ("version bump",
# 2026-01-29). Pin this SHA explicitly — if the fork's main moves,
# the diff would silently mix upstream churn into our delta.
git checkout 16e087a
cd /path/to/xous-app-signal
diff -ruN -x .git -x target -x Cargo.lock \
    -x ed25519-dalek -x x25519-dalek \
    /tmp/curve25519-upstream/ vendor/curve25519-dalek/ \
    > vendor/curve25519-dalek.diff
```

`.gitignore` filters: regenerated diff files should be checked in
verbatim. CI may diff the regenerated file against the checked-in one
to detect drift.

## When upstream needs to update

Two scenarios:

**Crypto fix landed upstream**: bump the pin (re-vendor). Run
`git log --oneline upstream..pin` to enumerate what we'd be giving
back / re-applying, then re-port the local changes onto the new pin.
Keep the diff as small as possible — every line of local change is
one more thing to re-justify per audit cycle.

**Local change is no longer needed** (e.g. upstream merges PR equivalent
of our patch): remove the local change, regenerate the `.diff`, file
the smaller diff. The eventual goal is `*.diff` files that are empty.

## Status

| File | Populated | Last regen |
|---|---|---|
| `presage.diff` | yes (329 lines, 17K) | 2026-07-06 |
| `libsignal-service-rs.diff` | yes (1646 lines, 70K) | 2026-05-14 |
| `curve25519-dalek.diff` | yes (667 lines, 26K) | 2026-07-06 |

The 2026-07-06 regeneration reflects the prune of never-compiled
workspace members (presage-cli, presage-store-sled,
presage-store-sqlite, ed25519-dalek, x25519-dalek); the presage line
count dropped because the old diff carried those members' divergence.
