# Stage 13b-3 — PDDB auto-mount investigation

**Date.** 2026-05-06
**Status.** Investigated, **deferred**. Auto-mounting PDDB on rv32
inside Renode without manual UI input requires a multi-layer
xous-core upstream patch (~50–100 LoC across pddb + rootkeys) that
lives outside our standalone workspace's scope. The Stage 13b-2
deliverable is unaffected — the IPC client is validated against
the live PDDB server even in the unmounted state.

This report documents the depth of the problem, the pragmatic
attempts that didn't work, and the design for a clean future
solution. Stage 13c (mock HTTP/WS transport) is recommended as
the next stage because it's independent and immediately useful.

---

## 1. The dependency chain that gates auto-mount

The path from "boot" to "PDDB mounted" goes through three security
layers, each of which is UX-driven by design:

```
pddb_os::pddb_mount()                  services/pddb/src/backend/hw.rs
  ├── fast_space_read()
  ├── syskey_ensure()                  ← first blocker
  │     #[cfg(feature = "gen1")]
  │     while self.try_login() != PasswordState::Correct {
  │         clear_password();
  │         modals.show_notification(t!("pddb.badpass_infallible"));
  │     }                              loops forever waiting on
  │                                    GAM modals to provide input
  └── (if syskey set) load system basis
```

`syskey_ensure` calls `try_login`, which checks the on-flash
`StaticCryptoData` (SCD). When the flash is blank (all 0xFF) on
first boot, SCD has `version == 0xFFFF_FFFF` and `try_login`
returns `PasswordState::Uninit`. The loop in `syskey_ensure` then
spawns a Modals notification and re-tries. Without UI input, the
notification has no responder; the loop is unkillable.

To unblock automatically we'd need either:

a. **Bypass `syskey_ensure`'s modal loop.** A new cfg-gated path
   that synthesizes a system basis key directly. But the format
   step that follows depends on `rootkeys.is_initialized()`, which
   is the *second* blocker — root-keys initialization itself is a
   separate UI flow with its own modals (gateware verification,
   key derivation prompts).

b. **Pre-seed the flash** with a known SCD + known
   `BasisKeys` + known root-keys state, all consistent with a
   well-known password. The flash region for PDDB on Precursor is
   `[0x01D8_0000, 0x01D8_0000 + 4 MiB)` per
   `libs/precursor-hal/src/board/precursor.rs:31`. Splicing
   `tools/pddb-images/hosted.bin` (a 4 MiB hosted-mode dump) into
   `renode.bin` at this offset isn't enough — root-keys lives in a
   different flash region with its own derivation, and the SCD's
   key wrapping uses values from the root-keys area.

c. **Fully hand-rolled PDDB initialization in our app.** xas could
   issue PDDB's `TryMount` opcode with a hardcoded password as part
   of boot. But `TryMount` itself routes through GAM modals on
   error, and on first boot, the format step also goes through
   modals.

None of these is a one-evening patch.

---

## 2. What was tried this stage

### 2.1 `tools/pddb-images/renode-formatted.bin` swap

The dev box had `renode-formatted.bin` (md5
`5097cfa89015a8daab0ac8b3279404d1`) — a 128 MiB flash image
captured by some earlier session. Pragmatic test: copy it to
`renode.bin` and re-run the probe-pddb-real Robot test.

```
$ cp tools/pddb-images/renode-formatted.bin tools/pddb-images/renode.bin
$ renode-test xas-pddb-real-probe.robot
```

Result — identical to the unmounted state:

```
probe-pddb-real: connected in 4ms, mounted=false
probe-pddb-real: put FAIL: KeyRequest: Uninit
probe-pddb-real: get FAIL: KeyRequest: Uninit
probe-pddb-real: list_keys OK []
probe-pddb-real: delete FAIL: DeleteKey: Create
probe-pddb-real: post-delete list empty
probe-pddb-real: probe done in 45ms
```

The pre-formatted image either:
- Was not actually formatted to a usable state (perhaps captured
  mid-test from a hosted-mode run, with formatting that requires
  the hosted spinor backend), or
- Was formatted with a password we don't know and have no path to
  inject into our rv32 build.

Either way, the simplest possible fix doesn't deliver auto-mount.

### 2.2 Searching for pre-existing test bypasses

`services/pddb/src/main.rs` has an `Opcode::IsMounted` block that
defers responses to a `mount_notifications` queue when no basis is
cached. Under `cfg(not(target_os = "xous"))` (hosted) the response
is short-circuited to a synthesized success — but that path
doesn't exist for rv32. Similarly, `pddbtest + autobasis` cfg
combos provide bypasses for *secondary* basis listing
(`pddb_get_all_keys`), but the *system basis* still goes through
the password loop.

There's no existing "rv32 test mode" that bypasses syskey_ensure.
We'd need to add one.

---

## 3. Sketch of a clean dev-mount patch

For when this becomes worth the cost, here's the design that
drops in cleanly:

```toml
# services/pddb/Cargo.toml
[features]
# Existing: gen1 / autobasis / pddbtest / ci / deterministic / ...

# NEW Stage 13b-3 candidate:
dev-mount = ["gen1"]    # rv32 + bypasses for automation
```

```rust
// services/pddb/src/backend/hw.rs

const DEV_MOUNT_PASSWORD: &str = "xas-dev-mount";
const DEV_MOUNT_BASIS_NAME: &str = "sys.basis";  // PDDB_DEFAULT_SYSTEM_BASIS

fn syskey_ensure(&mut self) {
    #[cfg(feature = "dev-mount")]
    {
        // Skip the modal-driven password loop. If the SCD is blank,
        // bypass `pddb_format`'s rootkeys.is_initialized() check
        // and format with a synthesized password. If the SCD has
        // been written before, attempt login with the dev password.
        match self.try_login_dev() {
            PasswordState::Correct => return,
            PasswordState::Uninit => {
                self.pddb_format_dev().expect("dev-mount format failed");
                let _ = self.try_login_dev();
                return;
            }
            PasswordState::Incorrect => {
                panic!("dev-mount: stored PDDB has different password \
                       than DEV_MOUNT_PASSWORD; flash backing must be wiped");
            }
        }
    }

    #[cfg(all(feature = "gen1", not(feature = "dev-mount")))]
    while self.try_login() != PasswordState::Correct {
        // ... existing modal loop ...
    }
}

#[cfg(feature = "dev-mount")]
fn try_login_dev(&mut self) -> PasswordState {
    // Skip rootkeys; derive AES key directly via bcrypt(DEV_MOUNT_PASSWORD,
    // salt-from-SCD). Then attempt the standard pt-key + data-key
    // recovery from the wrapped form in SCD. Return Correct/Incorrect/Uninit.
    todo!("see services/pddb/src/backend/hw.rs::try_login for the
           non-dev-mount equivalent; copy + remove rootkeys calls")
}

#[cfg(feature = "dev-mount")]
fn pddb_format_dev(&mut self) -> Result<()> {
    // Skip rootkeys.is_initialized() check.
    // Use bcrypt(DEV_MOUNT_PASSWORD, fixed-salt) instead of
    // rootkeys.aes_kwp_key() for the key wrapping.
    // Otherwise mirror pddb_format's structure exactly.
    todo!("see pddb_format above; replace rootkeys-derived keys with
           dev-mount-derived equivalents")
}
```

Estimated effort: 100–150 LoC of careful crypto-aware code, plus
test fixtures, plus a corresponding patch in `services/root-keys`
for the cases where pddb's other code paths still consult
rootkeys (notably `aes_kwp_key` for basis migration). Easily a
week of focused work to land cleanly without breaking the
non-dev-mount paths.

This is upstream xous-core work. It belongs on the
`tunnell/xous-core/xas` branch (or, ideally, upstreamed to
betrusted-io as a `dev-mount` feature for everyone's CI). Either
way, it's not deliverable inside our standalone workspace.

---

## 4. Recommendation: defer 13b-3 in favor of 13c

PDDB persistence in Renode is **nice-to-have for CI**; it's not
required for any deliverable on the path to a Signal client. The
real Signal client deployment story is:

1. User flashes the rv32 image to Precursor hardware.
2. First boot triggers PDDB initialization: user types a password
   via the GAM modal.
3. PDDB persists across reboots forever after. xas's
   `with_pddb_backend` constructor (Stage 13b-2) just works.

This is the *intended* Precursor ux. Real-hardware first-boot is
the production path, not a test-environment artifact.

For our automated-test story, the more useful next stage is
**Stage 13c — mock HTTP/WS transport**. That unblocks
end-to-end flow testing (link / receive / send) in Renode,
which is the kind of regression test that pays off every commit.
PDDB persistence in Renode isn't on the critical path, so let's
spend the effort where it has higher leverage.

If a future need does push 13b-3 onto the critical path — e.g.
"test session-store recovery across reboots" — the design in §3
is the starting point.

---

## 5. Files touched

```
A  stage/REPORT-13b-3.md             (this file)
M  docs/ROADMAP.md                    (Stage 13 status update)
```

No code changes. The probe-pddb-real test artifacts and IPC client
delivered in Stage 13b-2 remain — they continue to validate the
IPC path against the live (unmounted) PDDB server.

---

## 6. Stage 13 phasing — updated

| sub-phase | status | next? |
|-----------|--------|-------|
| 13a | landed | — |
| 13b | landed (probe) | — |
| 13b-2 | landed (IPC client + real backend) | — |
| **13b-3** | **investigated, deferred** | only revisit if persistence-in-Renode goes onto the critical path |
| 13c | scoped, not started | **recommended next** — independent of 13b track |
| 13d | deferred (u32e backend) | post-MVP perf |
| 13e | scoped | physical hardware deploy; absorbs 13b-3's "real-hardware first-boot" by definition |

The 13b track effectively lands at 13b-2: real KvBackend + IPC
client wired and validated. 13b-3's persistence test bumps into
13e, where manual first-boot init makes the question moot.
