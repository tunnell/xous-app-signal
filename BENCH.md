# BENCH.md — Phase 4a baseline measurements

Per `PLAN.md` §10 / §11.9. All numbers here are **host-side on x86_64**
(`tunnell@<this dev box>`, 2026-05-11) unless explicitly marked otherwise.
No Pi rig / Precursor PVT2 was available during this measurement pass;
rv32 numbers are projection only and labeled as such.

Bench source: `crates/xous-net-bridge/tests/handshake_bench.rs`. Re-run
with:

```sh
cargo test --release -p xous-net-bridge --test handshake_bench -- --nocapture
XAS_BENCH_NET=1 cargo test --release -p xous-net-bridge --test handshake_bench -- --nocapture --test-threads=1
```

---

## 1. TLS handshake cost: Stage 0 vs. baseline

The bench drives `ITERS = 20` sequential handshakes per scenario and
reports median + p99 wall time. Each "shared" run reuses one
`Arc<ClientConfig>` (Stage 0 path); each "per-call" run constructs a
fresh `ClientConfig` per handshake (pre-Stage-0 path). The shared runs
also report inserts / takes(some) / takes(none) on a custom
`ClientSessionStore` wrapper so we can distinguish "resumption is
working but provides no measurable savings" from "resumption never
engaged."

| Target | Scenario | median (ms) | p99 (ms) | inserts | takes(some) | takes(none) |
|---|---|---:|---:|---:|---:|---:|
| 127.0.0.1 (local rustls + rcgen self-signed) | shared | 0.91 | 3.01 | 80 | 19 | 1 |
| 127.0.0.1 (local rustls + rcgen self-signed) | per-call | 1.15 | 1.23 | — | — | — |
| cloudflare.com:443 (TLS 1.3, tickets issued) | shared | 73.38 | 83.12 | 40 | 19 | 1 |
| cloudflare.com:443 (TLS 1.3, tickets issued) | per-call | 77.68 | 84.93 | — | — | — |
| chat.signal.org:443 (production Signal endpoint) | shared | 103.64 | 109.51 | **0** | **0** | **20** |
| chat.signal.org:443 (production Signal endpoint) | per-call | 103.25 | 108.50 | — | — | — |

Reproducer: `git checkout dd492dc &&` the cargo commands above.

### What the cache columns mean

For 20 sequential handshakes through one `Arc<ClientConfig>` with TLS
1.3 + the server cooperating:

- **inserts**: total TLS 1.3 NewSessionTicket frames received and
  stored. rustls's default server config issues 4 tickets per
  handshake; the bench's local server uses default behavior, so
  `4 × 20 = 80` is expected. Cloudflare appears to issue 2 per
  handshake (`2 × 20 = 40`). chat.signal.org issues **0**.
- **takes(some)**: handshakes that found a cached ticket and offered
  PSK resumption. Expected: `19` (every handshake after the first).
- **takes(none)**: handshakes that looked up a ticket and found
  nothing. Expected: `1` (the first handshake, before any ticket has
  been stored).

The local-server and Cloudflare numbers match the expected pattern
exactly. **chat.signal.org never issues a session ticket**, so the
cache stays empty and every handshake is full.

---

## 2. The load-bearing finding: Stage 0 is a no-op against production Signal

`inserts = 0` against `chat.signal.org:443` means TLS session resumption
is **not happening** against the actual production Signal endpoint —
not because of any bug in xas, but because Signal's TLS terminator does
not issue session tickets. The wall-clock medians confirm this: 103.64
vs 103.25 ms (shared vs per-call) is well within run-to-run variance.

The control target (`cloudflare.com:443`) confirms our Stage 0 code is
correct — Cloudflare issues tickets, our `ClientSessionStore` records
them, and resumption engages on every handshake after the first
(`takes(some) = 19/20`). The savings against Cloudflare are
nonetheless modest: 77.68 → 73.38 ms median = **~5% reduction** on
x86_64. That's because:

- TLS 1.3 1-RTT resumption still requires one full network round trip.
- The asymmetric-crypto work it saves (ECDSA cert chain verify, ECDHE
  key share validation) is cheap on x86_64 — typically single-digit
  milliseconds — so it's swamped by network RTT (~25-60 ms in our
  setup).

This invalidates the `PLAN.md §2` hypothesis that "TLS handshake
dominates the send pipeline on rv32" **for resumption-eligible
savings**. The handshake may still dominate on rv32 because the
asymmetric crypto scales differently, but Stage 0 cannot help against
chat.signal.org because resumption is unilaterally disabled
server-side.

### Implications for the PLAN

- **Stage 0 stands as a defense-in-depth change** — it's correct,
  tested, and won't regress anywhere — but it doesn't move the user-
  visible latency needle on production. Recommend keeping the commit
  and the test, but stopping any further work that's gated on Stage 0
  delivering savings.
- **`PLAN.md §4` decision tree should pivot now**, not after rv32
  numbers. Even if rv32 numbers showed TLS handshake at 90% of
  pipeline, Stage 0 still wouldn't engage against Signal — the issue
  is server policy, not crypto cost.
- **The next-most-leveraged interventions are Stage 1b
  (fix `save_message` false-Failed) and Stage -1 (UX-only).** Both
  target the user-visible bug from issue #1's 2026-05-11 comment
  directly, neither depends on resumption working.
- **Stage 1a (force-fresh WS per send) and Stage 1' (pre-open WS on
  compose entry) become the latency-targeting options** since they
  short-circuit the WS rotation race without relying on cheaper TLS.
  Their value depends on the rv32 handshake cost — which is still
  unmeasured — so any commit there needs hardware numbers first.

---

## 3. Send-pipeline measurement — what's missing

This BENCH.md captures **TLS handshake cost only**, not the full
`manager.send_message` pipeline that `PLAN.md §4.1` calls out. The
gap exists because the full-pipeline measurement requires either:

- **A real Signal account linked to `signal-cli`** as the test peer
  (`tests/hosted/scan_receive.sh`, per `BUILDING.md §2.5`). We have
  signal-cli installed but no `tests/hosted/test_env` file
  (gitignored, expects real Signal numbers).
- **A Precursor PVT2 + Pi flashing rig** for rv32 numbers. None
  currently available to the agent doing this measurement.

Without those, the `worker/send: pipeline_ms=...` log line added in
commit `4eedbc9` has no data points to report. Next iteration should
either (a) provision a `test_env` with disposable Signal accounts, or
(b) wire a mocked `manager.send_message` into the existing hosted-mode
test harness so we can measure pipeline cost minus the live Signal
component.

---

## 4. Projection to rv32 (caveats)

`PLAN.md §9.4` asks for per-operation projection multipliers rather
than a single blanket factor. We can offer two anchored estimates:

| Operation | x86_64 cost (this bench) | rv32 multiplier (assumed) | rv32 projection |
|---|---|---:|---:|
| Full TLS 1.3 handshake to Cloudflare-class server | ~78 ms | 10× (ECDSA verify dominated) | ~780 ms |
| Full TLS 1.3 handshake to chat.signal.org | ~103 ms | 10× | ~1.0 s |
| Stage 0 savings per resumed handshake (Cloudflare) | ~4 ms | 30× (asymmetric crypto only) | ~120 ms |
| Stage 0 savings per resumed handshake (Signal) | 0 ms (server doesn't issue tickets) | — | 0 ms |

The 10× / 30× multipliers are upper-bound estimates from public
benchmarks of ring's ECDSA-verify on small RISC-V vs. x86_64; they
should be treated as order-of-magnitude only. **A single rv32
handshake measurement against chat.signal.org would replace this
entire table with real numbers** — flagging it as the highest-value
next data point.

---

## 5. What to revisit when more data is available

1. **Pipeline numbers (`worker/send: pipeline_ms=…`)** from a hosted-
   mode run with a real Signal account, OR from a Precursor on the Pi
   rig. Either one would settle whether `PLAN.md §4`'s "≤ 15 s rv32
   projection" threshold is met by current code with no further
   changes.
2. **Did chat.signal.org's ticket policy change?** Re-run §1 monthly
   and compare against the `inserts = 0` baseline above. Any change
   would invalidate the "Stage 0 is a no-op against production"
   conclusion.
3. **Per-operation rv32 multipliers** from a focused micro-bench
   (ECDSA verify alone, AES-GCM alone, HKDF alone) on a Precursor —
   would let us project specific pipeline contributors instead of
   guessing global factors.

---

## 6. Hardware completion procedure (fresh-machine handoff)

Everything in §1–§5 above was measured host-side on x86_64. The
remaining numbers needed to retire issue #1 are rv32-side and
require a Precursor PVT2. This section is for a fresh agent on
another machine who needs to pick up where this branch left off.

### 6.1 What you need

- A **Precursor PVT2** running a current Xous gateware (the iSerial
  from `lsusb -v 2>&1 | grep iSerial` while in loader mode shows the
  gateware build hash; record it before flashing — you'll feed it back
  to xtask as `--git-describe` / `--git-rev`).
- A **USB-C cable that supports data**.
- A **Raspberry Pi 4B with the betrusted debug HAT** for reliable
  flash + persistent UART log capture. Direct-from-host flashing works
  too (`tools/usb_update.py --bounce`) but you lose the continuous
  UART log, which is the primary diagnostic surface for this work.
- A **Linux x86_64 build host** with ~20 GB free disk, rustup
  (`rustup default stable` produces 1.95.x), and the Debian/Ubuntu
  packages from `BUILDING.md §0`.
- **Two phone numbers**: one for the Signal account xas links to as
  a secondary device, one for the `signal-cli` test peer. Disposable
  numbers (e.g. JMP.chat / Voice) are fine; both accounts MUST be on
  real Signal, not a test/staging environment.

### 6.2 First-time setup

1. **Clone with the exact layout** per `BUILDING.md §1` —
   `xous-app-signal/` and `xous-core/` (branch `xous-app-signal` of
   `tunnell/xous-core`) as siblings, plus the `repos/xous-core`
   symlink at the parent level. The relative path `../repos/xous-core/`
   is hard-coded into `xous-app-signal/Cargo.toml` and the build will
   fail immediately if it's wrong.
2. **Switch xous-app-signal to this branch**:
   ```sh
   cd xous-app-signal
   git fetch origin
   git checkout issue-1-send-latency
   ```
3. **Install the Xous std bundle** per `BUILDING.md §0` step 2 / §1.5:
   ```sh
   cd ../xous-core && cargo xtask install-toolkit
   ```
   On rustup ≥ 1.28 this is a hard prerequisite even for hosted-mode
   builds.
4. **Bootstrap `services/gam/src/apps.rs`** per `BUILDING.md §2.1`.
   The file is gitignored; copy the literal block from §2.1 verbatim.
5. **Wire up the Pi rig** per `BUILDING.md §3.3 option A`. Copy
   `xous-core/tools/usb_update.py` once; the `xous.img` will be
   re-scp'd for each flash. Start `screen -dmS uart cat /dev/ttyAMA0
   >> uart-log` and keep it running across all flashes — the boot
   trace and `worker/send: pipeline_ms` lines you need land in
   that file.
6. **Configure `tests/hosted/test_env`** per `BUILDING.md §2.5`:
   ```sh
   cd ../xous-app-signal
   cp tests/hosted/test_env.example tests/hosted/test_env
   $EDITOR tests/hosted/test_env   # fill in TEST_PEER_NUMBER / TEST_XAS_NUMBER
   ```
   Both numbers are required for §6.4's harness.

### 6.3 Build, flash, link

```sh
# In xous-app-signal/
bash tests/precursor/build-and-bundle.sh
# Override GIT_DESCRIBE / GIT_REV env vars if your device's gateware
# differs from the default. The image lands at
# ../xous-core/target/riscv32imac-unknown-xous-elf/release/xous.img
```

Flash via the Pi:

```sh
scp ../xous-core/target/riscv32imac-unknown-xous-elf/release/xous.img \
    pi@<pi-ip>:~/xous-flash/xous.img
ssh pi@<pi-ip> 'cd ~/xous-flash && python3 usb_update.py -k xous.img --bounce'
```

Boot into Xous, unlock PDDB, run the wlan recipe from `BUILDING.md §3.4`
(2.4 GHz network only; phone hotspot must be forced to compatibility
mode). Link xas to your `TEST_XAS_NUMBER` Signal account per
`BUILDING.md §3.4` step 4. **Save the linked-device slot** — you may
need a few iterations.

### 6.4 The actual measurement (this is what we need)

The instrumentation already in this branch logs `pipeline_ms` for
every send attempt. Drive 20+ sequential sends and harvest the
numbers from UART:

```sh
# On the Pi, drive sends from signal-cli to the xas device:
signal-cli -u "$TEST_PEER_NUMBER" send -m "bench send 1" "$TEST_XAS_NUMBER"
# (or, to measure xas-side outbound, use xas's UI to send to TEST_PEER_NUMBER —
# stdin_ui has a non-modal compose path; type and press Enter)
```

For an **outbound-from-xas** measurement (what PLAN.md §4 actually
gates on), the cleanest harness is:

1. Power-cycle Precursor between runs to ensure cold-start state
2. From a fresh boot + link, send 20 messages from xas to
   `TEST_PEER_NUMBER` with ~10 s spacing
3. Each `worker/send: attempt N returned pipeline_ms=X result=Y` line
   in the UART log captures one data point
4. Also harvest `xous-net-bridge: tls_connect ... setup_ms=...` lines
   — these capture how often the TLS layer is reconnecting

Extract the numbers:

```sh
ssh pi@<pi-ip> "grep 'worker/send:.*pipeline_ms=' ~/uart-log | tail -100"
ssh pi@<pi-ip> "grep 'tls_connect:' ~/uart-log | tail -100"
```

### 6.5 Numbers to capture, where they go

The fields to fill into `BENCH.md`:

| Section | What to add | Reproducer |
|---|---|---|
| `BENCH.md §1` table | A 7th and 8th row for rv32: `chat.signal.org:443 / shared` and `chat.signal.org:443 / per-call` medians, with the comment that resumption is server-disabled so the two should be near-identical | Run the bench from §6.4 *twice*: once on this branch (Stage 0 active), once after `git revert dd492dc`. Median of 20 each. |
| `BENCH.md §3` | Replace "This BENCH.md captures TLS handshake cost only" with the actual pipeline median + p99 from §6.4 step 4. | The grep command above; `awk '{print $NF}'` to pull just the ms values; pipe to `sort -n` then take the 10th item (median of 20). |
| `BENCH.md §4` | Replace the row "Full TLS 1.3 handshake to chat.signal.org" / `~103 ms / 10× / ~1.0 s` with the actual measured rv32 number. Update the projection table to be observation, not estimate. | Same as §1 — the `tls_connect: setup_ms` line is a lower bound on handshake; pipeline_ms includes everything else. |
| New `BENCH.md §7` | The Stage -1 receipt-rescue rate. Send 20 messages; count how many `worker/send: ts=N grace expired` lines appear vs `worker/send: ts=N confirmed by DELIVERY receipt`. The former are real failures; the latter are messages Stage -1 rescued. | grep `worker/send:` for both substrings, count. |

### 6.6 Decision the maintainer needs to make after §6.5

`PLAN.md §4.2` says: if median pipeline ≤ 15 s on hardware, stop. If
not, escalate to Stage 1' or Stage 1a.

Three plausible outcomes:

1. **Pipeline median ≤ 15 s, false-Failed rate near zero** →
   issue #1 is closed by this branch. Mark it done.
2. **Pipeline median ≤ 15 s but false-Failed rate is non-trivial** →
   Stage -1's grace window is too short or DELIVERY receipts aren't
   arriving in the window. Diagnostic: count `grace expired` lines.
   Either raise `PENDING_RECEIPT_GRACE` (currently 30 s) or investigate
   why receipts are slow.
3. **Pipeline median > 15 s** → Stage 1' (pre-open WS on compose) or
   Stage 1a (force-fresh WS per send, vendored presage). PLAN.md §3
   has the design sketches; both are host-implementable but should be
   driven by these hardware numbers, not speculation.

In all three cases, the next commit on this branch should be the
BENCH.md update with the hardware numbers, before any further
implementation work.

### 6.7 What NOT to do

- Don't push to `main` or `dev`. This branch ships via PR review.
- Don't `git revert` Stage 0 (`dd492dc`) permanently — it's correct
  defense-in-depth even with the chat.signal.org no-op finding, and
  the integration test in `crates/xous-net-bridge/tests/tls_resumption.rs`
  guards against accidental regression. The revert in §6.5 is for one
  measurement, not a code change.
- Don't write rv32 numbers as if they were x86_64 — always cite the
  source per `PLAN.md §11.9`.
- Don't ship a Stage 1' / Stage 1a commit without first updating
  BENCH.md with the rv32 numbers that justify it. The PLAN's
  decision tree is "measure first."
