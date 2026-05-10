# Building xas

Two paths are supported, **independently**:

1. **Hosted mode** — runs xas inside a Linux X11 emulator of the
   Xous kernel. Best for UI iteration, unit testing, and
   reproducing logic bugs without flashing a device. ~10 min to
   first-run; needs no special hardware.
2. **Hardware mode** — builds a signed kernel image for a
   Precursor PVT2 device and flashes it over USB. Best for
   exercising the actual rv32 net stack and Signal-server
   round-trips. ~30 min to first-flash; needs a Precursor and a
   USB cable (a Raspberry Pi with the betrusted debug HAT is
   strongly recommended for reliable flashing + UART logging).

Both paths share the same source tree. You only need to set up
the parts you intend to use.

This document is meant to be followed end-to-end with no prior
context. If a step doesn't work as written, the document is
wrong — please open an issue.

---

## 0. Prerequisites

### Required for both paths

- **Rust toolchain ≥ 1.95**, installed via [rustup](https://rustup.rs).
  The exact version is pinned by the workspace's `rust-toolchain.toml`;
  rustup will install it automatically on first `cargo` invocation.
- **Git** ≥ 2.30.
- **A working C compiler and pkg-config** (for some transitive
  build deps). On Debian/Ubuntu: `apt install build-essential
  pkg-config libssl-dev`.
- **Python 3.8+** (used by the Xous flash tool, even on hosted
  builds — it's also part of the `xtask app-image-xip` pipeline).
- **~10 GB free disk space** for build artifacts.

### Required for hardware path only

- The **Xous Rust target sysroot** for `riscv32imac-unknown-xous-elf`.
  See section 1.5 for what this is, why it's required, and how to
  install it. The summary: `riscv32imac-unknown-xous-elf` is a Rust
  tier-3 target — the compiler knows the target name but the std
  library binaries are not shipped via rustup. You need to install
  them once before you can cross-compile xas. **`rustup target add
  riscv32imac-unknown-xous-elf` is NOT enough by itself** — see
  section 1.5 for the actual install path.
- A **Precursor PVT2** (the RISC-V hardware device).
- A **USB-C cable** that supports data (not power-only) — to
  connect Precursor to your build host (or to a Raspberry Pi).
- **Optional but strongly recommended:** a **Raspberry Pi 4B**
  with the [betrusted debug HAT](https://www.crowdsupply.com/sutajio-kosagi/precursor)
  for reliable flashing and continuous UART log capture. The Pi
  approach is what's documented here; flashing directly from the
  build host works the same way (`python3 tools/usb_update.py`)
  but you lose the persistent UART log that is invaluable when
  things go wrong.

### Required for hosted path only

- **An X11 display** — `xset q` should return without error.
  Over SSH, use `ssh -X` or `ssh -Y`. On a headless box, install
  `xvfb` and run hosted commands under `xvfb-run` (the
  smoke-test in §2.5 documents the exact wrapper).
- **A real Signal account** to link from (only needed for the
  manual flow in §2.4 — the headless smoke-test in §2.5 stops
  at QR generation and does not need a phone).
- **`signal-cli`** installed on the same machine, used as the
  test peer for sending/receiving messages.
  ([install instructions](https://github.com/AsamK/signal-cli#installation))
- For the §2.5 headless smoke-test only: `xdotool` and `xvfb`.
  On Debian/Ubuntu: `apt install xdotool xvfb`.

---

## 1. Clone the source

xas depends on a forked `xous-core` because two upstream bugs
need to be patched for the Wi-Fi + WebSocket path to work
reliably. Use our forks (or apply the patches in
the README's Upstream patches section to upstream).

The forks below are the published source of truth — pull `main`
for the latest released code; pull `dev` if you want the
in-progress branch. (See `tests/README.md` for the
`main` vs `dev` convention.)

```sh
mkdir -p ~/code/xas && cd ~/code/xas

# xas itself (default branch is 'main' = released code)
git clone https://github.com/tunnell/xous-app-signal.git

# xous-core (kernel + services). The xas branch carries the
# net-service encoding fix described in the README's Upstream
# patches section.
git clone --depth 1 -b xas https://github.com/tunnell/xous-core.git

# xous-app-signal's workspace Cargo.toml uses paths like
# `../repos/xous-core/...`, i.e. relative to xous-app-signal's
# *parent*. So the symlink lives at the parent level — not inside
# xous-app-signal/. (Putting it inside xous-app-signal/repos/ as
# earlier revisions of this doc said is a no-op: cargo never
# looks there.)
mkdir -p repos
ln -s ../xous-core repos/xous-core
```

Verify the layout:

```
~/code/xas/
├── repos/
│   └── xous-core -> ../xous-core      # path used by Cargo manifests
├── xous-app-signal/
│   ├── Cargo.toml
│   ├── crates/
│   └── vendor/                        # vendored presage + libsignal-service-rs
└── xous-core/
    ├── kernel/
    ├── services/
    └── xtask/
```

`repos/xous-core` must point at the same checkout you cloned as
`~/code/xas/xous-core`. Verify with `ls repos/xous-core/services/trng/Cargo.toml`
— that file must exist, otherwise the very first crate cargo tries
to load (`crates/xous-modals-ipc`) will fail with `failed to read
'.../repos/xous-core/services/trng/Cargo.toml'`.

---

## 1.5. Install the Xous Rust target sysroot (hardware path only)

**Skip this section if you only intend to use hosted mode.** It
applies only when you'll cross-compile xas for the Precursor's
`riscv32imac-unknown-xous-elf` target.

### Why this is a separate step

When you build a Rust crate for a target, the compiler needs a
precompiled `std` (and friends — `core`, `alloc`,
`compiler_builtins`, etc.) that's been built for that target.
Rust's release process distributes these for "tier-1" and
"tier-2" targets via `rustup target add`. **`riscv32imac-unknown-xous-elf`
is a tier-3 target**, which means the Rust project supports the
target *in source* but doesn't build or distribute the std
binaries through rustup.

You can confirm this by running:

```sh
rustup target add riscv32imac-unknown-xous-elf
```

You'll see:

```
warn: skipping unavailable component rust-std for target
      riscv32imac-unknown-xous-elf
```

The target is "added" (recognized), but the precompiled
`rust-std` component isn't downloaded — it doesn't exist as a
published rustup component. Try to build now and you'll get
errors like `error[E0463]: can't find crate for `core``.

You have to install the sysroot some other way.

### How to install it (recommended path)

The cleanest install is via xous-core's xtask, which on a fresh
box ends up running `rustup target add` (with the same warning)
and **also** has historically grabbed a precompiled sysroot from
the Xous community's distribution channel. The exact mechanism
depends on the xous-core revision.

```sh
cd ~/code/xas/xous-core
cargo xtask install-toolkit
```

This runs non-interactively (no prompt). The first run takes a few
minutes (downloads + unpacks std + companion crates). Pass `--force`
to overwrite an existing sysroot, e.g. after a Rust toolchain bump.

To verify success:

```sh
ls ~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/riscv32imac-unknown-xous-elf/lib/ \
    | grep '^libstd-'
```

You should see one or more `libstd-*.rlib` files. If the
directory is empty or doesn't exist, the install didn't take —
fall back to the build-from-source path below.

### Fallback: build the sysroot from source

If `cargo xtask install-toolkit` doesn't successfully populate the
sysroot (the project ships a stale `docs/build-rust-sysroot.sh`
which is only correct for the pre-Rust-1.55 source tree layout
and will not work as-is), you have two options:

**Option A — `-Z build-std`** (works on any nightly-capable
Rust). Add `-Z build-std=std,panic_abort` to your cargo
invocations. This rebuilds std from source on every clean build,
which is slow (~5 min added to each cold build) but works
without any extra setup beyond `rustup component add rust-src`.
You may need to also `RUSTC_BOOTSTRAP=1 cargo ...` if your
toolchain is stable rather than nightly.

**Option B — manually build the sysroot** from the Xous Rust
fork (`xous-os/rust`). The script that does this is in
`xous-core/docs/build-rust-sysroot.sh`, but it's stale: it
references the pre-1.55 `src/libcore`, `src/liballoc`, `src/libstd`
layout. To make it work today, you'd need to update the paths
to `library/core`, `library/alloc`, `library/std` and possibly
chase down a few related changes. Allow ~1-2 hours.

If you find yourself reaching for option B, please open an issue
at <https://github.com/tunnell/xous-app-signal/issues> — the
goal is for option A to "just work" via `cargo xtask
install-toolkit` and the manual fallback to be unnecessary.

### Why this isn't easier

Three things conspire to make this awkward:

1. The Rust project hasn't promoted `riscv32imac-unknown-xous-elf`
   to tier-2. That would require ongoing maintainer commitment to
   keep the target buildable on every Rust release. The Xous
   community has chosen to keep the target tier-3 (lower
   maintenance burden) for now.
2. Tier-3 targets don't get precompiled rust-std distributed via
   rustup — every project that uses one has to bootstrap a
   sysroot somehow.
3. The Xous community could publish a prebuilt sysroot tarball
   per Rust release as a workaround (Option C, not yet done) —
   that would make the install one-line: download, extract,
   point cargo at it. We've filed this as an upstream improvement
   suggestion; until then, the bootstrap dance above is the path.

## 2. Hosted-mode path (Linux x86_64, no hardware)

### 2.1 Bootstrap `services/gam/src/apps.rs` (one-time, both paths)

`xous-core/services/gam/src/apps.rs` is **auto-generated and
gitignored**. It's normally written out by xous-core's xtask
(`generate_app_menus()` in `xtask/src/app_manifest.rs`) as a side
effect of `cargo xtask app-image-xip` / `cargo xtask run`. A fresh
checkout has no `apps.rs`, so a standalone `cargo build` against
`gam` fails with:

```
error[E0583]: file not found for module `apps`
  --> .../repos/xous-core/services/gam/src/lib.rs:11:1
```

xas references `gam::APP_NAME_XAS`, so the generated stub has to
include `xas` in its apps list (`cargo xtask dummy-template`
generates an empty stub that compiles `gam` but then fails the xas
link with `error[E0425]: cannot find value APP_NAME_XAS`).

Easiest workaround until xtask grows a proper "manifest-for &lt;app&gt;"
verb: write the file by hand, once. Run from `~/code/xas/`:

```sh
cat > xous-core/services/gam/src/apps.rs <<'EOF'
#![cfg_attr(rustfmt, rustfmt_skip)]
// Hand-written stand-in for xtask-generated apps.rs.
// Will be overwritten by `cargo xtask app-image-xip` (hardware path).
pub const APP_NAME_XAS: &'static str = "Signal";

pub const EXPECTED_APP_CONTEXTS: &[&'static str] = &[
    APP_NAME_XAS,
];
EOF
```

This file is overwritten by `cargo xtask app-image-xip` during
the hardware build (with both `xas` and `vault` entries) — that's
fine; the hand-written version is only needed for the standalone
hosted `cargo build` in 2.2.

### 2.2 Build

```sh
cd ~/code/xas/xous-app-signal
cargo build --release -p xous-app-signal --features pddb-real,hosted
```

First build downloads ~500 MB of crates and takes 5–15 minutes
on a recent laptop. The output binary is at
`target/release/xas`.

### 2.3 Run hosted Xous with xas bundled

From the **xous-core** directory:

```sh
cd ~/code/xas/xous-core
cargo xtask run xas:../xous-app-signal/target/release/xas
```

This boots a Linux process that emulates the full Xous kernel
plus all required services, with xas bundled as the application.
A small minifb window labelled "Precursor" appears.

### 2.4 First-run flow inside the hosted window

1. **Unlock PDDB**: enter any password the first time (it
   bootstraps a fresh encrypted store).
2. **Open xas**: from the launcher, navigate to Apps → xas.
3. **Link**: pick "Link device". A QR code appears in a modal.
   Scan it from the **Signal app on your phone** (Settings →
   Linked Devices → Link a Device). This consumes a linked-device
   slot on your account; remove it from your phone when done.
4. **Receive**: send a Signal message from another account to
   the linked phone. xas's home screen should show the new
   conversation row in seconds.
5. **Send**: open the conversation, type a message, press Enter.

Hosted mode uses your **real Signal account** and goes through
real Signal servers, so any Signal Protocol bug you hit on
hardware should also reproduce here (with much less iteration
cost).

### 2.5 Headless link smoke-test

xas ships a script that boots hosted Xous, drives the link flow
via XSendEvent, and gates on the link URL reaching the UI —
useful as a CI sanity-check. PASS criterion is two log lines:
`worker/link: URL received from libsignal: sgnl://linkdevice?...`
and `xas/gam_app: link URL = sgnl://linkdevice?...`.

The script still needs an X server (it greps for the "Precursor"
window with `xdotool` and injects keystrokes via `libX11.so.6`),
but a real display is not required — `xvfb-run` works:

```sh
cd ~/code/xas/xous-app-signal
export XOUS_CORE_DIR=$(pwd)/../xous-core
export XAS_BIN_PATH=$(pwd)/target/release/xas
export LINK_TIMEOUT=180 BOOT_TIMEOUT=300

# headless box (no $DISPLAY): wrap in Xvfb
xvfb-run -a -s "-screen 0 1024x768x24" bash tests/hosted/test_link_qr.sh

# real X11 box: just run it
bash tests/hosted/test_link_qr.sh
```

End-to-end (kernel boot + drive + link URL emission) takes ~2 min
on a fresh build. Exit codes: `0` PASS, `2` Xous never finished
booting (raise `BOOT_TIMEOUT`), `3` Precursor X11 window not
found, `4` link URL never emitted (raise `LINK_TIMEOUT` or set
`KEEP_LOGS=1` and inspect `/tmp/xas-hosted-test.*`).

`INSPECT_HOLD=NN` (seconds) keeps the kernel alive after the
test passes, so you can scan the QR from a phone for an
end-to-end manual link.

### 2.6 Hosted-mode unit tests

```sh
cd ~/code/xas/xous-app-signal
cargo test --features hosted -p xous-app-signal --bins
```

22 tests covering the dialogue model, message rendering, and
contact-name resolution. Should all pass green.

---

## 3. Hardware path (Precursor PVT2)

If you skipped section 2 entirely, you still need the
`apps.rs` bootstrap from §2.1 — the standalone `cargo build`
in 3.1 has the same gam dependency and fails the same way
without it.

### 3.1 Build the rv32 xas binary

The fastest path is the precursor test script, which does the
build + bundle + flash for you (see section 3.2):

```sh
cd ~/code/xas/xous-app-signal
bash tests/precursor/build-and-bundle.sh
```

If you only want the rv32 binary (no kernel image), the
underlying `cargo build` is:

```sh
cd ~/code/xas/xous-app-signal
cargo build --target riscv32imac-unknown-xous-elf --release \
    -p xous-app-signal --features pddb-real,precursor
```

Output: `target/riscv32imac-unknown-xous-elf/release/xas`
(~55 MB ELF). First build is ~10 min; incremental builds are
~1 min.

### 3.2 Bundle a signed kernel image

```sh
cd ~/code/xas/xous-core
cargo xtask app-image-xip \
    xas:../xous-app-signal/target/riscv32imac-unknown-xous-elf/release/xas \
    vault \
    --kernel-feature big-heap \
    --gdb-stub \
    --git-describe v0.9.8-791-gc707f9d8 \
    --git-rev c707f9d8
```

Notes on the flags:

- `xas:..` is the path to the rv32 binary built in 3.1. (Earlier
  doc revisions said `dist/xas-rv32/xas`; that path doesn't
  exist — cargo writes directly to `target/<triple>/release/xas`,
  and `tests/precursor/build-and-bundle.sh` reads it from there.)
  `vault` is bundled alongside as a co-resident app (xas's
  launcher navigation lives inside vault's launcher conventions).
- `--kernel-feature big-heap` raises the per-process heap cap
  from 512 KiB → 12 MiB. Required because the libsignal +
  presage + rustls + smol working set exceeds the default cap
  during link.
- `--gdb-stub` enables the in-kernel GDB stub on the secondary
  UART. Optional for normal use; needed for any GDB session.
- `--git-describe` and `--git-rev` should match the SoC version
  on your device. Run `lsusb -v 2>&1 | grep iSerial` while
  Precursor is plugged in (loader window) — the iSerial includes
  the gateware build hash. If unsure, use the values shown above
  (the most-recent stable PVT2 SoC).

Output: `target/riscv32imac-unknown-xous-elf/release/xous.img`
(~13 MB signed kernel image). Bundling step is 1–2 min.

### 3.3 Flash via USB

The `xous-core/tools/usb_update.py` script speaks to the
Precursor's loader-mode USB endpoint (USB ID `1209:5bf0`) and
writes the kernel partition. The flash takes ~25 min.

#### Option A: Pi-hosted flash (recommended)

If you have a Pi with the betrusted debug HAT:

```sh
# On the Pi, set up the directory structure (one-time)
mkdir -p ~/xous-flash
scp ~/code/xas/xous-core/tools/usb_update.py pi@<pi-ip>:~/xous-flash/

# For each flash:
scp ~/code/xas/xous-core/target/riscv32imac-unknown-xous-elf/release/xous.img \
    pi@<pi-ip>:~/xous-flash/xous.img
ssh pi@<pi-ip> 'lsusb | grep 1209'   # confirm Precursor visible (1209:5bf0)
ssh pi@<pi-ip> 'cd ~/xous-flash && python3 usb_update.py -k xous.img --bounce'
```

The `--bounce` flag automatically reboots the Precursor into
running mode after the flash completes. **Do not omit it** unless
you intend to re-flash before booting.

The Pi rig also captures the Precursor's primary UART continuously
(via `screen -dmS uart cat /dev/ttyAMA0 >> uart-log`). Keep this
running across flashes — kernel boot logs go straight to that file
and are essential for debugging.

#### Option B: Direct flash from build host

If your build host can talk USB to the Precursor directly (no Pi
in between):

```sh
cd ~/code/xas/xous-core
lsusb | grep 1209   # confirm Precursor visible (1209:5bf0)
python3 tools/usb_update.py \
    -k target/riscv32imac-unknown-xous-elf/release/xous.img \
    --bounce
```

You won't have continuous UART logging this way, but the flash
itself works the same.

### 3.4 First-time setup on the device (after flash)

When the Precursor boots into Xous:

1. **Unlock PDDB**: enter your PDDB password (set once on first
   boot; change with the `pddb` shellchat command).
2. **Wi-Fi**: switch to shellchat from the launcher and run
   **in this exact order**:
   ```
   wlan off
   wlan on
   ssid scan
   wlan status     # poll until "Connected"
   net ping 1.1.1.1   # sanity-check IP works
   ```
   **Use a 2.4 GHz network only** — Precursor's WF200 radio is
   single-band 802.11 b/g/n. 5 GHz networks won't appear in
   `ssid scan`. Phone hotspots default to 5 GHz now; force
   2.4 GHz mode (or "compatibility mode") in the hotspot settings.
3. **Open xas**: from the launcher, navigate to Apps → xas.
4. **Link**: pick "Link device". A QR code appears. Scan it
   from the Signal app on your phone (Settings → Linked Devices →
   Link a Device). Linking takes 1–4 minutes after you scan; do
   not power-cycle.
5. **Test send/receive**: send a message from another Signal
   account to your linked phone. xas should show it in seconds.
   Send a reply — first send takes 1–4 minutes due to a known
   Signal-server WebSocket-rotation issue (see
   the transport-refactor roadmap item).

---

## 4. Common troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `lsusb \| grep 1209` shows nothing | Precursor not in loader mode | Hold left-side button while plugging in USB; release after 2 seconds |
| `lsusb \| grep 1209` shows `1209:3613` not `1209:5bf0` | Precursor in running mode, not loader | Hold left-side button + paperclip-reset |
| `failed to read .../repos/xous-core/services/trng/Cargo.toml` (cargo build, very early) | `repos/xous-core` symlink missing or in the wrong place | See section 1 — symlink lives at `<workspace-parent>/repos/xous-core`, *not* inside `xous-app-signal/`. Run `ln -s ../xous-core repos/xous-core` from the workspace parent. |
| `error[E0583]: file not found for module 'apps'` in `services/gam/src/lib.rs` | `gam/src/apps.rs` not bootstrapped | See section 2.1 — write `apps.rs` by hand (with `APP_NAME_XAS`) before the standalone hosted build. |
| `usb_update.py` permission denied (Linux host) | udev rule missing | Add `tools/49-precursor.rules` to `/etc/udev/rules.d/` and `udevadm control --reload`, or run with sudo (not recommended) |
| Hosted xas shows "OOM during link" | Default heap cap too low | Run with `RUST_LOG=info` to see allocator messages; rebuild with `--features pddb-real,hosted` (the dist build is otherwise too lean) |
| Hardware link succeeds but no messages flow | Wi-Fi connected to 5 GHz, or DNS broken | Re-run the wlan recipe; verify `net ping chat.signal.org` works before opening xas |
| Send fails with "WebSocket closing" within 30s | Older xous-core without the encoding fix | Confirm you cloned the `xas` branch of `tunnell/xous-core`, not upstream `betrusted-io/xous-core` (or apply patches in the README's Upstream patches section) |
| Flash completes but device boots into the old image | Loader didn't validate the new signature | Re-flash; if it persists, check `tools/usb_update.py` log for verification errors |

---

## 5. Verifying your build matches mine

Quick checks before reporting issues:

```sh
# In xous-app-signal:
git rev-parse HEAD   # should match origin/main (or origin/dev if developing)
cargo --version      # should be 1.95.0 or newer
ls vendor/presage/.git 2>&1   # should NOT exist (we vendor as plain dirs)

# In xous-core:
git branch --show-current   # should be 'xas'
git log --oneline -1 services/net/src/std_glue.rs   # should show the byte-1 fix
```

A successful hardware build produces an image of size
~12.86 MB. md5sum is non-deterministic (timestamp embedded in
the build) but the size should be within 1 KB of that.

---

## 6. Getting help

- File issues at <https://github.com/tunnell/xous-app-signal/issues>.
- This README's "Feature support" section tracks what works
  and what doesn't.
- The README's "Upstream patches" section documents the patches
  xas requires that aren't yet upstream.
