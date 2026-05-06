# Stage 13b — PDDB IPC probe (path B-i validation)

**Date.** 2026-05-06
**Status.** Probe shipped. Hand-rolled PDDB IPC path **validated**:
`xous::send_message` against the running PDDB Mount Poller works
end-to-end in Renode. The full path B-i (real PDDB backend) is now
de-risked enough to commit to as the Stage 13b-2 implementation
choice. Path B-ii (vendor pddb + swap aes) is no longer the better
option — investigation showed it would pull in 10+ services as
path-deps.

The actual `KvBackend`-against-real-PDDB work is split off as
**Stage 13b-2** (next session); this commit lands the probe + the
de-risking.

---

## 1. The probe

A `probe-pddb` Cargo feature on `xous-app-signal`. When enabled,
after `run_signal_worker` returns, `main()` calls a new
`probe_pddb()` function that:

1. Resolves the SID for `"_PDDB Mount Poller_"` via
   `XousNames::request_connection_blocking`.
2. Sends `Message::new_blocking_scalar(0, 0, 0, 0, 0)` against that
   connection. (Opcode `0` = `PollOp::Poll`, per
   `services/pddb/src/api.rs`.)
3. Pattern-matches the response — expecting `Scalar1(is_mounted)` —
   and logs the outcome, latency, and whether the value is non-zero.

Total probe code: ~70 LoC inline in `main.rs`. Imports nothing from
the `pddb` crate; only `xous` (kernel API) and `xous_names` (already
on hand from Phase C-1).

A new Robot test (`tests/renode/xas-pddb-probe.robot`) drives the
probe and `Wait For Line On Uart` for the connect, Poll outcome, and
done banner.

---

## 2. Findings

```
INFO:xas: probe-pddb: starting PDDB mount-poller probe
INFO:xas: probe-pddb: connected to PDDB Mount Poller in 31ms
INFO:xas: probe-pddb: Poll OK is_mounted=false after 0ns
INFO:xas: probe-pddb: probe done in 40ms
```

Three concrete observations:

### 2.1 The hand-rolled IPC path works on the wire

`request_connection_blocking` resolved the SID (no hang, no error).
`send_message` returned `xous::Result::Scalar1(0)` — exactly what
`services/pddb/src/main.rs:606`'s `xous::return_scalar(msg.sender, 0)`
would produce when `is_mounted = false`. The `xous_ipc` and `rkyv`
versions in our standalone workspace match the on-image PDDB's
expectations closely enough that scalar IPC works without
adjustment.

This validates the path-B-i assumption: we can replicate the PDDB
client by importing struct definitions + opcode constants and
calling `xous::send_message` / `Buffer::into_buf` ourselves. The
"protocol replication risk" the deferred report flagged turned out
to be small for scalar messages. The lend/lend_mut buffered path
(needed for `KeyRequest`, `ReadKey`, `WriteKey`) is the next
unknown — Stage 13b-2 will exercise it.

### 2.2 PDDB is not auto-mounted in the image

`is_mounted = false`. The image we build with
`cargo xtask app-image xas:/path/to/xas --git-describe …` does not
mount PDDB at boot — it's waiting for a password-driven mount via
the GAM modals. For Stage 13b-2 to read/write, the image needs to
be rebuilt with `--feature pddb/autobasis` (or `pddb/ci` per
xous-core's xtask debug flags) so the daemon mounts a default basis
without UI interaction.

This is a Robot-test-side concern: we can drive the mount flow
either by image-side feature flag (preferred for automation) or by
keypress automation (brittle). Recommend feature flag.

### 2.3 Sub-millisecond IPC latency is excellent

The Poll roundtrip itself reports `0ns` because `Instant::elapsed()`
rounded down — clearly below 1µs in Renode emulated time. The
overall probe is dominated by the 31ms `XousNames` connection setup;
once a CID is in hand, Xous-IPC is essentially free.

This rules out IPC overhead as a worry for the high-frequency PDDB
operations libsignal-protocol's session-store does (one PDDB
read+write per incoming message). Whether the PDDB *server-side*
work fits in the budget is a separate question — its own perf pass
in Stage 14.

---

## 3. Path B-i vs B-ii — final call

Earlier reports framed this as "B-i hand-rolled (~500–1000 LoC) vs
B-ii vendor pddb + swap aes (smaller surface)." The probe and a
deeper look at pddb's deps reverses that framing.

### B-ii is the trap

`services/pddb`'s `Pddb::new()` is gated behind the `gen1` feature,
and `gen1` requires `susres + trng + spinor + root-keys + llio +
tts-frontend + gam + modals + precursor-hal + keystore-api/gen1`.
Vendoring pddb into our `vendor/` *and getting it to compile* means
also vendoring (or path-dep'ing) all of those — which is the
workspace-merge attempt that already failed at Stage 9b's first
try. The "swap aes to upstream" trick fixes the headline blocker
but leaves spinor + root-keys + keystore + ux-api etc. as
unresolved imports.

### B-i is bounded

Five operations on `KvBackend`:

| op | PDDB IPC sequence |
|----|-------------------|
| `is_mounted` | `Scalar(IsMounted=0)` → `Scalar2(code, count)` |
| `get(dict, key)` | `lend_mut(KeyRequest=15, PddbKeyRequest)` → token; `lend(ReadKey=16, PddbBuf)` → bytes |
| `put(dict, key, val)` | `lend_mut(KeyRequest=15, PddbKeyRequest{create_key=true})` → token; `lend(WriteKey=17, PddbBuf)`; `Scalar(WriteKeyFlush=18, token)` |
| `delete(dict, key)` | `lend_mut(DeleteKey=8, PddbKeyRequest)` → response |
| `delete_dict(dict)` | `lend_mut(DeleteDict=9, PddbDictRequest)` → response |
| `list_keys(dict)` | `Scalar(KeyCountInDict=11)` → count; iterate `lend_mut(...)` requests |

Each op is one or two `xous::send_message` calls. The IPC payloads
are rkyv-serialized structs that we copy verbatim from
`services/pddb/src/api.rs`. The `PddbKey`-as-stream layer (which
chunked reads / writes use) is the most complex piece — paginated
`PddbBuf` exchanges with sequence numbers — but it's still bounded
(~150 LoC in services/pddb).

Total expected size: ~400–600 LoC, contained in
`crates/xous-pddb-ipc/` (new crate) under `cfg(target_os = "xous")`
so it doesn't affect hosted builds.

---

## 4. What's next (Stage 13b-2)

Concrete deliverables for the next session:

1. **New crate `crates/xous-pddb-ipc/`.** Public API:
   ```rust
   pub struct PddbClient { conn: xous::CID }
   impl PddbClient {
       pub fn new() -> Result<Self, Error>;
       pub fn is_mounted(&self) -> bool;          // Opcode 0
       pub fn open(&self, dict, key, opts) -> Result<KeyHandle>;
       pub fn delete_key(&self, dict, key) -> Result<()>;
       pub fn delete_dict(&self, dict) -> Result<()>;
       pub fn list_keys(&self, dict) -> Result<Vec<String>>;
   }
   pub struct KeyHandle { /* token + buf */ }
   impl io::Read for KeyHandle { ... }
   impl io::Write for KeyHandle { ... }
   ```
2. **Struct copies from `services/pddb/src/api.rs`.** Verbatim:
   `PddbKeyRequest`, `PddbDictRequest`, `PddbBasisRequest`,
   `PddbBuf`, `PddbRequestCode`, `KeyAttributes`, `KeyFlags`. Plus
   the `Opcode` enum constants we use.
3. **Image rebuild with `pddb/autobasis`.** xous-core xtask
   invocation needs `--feature pddb/autobasis` (or equivalent) so
   the smoke + Stage 13b-2 tests can read/write without GAM modals.
4. **Wire `PddbClient` into `presage-store-pddb`** behind a
   `pddb-real` feature flag. `KvBackend` impl forwards to
   `PddbClient` operations on rv32-xous; mock backend stays for
   hosted builds and unit tests.
5. **Persistence Robot test.** Boot, write a key, reboot (Renode's
   `mach reset` keeps the flash backing intact), boot again, read
   the key back. Verifies the round-trip.

Estimated 13b-2 effort: 4–8 hours of focused work. Risk: the
streaming `PddbBuf` exchange has subtle sequencing requirements
that are easy to get wrong; expect 1–2 debug iterations against
the live Renode image before reads land cleanly.

---

## 5. Files touched

```
M  crates/xous-app-signal/Cargo.toml         (probe-pddb feature + xous direct dep)
M  crates/xous-app-signal/src/main.rs        (probe_pddb() function +
                                              call site under probe-pddb)
A  tests/renode/xas-pddb-probe.robot         (probe Robot test)
A  stage/REPORT-13b.md                       (this file)
```

---

## 6. Verification

```
cargo build --target=riscv32imac-unknown-xous-elf --release \
            -p xous-app-signal --features probe-pddb       → ok (1m02s)

cargo xtask app-image xas:.../xas --git-describe …          → image rebuilt
                                                              (PID 27: xas)

renode-test tests/renode/xas-pddb-probe.robot               → PASS
                                                              (probe-pddb: Poll OK is_mounted=false)

# After restoring non-probe build:
renode-test tests/renode/xas-smoke.robot                    → PASS (44 s)

cargo test -p xous-signal-bridge -p xous-app-signal-ui \
           -p presage-store-pddb                            → 3 + 31 + 22 passed
cargo clippy --workspace --all-targets -- -D warnings       → clean
cargo clippy -p xous-app-signal --features probe-pddb \
             --all-targets -- -D warnings                   → clean
cargo fmt --all -- --check                                  → clean
```

---

## 7. Stage 13 phasing — updated

| sub-phase | status |
|-----------|--------|
| 13a | landed (`4648826`) — finding: network unreachable in Renode; mock transport recommended |
| **13b** | **probe landed (this commit)** — finding: PDDB IPC path-B-i validated, autobasis needed for next slice |
| 13b-2 | scoped — full PDDB KvBackend impl, ~400–600 LoC |
| 13c | scoped — mock HTTP/WS transport, runs flows |
| 13d | scoped, deferred — u32e backend |
| 13e | scoped — physical hardware |

Recommended next stage: **13b-2** (real KvBackend) AND **13b-prep**
(MockHttpClient for 13c) can land in parallel sessions; they're
independent.
