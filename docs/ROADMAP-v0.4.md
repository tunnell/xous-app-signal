# v0.4 roadmap — the trust release

Date: 2026-08-01. Method: multi-agent research (eight evidence-cited
reports), a three-lens proposal panel, an adversarial critique of the
panel, and four de-risk spikes, all run against dev `fcbc016` /
xas-integration `c4c3fd625` / the xas-v0.3 fork pins. The evidence base
lives in `ignore/research-v0.4/` (untracked); the spike branches are
local: `spike/rebase-{f449930,4c671ea47,pins}`, `spike/group-guard`,
`spike/expiry-bindings`, `spike/parity-cron`. Disclosure per README
policy: drafted with AI-agent assistance; claims carry citations a
reviewer can check (file:line refs are at the revs above).

**Theme.** v0.4 is the trust release: xas stops silently misdelivering,
leaking, retaining, or corrupting anything it touches, and every
remaining failure is loud. Feature work that does not serve that
(history, group UI, pocket UX) waits for v0.5 or for its gating
measurement.

**Hardware budget.** Two flash sessions, batched into the two flash sessions below.
Everything else validates hosted first. One-session fallback included.

## Sequencing spine

```
0. branch protection on dev (before any churn merges; absent today on
   dev AND main, verified 2026-08-01 — the v0.3 tag was cut on red CI)
1. promote the spike-banked S items: parity cron, group guard
2. coupled fork rebase (early; owns the worker cmd.rs Event-churn window)
3. prekey re-upload on the rebased base (#462-shaped naming)
4. expiry enforcement + real wipe (the L anchor; half banked)
5. log-leak kill · delivery receipts · susres hook + clock preflight ·
   status honesty
6. flash session 1 (account/link) mid-cycle · session 2 (RC)
```

Cut order if the cycle runs short: item 8's non-rider parts, then item 6,
then item 5's PII-redaction half (keep the URL gate). Items 2–4 and the
URL gate do not get cut.

## The slate

### 1. Coupled fork rebase: ls-rs `f449930` + presage `4c671ea47` (M code, M verification)

Spike-proven CLEAN: 5 conflict hunks across both forks (one semantic,
~10 lines), hosted suites identical to baseline, rv32 ELF builds at
libsignal 0.94.4. Delivers free: the dc0720a 409-body capture (today an
unexpected 409 is mis-parsed as MismatchedDevices with the body
discarded), capability parity with Signal-Server, AEP linking. Drops the
hand-ported capability patch `3e17acde3`; keeps `dfabe6b06`
(close-code accessor — spike verified it is NOT subsumed upstream).
Riders that land with it: align `DeviceCapabilities::default()` with
`LinkCapabilities` (today every stream open re-announces
usernameChangeSyncMessage=false), and persist the provisioned master key
/ stop answering KEYS sync requests with a fabricated one.
xas-side adaptation is banked on `spike/rebase-pins`: prost 0.14,
Metadata timestamp DateTime migration (on-disk format preserved), three
new Store methods (AEP implemented for real — the new linking path has
no fallback), DecryptionError arm. Kyber ext stubs stay dead at the new
base (zero call sites — checked; keep checking on every future bump).
Preconditions: protoc ≥3.15 (system 3.12.4 fails on proto3 `optional`),
and pin the toolchain (`rust-toolchain.toml` channel=stable orphans the
rv32 sysroot on every stable bump).
Hardware: full link/receive/send regression — session 1. Risk: 0.94.4
runtime behavior on device and live AEP provisioning are compile-proven
only.
Files: fork branches + tags, Cargo.toml pins, docs/FORKS.md.

### 2. Prekey-storm closure cluster (M)

The only observed device-killing failure (storm: ids 2→177+, ~6 MB
pinned kyber, watchdog reset), attacked at all three points:
- **2a recovery** — idempotent orphaned-batch re-upload on the rebased
  ls-rs base; store methods named after upstream #462's surface so the
  eventual convergence is a rename, not a rewrite. Open question that
  needs a live test: does the server accept an identical re-PUT batch.
- **2b seed** — pre-link stale-store guard: is_registered/protocol-dict
  check → prompt routing through the existing Wipe path. Never a wipe
  inside handle_link_device (`fa1c37b`'s lesson: a link-time wipe hung
  the device).
- **2c trigger** — weekly capability-parity CI check, DONE on
  `spike/parity-cron` (live PASS 6/6; would have caught the 2026-07
  link 409 before hardware did). Registers only from the default branch.
- **2d process** — branch protection + actions/checkout Node-20 bump.
Maintainer decision needed: validation account for 2a — a staging
registration, or the live account with a stated risk plan (see
Open questions, below).
Hardware: session 1 (account/link).

### 3. Group misfile guard (S — banked on `spike/group-guard`)

Inbound GV2 texts currently file under the sender's 1:1 thread; a reply
delivers a private DM to that one member. The spike ships: worker
extracts group_v2.master_key into Event::Message (sync-sent transcripts
covered both directions, tested), UI files group traffic under a
`Uuid::new_v5(master_key)` pseudo-thread with a `[group]` label, and
compose refuses group threads with an explanatory line — tag, never a
silent drop. Full ThreadKey typing stays out (rides the v0.5 gam_app
split). Rebase the spike commit onto the post-rebase dev.
Rider: attachment placeholder row — the empty-body-with-pointer skip
sits in the same worker filter region; a `[attachment — view on phone]`
stub ends the silent drop of attachment-only messages.
Hardware: none (group smoke rides session 1).

### 4. Expiry enforcement + real wipe (L — half banked on `spike/expiry-bindings`)

The worst obligation gap: presage persists every message to plaintext
PDDB dicts and nothing anywhere deletes expired disappearing messages;
`clear_messages` is a no-op and `Store::clear` leaves `signal.threads.*`
plus sticker packs in flash — "Wipe" over-promises today. Also the gate
for v0.5's history headline.
Banked by the spike: DictBulkDelete=55 + list_dicts bindings against the
shipping kernel (zero kernel changes; list_dicts honors the server's
ascending-index token latch), chunked `KvBackend::delete_keys`
(223 keys/IPC), a real `clear_messages`, and a receive-time-anchored
sweep that doubles as the boot retro-sweep via serde-default
StoredMessage fields (v0.3-shape compatible).
Remaining (M): worker Cmd/Event wiring, boot/idle scheduling next to the
existing pending-send sweeper, route `Store::clear` through chunked
deletes, evict expired rows from the hydrated UI store, wire into
logout/wipe, `flush_space_update` after bulk churn.
Semantics: receive-anchored expiry over-deletes and never over-retains —
privacy-conservative, and with no read path in v0.4 the over-deletion is
invisible; re-examine the anchor when history ships. Note: Contact
already persists expire_timer(+version), so the send-side timer source
exists.
Hardware: chunked-delete flash wall-time vs one dict_remove — session 2;
ship with conservative pacing if unmeasured.

### 5. Log-leak kill: URL gate + PII redaction (M)

URL gate: the provisioning URL is info-logged at BOTH worker lib.rs:619 and
gam_app.rs:1549; UART access during the pairing window pairs an
attacker's device. Gate both sites and both test_link_qr.sh greps
(:184, :187) behind a default-off `link-uri-uart` feature; default build
emits length only. PII redaction: redact ACI/e164/device-name per the code-map
inventory (author labels at gam_app.rs:1606, PDDB perf lines keyed by
peer ACI, worker :1731 e164) behind a `verbose-pii` feature. The log
lines are a de-facto grep contract for the hosted tests — update the
greps in the same commits.
Hardware: none (UART capture review rides session 1 free).

### 6. Delivery receipts, outbound (M)

No layer sends DELIVERY receipts, so senders see permanently-undelivered
exactly when the phone is off — this device's core scenario. Zero fork
changes needed (ContentBody::ReceiptMessage). The real design work,
named: the receive stream holds `&mut manager`, so emission must queue
through the op channel (or send in-stream), and receipts must batch
across a reconnect-drain burst — a phone-off night delivers N queued
messages whose N receipts must not serialize through the send retry
loop.
Hardware: none — hosted links to the real server, so the phone-off
delivered-checkmark test is hosted-reproducible; device ride-along only.

### 7. Susres resume hook + clock-sanity preflight (S + S)

Hook: xas registers no susres handler today; suspend kills every
TCP/TLS/WS session while RAM survives, and ticktimer freezes across
suspend so no timer can notice — messages silently stop after every
sleep. Pre-suspend flush; post-resume signal the worker to tear down and
reconnect (one reconnect owner, shared with the existing 4409/1001
throttling).
Preflight: `LocalTime::get_local_time_ms() == None` detects the exact
unset-RTC state that broke all TLS in v0.3 with a cryptic error; show a
guided set-the-clock screen mirroring the no-internet preflight.
Hardware: susres check ~5 min in session 2; preflight is hosted.

### 8. Status honesty + hygiene riders (M)

Busy/status line during the 10.7 s send and multi-minute link (the
chat-lib busy-bumper norm); Event::LinkState + a row-0 connection-down
glyph — one row-0 layout decision owns glyph + unread badge before
either lands; focus-gated rendering (xas currently posts frames while
backgrounded); alloc_hint at the store-open sites; `n`/`u` triage keys;
terminal-close copy (after a terminal 4409/4401, say "relaunch by
<date> or this device unlinks" — the 30–45-day server window is real);
WriteKeyBatch client-opcode deletion (op 57 never shipped in any kernel;
every device send pays a doomed IPC — server patch archived at
`archive/writekeybatch-server-8f3894f2d`); doc truth-sync (UI.md, this
file's per-section statuses in REFACTOR-PROPOSALS, the false handle_send batch
claim, the stack eager-commit comment, the stale aes comment,
tests/README tier honesty: renode cannot catch server drift, hosted
cannot catch RTC or flash-latency bugs).
Hardware: glyph honesty under real wifi churn is device-only — observe
in session 1.

## Pocket mode: measure first

No LED exists (verified against the full llio surface); suspend-based
notification has no physics (the EC never powers the SoC on packet rx).
The honest tier is always-awake pocket mode — and its go/no-go number
does not exist yet: published bounds are only 23–46 mA floor to
150 mA (~7 h). Session 2 takes the gas-gauge awake-idle measurement; the
unread badge ships in item 8; the Pocket/Desk toggle, vibe cadence, and
soak are GATED on the measured number. F2 stays contact sync — it is the
only sync trigger (gam_app.rs:1386); a future pocket toggle finds
another home.

## Not in v0.4

- **History read path** — the v0.5 headline. XL once honestly counted
  (list_dict retro-enumeration, scroll/paging UI that the single-TextView
  render model does not have, INBOX_CAPACITY flipping to render-window).
  Its gate (expiry) ships now; the load primitive exists
  (DictBulkRead=52, verified shipping); READ-receipt display moves with
  it.
- **Group send / group UI** — ThreadKey typing waits for the gam_app
  split; fan-out multiplies the send pipeline.
- **Attachment fetch/render** — CDN cert coverage unverified; 16 MiB;
  the placeholder covers the social break.
- **Outbound READ receipts** — needs Configuration-sync fork work;
  not-sending is privacy-conservative.
- **gam_app split (item 1.3) + typed WorkerError (item 1.1)** — the split's
  in-cycle forcing function dissolved when group typing deferred;
  WorkerError re-scopes against the post-rebase error surface. Both v0.5.
- **Logout drain-then-wipe (item 2.2 rework)** — unobserved race; deferred
  behind the rebase settling.
- **Suspend/network-wake notification** — no wake path exists; settled.
- **libs/chat adoption; global aes patch** — feature-frozen upstream, no
  conversation list, path-dep cascade; copy its idioms (busy bar, IME
  gotinput, menu_matic) instead; the aes provenance decision deferred.
- **Baochip investment** — baosec cannot run xas (no network, no
  keyboard, 128×128 OLED, ~10 MiB backing). Free rules only: no new
  fixed-pixel layout constants, prefer ux-api types when touching UI,
  and a per-release `git grep baosor -- xtask` tripwire on upstream.
- **Dedicated xas PDDB basis** — gen1 has no programmatic basis-password
  path; every unlock is a user modal. UX-policy decision, not this cycle.
- **Signed-prekey age rotation** — upstream-shared hygiene; the active
  wound is the orphan storm.
- **Upstream submission batch** — standing cadence (the ~11 clean
  xous-core candidates are inventoried in ignore/research-v0.4/
  fork-drift.md), not release scope.

## Flash sessions

**Session 1 — account/link (mid-cycle, after items 1–3 and 5 land):**
link on the rebased pins (AEP flow) → stale-store re-link prompt → wipe
→ re-link → capability record spot-check in logs → forced replenish +
orphaned-batch re-upload validation → inbound group-message smoke →
UART capture review (no URL, no PII) → busy line on a real send +
connection glyph during a wifi drop.

**Session 2 — steady-state / RC:** expiry sweep grind + chunked-delete
flash timing → susres suspend/resume/reconnect (~5 min) → gas-gauge
awake-idle measurement → triage/UI feel pass → full v0.3 regression
(link, receive, send < 60 s) → tag v0.4 on green, protected CI.

**One-session fallback:** run session 1 plus the 5-minute susres check;
ship expiry with conservative pacing and carry the flash-cost number as
known-unmeasured.

## Open questions (maintainer)

1. Re-PUT validation account (item 2a): does a Signal staging
   registration exist for xas, or do we validate against the live
   account with an explicit risk plan? Blocks enabling the re-upload
   path, not building it.
2. Push the two safety branches somewhere durable:
   `archive/writekeybatch-server-8f3894f2d` (xous-core, local-only) and
   the spike branches worth keeping.
3. The v0.3 hardware friction log (`ignore/xas/friction-log.md`,
   other machine) — fold its doc-gap entries into item 8's truth-sync
   when available.

## Verification gates

Every structural commit keeps the established gate: warning-set diff,
test-list diff, hosted suites (39 unit + net-bridge + store), rv32
release build, nightly fmt. Fork/transport changes additionally need
session-1 evidence — hosted PASS is a sanity check, not a ship gate.
