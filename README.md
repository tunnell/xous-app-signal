# xous-app-signal (`xas`)

Unofficial Signal client for [Xous](https://github.com/betrusted-io/xous-core)
on [Precursor](https://www.crowdsupply.com/sutajio-kosagi/precursor).

**Prototype. Not for production use.**

Built on the Whisperfish Rust stack —
[`presage`](https://github.com/whisperfish/presage),
[`libsignal-service-rs`](https://github.com/whisperfish/libsignal-service-rs),
and [`signalapp/libsignal`](https://github.com/signalapp/libsignal) — rather
than reimplementing the Signal protocol from primitives. The on-device binary
is named **`xas`** ("**X**ous **a**pp **s**ignal").

The driving project value is end-user verifiability: a user who buys a
Precursor should be able to read every line of Rust that ends up on their
device. The design therefore leans on upstream community-maintained code
(reused as-is or with small reviewable patches) and minimizes bespoke
Xous-specific glue.

---

## Why a Signal client on Precursor

Smartphones are the primary target for surveillance of journalists and human-rights workers. The [Pegasus Project](https://forbiddenstories.org/about-the-pegasus-project/) — coordinated by Forbidden Stories with forensic support from [Amnesty International's Security Lab](https://securitylab.amnesty.org/case-study-the-pegasus-project/) — documented commercial spyware on devices belonging to journalists, activists, and dissidents in [over 50 countries](https://www.eff.org/deeplinks/2026/04/digital-hopes-real-power-how-arab-spring-fueled-global-surveillance-boom). Zero-click exploits like [BLASTPASS](https://securitylab.amnesty.org/latest/2023/12/india-damning-new-forensic-investigation-reveals-repeated-use-of-pegasus-spyware-to-target-high-profile-journalists/) install spyware without the target tapping anything. End-to-end encryption in apps like Signal is necessary but insufficient when the host operating system itself is compromised — once Pegasus is on the phone, [it can read messages and turn on the microphone and camera regardless of which app you used](https://forbiddenstories.org/about-the-pegasus-project/).

[Precursor](https://www.crowdsupply.com/sutajio-kosagi/precursor) is an open-hardware mobile device built around the principle of [evidence-based trust](https://www.bunniestudios.com/blog/2022/precursor-from-boot-to-root/): every layer from the FPGA bitstream up through the [Xous microkernel](https://betrusted.io/) is inspectable and reproducible, and Precursor [generates and seals its own keys](https://www.bunniestudios.com/blog/?p=5979) without relying on factory secrets. Until now most Precursor applications have focused on credential storage (password managers, FIDO/U2F, cryptocurrency wallets). This project fills the missing piece — a hardened **communications** device — by porting Signal's secondary-device protocol to Xous so that messages a journalist or activist sends from their pocket are protected by hardware they can audit themselves.

**Short-term goal**: a hardened communications device for journalists and activists who already have access to Precursor.

**Long-term goal**: a cheaper successor (the [Betrusted](https://betrusted.io/) ASIC currently in development) that puts this same threat model in reach of users in the Global South, where surveillance pressure is highest and where activists and journalists [most often lack resources to recover from compromise](https://www.amnesty.org/en/latest/news/2024/12/serbia-authorities-using-spyware-and-cellebrite-forensic-extraction-tools-to-hack-journalists-and-activists/).

**Scope tradeoff**: Precursor's hardware constraints (memory budget, no GPU, single-screen monochrome display) mean this client implements a deliberately minimal Signal feature set: secondary-device link, send/receive of text messages. Group calls, video, stickers, message-history transfer, and other features that depend on richer hardware or large state are out of scope. The goal is "your most sensitive conversations on hardware you can audit," not "feature parity with the mobile app."

---

## Status

| Capability                                | Hosted (Linux X11)     | Precursor hardware                       |
|-------------------------------------------|------------------------|------------------------------------------|
| Link as secondary device (QR scan)        | working                | **working**                              |
| Persistence across kernel restarts (PDDB) | working                | working                                  |
| Auto-reload registration on boot          | working                | working                                  |
| Receive 1:1 text messages                 | working                | **working**                              |
| Send 1:1 text messages (UUID or e164)     | working                | broken — `"WebSocket closing while waiting for a response"` (likely WS keepalive not firing on rv32) |
| Contact name / phone display              | wired, lightly tested  | wired, lightly tested                    |
| Wi-Fi onboarding from inside xas          | n/a (host stack)       | not yet — use `wlan join` from shellchat |
| Group messaging / attachments / calls     | out of scope (for now) | out of scope (for now)                   |

Hardware bring-up is in progress. **Milestones reached on real
Precursor:** boot to PDDB unlock; DNS resolution including CNAME chains;
TLS to chat.signal.org against the pinned CA; WebSocket upgrade;
provisioning-UUID receive + QR render on the physical LCD; phone-side
QR scan; ProvisionEnvelope decrypt on rv32 (Curve25519 + AES-CBC + HMAC);
prekey generation (signed Curve25519 + Kyber-1024 last-resort);
`POST /v1/devices/link`; `LinkComplete` + persistence to PDDB; inbound
Signal-protocol message decrypt + display on xas's inbox screen.

**The remaining gap is send.** First send fails with a libsignal-service
error: `"WebSocket closing while waiting for a response"`. The most likely
cause (per the project's `linking.md` field guide) is that the WebSocket
keepalive isn't firing on rv32, so Signal's server idle-closes the auth
WS while xas is still waiting to use it. The current build catches that
error cleanly and surfaces it in the UI; root-cause fix is parked in
`../CHORES.md`.

The same code path runs in hosted-mode emulation on Linux for fast UI
iteration; every UI primitive talks to the GAM service via Xous IPC and is
target-agnostic.

For full state, in-flight builds, and how to continue, see the docs in this
repo's parent directory:
- `OVERVIEW.md` — landing page with current state at a glance.
- `STATE.md` — what's working / broken / in-flight, plus build + flash commands.
- `UI-DESIGN.md` — comprehensive plan for the next UI redesign (conversation-list home).
- `CHORES.md` — deferred follow-ups (Wi-Fi onboarding rework, dep audits, send fix).

---

## Quickstart (hosted)

These instructions exercise the link → receive → send loop on a Linux dev
machine using xous-core's hosted-mode emulator, without flashing real
hardware. Two real Signal accounts are required.

### Prerequisites

- Linux x86_64, an SSH session with X11 forwarding (or a local desktop).
- Rust 1.95+ via rustup.
- A working X11 display (`xset q` returns instantly).
- Two Signal accounts: one with the
  [Signal Android/iOS app](https://signal.org/), one with
  [signal-cli](https://github.com/AsamK/signal-cli) installed and linked
  as a secondary on the second account. signal-cli is your test peer.
- A clone of `betrusted-io/xous-core` (or a compatible fork) at
  `../repos/xous-core` relative to this checkout.

### Build and run

```sh
# 1. Clone next to a xous-core checkout:
mkdir signal && cd signal
git clone https://github.com/betrusted-io/xous-core repos/xous-core
git clone <this repo> xous-app-signal

# 2. Build the hosted xas binary:
cd xous-app-signal
cargo build --release -p xous-app-signal --features pddb-real,hosted

# 3. Boot xous-core hosted with xas bundled. Run from xous-core's root:
cd ../repos/xous-core
cargo xtask run xas:../../xous-app-signal/target/release/xas
```

Once an X11 window labelled "Precursor" appears, navigate launcher → Apps
→ xas, pick "Link device", and accept the default device-name. A QR code
will appear; scan it from the Signal app on your phone.

### Headless link verification

`tests/hosted/test_link_qr.sh` drives the boot → launcher → xas → Link
sequence headlessly and gates on the provisioning URL appearing in the
kernel log. Use it as a smoke test:

```sh
INSPECT_HOLD=900 bash tests/hosted/test_link_qr.sh
```

The `INSPECT_HOLD` env var keeps the kernel alive for the given seconds
after the QR appears so you can scan from your phone.

### End-to-end receive + send

After linking, your other Signal account can send messages to xas. The
kernel log will show:

```
xas/gam_app: inbound message from <name-or-phone-or-uuid> (N bytes)
```

To send back, navigate to xas Menu → Send. Recipient accepts either a UUID
(ACI) or an e164 phone number (`+15551234567`); the latter is resolved
against the contact list synced from the linked phone.

---

## Hardware deploy

Hardware build, flash, and test commands are documented in
[`../STATE.md`](../STATE.md). Summary:

```sh
# Build the rv32 xas binary:
cd xous-app-signal && cargo xtask dist

# Bundle into a signed kernel image (apps + a few services XIP'd
# from flash to free RAM; kernel needs big-heap for libsignal):
cd ../xous-core && cargo xtask app-image-xip \
    xas:../xous-app-signal/dist/xas-rv32/xas vault \
    --kernel-feature big-heap \
    --git-describe v0.9.8-791-gc707f9d8 --git-rev c707f9d8

# Flash kernel-only (recoverable, ~25 min):
python3 tools/usb_update.py \
    -k target/riscv32imac-unknown-xous-elf/release/xous.img --bounce
```

Pre-flash, confirm the Precursor is in the loader window
(`lsusb | grep 1209` should show `1209:5bf0`, not `1209:3613`).

`big-heap` is a xous-core kernel feature that raises the per-process heap
cap from 512 KiB to 12 MiB; xas's libsignal+presage+rustls+smol working set
exceeds the default cap during link. App-image-XIP keeps app and large
service code in flash rather than RAM, freeing ~5 MiB on the 16 MiB SoC.

Wi-Fi must be configured via shellchat before launching xas; in-app
onboarding is deferred (see `../CHORES.md`). The sequence that has
worked reliably in practice:

```
wlan off
wlan on
ssid scan
wlan status     # repeat until it shows "connected"
```

`wlan off` first resets any half-associated state from a previous boot;
`wlan on` powers the radio; `ssid scan` triggers association against
the SSID/PSK already saved on the EC; `wlan status` is the readiness
gate. If `wlan status` never reports connected, the SSID/PSK may need
to be (re)set via `wlan setssid <name>` + `wlan setpass <pw>` (one-time;
the EC remembers them across reboots).

---

## Layout

```
xous-app-signal/
├── crates/
│   ├── presage-store-pddb/     storage trait impls over PDDB
│   ├── xous-net-bridge/        sync TLS + WS pump + channel bridge
│   ├── xous-pddb-ipc/          hand-rolled PDDB IPC client
│   ├── xous-modals-ipc/        hand-rolled modals IPC client
│   ├── xous-signal-bridge/     Manager-on-worker + IPC forwarder
│   ├── xous-app-signal/        binary entry point (binary name: `xas`)
│   └── xous-app-signal-ui/     stdin-driven UI (legacy; gam_app.rs is current)
├── docs/                       design docs (REPORT, CALL_GRAPH, ROADMAP)
├── stage/                      per-stage execution reports
├── tests/hosted/               headless link/receive harness
└── vendor/                     vendored forks of presage / libsignal-service-rs / curve25519-dalek
```

---

## Tests

```sh
cargo test --workspace --features pddb-real
```

22+ unit tests covering the PDDB store traits, plus the headless link test.

---

## License

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

This project is dual-licensed under the terms of the AGPL 3.0 license, as
a derivative work; and under the terms of the Apache 2.0 license.
`SPDX-License-Identifier: AGPL-3.0 OR Apache-2.0`

You can choose between one of them if you use this work.
* [AGPLv3.0](https://www.gnu.org/licenses/license-list.html#AGPLv3.0)
* [Apachev2.0](https://www.apache.org/licenses/GPL-compatibility.html)

We have a **desire** to license xas under Apache-2.0 so that elements may
be readily incorporated into other future
[Xous](https://github.com/betrusted-io/xous-core) related projects.
We are **required** to license any derivative works of
[libsignal](https://github.com/signalapp/libsignal) under the AGPL-3.0 license.
