# Refactor proposals — 2026-07 structural review

Date: 2026-07-06. Successor to the 2026-05-09 post-MVP review
(`docs/REFACTOR-PROPOSALS.md` at `f5f046b`, since acted on and
deleted — P1/P2/P3/P5/P7/P9 landed; P4/P6/P8 fold into items below).

Method: an AI-assisted multi-agent review — parallel deep-reads of
all six first-party crates, the issue tracker (26 issues / 13 PRs),
betrusted-io/xous-core PRs and app conventions (mtxchat, libs/chat,
sigchat), and the xous-book — followed by adversarial verification
of every proposal against the code at `refactor/worker-op-channel`
HEAD `d55281d`. Verdicts: 4 confirmed, 8 confirmed-with-corrections,
0 refuted. Disclosure per README's contribution policy: drafted with
AI agents; every claim carries a file:line cite a reviewer can check.

Facts that changed the plan (all verified against GitHub 2026-07-06):

1. xous-core#877 **merged** 2026-06-02 (`2005a801c`);
   rust-lang/rust#156414 **merged**; ls-rs#431 still an open draft;
   xous-core#880 (reaper fix) still open. Docs synced in this branch.
2. **betrusted-io/sigchat is the maintainer-blessed precedent**: chat
   apps live out-of-tree (evicted from xous-core in `d8542dab7`);
   accepted upstream footprint is a one-line `apps/manifest.json`
   entry. Out-of-tree is the design, not a compromise.
3. **xous-core `libs/chat` exists explicitly so protocol apps don't
   rebuild chat UI** (mtxchat uses it). Blocker found: `chat → pddb →
   services/aes` (hardware fork) collides with libsignal's crates.io
   `aes` unless the workspace patches `aes` globally — a
   crypto-provenance decision. Gate any adoption spike on that first.
4. **Transport direction is settled** (issue #1 thread): per-send
   fresh WS self-induces 4409 displacement; the north star is one
   long-lived identified WS with typed close-code handling. The old
   PLAN.md's Stages 1a/1' are retired (plan deleted 2026-07; history
   in issue #1 and git).
5. Issue #37 is the maintainer's v1 backlog; PR #38 (worker
   op-channel, in flight) is the ownership template. Items below
   slot into that frame.

---

## Wave 0 — housekeeping (S; shipped on this branch)

- **0.1 Delete `xous-modals-ipc` + the dead auto-link probe.**
  Superseded the day it was created by `gam_app::drive_link` over
  the upstream modals client; cfg'd out of every real build since.
- **0.2 Prune never-compiled vendored members** (presage-cli,
  presage-store-sled, presage-store-sqlite, ed25519-dalek,
  x25519-dalek; ~5.5k+ LoC). Requires the vendored-manifest edits
  documented in the commit; diffs + vendor/README.md regenerated,
  curve25519-dalek baseline pinned (`16e087a`).
- **0.3 Doc truth-sync** (README upstream-patches, status block on
  the since-deleted PLAN.md, Cargo.toml comments, ARCHITECTURE.md
  §12 invariants).
- **0.4 rustfmt.toml + CI** (fmt soft-gate until a repo-wide
  `cargo fmt` lands post-PR#38; hosted tests + rv32 build as hard
  gates; CI clones the xous-core sibling per BUILDING.md §1).

## Wave 1 — structural core

### 1.1 Typed `WorkerError`, classified once at the failure site (M)
Errors are string-matched today (`xous-signal-worker/src/lib.rs`
`contains("websocket closing")` and friends; two live sites) and
distinct failures collapse into the same `Event` variants. One
`WorkerError` enum + a single classifier at the presage boundary;
map to `Event` variants in exactly one place. Zero new deps
(hand-written Display keeps proc-macros out of the trusted graph).
Tracker: #37 item 10. Prerequisite for Wave 2's retry work.

### 1.2 Extract a `MessageStore` mutation funnel in the UI (M)
`gam_app.rs` mutates message state from ~15 scattered sites (three
hand-rolled INBOX_CAPACITY evictions, duplicated send-path writes, a
triplicated unlink wipe). Funnel every mutation through one pure-data
type (`dialogue.rs` is already the designated schema). Correction
from verification: `reset_to_unlinked` must live on `App` (it also
clears identity/compose/cursor fields) and call `store.clear()`.
This turns PDDB persistence (#2) from a 15-site diff into a
1-type diff.

### 1.3 Split gam_app.rs (~2.1k lines) into a module tree (M)
Three parallel god-matches over the same `Screen` enum (keys,
render, events) plus IPC setup, forwarder thread, modal flows, and
helpers in one file. The target pattern is the per-screen structs
with `handle_key → Transition` dispatch and a single screen→Cmd
binding point (`on_screen_entered`) that the removed `stdin_ui`
module used (deleted 2026-07 as a drifted duplicate; pattern
recoverable from git history).
Port to: `api.rs` (XasOp), `screens/<name>.rs`, `store.rs` (1.2).
Retire the forwarder deque (`Mutex<VecDeque<Event>>` + scalar poke)
for the book-blessed send-to-own-SID idiom. Fold in two hygiene
fixes: private `xous::create_server()` SID instead of registering
`_xas_` in xous-names with unlimited connections, and IME
(`gotinput_id`, UxType::Chat) for compose instead of raw-key
parsing. Known limit: this does NOT fix modal blocking
(`show_notification` parks the GAM loop until dismissal) — that is
its own follow-up.

## Wave 2 — transport + ownership (aligned with the settled north star)

### 2.1 Give ws_pump a lifecycle (M)
(a) split `open()` from `spawn_pump()` (handshake is fused into the
setup-thread closure); (b) add a `WsStats` handle — `opened_at`,
`last_rx`, `close_code`, shutdown flag; **32-bit atomics only**
(rv32imac has no 64-bit atomics, as `tls.rs` documents); (c) fix the
leak: the reader's idle-timeout branch never checks
`tx.is_closed()`, so a discarded pump keeps 3 threads + a socket
alive until the server closes — on a 16 MiB machine. Typed
close-codes out of the pump feed 1.1's enum (1000/1001 vs 4409).

### 2.2 Finish Manager single-ownership; Logout drain-then-wipe (M)
Residue after PR #38: `Cmd::Logout` still bypasses the op channel
(`manager_ops=None` path). Route it through as drain-then-wipe.
Correction: read-only ops must reply from task-local cached state —
the receive stream holds `&mut manager`, so querying the Manager
forces a WS teardown/reopen. Cross-refs #9, #37 item 4.

### 2.3 Fold the 62s send retry loop into manager_task (L)
Park in-flight sends in task-local state next to
`pending_unconfirmed_sends`; let stream-reopen own retry, with an
explicit retry timer (a healthy reopened stream yields no loop
iteration until the next rotation, so timer-driven, not
iteration-driven). Keep the old plan's Stage 1b distinction: cipher-sent vs
save_message failure are different outcomes and the Event vocabulary
(1.1) should say so. Depends on 1.1 + 2.1.

## Wave 3 — features + platform alignment

### 3.1 PDDB message persistence — the read path (#2) (L)
The store already persists per-thread dicts; missing: a
thread-descriptor index (Home can't enumerate threads),
`Cmd::LoadThread`/`Cmd::ListThreads` + `Event::ThreadHistory`, a
per-thread last-read key, and INBOX_CAPACITY semantics flipping from
truth-eviction to render-window paging. Correction: the
`WriteKeyBatch` server branch cited in xous-pddb-ipc doc comments
does not exist on any remote — treat batching as a capability probe
with chunked fallback, and fix or drop those comments. New dicts
follow `<app>.<purpose>` naming (`xas.state`, ...), pass
`alloc_hint`, register `key_changed_cb`.

### 3.2 Harden the out-of-tree posture — the sigchat shape (M)
Drop the stale `apps/xas/` snapshot from the kernel-side fork (the
documented build injects the out-of-tree ELF via the `xas:<path>`
cratespec); re-pin getrandom as a git-rev dep on
betrusted-io/xous-core (≥ `2005a801c`); upstream a one-line
`apps/manifest.json` entry to betrusted-io (sigchat precedent; DCO +
`Assisted-by:` + one concern per PR). Keep linking
`gam::APP_NAME_XAS` — sigchat does the same.

### 3.3 One conversation UI (L–XL, decision spike)
Order: (1) resolve the `aes` patch-contamination question; (2) only
if it clears, spike libs/chat for the Thread surface. Likely
outcome: it doesn't clear — then port gam_app to the Transition
pattern (1.3). (The original "keep stdin_ui as the host-side
harness" option is gone: stdin_ui was deleted 2026-07; unit
coverage moved to `store.rs` + hosted-Xous emulation.)

### 3.4 Vendor-fork bookkeeping (S–M; partially shipped in Wave 0)
Remaining: reproducible diff labels (mtimes still embed clone
times), and re-align the vendored keepalive patch to the builder
shape when ls-rs#431 moves. (May review's P6 "revisit in 6 months"
falls due ~2026-11.)

---

## 4. Security & hardening backlog (absorbed from issue #37)

The v1 security/hardening umbrella (issue #37, closed 2026-07-25 when
roadmap tracking moved from the issue tracker into this document)
carried ten prioritized items from the post-v0.2 deep-read. Three
exited earlier: item 9 shipped as SECURITY.md (PR #51), item 10 is
§1.1 (typed WorkerError), item 4 is §2.2 (logout drain-then-wipe).
The seven residual items, ranked active leak > latent leak >
defense-in-depth > audit-readability (full rationale in the #37
thread):

### 4.1 [sec] Drop the provisioning URL from info-level logs (S)
Active leak: the `sgnl://...` link URL is info-logged for the ~60 s
pairing window; UART access lets an attacker pair their own device.
Must land together with the closed #30's dev-ux ask: a default-off
feature gate (e.g. `link-uri-uart`) that preserves the
signal-cli-pairing pipeline and the test_link_qr.sh gate lines, with
the default build emitting length only.

### 4.2 [sec] Redact ACI / phone / device-name from info logs (M)
Active leak: per-receive lines emit UUIDs, e164 numbers, and device
names; one boot capture reconstructs the contact graph. Workspace
redaction helper (last-4 / hashed-prefix), `verbose-pii` feature for
ops triage. Sites: worker load_registered / handle_send /
process_received, PddbBackend::get key logging, BufferingBackend
perf-event strings.

### 4.3 [sec] SecretBox + ZeroizeOnDrop for message bodies and key bytes (M)
Latent leak: decrypted text and 32-byte key buffers live as bare
String/Vec across Event::Message, InnerSend, the UI message store,
profile-key fetches, and registration intermediates. Wrap in
`secrecy::SecretBox`, redacting Debug impls, `.expose_secret()` at
read sites.

### 4.4 [sec] TLS session tickets into the type system (M)
`CountingResumptionStore` holds bearer-token-class ticket bytes
without Zeroize or a witness type; nothing type-blocks a future
"persist tickets to PDDB" patch. SecretBox the inner blob and add
the SECURITY.md review gate for any persistence change.

### 4.5 [sec] Take libsignal's send path off catch_unwind (M local / L upstream)
A panic mid-send can leave session state inconsistent while the
worker keeps accepting sends. Either audit panic-able operations
upstream or tear down manager_task on caught panic and force
re-load_registered.

### 4.6 [sec/maint] Witness types for trust transitions (M per boundary)
TLS-verified / session-established / MAC-checked / durability
transitions are all bare Result today. Zero-sized witness types,
one boundary per release.

### 4.7 [sec] Tier-A/B lint headers across the security-sensitive crates (S + L)
`#![forbid(unsafe_code)]`, `deny(clippy::unwrap_used)`,
`deny(clippy::indexing_slicing)` etc. per the rustls/RustCrypto
convention; the long tail is refactoring the surfaced panic sites
to Result.

---

## Sequencing

```
Wave 0: shipped on this branch
Wave 1: 1.1 typed WorkerError → 1.2 MessageStore funnel → 1.3 gam_app split
Wave 2: 2.1 ws_pump lifecycle → 2.2 logout/ownership → 2.3 retry fold-in
Wave 3: 3.1 persistence (needs 1.2) · 3.2 fork-delta shrink ·
        3.3 UI spike (gate: aes provenance) · 3.4 vendor bookkeeping
Wave 4: security backlog §4 (4.1/4.2 first: active leaks; 4.1
        pairs with the closed #30 feature gate)
```

Every structure-only commit keeps the PR #34 verification gate:
warning-set diff, `cargo test -- --list` diff, hosted 53/53,
net-bridge + store suites, rv32 release build. Log lines are a
de-facto public API until #12 lands — enumerate the grepped contract
before renaming any log prefix. Transport changes additionally need
Renode or hardware evidence per AGENTS.md (hosted PASS is a sanity
check, not a ship gate).
