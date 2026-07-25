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

**Scope tradeoff**: Precursor's hardware constraints (16 MiB
total RAM, no GPU, single-screen monochrome 336×536 display, no
audio/video hardware) mean this client implements a deliberately
minimal Signal feature set. The goal is "your most sensitive
conversations on hardware you can audit," not "feature parity
with the mobile app." The feature support matrix below makes the
tradeoffs explicit.

---

## Feature support

Verified working on Precursor PVT2 hardware unless noted. Each
capability that maps to a Signal app feature is listed; the third
column says whether the gap is fundamental (hardware can't do it)
or a roadmap item (planned but not yet built).

### Messaging

| Feature | Status | Note |
|---|---|---|
| Link as a secondary device (QR scan) | ✅ | Boot, PDDB unlock, QR scan, decrypt ProvisionEnvelope, register, persist |
| Receive 1:1 text messages | ✅ | Near-instant once linked. Verified on hardware (2026-05-11) receiving DMs from multiple distinct senders. |
| Send 1:1 text messages | ✅ | Latency 1–4 min — Signal edge-server WS rotation race; transport refactor on roadmap |
| Conversation list (Home) | ✅ | Per-thread last-message + relative timestamp + unread indicator |
| Per-thread message view | ✅ | Optimistic-render compose; auto-mark-read on thread open |
| Group chats (read or write) | ❌ | Roadmap. Adds ~1 MiB of state per group + UI surface |
| Disappearing messages | ❌ | Body still displays but no timer indicator; xas doesn't honor the expire_timer (no auto-delete). Roadmap |
| Typing indicators | ❌ | Roadmap. Low priority |
| Read receipts (sending) | ❌ | Auto-mark-read on thread open updates UI state only; xas doesn't call any send-receipt API. Roadmap |
| Stories | ❌ | Out of scope. Built around media |

### Media + attachments

| Feature | Status | Note |
|---|---|---|
| Image / video / file attachments (send or receive) | ❌ | Out of scope. Display + storage budget too small for media-first UX |
| Voice notes | ❌ | Hardware: no microphone codec wired through Xous |
| Stickers | ❌ | Out of scope. Display is monochrome |
| Emoji reactions | ❌ | Inbound reactions arrive as DataMessages with empty body and are silently dropped at process_received. No outbound UI either. Roadmap |

### Calling

| Feature | Status | Note |
|---|---|---|
| Voice calls | ❌ | Hardware: no audio path on Precursor |
| Video calls | ❌ | Hardware: no camera, no codec |

### Account + profile

| Feature | Status | Note |
|---|---|---|
| Display name + phone number on Profile | ✅ | Read from registration data |
| Profile editing (name / picture / about) | ❌ | Roadmap. Read-only today |
| Username (`@alice.42`) on Profile | ❌ | No API to read one's own Signal username in our build (RegistrationData has no username field; Profile struct has no username field). The primary phone holds that state |
| Username lookup in "New chat" | ✅ | F1 → enter `name.42` → presage's `lookup_username` resolves to ACI; UI opens a Thread |
| Phone-number lookup in "New chat" | ❌ | Needs CDSI which requires boring-sys (BoringSSL) — disabled in this build because it can't target rv32-xous |
| Logout | ✅ | Settings → Logout: confirmation modal, then the worker wipes link state from the PDDB (`Cmd::Logout`) and the UI returns to the pre-link menu |
| Multiple linked accounts | ❌ | Single-account device by design |
| Primary registration (this device IS the primary) | ❌ | Out of scope. Secondary-device only — your phone stays primary |

### Hardware integration

| Feature | Status | Note |
|---|---|---|
| Wi-Fi (2.4 GHz only — Precursor is single-band) | ✅ | Configured via shellchat (`wlan off; wlan on; ssid scan; wlan status`); no in-app onboarding |
| Hardware-rooted key sealing (PDDB) | ✅ | Inherited from Xous; xas writes registration + sessions through `presage-store-pddb` |
| Sealed sender (Signal Protocol) | ✅ | Handled transparently by libsignal-service-rs |
| PQXDH (post-quantum key agreement) | ✅ | libsignal default; verified working on rv32 |

### Things handled by upstream code (not xas's own logic)

xas leverages the Signal Protocol implementation in
[`signalapp/libsignal`](https://github.com/signalapp/libsignal)
(via [`libsignal-service-rs`](https://github.com/whisperfish/libsignal-service-rs)
and [`presage`](https://github.com/whisperfish/presage)). Cryptographic
primitives — Double Ratchet, X3DH/PQXDH, sealed sender, prekeys,
identity keys — are **not reimplemented** here. See
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for what xas adds
on top of those libraries (mostly: Xous IPC plumbing, a
sync-blocking-on-async transport bridge, and the GAM-rendered
UI).

---

## Where to learn more

- [`BUILDING.md`](BUILDING.md) — clone-to-running instructions
  for both hosted-mode emulator and Precursor hardware paths
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — how xas
  works, written for a Rust developer with passing crypto
  knowledge
- [`tests/README.md`](tests/README.md) — overview of the four
  testing approaches (unit / hosted / Renode / Precursor),
  pros and cons, and the dev/main branch convention
- [`tests/precursor/README.md`](tests/precursor/README.md) —
  hardware-test workflow: build, flash, watch UART (read this
  BEFORE running any flash command — its "Brick prevention"
  section is non-negotiable)

---

## Building and testing

End-to-end build instructions for both the **hosted-mode**
emulator (Linux x86_64, no hardware needed) and the **Precursor
hardware** path live in [`BUILDING.md`](BUILDING.md). It's
written to be followed cold — clone, set up toolchain, build,
run, link to a Signal account.

For testing, see [`tests/README.md`](tests/README.md), which
compares the four available approaches (unit tests / hosted /
Renode / Precursor) with a pros-cons table so you can pick the
right one for the change at hand. Hardware-test scripts (build,
flash, watch UART) and the brick-prevention rules live in
[`tests/precursor/README.md`](tests/precursor/README.md) — read
it before running any flash command.

---

## Upstream patches

xas tracks three upstream fixes. Status as of 2026-07-25: **#1 and
#3 below are merged upstream**; #2 was **closed unmerged** by its
author on 2026-07-18, so its fix stays vendored. The
pinned `xas-v0.2` branch of [`tunnell/xous-core`](https://github.com/tunnell/xous-core)
and the vendored copy of `libsignal-service-rs` carry whatever has
not yet reached a release xas builds against.

1. **`betrusted-io/xous-core` net-service encoding fix** —
   [betrusted-io/xous-core#877](https://github.com/betrusted-io/xous-core/pull/877),
   **merged upstream 2026-06-02** (commit `2005a801c`).
   The kernel writes `NetError` codes at byte 4 of the response
   buffer; the Rust stdlib's Xous backend reads from byte 1 in
   the recv path. The mismatch made `ErrorKind::TimedOut`
   unreachable from `TcpStream::recv` — fatal for any
   long-lived WS that uses `set_read_timeout` to interleave
   reads and writes. The fix mirrors the code at byte 1 too.
   Any `betrusted-io/xous-core` checkout at or after `2005a801c`
   carries it. `BUILDING.md` still instructs you to clone the
   pinned `xas-v0.2` branch of `tunnell/xous-core`, which carried
   the identical commit pre-merge — the pin remains for the other
   deltas it holds (CNAME-chain DNS fix, `services/net` reaper fix
   from tunnell/xous-core#26 — filed upstream as
   [betrusted-io/xous-core#880](https://github.com/betrusted-io/xous-core/pull/880),
   closed unmerged pending the upstream Renode-CI net refactor —
   a hosted-mode PDDB tweak, and the
   `apps/manifest.json` registration for xas).
2. **`whisperfish/libsignal-service-rs` keepalive tolerance** —
   [whisperfish/libsignal-service-rs#431](https://github.com/whisperfish/libsignal-service-rs/pull/431)
   (closed unmerged 2026-07-18).
   Upstream closes the WS the moment any keepalive is
   outstanding. Under any non-zero scheduling jitter (e.g.
   rv32) this races and closes healthy connections. The PR
   proposed an opt-in `with_max_outstanding_keepalives(...)`
   constructor so callers like xas could tolerate the race
   without changing default behavior for other consumers. The
   patch lives in `vendor/libsignal-service-rs/` in this repo as
   a constant `MAX_OUTSTANDING_KEEPALIVES = 3` (semantically
   equivalent for our use); with the PR closed, the vendored
   constant is the long-term shape — no upstream re-alignment is
   pending.
3. **`rust-lang/rust` Xous std-side recv encoding** —
   [rust-lang/rust#156414](https://github.com/rust-lang/rust/pull/156414),
   **merged**. The long-arc fix that makes #1 unnecessary at the
   std level — change the recv decode to read byte 4 (matching
   the send decode). Once the fix propagates to the stable Rust
   release the toolchain pin uses, the byte-1 mirror from #1
   becomes belt-and-suspenders rather than load-bearing.

With #1 and #3 merged and #2 closed, no upstream merge is
pending. The keepalive tolerance remains a vendored delta,
tracked in `vendor/libsignal-service-rs.diff`.
BUILDING.md keeps pointing at the pinned `xas-v0.2` fork branch
until a future xas release re-pins against an upstream
`betrusted-io/xous-core` that includes `2005a801c` and a
resolution for the reaper fix (#880, closed unmerged pending
the upstream Renode-CI net refactor).

---

## Layout

```
xous-app-signal/
├── crates/
│   ├── presage-store-pddb/     storage trait impls over PDDB
│   ├── xous-net-bridge/        sync TLS + WS pump + channel bridge
│   ├── xous-pddb-ipc/          hand-rolled PDDB IPC client
│   ├── xous-signal-worker/     presage::Manager on worker thread + Cmd/Event channels
│   ├── xous-app-signal/        binary entry point (binary name: `xas`)
├── docs/                       ARCHITECTURE.md (reader's-eye-view of the codebase)
├── tests/                      hosted-mode + Renode + precursor (hardware) test harnesses
└── vendor/                     vendored forks of presage / libsignal-service-rs / curve25519-dalek
```

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

---

## Contributing — including AI-assisted contributions

Contributions are welcome via pull request against the `dev`
branch. See [`tests/README.md`](tests/README.md) for the
branch convention and what release-cycle gates a PR has to
pass before it can land in `main`.

**AI-assisted contributions are explicitly welcome**, on one
condition: **disclose them**. AI coding agents have been used
in this codebase's development, and the project is honest about
that — both because end-user verifiability is a stated value and
because reviewers benefit from knowing where to look harder.

Concretely:

- If you used an AI agent to help write the diff, mention it in
  the PR description. A short note is fine — "drafted with an AI
  agent and reviewed line-by-line" or similar. No need to name
  the specific tool.
- The author of the commit is still you. AI agents are tools,
  not co-authors. Don't add AI-attribution trailers to commit
  messages.
- Apply the same review discipline you'd apply to any code: the
  PR is your work in the sense that you're vouching for it.
  Read every line you submit.

The reason for the disclosure norm is alignment with the
project's threat model: users of this client need to be able to
audit it. Knowing which sections were AI-assisted lets reviewers
weight their attention.
