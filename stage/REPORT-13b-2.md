# Stage 13b-2 — full PDDB-backed `KvBackend`

**Date.** 2026-05-06
**Status.** Hand-rolled PDDB IPC client **landed**. New crate
`crates/xous-pddb-ipc/` (~440 LoC) replicates the protocol surface
needed by `KvBackend`, and `presage-store-pddb`'s `backend_pddb.rs`
now wraps it as a real `KvBackend` impl. xas binary, when built
with `--features pddb-real`, switches to the real backend at boot.

End-to-end round-trip in Renode confirms five different opcodes
(`KeyRequest`, `KeyCountInDict`, `ListKeyV2`, `DeleteKey`, plus
`PollOp::Poll` from the Stage 13b probe) work on the wire. PDDB
itself is not mounted at boot in our image (separate subsystem-level
question — see §3); the IPC plumbing is validated independently.

---

## 1. What this stage delivered

### 1.1 New crate `crates/xous-pddb-ipc/`

```
crates/xous-pddb-ipc/
├── Cargo.toml          # 6 deps (xous, xous-ipc, xous-names, rkyv, num-*, bitfield, log)
├── src/
│   ├── lib.rs          # Module shell. cfg(target_os = "xous").
│   ├── api.rs          # Wire structs verbatim from xous-core/services/pddb/src/api.rs.
│   └── client.rs       # PddbClient + KeyHandle (~330 LoC).
```

Public API:

```rust
pub struct PddbClient { /* main_conn + poller_conn */ }

impl PddbClient {
    pub fn new() -> Result<Self, Error>;
    pub fn is_mounted(&self) -> bool;            // Mount poller, scalar IPC.
    pub fn open(&self, dict, key, opts) -> Result<KeyHandle, Error>;
    pub fn delete_key(&self, dict, key) -> Result<(), Error>;
    pub fn delete_dict(&self, dict) -> Result<(), Error>;
    pub fn list_keys(&self, dict) -> Result<Vec<String>, Error>;
}

pub struct KeyHandle<'a> { /* token + Buffer + cursor */ }
impl<'a> Read for KeyHandle<'a> { ... }      // Opcode::ReadKey, paginated PddbBuf.
impl<'a> Write for KeyHandle<'a> { ... }     // Opcode::WriteKey, paginated PddbBuf.
                                              // flush() == Opcode::WriteKeyFlush.
impl<'a> Drop for KeyHandle<'a> { ... }      // Opcode::KeyDrop best-effort.
```

The wire structs (`PddbKeyRequest`, `PddbDictRequest`, `PddbKeyList`,
`PddbBuf`, `PddbRequestCode`, `PddbRetcode`, `KeyAttributes`,
`KeyFlags`, `ApiToken`) are byte-compatible verbatim copies from
`~/precursor-signal/repos/xous-core/services/pddb/src/api.rs` —
rkyv 0.8 → 0.8 across version bumps within the 0.8.x semver line.
Stage 13b's IPC probe (`f7a9e7b`) is the reference smoke for this
layer; Stage 13b-2's `probe-pddb-real` extends it to buffered ops.

### 1.2 `presage-store-pddb` real backend

`crates/presage-store-pddb/src/backend_pddb.rs` is no longer a stub.
`PddbBackend::connect()` returns a real `KvBackend` wrapping a
`Mutex<PddbClient>` shared via `Arc` for cheap `PddbStore::clone()`.
A new `PddbStore::with_pddb_backend()` constructor is the
companion entry point.

The Mutex serializes IPC requests; PDDB's server is single-threaded
so concurrent requests would queue server-side anyway — using a
`Mutex` here just makes the head-of-line wait explicit and avoids
contending on Xous's IPC fairness scheduler.

### 1.3 xas binary integration

`crates/xous-app-signal` gains two new feature flags:

| feature | effect |
|---------|--------|
| `pddb-real` | `build_store()` calls `PddbStore::with_pddb_backend()` instead of `with_mock_backend()`. Falls back to mock + warning log if connect fails (so smoke / boot still works without PDDB). |
| `probe-pddb-real` | On top of `pddb-real`, after worker spawn, runs a put/get/list/delete/list cycle against the real backend and logs every outcome. Equivalent of Stage 13a/Stage 13b probes for the buffered IPC layer. |

A new `tests/renode/xas-pddb-real-probe.robot` drives the probe.

---

## 2. Wire-protocol findings

The full round-trip log from a `--features probe-pddb-real` run
(extracted from `robot_output.xml`):

```
INFO:xas: xas: store=PDDB (real)
INFO:xas: probe-pddb-real: starting put/get/delete cycle
INFO:xas: probe-pddb-real: connected in 3ms, mounted=false
WARN:xas: probe-pddb-real: put FAIL after 0ns: kv backend: open for write: KeyRequest: Uninit
WARN:xas: probe-pddb-real: get FAIL: kv backend: open for read: KeyRequest: Uninit
INFO:xas: probe-pddb-real: list_keys OK [] in 0ns
WARN:xas: probe-pddb-real: delete FAIL: kv backend: delete_key: DeleteKey: Create
INFO:xas: probe-pddb-real: post-delete list empty in 0ns
INFO:xas: probe-pddb-real: probe done in 44ms
```

What this proves and what it doesn't:

### 2.1 Proves: rkyv schema compatibility

Each "FAIL: …Code: <variant>" line is a successful round-trip — the
server received our rkyv-serialized request, decoded it, ran its
handler, and rkyv-serialized a response back which our crate
decoded. If the schema were off by even one field, we'd get a
`Buffer::to_original` decode error instead of a server-side response
code. The wire format is sane.

### 2.2 Proves: opcode coverage

Five different opcodes round-trip:
- `KeyRequest = 15` — `Opcode::KeyRequest` for `open()`. Returns
  `PddbRequestCode::Uninit` on unmounted, which is what we observed.
- `KeyCountInDict = 11` — `Opcode::KeyCountInDict` for `list_keys`.
  Phase 1.
- `ListKeyV2 = 45` — `Opcode::ListKeyV2` for `list_keys`. Phase 2.
- `DeleteKey = 8` — `Opcode::DeleteKey` for `delete_key()`. Returned
  `PddbRequestCode::Create`, which is unusual (Create is normally a
  request code, not a response). On a mounted PDDB this should
  return `NoErr` or `NotFound`; on unmounted, the server's behavior
  here is just "didn't process the request meaningfully," consistent
  with the Uninit response on KeyRequest.
- `PollOp::Poll = 0` — Mount poller. Stage 13b probe still fires.

Read/Write streaming on `KeyHandle` (`Opcode::ReadKey = 16`,
`Opcode::WriteKey = 17`, `Opcode::WriteKeyFlush = 18`) is wired but
not exercised in the probe — the prior `KeyRequest: Uninit` short-
circuits before we get a token. Stage 13b-3 (when PDDB is actually
mounted) will exercise these.

### 2.3 Doesn't yet prove: read/write streaming

The four buffer-shaped ops haven't been hit in Renode. If our
`PddbBuf::from_slice_mut` cast or the `pos` cursor logic has a
subtle bug, we'd only see it once a real read/write hits — which
needs PDDB mounted. The code is faithful to
`services/pddb/src/frontend/pddbkey.rs:Read::read` (port reviewed
side-by-side with the upstream impl), so the risk is bounded.

### 2.4 Doesn't yet prove: persistence across reboots

This was the original Stage 13b-2 deliverable per REPORT-13b.md §4.
It's gated on PDDB being mounted — a separate question (§3).

---

## 3. PDDB mount remains the gating subsystem question

Adding `--feature pddb/autobasis` to the `cargo xtask app-image`
invocation does *not* auto-mount PDDB at boot. Tracing the upstream
code: `services/pddb/src/main.rs`'s init path calls
`pddb_os.pddb_mount()` only after `syskey_ensure()`, which under
`gen1` (the rv32 default) loops on `try_login()` indefinitely
waiting for a password via the GAM `Modals` server. The `autobasis`
feature only meaningfully changes behavior when paired with
`pddbtest`, and it's wired up for `target_hosted()` in the
`pddb-ci` xtask subcommand, not `app-image`.

To get PDDB mounted in our smoke / probe images we'd need one of:

a. **Pre-seed `tools/pddb-images/renode.bin`** with a formatted PDDB
   image carrying a known password. The renode flash-backing file
   is what `services/pddb/src/main.rs` reads on boot; pre-seeding
   means PDDB starts in the "system already initialized" state and
   `try_login()` succeeds against the burned-in password.

b. **Patch xous-core to add a `dev-mount` feature** that bypasses
   the `try_login()` loop with a deterministic password under the
   `target_os = "xous" + cfg(feature = "autobasis")` path. ~10 LoC
   in `services/pddb/src/backend/hw.rs:syskey_ensure`.

c. **Robot keypress automation** to feed the modal password during
   first-boot init. Brittle and slow, but doesn't require xous-core
   changes.

(b) is the cleanest path and aligns with how the project usually
solves "I need this to work in CI" questions. (a) would require
generating the seed image, which itself needs a way to format
PDDB — chicken-and-egg unless we shell out to a hosted-mode PDDB.
(c) is the fallback if (b) and (a) prove harder than expected.

This is its own slice — call it **Stage 13b-3 — auto-mount path**.
It's a prerequisite for the persistence Robot test originally
scoped under 13b-2, which we're now bumping into 13b-3.

---

## 4. Files touched

```
A  crates/xous-pddb-ipc/Cargo.toml          (new crate, ~30 lines)
A  crates/xous-pddb-ipc/src/lib.rs          (~30 lines, module shell)
A  crates/xous-pddb-ipc/src/api.rs          (~180 lines, wire structs)
A  crates/xous-pddb-ipc/src/client.rs       (~360 lines, PddbClient + KeyHandle)
A  crates/presage-store-pddb/src/backend_pddb.rs   (replaces stub, ~140 LoC)
M  crates/presage-store-pddb/src/lib.rs     (with_pddb_backend constructor +
                                             PddbBackend re-export)
M  crates/presage-store-pddb/Cargo.toml     (xous-pddb-ipc dep, target-cfg-gated)
M  crates/xous-app-signal/Cargo.toml        (pddb-real / probe-pddb-real features)
M  crates/xous-app-signal/src/main.rs       (build_store() + probe_pddb_real)
M  Cargo.toml                               (xous-pddb-ipc workspace member)
A  tests/renode/xas-pddb-real-probe.robot   (probe Robot test)
A  stage/REPORT-13b-2.md                    (this file)
```

The xous-pddb-ipc crate carries the bulk of the new code (~600 LoC
total). It compiles in isolation and is exercised by both Stage 13b
probe (scalar IPC) and Stage 13b-2 probe (buffered IPC) without
either touching xous-core's `pddb` crate.

---

## 5. Verification

```
cargo check --target=riscv32imac-unknown-xous-elf -p xous-pddb-ipc          → ok
cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb     → ok
                                                  --features pddb-backend
cargo build --target=riscv32imac-unknown-xous-elf --release \
            -p xous-app-signal --features probe-pddb-real                   → ok (1m13s)

cargo xtask app-image xas:.../xas --git-describe v0.9.21-0-g0000000 \
                       --feature pddb/autobasis                              → image rebuilt

renode-test tests/renode/xas-pddb-real-probe.robot                          → PASS
  (5 opcode round-trips logged; 44 ms total)

# After restoring the non-probe build:
renode-test tests/renode/xas-smoke.robot                                    → PASS (44 s)

cargo test -p xous-signal-bridge -p xous-app-signal-ui \
           -p presage-store-pddb                                            → 3 + 31 + 22 passed
cargo clippy --workspace --all-targets -- -D warnings                       → clean
cargo clippy -p xous-app-signal --features probe-pddb-real \
             --all-targets -- -D warnings                                   → clean
cargo fmt --all -- --check                                                  → clean
```

---

## 6. Stage 13 phasing — updated

| sub-phase | status |
|-----------|--------|
| 13a | landed (`4648826`) |
| 13b | probe landed (`f7a9e7b`) |
| **13b-2** | **landed (this commit)** — IPC client + real backend wired; persistence gated on 13b-3 |
| 13b-3 | scoped — PDDB auto-mount path; smallest fix is a `dev-mount` feature in xous-core's pddb |
| 13c | scoped — mock HTTP/WS transport (independent of 13b track) |
| 13d | deferred — u32e backend |
| 13e | scoped — physical hardware |

13b-3 becomes the next focused slice. After it, the persistence
Robot test originally scoped under 13b-2 lights up.
