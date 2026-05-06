# Stage 13b-3 — PDDB auto-mount: not needed

**Date.** 2026-05-06
**Status.** **Skipped.** Auto-mounting PDDB inside Renode without
manual UI input is genuinely hard (multi-layer xous-core bypass).
But on closer look, **we don't need it for any deliverable on the
path to a working Signal client.** The Stage 13b-2 IPC client and
real `PddbBackend` are valid as-is; their target consumer is real
Precursor hardware, where the user types a password once on first
boot and PDDB persists thereafter — the standard Precursor ux.

This report exists to document the realization and redirect the
next stage. No code change.

---

## 1. Why automount isn't needed

Three test surfaces exist; each has its own story:

| Surface | Backend | Why this works |
|---------|---------|----------------|
| Hosted unit tests (~22 + 31 + 3 currently passing) | Mock | Tests don't need persistence — same process lifetime as the test, in-memory state is fine. |
| Renode CI smoke / probe tests | Mock OR real-but-unmounted | Smoke asserts on boot lines (Stage 9b-deploy B). Probes (13a/13b/13b-2) confirm IPC plumbing. None of these need an actually mounted PDDB. |
| Real Precursor hardware deploy | Real PDDB, manually initialized | User goes through PDDB first-boot password modal once; PDDB persists across reboots forever. |

Persistence-across-reboots is only meaningful on real hardware,
where it's already free (PDDB is what xous-core ships). Putting
synthetic persistence into Renode would mean carrying ~150 LoC of
xous-core upstream patches across `services/pddb` and
`services/root-keys` — to test a property that hardware tests
already give us for free.

## 2. The 2× rule

For the rest of Stage 13: testing infrastructure (Renode .resc,
.robot, xtask glue, mock harnesses) should stay under **2× the
project's actual code size**. Today: project is ~7,400 LoC across
the workspace's `crates/`; test infra is ~500 LoC (xtask + Renode
files). The headroom is generous. But it's a useful guideline:
each additional Renode-only piece needs to justify its weight
against actual hardware testing.

What this means in practice:

- **In** Renode: smoke (boot + log lines), the three `probe-*`
  features (network, PDDB, future flow), and trivial Robot tests
  per probe. The work to date.
- **Out** of Renode: comprehensive flow regression tests, mocked
  Signal harness, mocked PDDB persistence — these are 1000s of LoC
  with low ROI for an MVP. Hardware iteration is faster.

## 3. The path to "something usable on Precursor"

The user's plan: "we can flash a few times and just try it."
That's the right call. Here's the punch list:

### 3.1 Code currently in place (sufficient)

- ✓ rv32 image with `xas` registered as launcher app (Stage 9b-deploy B)
- ✓ Real `__getrandom_v03_custom` via xous-core's TRNG (Phase C-1)
- ✓ `xous-net-bridge` with `SyncHttpClient` (Stage 6, untested on hw)
- ✓ Real `PddbBackend` via `xous-pddb-ipc` (Stage 13b-2)
- ✓ `xous-api-log` plumbing reaches UART
- ✓ Worker thread + presage Manager state machine (Stages 8–12)

### 3.2 Code missing for a meaningful hardware test

The current xas binary's `main()` calls
`Ui::new(cmd_tx, event_rx).run()` — a hosted-mode UI loop that
EOFs on stdin and returns within ~1s on Xous. On Precursor
hardware: user clicks "Signal" in the launcher, splash flashes,
app exits. Not a test.

**Smallest patch for a meaningful hardware probe:**

Add an `auto-link` feature flag (~30 LoC) to `xous-app-signal`:

```rust
#[cfg(feature = "auto-link")]
{
    let device_name = "xas-hardware-probe".to_string();
    cmd_tx.send_blocking(Cmd::LinkDevice { device_name }).ok();
    while let Ok(event) = event_rx.recv_blocking() {
        match event {
            Event::LinkUrl(url) => log::info!("xas: link URL = {}", url),
            Event::LinkComplete { aci, phone, .. } => {
                log::info!("xas: linked aci={} phone={}", aci, phone);
                break;
            }
            Event::LinkError(e) => {
                log::warn!("xas: link error: {}", e);
                break;
            }
            other => log::info!("xas: event {:?}", other),
        }
    }
}
```

That's roughly the same shape as the existing `probe-flow` and
`probe-pddb-real` features. With it:

1. Build: `cargo build --target=… --release -p xous-app-signal --features pddb-real,auto-link`
2. Bundle into image via xous-core's xtask
3. Flash via `tools/updater/precursorupdater/precursorusb.py`
4. Boot, click "Signal", watch UART (via JTAG / `xous-debug-cli`)
5. Read the provisioning URL off the UART, scan with the Signal
   phone app
6. Observe whether the link completes

What we learn from one flash, regardless of outcome:

- Did xous-net-bridge's TLS handshake reach Signal's servers? (DNS + WiFi)
- Did the WS provisioning channel open?
- Did getrandom + ML-KEM-1024 + curve25519 produce valid keys?
- Did PDDB write the resulting registration data?

If link succeeds, the whole stack works end-to-end. If it fails,
the UART log narrows the failure to a single layer; iterate from
there. **One or two flash cycles should resolve "does it work?"**.

### 3.3 What we don't need

- **Stage 13c (mock HTTP/WS in Renode).** Skip. The infra cost
  (mocked Signal-style harness with canned WS frames, request-
  matching logic, regression fixtures per flow) easily exceeds
  500 LoC — close to the entire test-infra budget under the 2×
  rule. Hardware iteration is faster and tests the real thing.
- **Stage 13d (u32e backend).** Defer. It's a perf optimization;
  link-once-then-receive flows aren't gated on it.
- **Stage 13e in its previous "comprehensive" framing.** Replaced
  with the §3.2 punch list — the smallest feature-flagged probe
  that exercises the full Signal stack on real hardware.

## 4. Recommended next stage

**Stage 14a — auto-link hardware probe.** Land the feature in
§3.2; produce a flashable rv32 image; document the flash
procedure. ~1 hour of code + ~1–2 hours of hardware-iter
debugging on first flash.

If link succeeds first try: congratulations, the MVP is reachable
via a sequence of similarly small "drive flow X" features. Stage
14b is "auto-receive a single message"; 14c is "auto-send a
single message." Each is feature-flagged, each is one Robot probe
on the hardware side (or just UART log inspection).

If link fails: the diagnosis is in the UART. Likely failure
modes, in rough order of probability:

1. WiFi not configured on the Precursor.
2. Net stack timeout on DNS or TCP — same shape as Stage 13a's
   Renode finding, except now we can actually fix it.
3. TLS handshake — rustls roots vs. Signal's cert chain.
4. WS frame parsing — tungstenite version drift.
5. PDDB write fails — the IPC client hits a real-data path that
   the unmounted-state probes didn't exercise.

Each is a 1-flash-iter to surface and 1-flash-iter to fix.

## 5. ROADMAP update

`docs/ROADMAP.md` updated to:
- State the 2× infra rule on Stage 13.
- Drop Stage 13c from the critical path.
- Replace Stage 13e's comprehensive framing with the §3.2 +
  §4 hardware-deploy punch list.
- Note that Stage 13b-3 was investigated and skipped (no code
  change), with this report as the decision record.

## 6. Files touched

```
M  stage/REPORT-13b-3.md             (this file — replaces the prior
                                       deferred-investigation framing)
M  docs/ROADMAP.md                    (Stage 13 section update)
```

Stage 13b-2's deliverables remain intact and accurate. The IPC
client and real `PddbBackend` are production-ready against real
hardware — they were never a problem; the problem was the framing
that put them in a Renode-CI persistence-test box.
