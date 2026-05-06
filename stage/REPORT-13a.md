# Stage 13a — exploratory network reachability probe

**Date.** 2026-05-06
**Status.** Probe shipped, ran end-to-end in Renode, returned
concrete findings. **Network does not carry outbound traffic from
xas in the baseline Renode emulation.** Recommends Stage 13c "if
network probe fails" branch (mocked HTTP/WS at the transport layer)
as the next step.

---

## 1. What the probe is

A `probe-flow` Cargo feature on `xous-app-signal`. When enabled,
after `run_signal_worker` returns, `main()` calls a `probe_network()`
function that fires three sequential `TcpStream::connect_timeout`
attempts (10s timeout each) against well-known endpoints and logs
the outcome through `xous-api-log` to the SoC's `sysbus.console`.

The three targets are deliberately diverse:

| target | rationale |
|--------|-----------|
| `8.8.8.8:53` | Google DNS over TCP. No DNS lookup needed. Probes "is there a route to any public IP?" |
| `1.1.1.1:443` | Cloudflare HTTPS port. Same shape but a different provider — rules out per-provider filtering. |
| `chat.signal.org:443` | Real Signal endpoint. Requires DNS, so this also probes the Xous `dns` service. |

A new Robot test, `tests/renode/xas-probe.robot`, drives the probe
and `Wait For Line On Uart` for each per-target outcome plus the
final `network probe done` banner. Total wall-clock for the test is
~50 s including boot.

---

## 2. Findings

```
INFO:xas: probe: starting network reachability probe
WARN:xas: probe: google-dns        CONNECT FAIL to 8.8.8.8:53     after 345ms: invalid address
WARN:xas: probe: cloudflare-https  CONNECT FAIL to 1.1.1.1:443    after   4ms: invalid address
WARN:xas: probe: signal-prod       resolve FAIL                   after   9ms: DNS failure
INFO:xas: probe: network probe done
```

Three conclusions:

### 2.1 The Xous net stack rejects external IPs with "invalid address"

Both raw-IP TCP connects (no DNS involved) failed with `invalid
address` — not `connection timeout`, `connection refused`, or `host
unreachable`. That's an *upstream-of-network* failure: the address
parsed cleanly into a `SocketAddr` (it has to, or `to_socket_addrs`
would have failed first), but the Xous `net` service rejected the
connect call. Likely root causes:

- **No interface up.** WF200 wifi peripheral in Renode is emulated as
  a stub (per `emulation/peripherals/WF200.cs` — primarily mock
  responses, not a working wifi-to-internet bridge). Without an
  interface bound to a route, every external destination is "invalid".
- **No route in the route table.** Even if the WF200 peripheral
  reported "associated", we'd need a kernel-side route to forward
  packets to the right interface. The default Xous image doesn't
  configure one for the emulated WF200.

### 2.2 DNS fails the same way

`signal-prod` (chat.signal.org:443) failed at the resolve step with
`DNS failure` after 9ms — fast enough that the lookup never even
reached the `dns` service's external query path. The Xous `dns`
service is itself running (PID 13 in the boot trace), so the `dns`
side of the IPC handshake works; but the resolver's onward query
to a real DNS server has nowhere to go for the same reason as 2.1.

### 2.3 The 345ms vs 4ms vs 9ms latencies are interesting

The Google DNS probe took 345ms — the Cloudflare probe 4ms — the
Signal resolve 9ms. The 345ms is the cold-cache cost of *the first*
connect attempt (likely lazy interface initialization on the Xous
side). Subsequent attempts short-circuit to `invalid address` without
even trying the wire. This pattern matches what's usually observed
on Xous when WF200 isn't actually associated.

---

## 3. What this means for Stage 13 going forward

**Don't try to fix WF200 emulation.** Two reasons: (a) it's xous-core
+ Renode-side work (out of our standalone workspace's scope), and
(b) even with a working wifi stub, we'd need an emulated wifi
gateway in Renode to bridge to actual public endpoints — that's
hosted-Linux-side TUN/TAP infrastructure that takes its own
sub-stage to land.

**Take the Stage 13c "If 13a network probe fails" branch.** Mock
the HTTP/WS transport at the `libsignal_service::transport::HttpClient`
trait level. The Stage 6 `xous-net-bridge::SyncHttpClient` already
implements that trait; replacing it with a `MockHttpClient` (or a
recording-replay client driven by JSON fixtures) under a feature
flag lets us drive the link/receive/send flows in Renode against
canned responses. From the binary's perspective the call surface is
identical — it goes through `transport::set_http_client(...)`.

**Concrete next step (Stage 13b-prep, before doing 13b/13c).**
Sketch a `MockHttpClient` in `xous-net-bridge` (or a new
`xous-mock-transport` crate) that:

- Takes a routing table: `(method, url-prefix) → fixture`.
- Returns canned `HttpResponse` for matched requests.
- Logs every request that doesn't match (so we know what fixtures
  to add as we hit new flows).

For the link flow specifically, the fixture set is small (per
`docs/CALL_GRAPH.md`):

- `GET /v1/registration` → 401 with sealed-sender cert
- `PUT /v1/devices/...` → 200 with provisioning code
- WS `/v1/websocket/provisioning/...` → canned device-link envelope

Stage 13c can then use this harness to run an actual `Cmd::LinkDevice`
end-to-end and verify the worker emits `Event::LinkUrl` and
`Event::LinkComplete` against the canned harness.

---

## 4. Files touched

```
M  crates/xous-app-signal/Cargo.toml         (probe-flow feature added)
M  crates/xous-app-signal/src/main.rs        (probe_network() function +
                                              call site under probe-flow)
A  tests/renode/xas-probe.robot              (probe test)
M  docs/ROADMAP.md                           (Stage 13 spec added,
                                              including 13a–13e
                                              sub-phases)
A  stage/REPORT-13a.md                       (this file)
```

The probe code is a single ~70-line function gated behind a
default-off feature; the production smoke build is unaffected.

---

## 5. Verification

```
cargo build --target=riscv32imac-unknown-xous-elf --release \
            -p xous-app-signal --features probe-flow      → ok (1m01s)
cp .../xas dist/xas-rv32/xas
cd ~/precursor-signal/repos/xous-core
cargo xtask app-image \
    xas:.../dist/xas-rv32/xas --git-describe v0.9.21-0-g0000000  → ok
cd ~/precursor-signal/xous-app-signal
renode-test tests/renode/xas-probe.robot      → PASS (49 s)
renode-test tests/renode/xas-smoke.robot      → still PASS (44 s)
                                                (after restoring the
                                                 non-probe build)

cargo test --workspace                                    → 22+31+3 passed
cargo clippy --workspace --all-targets -- -D warnings     → clean
cargo clippy -p xous-app-signal --all-targets \
             --features probe-flow -- -D warnings         → clean
cargo fmt --all -- --check                                → clean
```

---

## 6. Stage 13 phasing — updated

| sub-phase | status |
|-----------|--------|
| 13a — Probe | **landed** (this commit) — finding: network unreachable, mock transport recommended |
| 13b — Real PDDB | scoped, not started |
| 13c — Real flows (mocked transport) | scoped; 13b prerequisite |
| 13d — u32e backend | scoped, deferred (per Phase C-3) |
| 13e — Physical hardware | scoped; 13c+13b+13d prerequisite |

Recommend executing 13b (real PDDB) and the new 13b-prep
(MockHttpClient sketch) before 13c — they're independent and can
land in parallel sessions.
