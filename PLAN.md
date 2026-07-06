# PLAN: xous-app-signal#1 — usable send latency

Working plan for resolving
[tunnell/xous-app-signal#1](https://github.com/tunnell/xous-app-signal/issues/1).
Self-contained so a fresh agent on a different machine can pick it up.
**Phase 4 stages must be implemented one commit at a time and approved
between commits.** Do not push anything to the GitHub remote without
explicit maintainer approval — see §11.

This document is the second revision after a fact-checking pass. Lines
flagged with **[verified]** were checked against the repo at HEAD
`b9084ae` on 2026-05-11. Lines flagged with **[hypothesis]** are
unmeasured claims that drive the proposed sequencing and must be tested
by Phase 4a instrumentation before any behavior change ships.

> **Status update (2026-07-06).** Phases 1–4b are complete on `dev`
> (instrumentation + Stage 0 landed in `bf89439`; measurements in
> `BENCH.md`). BENCH.md §2's load-bearing finding: **chat.signal.org
> never issues TLS session tickets**, so Stage 0 is a no-op against
> production — correct code, kept as defense-in-depth, but it cannot
> move send latency. Per §4's decision point the plan pivoted.
> **Stage 1a (per-send fresh WS) and Stage 1' (pre-open on compose
> entry) are retired**: issue #1's later investigation showed a
> second authenticated WS for the same (account, deviceId)
> self-induces 4409 displacement storms; the settled direction is
> one long-lived identified WS with typed close-code handling.
> Remaining candidates from this plan: Stage 1b (separate
> `save_message` failure from send failure) and Stage -1 (defer the
> Failed indicator). Upstream PR status: xous-core#877 **merged**
> 2026-06-02 (`2005a801c`); rust-lang/rust#156414 **merged**;
> whisperfish/libsignal-service-rs#431 still an open draft.

---

## 0. Context you need before starting

### 0.1 Repo state at time of writing (2026-05-11)

- Default branch: `main`. Active development: `dev`. Both currently at
  the same commit (`b9084ae`), tagged `v0.1`. The release is the
  integration baseline you should branch from. **[verified]**
- `xous-app-signal` depends on a forked `xous-core`. Clone the branch
  `xous-app-signal` of [`tunnell/xous-core`](https://github.com/tunnell/xous-core/tree/xous-app-signal),
  not upstream `betrusted-io/xous-core`. The fork carries three patches:
  PR #877's byte-1 net-encoding mirror, a DNS CNAME fix, and a
  hosted-mode PDDB tweak. See `README.md` "Upstream patches" and
  `BUILDING.md` §1 for the full provenance.
- Three upstream PRs are filed but unmerged at time of writing:
  betrusted-io/xous-core#877, whisperfish/libsignal-service-rs#431,
  rust-lang/rust#156414. This work does NOT depend on any of them
  merging.

### 0.2 Build environment — read `BUILDING.md` end-to-end before doing anything

The full build setup is documented at `BUILDING.md`. Two non-obvious
points the new agent will hit:

1. `rust-toolchain.toml` pins channel `stable` (currently 1.95). It is
   **not** nightly.
2. The Xous tier-3 target `riscv32imac-unknown-xous-elf` needs a
   one-time `cargo xtask install-toolkit` from the `xous-core` checkout
   to populate the rv32 sysroot. On rustup ≥ 1.28 this *may* hard-error
   even on hosted-mode builds. On rustup 1.29.0 (this dev box) it
   downgrades to a warning. If you get the hard error, run
   `cd xous-core && cargo xtask install-toolkit` per `BUILDING.md` §0.

### 0.3 Issue #1 — what the maintainer wrote

Read [`issue #1`](https://github.com/tunnell/xous-app-signal/issues/1)
including the comment dated 2026-05-11 documenting the false-`Failed`
indicator on actually-delivered messages. The user-visible urgency is
two-fold: "sends are slow" **and** "users see `!` on messages the
recipient already replied to."

### 0.4 What is verified on hardware as of 2026-05-11

- v0.1 boots, links, sends, receives DMs from multiple distinct senders.
- Keepalive tolerance (vendored libsignal `MAX_OUTSTANDING_KEEPALIVES=3`)
  exercises in practice: `outstanding=1 threshold=3 → "within tolerance,
  continuing"`. **[verified]** — `vendor/libsignal-service-rs/src/websocket/mod.rs:306-320`.
- Server-side WS rotation observed at ~30-60 s cadence with `code=1001
  "Connection Idle Timeout"`.
- Send latency 1–4 min in worst case, retries succeed eventually.
- The retry loop in `crates/xous-signal-worker/src/lib.rs:1180-1226`
  works as designed **[verified]** but cannot distinguish "server didn't
  see the cipher" from "server received the cipher then closed before
  responding." The retry trigger is a string match on
  "websocket closing" at line 1208 **[verified]**.

### 0.5 Hardware access caveat

The maintainer's machine has a Pi rig (`pi@10.137.50.100`) that flashes
images to a Precursor PVT2 and captures UART logs. A fresh agent on
another machine **will not** have access to this rig. Plan around this:

- Phase 4a instrumentation can be measured on the host build
  (`cargo xtask run` from `xous-core/`), which boots Xous in an X11
  emulator on Linux x86_64. Real rv32 numbers need the maintainer's rig.
- Numbers reported in `BENCH.md` MUST cite host vs. rv32 source. When
  projecting rv32 from host, document the multiplier and the per-
  operation cost it's derived from (ECDSA verify vs. AES-GCM have
  very different ratios).

---

## 1. Confirmed facts from a verified read pass

### 1.1 TLS session resumption: tickets issued but cache is per-call, so effectively wasted

`crates/xous-net-bridge/src/tls.rs:62-68` **[verified]**:

```rust
let mut config = ClientConfig::builder()
    .with_root_certificates(roots)
    .with_no_client_auth();
if !alpn.is_empty() {
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
}
let config = Arc::new(config);
```

Important: rustls 0.22.2 `ClientConfig::builder()` produces a Config
whose `resumption` field is **already** `Resumption::in_memory_sessions(256)`
by default (see `client_conn.rs` `ClientConfig::resumption` initialization).
So resumption is **on by default** — but the 256-entry in-memory cache
**lives inside the ClientConfig**. Because xas constructs a fresh
ClientConfig on every call to `tls_connect`, every session ticket
issued by the server is stored in a cache that gets dropped as soon as
the connection completes. The very next `tls_connect` to the same host
starts with an empty cache and pays a full handshake.

The actionable fix is **not** to call `set_resumption(...)`. The
actionable fix is to give `SyncHttpClient` (and any other caller) a
shared `Arc<ClientConfig>` and have `tls_connect` consume that, instead
of constructing a fresh Config per call.

### 1.2 Both HTTP and WS callers funnel through `tls_connect` per call

`SyncHttpClient` at `crates/xous-net-bridge/src/http.rs:28-41`
**[verified]** holds `Arc<RootCertStore>`, **not** `Arc<ClientConfig>`.
Roots are shared, but the ClientConfig+SessionCache combo isn't:

- `SyncHttpClient::execute` (http.rs:46) — clones the RootCertStore via
  `(*self.roots).clone()` at line 48 and passes it to `sync_execute`,
  which calls `tls_connect` at line 96.
- `SyncHttpClient::connect_websocket` (http.rs:68) — same pattern at
  line 74, hands the RootCertStore to `crate::ws_pump::connect_websocket`.
- `ws_pump::handshake` (`crates/xous-net-bridge/src/ws_pump.rs:98-111`)
  **[verified]** calls `tls_connect(...)` at line 110.
- A third caller — `crates/xous-net-bridge/src/ws.rs::ws_connect`
  **[verified]** — also takes `roots: RootCertStore` and calls
  `tls_connect`. Stage 0 refactor must touch this caller too.

### 1.3 presage's WS lifecycle: BOTH WSes are long-lived

`vendor/presage/presage/src/manager/registered.rs` **[verified]**:

- **Lines 73-74**: the Manager holds **two** WS handles —
  `identified_websocket` and `unidentified_websocket` — both as
  `Arc<Mutex<Option<SignalWebSocket<...>>>>`.
- **Line 219**: `async fn identified_websocket(&self, force_new: bool)`
  — crate-private (no `pub`), so not callable from xas without a
  vendored-presage edit. `force_new=true` closes any existing identified
  WS and opens a fresh one.
- **Line 591**: `receive_messages` calls `self.identified_websocket(true)`
  at start — already forces fresh on receive entry.
- **Line 1382**: `new_message_sender` (private helper at lines 1381-1404)
  calls `self.identified_websocket(false).await?` and
  `self.unidentified_websocket().await?` to construct
  `MessageSender<S::AciStore>`. **This is the actual call site that
  exposes sends to the rotation race.**
- **Lines 1061 / 1187**: `identified_websocket(false)` is called a
  second time inside `send_message` (line 1061) and
  `send_message_to_group` (line 1187), feeding `save_message`. This
  runs *after* the cipher has been transmitted. A failure here is the
  primary mechanism behind the false-`Failed` indicator: cipher
  delivered, then save_message fails on a rotated WS, error surfaces
  with "websocket closing" substring, worker can't tell the difference
  from a real send failure.

Sealed-sender (default for DMs) flows through the `unidentified` WS.
Non-sealed-sender flows through `identified`. Both are constructed at
line 1382, both are long-lived, both rotate. The earlier mental model
of "route sealed-sender through unidentified WS to avoid the race"
**doesn't apply** — both WSes are subject to the same race.

### 1.4 Send path on the worker side

`crates/xous-signal-worker/src/lib.rs:1180-1226` **[verified]**:

- 6-attempt loop. Sleep schedule from `backoff_for` at line 1174:
  attempt 2 → 2 s, 3 → 4 s, …, 6 → 32 s. Worst-case wait between
  user-press and emitting `Event::SendError`: ~62 s.
- Retry trigger at line 1208: `msg.to_lowercase().contains("websocket closing")`.
  String-match because the error wraps through `presage::Error →
  ServiceError → SignalProtocolError` (verified at lib.rs:1155-1159).
- No exponential-backoff variant for other error classes; only
  WS-closing-shaped errors are retried.

### 1.5 The architecture's own framing of the problem

`docs/ARCHITECTURE.md` §8 says: *"on rv32 a single send pipeline often
takes longer than a WS lifetime."* That framing reframes the problem
from "WS rotation is the bug" to "send-pipeline duration vs. WS lifetime
is the ratio that drives the race." Anything that shrinks the pipeline
reduces race likelihood without touching WS lifecycle. Anything that
extends WS lifetime (or short-circuits the lifecycle) reduces it too.

### 1.6 Vendored libsignal already carries a downstream patch

`vendor/libsignal-service-rs/src/websocket/mod.rs:306` **[verified]** —
`const MAX_OUTSTANDING_KEEPALIVES: usize = 3;`. The upstream PR
(whisperfish/libsignal-service-rs#431) was reshaped to an opt-in
constructor; the vendored copy keeps the constant form. **This means:
editing `vendor/libsignal-service-rs/` for this work has precedent.**
The rule is "propose in PLAN.md and get approval," not "never edit
vendor/."

---

## 2. Reframed root cause

**It is not "the WS rotates."** WS rotation is unavoidable Signal-
server-side behavior (~30-60 s). The actionable root cause is the
**ratio of send-pipeline duration to WS lifetime.** On rv32 the
pipeline can take long enough that the race is near-certain.

Pipeline contributors, in approximate order of likely cost on rv32
**[hypothesis, must be measured by Phase 4a]**:

1. TLS handshake when reconnecting a closed WS (full ECDSA + ECDHE
   + AES-GCM ramp every time, ticket cache wasted per §1.1).
2. PQXDH session setup on first message to a new recipient device
   (curve25519 + Kyber768 KEM).
3. Per-message AES-GCM-SIV encryption per recipient device.
4. HTTP/WS framing overhead.
5. Server response wait (RTT-bound).

The ranking above is hypothesis. The actual breakdown could be very
different. **Stage 0's value depends on (1) being dominant; if Phase 4a
measurement shows (1) is < 30% of the pipeline, the plan should pivot
to whichever item dominates.**

---

## 3. Strategies considered

### Stage 0 — Enable usable TLS session resumption (`Arc<ClientConfig>` reuse)

**The change**: make `SyncHttpClient` (`http.rs`) hold an
`Arc<rustls::ClientConfig>` instead of `Arc<RootCertStore>`. Build that
ClientConfig once with `resumption = Resumption::in_memory_sessions(8)`
explicitly (current default of 256 is fine, but explicit-is-better),
and reuse it across `execute`, `connect_websocket`, and any other
caller. Refactor `tls_connect` to consume `Arc<ClientConfig>` instead
of `RootCertStore`. Update the third caller in `ws.rs::ws_connect` to
take Arc<ClientConfig> as well.

**Expected savings (if §2's hypothesis holds)**: full TLS handshake →
PSK-resumed handshake. On x86_64, full TLS 1.3 to a public endpoint is
typically 50-200 ms; resumed is 5-15 ms. On rv32 the asymmetric-crypto
skip is what dominates the savings — expected ratio is roughly 10-30×
depending on what fraction of the handshake is ECDSA-verify vs.
ECDHE-keygen vs. AES-GCM ramp. **Do not commit Stage 0 without Phase
4a numbers in `BENCH.md`.**

**Cost**:
- ~30 lines across `tls.rs`, `http.rs`, `ws_pump.rs`, `ws.rs`. The
  `tls_connect` signature flips from `(roots: RootCertStore, alpn: ...)`
  to `(config: Arc<ClientConfig>, ...)`. Roots get baked into the
  Config at construction time inside `SyncHttpClient::new`.
- Risk: minimal once measured. The pattern is rustls's documented
  recommendation.

**What it doesn't fix**:
- The first send-to-new-recipient still pays PQXDH setup.
- The race window doesn't close — it just becomes much smaller.
- Cross-edge-server ticket rejection (see §5.3).

### Stage 1' — Pre-open WS on compose-screen entry

**The change**: when the user transitions into `Screen::Thread`
(compose-mode entry), trigger an `identified_websocket(true)` (or
`unidentified_websocket()` re-issue) in parallel with their typing.
By the time Enter triggers `Cmd::Send`, the WS handshake has either
completed or nearly so.

**Caveats**:
- The pre-opened WS may itself rotate before Enter if the user pauses
  for >30 s. The race window shrinks but isn't eliminated.
- **Marginal value collapses once Stage 0 is in place.** If post-Stage-0
  TLS handshake is ~5 ms on host (~150 ms projected on rv32), pre-open
  is amortizing ~150 ms over the user's typing time. That's not user-
  perceptible. **Implement Stage 1' only if Phase 4a measurement after
  Stage 0 shows the handshake is still meaningful relative to total
  pipeline.**
- Cite: `Screen::Thread` transition is set in
  `crates/xous-app-signal/src/gam_app.rs` (search for `app.screen =
  Screen::Thread`).

### Stage 1 — Reduce pipeline-vs-rotation race directly

The original framing was "per-send fresh WS." After §1.3's verified
re-read, this needs sharpening. Two sub-options:

**Stage 1a — vendored presage edit to force fresh WS for the actual
transport call.** Modify `new_message_sender` at
`vendor/presage/presage/src/manager/registered.rs:1382` to call
`identified_websocket(true)` (or expose a parameter to the public
`send_message` API). Same for the unidentified case at line 1383.
This guarantees the WS used for the cipher is freshly opened per send.

   *Side-effect:* `receive_messages` (line 591) already calls
   `identified_websocket(true)` at start, then holds that handle for
   the duration of its read loop. Forcing fresh on each send would
   tear down the receive WS mid-poll. Receive would reconnect lazily
   on the next loop iteration; no message loss (server queues for the
   client) but **inbound latency spikes once per outbound message**.
   For a chat client this is a real UX regression — typing → respond
   cadence matters. State this trade-off explicitly when implementing.

**Stage 1b — fix the false-Failed indicator at its actual source.**
The send-time call at `registered.rs:1382` is what carries the cipher.
The subsequent `identified_websocket(false)` at line 1061 (1-to-1) /
1187 (group) is only used for `save_message`, which is a local-store
write that may sync to other linked devices but is **not** required for
the recipient to receive the message. The current code surfaces a
save_message failure as a send failure — that's the false-Failed
mechanism documented in issue #1's 2026-05-11 comment. Two narrower
fixes are possible without forcing fresh-WS-per-send:

   - In presage (vendored): change `send_message` to catch
     save_message errors and return `Ok(())` if the upstream send
     already succeeded. Plumb a partial-success signal back to the
     caller separately.
   - In xas worker: detect that `manager.send_message` succeeded
     at the transport layer (e.g. by adding an instrumentation log
     line inside the vendored presage to mark the cipher-sent point,
     then matching on that in the worker's error analysis). Save_message
     failure becomes `Event::SendComplete` plus a warning log, not
     `Event::SendError`.

Stage 1b is **strictly less invasive** than Stage 1a and addresses the
user-visible false-Failed bug directly. It does NOT reduce send
latency on the cipher-transport path itself — that's Stage 0's job.

**Recommended:** if Stage 0 alone hits the §4 thresholds, ship only
that. If the false-Failed indicator persists post-Stage-0 (it will,
because Stage 0 only shrinks the race; save_message can still fail on
the post-send WS use), add Stage 1b as a small follow-up. Stage 1a
should be the last resort because of the receive-latency regression.

### Stage 2 — Proactive rotation detection (the issue's Option B)

Client tracks WS-open time, proactively reconnects before Signal's idle
timeout fires. Deferred: more complex than Stage 1b, doesn't clearly
outperform "Stage 0 + Stage 1b" on user-visible metrics.

### Stage 3 — Plain HTTPS PUT to `/v1/messages/{aci}`

Bypass the WS for sends. Loses sealed-sender-by-default routing (xas
currently uses `unidentified_access=true online=false`) and weakens the
privacy story. Linked-device auth on plain HTTPS is non-trivial.
Deferred unless maintainer authorizes.

### Stage -1 — UX-only fix, no protocol work (sanity-check option)

Don't mark messages `Failed` until either (a) the worker's retry loop
exhausts all 6 attempts AND (b) no incoming delivery receipt is observed
within M seconds, where M ≥ the maximum send pipeline you've measured.
Combine with an "unconfirmed" intermediate state in the UI.

**Why this is on the list:** it directly resolves the user-visible
false-Failed bug with zero protocol changes. The trade-off: real
failures (TLS error, identity mismatch, recipient blocked) take longer
to surface to the user. For a low-volume chat app with attentive users
that may be acceptable.

**Why we still want Stage 0 even with Stage -1:** the underlying send
latency itself (1-4 min in worst case) is a real user complaint
independent of the false-Failed indicator. Stage 0 is the smallest
intervention that addresses pipeline duration. Stage -1 alone treats
the symptom; Stage 0 treats one of the underlying causes.

---

## 4. Recommended path and sequencing

The original plan recommended Stage 0 first. After review, **the
correct first step is instrumentation, not Stage 0.** You can't
validate Stage 0's premise (TLS handshake dominates the pipeline)
without baseline numbers.

1. **Phase 4a — instrument first.** Add timestamped log points around
   `tls_connect`, around `manager.send_message`, around the worker's
   retry loop. Behavior unchanged. Commit. Measure host-side: capture
   baseline pipeline breakdown over 20 sequential sends to a hosted
   peer (signal-cli running locally). Record results in `BENCH.md`.

2. **Decision point**. Read `BENCH.md` and the maintainer decides:
   - If TLS handshake ≥ 30 % of pipeline → proceed to Stage 0.
   - If TLS handshake < 30 % → pivot. The dominant contributor is
     somewhere else (PQXDH setup? PDDB write? network RTT?) and this
     plan needs revisiting.
   - If false-Failed is the only remaining user complaint after baseline
     measurement (i.e. latency feels OK on host) → consider Stage -1
     or Stage 1b before Stage 0.

3. **Phase 4b — Stage 0 if Phase 4a justifies it.** Commit. Re-measure.
   Compare to baseline in `BENCH.md`.

4. **Phase 4c — Stage -1 and/or Stage 1b if false-Failed persists.**
   The user-visible bug from issue #1's comment thread is independent
   of latency. Either of these resolves it without further protocol work.

5. **Phase 4d — Stage 1' or Stage 1a only if measured latency after
   Stages 0 + 1b is still unacceptable on hardware.** These add
   meaningful complexity (Stage 1') or vendored-presage maintenance
   burden (Stage 1a) and shouldn't ship speculatively.

### Stop-after-Stage-N criteria (explicit, derived from observed WS lifetime)

The observed WS rotation cadence is 30-60 s. To make the race
arbitrarily rare, we want **median pipeline ≤ 0.3 × min-rotation**
(roughly p99 pipeline ≤ min-rotation). That gives:

| After | Stop if (host median) | Stop if (rv32 projected) | Rationale |
|---|---|---|---|
| Phase 4a only | ≤ 3 s | ≤ 10 s | Latency already fine — only ship Stage -1 if false-Failed remains |
| Phase 4b (Stage 0) | ≤ 1 s | ≤ 8 s | Comfortably inside p99 ≤ 30 s rotation floor |
| Phase 4c (+ Stage 1b) | (false-Failed gone) | (false-Failed gone) | UX bug resolved, latency separately measured |
| Phase 4d | Should always meet target | Should always meet target | If not, escalate |

The "rv32 projected" column is host × N, where N is a per-operation
multiplier documented in `BENCH.md`. Different operations have different
ratios; do not assume a single global N.

---

## 5. Risks not covered by the issue text

### 5.1 Rate-limit risk from frequent fresh WSes

Stage 1a (force-fresh-per-send) would multiply WS-open rate per user.
For a single xas user this is fine; for a population of clients with
this pattern, Signal's rate limiter may push back. Mitigation:
prefer Stage 1b (no extra WS opens) over Stage 1a.

### 5.2 Concurrent-sends semantics

If the user mashes Enter twice fast, two `Cmd::SendMessage` arrive at
the worker before the first one's WS handshake completes.
**Unverified**: I have not confirmed whether the worker serializes
sends or processes them concurrently. Before any Stage 1 work,
verify by reading `manager_task` in `crates/xous-signal-worker/src/lib.rs`
and document the answer here. If sends are concurrent and Stage 1a is
ever implemented, two simultaneous force_fresh calls race against
each other on the identified_websocket Mutex.

### 5.3 TLS ticket rejection at load-balancer boundary

rustls's in-memory ticket cache stores tickets per server name, not per
backend. Signal's edge endpoints sit behind a load balancer; tickets
issued by backend A may not be honored on backend B. Resumption
hit-rate will be < 100 % even with the shared cache. Document this so
the post-Stage-0 measurement doesn't surprise anyone if savings are
smaller than projected. (Stage 0 is still worthwhile — even a 50 %
hit-rate halves the average handshake cost.)

### 5.4 rustls 0.22 + ring fork on rv32

`Cargo.toml` patches `ring` to a riscv-compatible fork. TLS resumption
performance depends on the patched ring's session-ticket crypto path.
If the ring patch lacks AES-NI fast paths (it has to, since rv32 has
no AES-NI), the absolute numbers will differ from rustls upstream
benches. Note this when interpreting `BENCH.md`.

### 5.5 Linked-device auth context per WS open

xas is registered as `deviceId = 2`. Each fresh authenticated WS must
present linked-device credentials, which presage handles via
`Manager::registration_data()` → `auth_credentials()`. This path is
already exercised by `receive_messages`'s
`identified_websocket(force_new=true)` at line 591 **[verified]**, so
Stage 1a wouldn't break new ground. But verify by reading the
auth-credential path before implementing Stage 1a.

### 5.6 Vendored presage and libsignal-service-rs already deviate from upstream

The libsignal `MAX_OUTSTANDING_KEEPALIVES=3` constant is the first
downstream patch. Any Stage 1a/1b vendored-presage edit becomes the
second. Track each in `README.md` "Upstream patches" section. Filing
an upstream presage issue asking for a `send_message(..., force_fresh:
bool)` parameter (or for save_message failures to be surfaced
separately from send failures) is the durable move regardless of which
stage ships; do it as a follow-up issue, not gated on this PR.

### 5.7 The false-Failed indicator outlives the latency fix

Stage 0 shrinks the race but does not eliminate it. As long as the
post-send `identified_websocket(false)` call at registered.rs:1061
exists and can fail with "websocket closing", **some** sends will
falsely surface as Failed. The plan must either accept that frequency
goes down by a large factor (acceptable if measurement supports it) or
add Stage 1b. The user-visible bug from issue #1 isn't fully resolved
by Stage 0 alone.

### 5.8 Pre-opened WS aging (Stage 1')

A pre-opened WS starts its rotation clock immediately. If the user
takes 45 s to compose, the pre-open's WS may rotate before Enter. The
race window shrinks (compared to a WS open at app launch) but does not
close. Worst case (compose for 90 s) the pre-open is actively unhelpful.

---

## 6. Proposed instrumentation diff (Phase 4a)

Add timestamped log points. Use the existing `worker/send:` prefix
convention from `docs/ARCHITECTURE.md §9`. Sketch:

```rust
// crates/xous-signal-worker/src/lib.rs around line 1193
let t_send_start = ticktimer.elapsed_ms();
let send_fut = std::panic::AssertUnwindSafe(
    manager.send_message(recipient.clone(), content_body.clone(), timestamp),
);
let outcome = send_fut.catch_unwind().await;
let t_send_done = ticktimer.elapsed_ms();
log::info!(
    "worker/send: attempt {} pipeline_ms={} ({:?})",
    attempt, t_send_done - t_send_start, outcome.as_ref().map(|o| o.is_ok()),
);
```

In `crates/xous-net-bridge/src/tls.rs::tls_connect`:

```rust
let t_tls_start = std::time::Instant::now();
let conn = ClientConnection::new(config, server_name)...;
let sock = TcpStream::connect((host, port))?;
sock.set_read_timeout(...)?;
let stream = StreamOwned::new(conn, sock);
log::info!(
    "xous-net-bridge: tls_connect {} done in {:?}",
    host, t_tls_start.elapsed(),
);
```

**Resumption-detection nuance**: rustls 0.22.2 does NOT expose a public
`handshake_kind()` API (verified at
`~/.cargo/registry/src/index.crates.io-*/rustls-0.22.2/src/common_state.rs:131`
— only `negotiated_cipher_suite()` is public). The cleanest signal in
this rustls version is to wrap `Resumption::store(Arc<dyn
ClientSessionStore>)` with a custom store that counts get/put calls,
then log the count. This doubles as the basis for the §7 test.

The peer_certificates-based proxy proposed in earlier revisions of this
plan is **wrong** — TLS 1.3 resumption can still cause the server to
send a Certificate message depending on `psk_key_exchange_modes`. Don't
use it.

---

## 7. Test plan

### 7.1 Host-side unit tests (new)

1. **`xous-net-bridge::tls::tests::resumption_enabled_in_built_config`**
   — construct the `ClientConfig` via the new `SyncHttpClient::new`
   path; assert `config.resumption.store()` (if surfaced) returns a
   non-empty store. If the API doesn't surface the field publicly, use
   a wrapper-injection test instead (see 7.2).

### 7.2 Host-side integration test (new)

2. **`xous-net-bridge/tests/tls_resumption.rs`** — stand up a local
   rustls `ServerConfig` with ticket issuance enabled. Build a
   `SyncHttpClient` whose `ClientConfig` uses a custom
   `ClientSessionStore` wrapper that counts get/put. Drive two
   consecutive HTTP requests. Assert that on the second handshake, at
   least one `get(...)` returned a non-None ticket (= resumption was
   attempted) AND the corresponding server-side log/counter shows it
   was accepted. This is the load-bearing test for Stage 0.

### 7.3 Existing test that must remain green

3. The existing ~50 hosted-mode tests (`cargo test --features hosted
   -p xous-app-signal --bins`) — must stay at the documented "53
   passed" baseline.

### 7.4 Retry-policy regression test (new)

4. **`xous-signal-worker/tests/send_retry_policy.rs`** — mock
   `manager.send_message` to return WsClosing errors 5× then succeed.
   Assert the worker retries with the documented backoff (2/4/8/16/32 s)
   and emits `Event::SendComplete` after the 6th success. Catches
   accidental policy changes during Phase 4 work. (This test is
   independent of Stage 0 but the plan introduces refactoring around
   the retry loop in Phase 4a, so the test guards against drift.)

### 7.5 Bench (informational, no pass/fail)

5. **`crates/xous-net-bridge/benches/send_latency.rs`** OPTIONAL —
   only if the maintainer wants automation. The benchmark mock (rustls
   server that closes with code 1001 at random intervals) is several
   hundred lines of code to write from scratch; if there isn't a
   pre-built mock to lean on, scope this out of Stage 0 and use manual
   measurement via §2.5's `tests/hosted/scan_receive.sh` harness.

### 7.6 What NOT to test

- No tests that hit real `chat.signal.org`.
- No tests that mock the full Signal protocol cipher stack.
- No tests in `vendor/`.
- **Dropped from earlier plan**: a "shared_clientconfig_returns_same_arc"
  unit test that just asserts `Arc::ptr_eq`. That verifies plumbing,
  not behavior; §7.2 covers the real signal.

---

## 8. Rollback plan

One commit per phase. Each commit references `#1` in the message.

- **Phase 4a (instrumentation) rollback**: revert the commit; log lines
  go away. No behavioral impact.
- **Phase 4b (Stage 0) rollback**: revert. `tls_connect` reverts to
  per-call ClientConfig. Latency reverts to baseline.
- **Phase 4c (Stage -1 or Stage 1b) rollback**: revert. False-Failed
  indicator returns; latency unchanged.
- **Phase 4d (Stage 1a / 1') rollback**: revert the vendored presage
  patch; `README.md` "Upstream patches" entry gets removed.

**Do not** combine multiple phases into a single commit.

---

## 9. Open questions for the maintainer

1. **`rustls` version**: workspace pins `=0.22.2` per `Cargo.lock`
   (rustls 0.23.40 also present transitively but not in our crates).
   `Resumption::in_memory_sessions(N)` is stable since rustls 0.21.
   No version bump needed. Confirm.
2. **Vendored presage modification — pre-approve or per-patch approval?**
   Stage 1a/1b require it; Phase 4a and Stage 0 do not.
3. **Stage -1 acceptability**: is treating the false-Failed indicator
   as a pure UX bug (Stage -1) acceptable for v0.1.x, or must Stage 1b
   ship in the same release?
4. **Acceptable hardware-projection method**: when no Pi rig is
   available, what's the maintainer's preferred way to project rv32
   numbers? Suggested: per-operation multipliers (ECDSA-verify
   x86_64→rv32 ~30×, AES-GCM ~5×, integer-only code ~3×) cited
   alongside each measurement, not a single blanket factor.
5. **PR scope**: should Phase 4a (instrumentation) ship as its own PR
   so baseline numbers can be reviewed before Stage 0 commits, or
   bundle everything into one PR with multiple commits? My
   recommendation: separate PRs for separate phases.

---

## 10. Phase-specific deliverables

| Phase | Deliverable | Output |
|---|---|---|
| 1 | Build environment confirmed | (this PLAN.md's §0.2) |
| 2 | Verified read-pass findings | (this PLAN.md's §1) |
| 3 | This document (revised) | `PLAN.md` ← **stop here, await Phase 4a approval** |
| 4a (post-approval) | Instrumentation | one commit + `BENCH.md` (baseline numbers) |
| 4a-review (mandatory) | Maintainer reads `BENCH.md`, picks next stage | (no code change) |
| 4b (post-approval) | Stage 0 implementation | one commit + `BENCH.md` update |
| 4b-tests (post-approval) | §7.1 + §7.2 + §7.4 tests | one commit per file, or one combined commit if small |
| 4c (post-approval, conditional) | Stage -1 and/or Stage 1b | one commit each |
| 4d (post-approval, conditional) | Stage 1a / 1' | one commit each |
| 5 (post-approval) | PR(s) | one per phase per §9.5 |

---

## 11. Hard stop rules (do not violate)

1. **No implementation past Phase 4a-review without explicit "go" from
   the maintainer.** The whole point of staged commits is to gate on
   measurement.
2. **No edits under `vendor/` without explicit per-stage approval.**
   Stage 1a/1b specifically call for it; that's a per-stage request,
   not a free pass.
3. **No `git push` to GitHub without explicit "push" from the
   maintainer.** Commits live on a local branch until told otherwise.
4. **No cryptography written by us.** Transport only.
5. **No changes to the `Event::SendComplete { timestamp }` /
   `Event::SendError { reason, timestamp }` UI interface.** The
   optimistic-Pending-message pattern depends on it.
6. **Match commit-message and PR style of `ARCHITECTURE.md`**: direct,
   technical, file:line cites, no marketing, no superlatives. No
   Co-Authored-By trailer.
7. **Each commit references `#1` in the message.** Each commit is one
   phase. No combined-phase commits.
8. **No `BUILD_NOTES.md`-style parallel docs.** Use `BUILDING.md` for
   build instructions, `BENCH.md` for measurements, `PLAN.md` for this
   plan.
9. **`BENCH.md` numbers MUST cite host vs. rv32 source.** Projected
   rv32 numbers must show the multiplier and per-operation derivation.

---

## 12. Estimated effort

- Phase 4a (instrumentation + bench harness for sequential sends): 2 h
- Stage 0 implementation: 1 h
- Stage 0 tests (§7.1, §7.2): 2 h (the custom ClientSessionStore
  wrapper is the bulk of the work)
- Stage -1 and/or Stage 1b: 1-2 h each
- Stage 1a / 1': 3-4 h each, including vendored-presage patch
  documentation
- PR write-up(s): 1 h per PR

Most-likely total if Stage 0 alone suffices: ~6 hours of focused work
plus measurement cycles.
