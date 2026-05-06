# RESUME.md — Where the project is, where it's going

A self-contained briefing for picking up xous-app-signal (xas) work
after a break. Read this once before touching anything; it'll save
you the cost of paging in 9 stages of context.

## What we're building

A Signal client for Xous (Precursor hardware) that uses the
Whisperfish Rust stack (`presage` + `libsignal-service-rs` +
`libsignal/protocol`) instead of reimplementing the Signal protocol.
The driving value is **end-user verifiability** — minimize bespoke
code, maximize use of upstream community-maintained libraries with
small reviewable forks.

MVP definition: three hardware-confirmed flows — link as secondary
device, receive one message, send one message.

Core architectural decisions (`docs/REPORT.md`):

- **Decision 1**: persist state in PDDB (Xous's plausibly-deniable
  database) directly, with a per-trait dictionary layout. No
  presage-store-cipher (PDDB already provides AEAD).
- **Decision 2**: replace tokio with smol-rs primitives
  (`async-executor::LocalExecutor`, `async-channel`, `async-lock`,
  `event-listener`, `futures-lite`, `futures-timer`, `async-task`).
- **Decision 3**: replace `reqwest` + `reqwest-websocket` (libsignal-
  service-rs's transport) with sync `rustls 0.22.2` + `tungstenite`
  hosted on an `async-channel`-bridged worker thread.
- **Decision 5**: PDDB write strategy is "accumulate in RAM, flush
  in batches." `SessionStore::store_session` writes to a
  HashMap-backed cache; `flush_sessions` persists.
- **Decision 6**: curve25519-dalek strategy is **vendored
  betrusted-io fork + version bump 4.1.2→4.1.3 + lizard module
  port** from signalapp/curve25519-dalek. Carries the u32e IP-core
  driver for Precursor.

## Repo locations

```
~/precursor-signal/
├── xous-app-signal/                  ← MAIN STANDALONE WORKSPACE
│   ├── Cargo.toml                    workspace root
│   ├── AGENTS.md                     project conventions
│   ├── README.md
│   ├── RESUME.md                     (this file)
│   ├── crates/
│   │   ├── xous-app-signal/          binary "xas"
│   │   ├── xous-signal-bridge/       Manager worker thread + IPC (Stage 8)
│   │   ├── xous-net-bridge/          TLS + sync HTTP + WS pump (Stage 6.1)
│   │   └── presage-store-pddb/       full storage trait surface (Stages 4–5)
│   ├── vendor/
│   │   ├── presage/                  tokio-removed fork (Stage 7)
│   │   ├── libsignal-service-rs/     reqwest-replaced fork (Stage 6)
│   │   └── curve25519-dalek/         betrusted-io fork + lizard port
│   ├── docs/
│   │   ├── REPORT.md                 design rationale + Decisions 1–8
│   │   ├── ROADMAP.md                stage-by-stage plan (0–12)
│   │   ├── CALL_GRAPH.md             per-command call graphs
│   │   ├── INTEGRATION.md            (semi-stale: Stage 9b merge recipe)
│   │   ├── INTEGRATION_STATUS.md     (current: why the merge approach hit
│   │                                  a blocker; three options I/II/III)
│   │   └── SYNC.md                   tunnell/xous-core branch model
│   ├── stage/
│   │   └── REPORT-{0,1,2,3,4,6,7,8,9a}.md
│   └── .cargo/config.toml            rv32 cfg flags
│
├── repos/
│   ├── xous-core/                    GIT clone of tunnell/xous-core
│   │                                 (fork of betrusted-io/xous-core)
│   │   ├── (vanilla on `dev`)
│   │   ├── (mirror of upstream/dev on `dev-for-xas`)
│   │   └── (apps/xas/ subtree only on `xas` branch — see below)
│   ├── libsignal/                    upstream signalapp/libsignal
│   ├── libsignal-service-rs/         upstream whisperfish/...
│   ├── presage/                      upstream whisperfish/...
│   ├── SparsePostQuantumRatchet/     upstream signalapp/spqr (v1.5.1)
│   ├── betrusted-curve25519/         upstream betrusted-io/curve25519-dalek
│   └── (other smol-rs / tungstenite / etc. checkouts)
└── (no separate xous-core-for-xas yet; the user opted not to create
    a second GitHub fork and we use the existing tunnell/xous-core)
```

## tunnell/xous-core branching model (apps/xas/ host)

Set up at Stage 9b checkpoint. Lives entirely on GitHub at
`github.com/tunnell/xous-core`. Three branches matter:

| Branch | Purpose |
|---|---|
| `dev` | Original fork default; not load-bearing for xas. |
| `dev-for-xas` | Tracks `betrusted-io/xous-core/dev` — fast-forward only, never carries xas commits. |
| `xas` | Carries `apps/xas/` subtree. Diff visible via PR `xas → dev-for-xas`. |

**PR**: https://github.com/tunnell/xous-core/pull/24

The `apps/xas/` subtree on the `xas` branch is currently a copy of
the standalone workspace's crates + vendor + docs + stage. **Not
wired into xous-core's build.** xous-core's top-level Cargo.toml,
`.cargo/config.toml`, and `services/{root-keys,shellchat}/Cargo.toml`
are unchanged from `dev-for-xas`. See
`apps/xas/docs/INTEGRATION_STATUS.md` for why.

`docs/SYNC.md` documents the branch-sync recipes (fast-forward
`dev-for-xas` from upstream, rebase `xas` onto fresh `dev-for-xas`).

## Stages: progress

All in the standalone workspace
(`/home/tunnell/precursor-signal/xous-app-signal/`):

| Stage | Status | What landed |
|---|---|---|
| 0 | ✓ | Workspace scaffold, [patch.crates-io] mirroring |
| 1 | ✓ | smol-rs LocalExecutor smoke test |
| 2 | ✓ | TLS smoke (rustls 0.22.2 + xous-net-bridge::tls_connect) |
| 3 | ✓ | Signal WS smoke (tungstenite 0.21 + pinned CA) |
| 4 | ✓ | StateStore + ContentsStore::profile (hosted-mode mock backend) |
| 5 | ✓ | All 9 storage traits + Store blanket (22 unit tests) |
| 6 | ✓ | libsignal-service-rs transport fork (sync HttpClient + WS pump) |
| 6.1 | ✓ | reqwest cleanup, getrandom 0.2/0.3 split, u32e_backend gate |
| 7 | ✓ | presage tokio-removal patch |
| 8 | ✓ | Manager worker thread + IPC (`xas: pong` + `whoami err` round-trip) |
| 9a | ✓ | getrandom 0.3 custom extern, PDDB feature flag, INTEGRATION.md |
| 9b | ⚠ | Workspace-merge attempt blocked; checkpoint reached |
| 10 | pending | Link as secondary device |
| 11 | pending | Receive one message |
| 12 | pending | Send one message |

Stage 9b discovered an architectural blocker: xous-core's
`[patch.crates-io].aes` redirects the `aes` crate to its IPC-shim
`services/aes`, which doesn't expose `Aes256Enc` that libsignal's
`zkgroup::profile_key` calls directly. There's no per-subtree
patch mechanism in cargo, so we can't use xous-core's aes patch
inside xous-core services AND bypass it for libsignal.

Two related conflicts: curve25519-dalek 4.1.2 (xous-core) vs 4.1.3
(zkgroup), and a getrandom 0.2/0.3 cycle through `imports/getrandom
→ rkyv → uuid`.

## The open architectural question

`docs/INTEGRATION_STATUS.md` lays out three options:

- **Option I** — separate workspace, custom xtask bundling. xas
  stays at `~/precursor-signal/xous-app-signal/`; a custom xtask
  builds the rv32 binary and injects it into xous-core's image
  pipeline. `apps/xas/` in tunnell/xous-core is a reference
  subtree, not a build target.
- **Option II** — make `services/aes` API-compatible with upstream
  `aes` 0.8.x. Add `Aes256Enc`, `Aes256Dec`, etc. wrappers to
  `xous-core/services/aes/src/lib.rs` so zkgroup's calls resolve.
  Plus the curve25519-dalek bump and getrandom 0.3 work from the
  9b attempt. xous-core surgery; lets us use the merge approach.
- **Option III** — per-package patch overrides via cargo
  source-replacement. Tooling-fragile, undocumented, fragile across
  cargo versions.

The user has stated a preference for **Option II** as long as we
align with `https://cryptography.rs/`'s curated crate
recommendations.

## The cryptography.rs / libsignal v0.91 reframing

A user-supplied analysis memo (full text in chat history; key
points captured here) reframes the project given that libsignal
v0.90+ migrated from `pqcrypto-kyber` (C FFI) to `libcrux-ml-kem`
(pure Rust). This is the change that makes the Xous port newly
tractable.

Key memo claims, verified against our checkout:

1. **Our libsignal tag is v0.91.0** (one minor newer than the memo's
   v0.90.x). All workspace crypto deps in libsignal v0.91 are
   pure-Rust: `aes 0.8`, `aes-gcm-siv 0.11`, `ctr 0.9`, `hkdf 0.12`,
   `hmac 0.12`, `sha2 0.10`, `subtle 2.6`, `libcrux-ml-kem 0.0.8`,
   `spqr` v1.5.1.
2. **C/C++ surface in libsignal** is confined to `rust/net*`,
   `rust/attest`, `rust/keytrans`, `rust/svrb` (all
   `tokio-boring-signal` / `boring-sys`). None of these are reached
   by the protocol path we use through libsignal-service-rs.
3. **`rust/protocol`, `rust/crypto`, `rust/account-keys`,
   `rust/poksho`, `rust/zkcredential`, `rust/zkgroup`,
   `rust/usernames`** all compile pure-Rust. No surgery required
   for crypto.
4. **The forced patch site** the memo flags is *one* defensive cfg
   guard in `rust/crypto/src/lib.rs` lines 6-7 (the
   `feature(stdsimd)` / `feature(aarch64_target_feature)` gates
   should be tightened so they never fire on rv32). Plus possibly
   one feature-flag adjustment in `rust/protocol/Cargo.toml` to
   pull only the strictly-required libcrux-ml-kem features.

cryptography.rs catalog (verified by fetch 2026-05-06):

- AEADs: `aes-gcm`, `aes-gcm-siv`, `chacha20poly1305`, etc.
- Block ciphers: `aes`, `des`. (No `Aes256Enc`-style entries — the
  curated page covers crate-level recommendations, not API
  specifics.)
- Hashes: `BLAKE2`, `BLAKE3`, `SHA-2`, `SHA-3`. SHA-1 not headline.
- KDFs: `argon2`, `HKDF`, `pbkdf2`, `scrypt`.
- Asymmetric: `curve25519-dalek`, `ed25519-dalek`, `x25519-dalek`,
  `k256`, `p256`, RSA, `ecdsa`.
- Post-quantum: **`ml-kem` (RustCrypto)** is listed; **`libcrux-ml-kem`
  is NOT**; FFI `pqcrypto`/`oqs` are listed but flagged as FFI.
- TLS: `rustls`, `webpki`.
- RNG: `rand`, `getrandom`.
- Defensive: `subtle`, `zeroize`.

**Tradeoff**: libsignal chose `libcrux-ml-kem` (formally verified
in F\*); cryptography.rs lists `ml-kem` (RustCrypto, FIPS-203
compliant). Both are pure-Rust on rv32. Sticking with libsignal's
`libcrux-ml-kem` minimizes libsignal divergence (a project value);
switching to `ml-kem` is a stronger cryptography.rs-alignment but
an extra libsignal patch. Recommendation: stick with libsignal's
choice unless rv32 build verification (Stage 6.5) finds otherwise.

## What changes given this reframing

The memo's architectural recommendations *largely confirm* what we've
been doing. The **one new thing** is an explicit Stage 6.5 — verify
that libsignal-protocol + zkgroup + signal-crypto + spqr build
green for rv32-xous, with the minimum patch series applied.

**The memo does NOT recommend dropping presage or libsignal-service-rs.**
Re-reading the memo: it says presage and libsignal-service-rs
already do the right thing (consume libsignal-protocol directly +
bring their own HTTP/WebSocket layer). Our forks of those
(replacing reqwest+tokio with rustls+tungstenite+smol) are the
correct pattern.

## Recommended next move

After the architectural-question discussion the user asked for:

1. **Adopt Option I** (separate workspace + xtask bundling) over
   Option II. Reason: Option II solves a problem we created by
   trying to merge into xous-core's tree; the merge isn't
   necessary if we keep our standalone workspace and bundle the
   binary via xtask. Option II would also require modifying
   xous-core's `services/aes` (xous-core surgery) to add upstream
   API-compatible wrappers — non-trivial and not aligned with
   "minimize divergence." The cryptography.rs reference doesn't
   change this analysis.
2. **Add Stage 6.5** (rv32 verification of libsignal's
   protocol/crypto/zkgroup crates). This is independent of the
   integration choice; it should land regardless.
3. **Update ROADMAP.md** to reflect both — Stage 6.5 inserted
   after Stage 6.1; Stage 9 split into 9a (workspace-internal,
   done) and 9b (xtask bundling instead of workspace merge).
4. **Stage 9b becomes**: write `xtask` crate that:
   - Builds the xas binary for rv32-release.
   - Either copies the ELF into `~/precursor-signal/repos/xous-core/apps/xas/`
     as a "binary-only" app (xous-core has prior art for this in
     `apps/app-loader`), OR registers it via the Renode boot script
     so the boot test loads our binary directly.
   - Wires Renode tests for the Stage 8 `xas: pong` smoke output.

Stages 10/11/12 (link, receive, send) follow once Stage 9b lands a
green Renode boot.

## Files to read before resuming

In priority order:

1. `RESUME.md` (this file).
2. `docs/REPORT.md` — the design Decisions 1–8.
3. `docs/ROADMAP.md` — current stage plan (will be updated as part
   of resume work).
4. `docs/INTEGRATION_STATUS.md` — Stage 9b checkpoint findings.
5. `stage/REPORT-9a.md` — most recent stage report.
6. `docs/SYNC.md` — branch model on tunnell/xous-core.
7. `docs/CALL_GRAPH.md` — per-command call graphs (relevant for
   Stages 10–12).

## How to verify the workspace is healthy after a break

```sh
cd ~/precursor-signal/xous-app-signal
cargo run -p xous-app-signal --bin xas
# Expected output ends with "xas: exiting"

cargo test -p presage-store-pddb
# Expected: 22 passed

cargo test -p xous-signal-bridge
# Expected: 3 passed

cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
# Expected: passes (Stage 9a verification)

cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean

cargo fmt --all -- --check
# Expected: clean
```

If any of these fail, something's drifted. Don't push forward
until they're green.

## Git state

Standalone workspace (`~/precursor-signal/xous-app-signal/`):
- HEAD: `Generalise AGENTS.md to drop tool-specific naming` or
  `Stage 9a: workspace-internal prep for the xous-core fork` — check
  with `git log --oneline -5`.
- `master` is the only branch; no remote.
- Every commit authored by `tunnell <2406627+tunnell@users.noreply.github.com>`.
  `xas <local@xas>` was rewritten out via `git filter-branch` after
  the user's email-anonymization request.

`tunnell/xous-core` (`~/precursor-signal/repos/xous-core/`):
- `dev`, `dev-for-xas`, `xas` branches.
- PR #24 (xas → dev-for-xas) is open and shows our diff.
- Origin = `tunnell/xous-core`; upstream = `betrusted-io/xous-core`.

## Open questions for the user

1. Confirm Option I (separate workspace + xtask bundling) is
   acceptable in light of the cryptography.rs / memo reframing,
   given that the memo's analysis confirms our existing
   architecture.
2. Whether to use libsignal's `libcrux-ml-kem` (stays on the
   upstream choice) or switch to `ml-kem` (RustCrypto;
   cryptography.rs-listed; larger libsignal patch). Recommendation:
   stay with `libcrux-ml-kem`; revisit if Stage 6.5 finds rv32
   build issues.
3. UI surface for Stage 10 (link-as-secondary): plain TTY menu
   inside xas, or integration with xous-core's `gam`/`menu`
   pattern (sigchat/mtxchat-style). User decision; gating Stage 10.
