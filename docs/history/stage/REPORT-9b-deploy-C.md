# Stage 9b-deploy Phase C — runtime backends

**Date.** 2026-05-06
**Status.** **Phase C-1 landed** (real `__getrandom_v03_custom` via
xous-core's TRNG client). **Phase C-2 deferred** (real PDDB
backend) — a one-feature dependency cascade pulls in the
workspace-merge blocker. **Phase C-3 deferred** (u32e backend) — a
type incompatibility between the betrusted-io u32e fork and the
libsignal lizard port can't be resolved without substantial
vendored-curve25519-dalek work.

End-to-end Renode smoke test still passes (44 s). The C-1 changes
add real entropy to the binary's getrandom path without enabling any
runtime flow that would actually trip getrandom on the smoke path.

---

## 1. Phase C-1 — landed

### 1.1 What it does

The Stage 9a `__getrandom_v03_custom` panic stub is replaced with a
real implementation that calls xous-core's `Trng::fill_buf`. The
flow:

1. Lazy-init a thread-local `Trng` instance (per-thread connection
   to the TRNG service via `XousNames::request_connection_blocking`).
2. Stream `len` random bytes into the caller's `dest`. Aligned u32
   words come from `Trng::fill_buf(&mut [u32; ≤1020])`; an unaligned
   tail (`len % 4 != 0`) is handled with a single 1-word scratch
   read.
3. Return `Ok(())` — or `getrandom::Error::UNSUPPORTED` on IPC
   failure.

The `OnceCell` thread-local matches the rest of the libsignal code
path: every getrandom call reuses the same TRNG connection, so we
pay the IPC connection-establishment cost once per thread.

### 1.2 Cargo.toml additions

Five path-deps + one crates.io rename:

```toml
[target.'cfg(target_os = "xous")'.dependencies]
…
trng = { path = "../../../repos/xous-core/services/trng" }
xous-names = { package = "xous-api-names", version = "0.9.71" }
xous-api-susres = { path = "../../../repos/xous-core/api/xous-api-susres" }
flatipc = { path = "../../../repos/xous-core/libs/flatipc" }
flatipc-derive = { path = "../../../repos/xous-core/libs/flatipc-derive" }
```

The `xous-names = { package = "xous-api-names" }` rename mirrors the
convention every other Xous app uses (`apps/repl/Cargo.toml`,
`apps/transientdisk/Cargo.toml`, etc.) so the import becomes
`use xous_names::XousNames;`.

### 1.3 Verification

```
cargo build --target=riscv32imac-unknown-xous-elf --release   → ok (1m11s)
cargo test                                                    → 22 + 31 + 3 passed
cargo clippy --workspace --all-targets -- -D warnings         → clean
cargo fmt --all -- --check                                    → clean

cargo xtask dist                                              → 52,726,076 B (was 52,472,060)
cargo xtask app-image xas:… --git-describe v0.9.21-0-g0000000 → image rebuilt
renode-test tests/renode/xas-smoke.robot                      → PASS (44 s)
```

ELF symbol audit:

```
__getrandom_v03_custom                   present
xous_api_names::XousNames::new           present
xous_api_names::XousNames::request_connection_blocking  present
xas::__getrandom_v03_custom::TRNG (thread_local OnceCell)  present
```

Binary size grew by 254 KB (52,472,060 → 52,726,076). That's the
TRNG client + xous-api-names + flatipc + xous-api-susres static
data. Negligible compared to the libsignal core bulk.

The smoke test passing is *necessary but not sufficient* — the
boot path doesn't trip getrandom (mock store, no real flow). The
first time the new wiring is exercised in a meaningful way will be
when a real flow runs (link/receive/send), which is gated on
Phase C-2 anyway.

---

## 2. Phase C-2 — deferred (real PDDB)

### 2.1 The blocker

`services/pddb`'s `Pddb::new()` is gated on the `gen1` feature,
which pulls in `trng + spinor + root-keys + keystore-api/gen1 +
llio + tts-frontend + gam + modals + precursor-hal`. Adding
`pddb = { path = "../../../repos/xous-core/services/pddb",
features = ["gen1"] }` means path-dep'ing all of those into our
workspace — which is the same workspace-merge attempt that already
failed at Stage 9b's first try because of `[patch.crates-io].aes`.

In particular `services/pddb`'s Cargo.toml has the line `aes =
{ path = "../aes" }` — a direct path-dep on xous-core's services/aes
IPC shim. The shim doesn't expose `Aes256Enc`, which libsignal's
zkgroup needs. With both dependencies in the same workspace, Cargo
would unify the `aes` crate name across the dep graph.

### 2.2 The two real options

a. **Hand-rolled PDDB IPC client.** Replicate just the IPC wire
   protocol for `KvBackend`'s five operations (`get`, `put`, `delete`,
   `delete_dict`, `list_keys`). PDDB's IPC surface for these is in
   `services/pddb/src/api.rs` (Opcodes) and `services/pddb/src/lib.rs`
   (the `Pddb::*` client side). Likely 500–1000 LoC of careful
   protocol replication. Avoids the workspace-merge blocker entirely.

b. **Vendor pddb with aes swapped to upstream.** Copy
   `services/pddb` into our workspace's `vendor/`, change the `aes`
   path-dep to `aes = "0.8"`, and verify `Pddb`'s internal calls to
   `aes::*` types still resolve. The upstream `aes` crate has the
   `Aes256Enc` zkgroup needs; the IPC-shim aes was only needed for
   HW acceleration, which we'd lose (slower but functional).
   Substantially larger surface than (a).

Both are at least one full stage of work. Mock PDDB is sufficient
for the smoke test today and for Stage 10/11/12's logic-side tests.
Real PDDB is gated on real runtime flows, which is its own follow-up.

---

## 3. Phase C-3 — deferred (u32e backend)

### 3.1 The first attempt

`.cargo/config.toml`'s `--cfg curve25519_dalek_backend="u32e_backend"`
re-enabled, plus `utralib = { features = ["precursor", "precursor-pvt"]
}` added to make the SOC's CSR map available to the IP-core driver.
The build got past utralib's gitrev gate but failed in the vendored
curve25519-dalek's `lizard` module.

### 3.2 The deeper problem

With `curve25519_dalek_backend = "u32e_backend"`, the crate's
`FieldElement` type aliases to `u32e::field::Engine25519` (the
IP-core engine state, which holds CSR pointers / partial computation
state) rather than to `FieldElement2625` (the limb-array
representation). The vendored libsignal lizard port (`lib.rs:102`,
"Ported verbatim from signalapp/curve25519-dalek (signal-curve25519-
4.1.3 tag)") freely mixes the two: e.g.
`lizard_ristretto.rs:42` is `&(&self.T + &FieldElement::ONE) *
&lizard_constants::DP1_OVER_DM1`, where `self.T` produces an
`Engine25519` and `DP1_OVER_DM1` is a `FieldElement2625`. The
multiplication operator isn't defined for that mix.

Lizard isn't optional — `zkgroup`'s `RistrettoPoint::from_uniform_bytes_single_elligator`
(`zkgroup/src/common/sho.rs:99` and friends) is called during
profile-fetch and group message paths. We can't gut lizard from the
build.

### 3.3 What re-enabling u32e would take

Two clean paths:

a. **Engine25519 ↔ FieldElement2625 conversions** in the vendored
   lizard module. The IP-core engine wraps a fiat-25519 limb
   representation internally (per
   `vendor/curve25519-dalek/curve25519-dalek/src/backend/serial/u32e/
   field.rs`, `pub struct FieldElement2625(pub(crate)
   fiat_25519_tight_field_element)`), so the conversion is
   mechanical: pull limbs out, re-pack. Need to do this for every
   mixed-arithmetic site in the lizard port without disturbing the
   other vendored deltas.

b. **A libsignal upstream patch** to make zkgroup's lizard use
   pure FieldElement2625 throughout (no Engine25519 mixing). Larger
   blast radius, harder to upstream, but a one-time fix.

Both are a stage of their own. For now the portable Rust backend is
correct and ~5–10× slower than u32e — acceptable for development,
not for a shipping product.

---

## 4. What this commit changes

```
M  .cargo/config.toml                       (Phase C-3 comment block updated)
M  crates/xous-app-signal/Cargo.toml        (Phase C-1 path-deps)
M  crates/xous-app-signal/src/main.rs       (real __getrandom_v03_custom)
A  stage/REPORT-9b-deploy-C.md              (this file)
```

No changes to other crates. The Phase C-1 surface is bounded to
the binary's main.rs and Cargo.toml.

---

## 5. Stage 9b-deploy phase status going forward

| phase | what | status |
|-------|------|--------|
| A | rv32 logger via xous-api-log | landed (`32094a5`) |
| B | image bundling + Renode smoke | landed (`7c9b353` + xous-core `4ddf6738a`) |
| C-1 | real getrandom via Trng | **landed (this commit)** |
| C-2 | real PDDB backend | deferred — needs hand-rolled IPC client or vendored pddb |
| C-3 | u32e backend | deferred — needs Engine25519↔FieldElement2625 conversions in lizard |

Next stages going forward, in roadmap order:

1. **Stage 13 — hardware deploy** (would naturally absorb C-2 and C-3
   as part of the on-device flow validation work).
2. **Stage 12+ network** — verify `xous-net-bridge`'s `SyncHttpClient`
   actually reaches Signal servers from inside Renode (or hardware).
   Renode's WF200 wifi peripheral may or may not be a working stack.
3. **Stage 14 — performance pass** — would absorb C-3 for real (u32e
   is a 5–10× speedup on curve ops; matters once flows actually run).

---

## 6. Reproducing C-1 locally

```sh
cd ~/precursor-signal/xous-app-signal
cargo xtask dist
cd ~/precursor-signal/repos/xous-core
git checkout xas
cargo xtask app-image \
    xas:$HOME/precursor-signal/xous-app-signal/dist/xas-rv32/xas \
    --git-describe v0.9.21-0-g0000000
cd ~/precursor-signal/xous-app-signal
renode-test tests/renode/xas-smoke.robot
# → Tests finished successfully :) (44 s)
```

Same flow as Phase B. The C-1 changes are transparent to the
pipeline — the binary just contains a real getrandom now.
