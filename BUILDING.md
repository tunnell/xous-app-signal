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

### Order of operations on a fresh box (READ THIS)

Two rustup pitfalls bite every fresh contributor in the first
five minutes. Resolve both before running any `cargo` command in
this workspace, **regardless of which path (hosted or hardware)
you intend to use**:

1. **`rustup` must have a default toolchain.** `xous-core/` is
   not pinned by a `rust-toolchain.toml`, so on a box where
   rustup has no default (`rustup default` prints `error: no
   default toolchain is configured`), every `cargo xtask …`
   command run from `xous-core/` fails with `error: rustup could
   not choose a version of cargo to run, because one wasn't
   specified explicitly, and no default is configured`. Fix
   once: `rustup default stable`.

2. **The Xous std bundle must be installed before any cargo
   build.** This workspace's `rust-toolchain.toml` declares
   `targets = ["riscv32imac-unknown-xous-elf"]`. On rustup
   ≥ 1.28 the previously-tolerated "warn: skipping unavailable
   component rust-std for target …" **may** be a hard error
   (`error: component 'rust-std' for target
   'riscv32imac-unknown-xous-elf' is unavailable for download
   for channel 'stable'`), depending on rustup state and prior
   toolchain installs. When it does hard-error, it fires on
   first `cargo` invocation and blocks both the hosted and
   hardware paths. Fix once by installing the betrusted-io
   Xous std bundle via xous-core's xtask **before** any other
   cargo command in this workspace.

   This step requires §1's `xous-core` clone to exist — so
   complete §1 first, then come back here:

   ```sh
   cd ~/code/xas/xous-core
   cargo xtask install-toolkit
   ```

   That bundle ships precompiled `libstd-*.rlib` for the
   `riscv32imac-unknown-xous-elf` target, which both satisfies
   the hard error and is a strict prerequisite for §3 (the
   hardware build). Hosted-only readers may be tempted to skip
   it; you can't, because the toolchain file fires before any
   feature flags are evaluated. See §1.5 for full detail.

If you skip these and start with §2.2, the symptom is a cryptic
rustup error before a single crate compiles. The smoke-test in
§2.5 disguises the same problem as `ERROR: Xous never booted` —
inspect `/tmp/xas-hosted-test.*/xous.log` and you'll see the
rustup error there.

---

## 1. Clone the source

xas depends on a forked `xous-core` because two upstream bugs
needed patching for the Wi-Fi + WebSocket path to work
reliably. The kernel-side encoding fix has since **merged
upstream** (betrusted-io/xous-core#877, commit `2005a801c`); the
keepalive-tolerance PR (whisperfish/libsignal-service-rs#431)
was **closed unmerged** and its fix is carried on the
rev-pinned `libsignal-service-rs` fork branch instead (see
`docs/FORKS.md` and the README's Upstream patches section for
links and status). The forks below remain the canonical source: the pinned
branch also carries fixes that are in no upstream release — the
DNS CNAME-chain fix, the `services/net` reaper fix, and the
`apps/manifest.json` registration for xas.

The forks below are the published source of truth — pull `main`
for the latest released code; pull `dev` if you want the
in-progress branch. (See `tests/README.md` for the
`main` vs `dev` convention.)

`~/code/xas/` below is just an example — any parent directory
works. What matters is the *layout*: `xous-app-signal/` and
`xous-core/` as siblings, plus a `repos/xous-core` symlink at the
same level. The Cargo manifests follow the relative path
`../repos/xous-core/...`; they do not care what the parent
directory is named.

```sh
mkdir -p ~/code/xas && cd ~/code/xas

# xas itself (default branch is 'main' = released code).
# If you're following an in-progress version of this doc, also:
#     cd xous-app-signal && git checkout dev
git clone https://github.com/tunnell/xous-app-signal.git

# xous-core (kernel + services). The `xas-v0.2` branch is the v0.2
# frozen release branch (see RELEASING.md for how releases pin
# xous-core) — it registers `xas` in apps/manifest.json (so
# `services/gam` knows to expose Signal as a launchable app),
# carries DNS / net / gam fixes the Signal app needs (including
# the services/net reaper fix from tunnell/xous-core#26), and
# includes the apps/xas/ subtree.
#
# Future xas releases will pin to their own frozen branches
# (`xas-v0.3`, etc.). The floating `xas` integration branch on
# tunnell/xous-core continues to advance for development, but
# released xas versions always build against a pinned snapshot.
#
# An older `xous-app-signal` branch also exists with similar
# content; it's kept around for historical compatibility but
# `xas-v0.2` has the more recent fixes (DNS CNAME chains,
# net-service instrumentation, gam Enter-key alias, etc.).
#
# Note: --depth 1 keeps the clone small (~250 MB vs ~2 GB full).
# If you want to verify the branch's commit history matches the
# table in §1's 'What each clone contributes', drop --depth 1
# here OR run `git fetch --unshallow` after cloning.
git clone --depth 1 -b xas-v0.2 https://github.com/tunnell/xous-core.git

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
│   ├── Cargo.toml                     # [patch] pins the Signal-stack forks (docs/FORKS.md)
│   └── crates/
└── xous-core/
    ├── kernel/
    ├── services/
    └── xtask/
```

The patched Signal-stack dependencies (presage,
libsignal-service-rs, curve25519-dalek) are not in-tree: they are
GitHub forks consumed at pinned revs via the workspace `[patch]`
entries, fetched by cargo on first build. `docs/FORKS.md` has the
pin matrix and compare URLs.

`repos/xous-core` must point at the same checkout you cloned as
`~/code/xas/xous-core`. Verify with `ls repos/xous-core/services/gam/Cargo.toml`
— that file must exist, otherwise the first path-dep cargo tries
to load (`gam`, from `crates/xous-app-signal`) will fail with
`failed to read '.../repos/xous-core/services/gam/Cargo.toml'`.

### What each clone contributes to the build

The two clones above plus the rev-pinned libsignal-service-rs
fork give you the effective fixes from all three upstream PRs xas
has tracked (two merged, one closed in favor of the fork carry).
If you ever need to verify "am I actually building with the
upstream PR content," this is the map:

| Upstream PR | Where it lives in your build | How |
|---|---|---|
| [betrusted-io/xous-core#877](https://github.com/betrusted-io/xous-core/pull/877) (kernel byte-1 mirror) | `xous-core/services/net/src/std_glue.rs::respond_with_error` on the pinned `xas-v0.2` branch | **Merged upstream 2026-06-02** as commit `2005a801c` — any `betrusted-io/xous-core` checkout at or after that commit carries it. The pinned `xas-v0.2` branch carried the identical commit pre-merge; the pin remains required for the deltas that are *not* upstream: the CNAME-chain DNS fix (`43dcb4a59`) required for Signal connectivity, the `services/net` reaper fix (tunnell/xous-core#26; upstream [#880](https://github.com/betrusted-io/xous-core/pull/880) closed 2026-07-17 unmerged — the maintainer wants net fixes to follow the Renode-CI refactor, so the fork carries it), a small PDDB hosted-mode test convenience (`c22cfc678`), and the `apps/manifest.json` xas registration. |
| [whisperfish/libsignal-service-rs#431](https://github.com/whisperfish/libsignal-service-rs/pull/431) (keepalive tolerance) | `src/websocket/mod.rs` on the `tunnell/libsignal-service-rs` fork branch `xous-782c0d6`, rev `86b9da7cde` (see `docs/FORKS.md`) | The fork uses a local `MAX_OUTSTANDING_KEEPALIVES = 3` constant. PR #431 proposed the same tolerance as an opt-in `with_max_outstanding_keepalives(...)` constructor (default = 1, preserves upstream behavior); it was **closed unmerged by its author on 2026-07-18**, so the fork constant is the long-term shape rather than a stopgap awaiting re-alignment. No action needed — cargo fetches the fork at the rev pinned in `Cargo.lock`, and the §5 lock check verifies it. |
| [rust-lang/rust#156414](https://github.com/rust-lang/rust/pull/156414) (std recv byte-4 decode) | **Not in your build (yet).** | PR #156414 fixes the bug at its actual source (the std-side recv decode reads byte 4 instead of byte 1). It **merged 2026-06-04** (milestone 1.98.0) but has not reached a stable Rust release yet, so the toolchain this workspace builds with still has the byte-1 bug — and it doesn't matter, because PR #877's kernel-side mirror writes the code at byte 1 too. Once a stable release carrying the fix reaches the toolchain pin, the kernel-side mirror becomes belt-and-suspenders rather than load-bearing. No action needed for the current build. |

---

## 1.5. Install the Xous Rust target sysroot (hardware path only)

**You already ran `cargo xtask install-toolkit` per §0.** This
section explains *why* that step was required and what to do if
it failed — skim now, return if you hit problems. It
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

On rustup older than ~1.28 you'll see a warning:

```
warn: skipping unavailable component rust-std for target
      riscv32imac-unknown-xous-elf
```

On rustup ≥ 1.28 the same condition **may** surface as a hard
error rather than a warning:

```
error: component 'rust-std' for target
       'riscv32imac-unknown-xous-elf' is unavailable for download
       for channel 'stable'
```

Whether you get the warning or the hard error depends on rustup
state and prior toolchain installs (we've seen rustup 1.29.0 emit
the warning where 1.28 emitted the hard error on the same
manifest). Either way, the target is "added" (recognized) but the
precompiled `rust-std` component isn't downloaded — it doesn't
exist as a published rustup component. With the hard-error
variant, every `cargo` command in this workspace fails before
compiling a single crate (because `rust-toolchain.toml` lists
the target). With the legacy warning you can attempt a build
and instead get `error[E0463]: can't find crate for `core``.

You have to install the sysroot some other way — see the
"Order of operations on a fresh box" callout in §0 and the
recommended path immediately below.

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

> **Heads-up on a fresh rustup install**: `xous-core/` carries
> no `rust-toolchain.toml`, so if rustup has no default
> toolchain set, the command above fails with `error: rustup
> could not choose a version of cargo to run`. Run `rustup
> default stable` once, or invoke explicitly via
> `rustup run stable cargo xtask install-toolkit`.

To verify success:

```sh
ls "$(rustc --print sysroot)/lib/rustlib/riscv32imac-unknown-xous-elf/lib/" \
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

`xous-core/services/gam/src/apps.rs` is auto-generated by xtask
(`generate_app_menus()` in `xtask/src/app_manifest.rs`) from
`xous-core/apps/manifest.json`, and is gitignored.

The `xas-v0.2` branch of `tunnell/xous-core` (cloned in §1) already
registers `xas` in `manifest.json` (alongside `vault`), so once
you've invoked xtask once (`cargo xtask run` in §2.3, or
`cargo xtask app-image-xip` in §3.2), `apps.rs` self-maintains
correctly forever after.

But §2.2 below — the standalone `cargo build` of `xous-app-signal`
— links against `gam::APP_NAME_XAS` and so needs `apps.rs` on
disk *before* xtask has been invoked. On a fresh checkout that
file doesn't exist yet, and `cargo build` fails with
`error[E0583]: file not found for module 'apps'`. Bootstrap it
once by hand. Run from `~/code/xas/`:

```sh
cat > xous-core/services/gam/src/apps.rs <<'EOF'
#![cfg_attr(rustfmt, rustfmt_skip)]
// Hand-written stand-in until xtask regenerates this file from
// xous-core/apps/manifest.json on first `cargo xtask run` /
// `app-image-xip` invocation. Mirrors xtask's eventual output
// for the xas entry (both the app name and its submenu) so
// readers don't hit a missing-symbol error on launcher-submenu
// code paths before xtask has run.
pub const APP_NAME_XAS: &'static str = "Signal";
pub const APP_MENU_0_XAS: &'static str = "Signal Submenu 0";

pub const EXPECTED_APP_CONTEXTS: &[&'static str] = &[
    APP_NAME_XAS,
    APP_MENU_0_XAS,
];
EOF
```

After §2.3 or §3.2 has run once, xtask overwrites this with the
full manifest-derived version (both `APP_NAME_XAS` and
`APP_NAME_VAULT` plus a vault submenu constant) — no further
hand-edits needed.

**Re-bootstrap on branch switches.** If you switch the xous-core
checkout to a branch whose `apps/manifest.json` differs (for
example, branching off `dev` to test a fix without the xas entry),
the cached `apps.rs` from the previous build can disagree with the
new manifest. Two failure modes to watch for:

- The build fails with `error[E0425]: cannot find value
  APP_NAME_XAS` (cached `apps.rs` was written on a manifest that
  doesn't have xas; cargo build runs before xtask in
  `tests/precursor/build-and-bundle.sh`). Re-bootstrap as above.
- The build *succeeds* but the device boots without `Signal` in
  the launcher menu (manifest has xas, cached `apps.rs` doesn't,
  bundle includes the xas binary but gam never registers it).
  Same fix: re-bootstrap.

Sanity check before flashing:
```sh
grep APP_NAME_XAS xous-core/services/gam/src/apps.rs
```
If this returns nothing, the launcher won't see Signal regardless
of what's in the bundled image. Hand-bootstrap and rebuild.

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

**Required env var for human-driven hosted runs**: set
`XAS_BYPASS_PREFLIGHT=1` before launching xtask. Hosted has no
real WF200 radio; `com.wlan_status()` always returns
`LinkState::Unknown`; xas's no-internet preflight then routes
`Link device` → `Screen::NoInternet` and the link flow never
proceeds. The env var is the documented escape hatch
(`gam_app.rs::check_internet`); production code on hardware still
runs the preflight as designed. The §2.5 smoke-test script sets
this env var itself; for human walkthroughs you have to set it:

```sh
XAS_BYPASS_PREFLIGHT=1 cargo xtask run xas:.../xas
```

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
but a real display is not required — `xvfb-run` works.

This step depends on `xdotool` and `xvfb` (Debian/Ubuntu:
`apt install xdotool xvfb`). They're listed in §0's hosted-path
prereqs, but flagging here too — a reader who skipped §0 because
they have a real `$DISPLAY` may not realize the script itself
needs `xdotool` regardless.

The script sets `XAS_BYPASS_PREFLIGHT=1` before launching xas.
Hosted has no real WF200 radio; `wlan_status()` returns `Unknown`,
which the production no-internet preflight (`gam_app::check_internet`)
treats as failure and routes Link → `Screen::NoInternet`. The env
var is the documented escape hatch — production code still runs
the preflight on hardware, but hosted tests opt out so the link
flow can proceed.

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
on a fresh build (boot itself is typically well under 90 s — the
defaults `BOOT_TIMEOUT=300` and `LINK_TIMEOUT=180` are deliberately
generous, so a successful run usually finishes in roughly half the
cap). Exit codes: `0` PASS, `2` Xous never finished booting (raise
`BOOT_TIMEOUT`), `3` Precursor X11 window not found, `4` link URL
never emitted (raise `LINK_TIMEOUT` or set `KEEP_LOGS=1` and
inspect `/tmp/xas-hosted-test.*`).

`KEEP_LOGS=1` also works on PASS — the preserved log directory is
useful for verifying the boot/drive/link timeline (`xous.log` has
the boot trace; `drive.log` has the keystroke driver's path) even
when everything went right.

`INSPECT_HOLD=NN` (seconds) keeps the kernel alive after the
test passes, so you can scan the QR from a phone for an
end-to-end manual link.

For the broader hosted send/receive harness (drives `signal-cli`
on the same host as the test peer, sends a message in, asserts
xas's render, then sends one back), copy
`tests/hosted/test_env.example` to `tests/hosted/test_env`
(gitignored) and fill in your `TEST_PEER_NUMBER` /
`TEST_XAS_NUMBER`. `tests/hosted/scan_receive.sh` and
`drive_link.py` consume those values via
`tests/hosted/test_helpers.sh`. The QR smoke-test above does
**not** need them.

### 2.6 Hosted-mode unit tests

```sh
cd ~/code/xas/xous-app-signal
cargo test --features hosted -p xous-app-signal --bins
```

~40 tests covering the dialogue model, the `MessageStore`
mutation funnel, message rendering, contact-name resolution, and
link/send/receive event dispatch. Should all pass green.

### 2.7 Renode tests (CI-grade harness)

The renode suite lives in `tests/renode/`: eight robots, one
CI-grade machine definition (`xas-ci.resc`), a shared robot resource
(`xas-ci-common.resource`), and the wrapper `run-renode-tests.sh`.

```sh
tests/renode/run-renode-tests.sh                   # xas-smoke.robot
tests/renode/run-renode-tests.sh xas-probe.robot   # one robot
tests/renode/run-renode-tests.sh --all             # all eight, serially,
                                                   # with a summary table
```

For each robot the wrapper builds the rv32 xas ELF with the feature
set that robot expects, re-bundles `loader.bin`/`xous.img` into
`$XOUS_CORE_DIR` **only when the (features, image features, ELF)
triple changed**, and runs `renode-test` under a hard wall-clock cap.
Feature map:

| robot | xas ELF features | image (xtask) features |
|---|---|---|
| `xas-smoke`, `xas-bulk-write-boot`, `xas-selective-sync`, `xas-instrument-noise` | `pddb-real,precursor` (canonical) | — |
| `xas-pddb-probe` | `precursor,probe-pddb` | — |
| `xas-probe` | `precursor,probe-flow` | — |
| `xas-send-batch` | `precursor,probe-send-batch` | — |
| `xas-echo` | `precursor,probe-echo` | `net/renode-minimal` |

`XAS_FEATURES` overrides the map for single-robot runs (ignored under
`--all`). After `--all`, the wrapper re-bundles the canonical image so
the xous-core tree never ends on a probe variant.

`xas-echo` is the first robot where the network stack must WORK (an
in-image `std::net` TCP echo over the smoltcp loopback, byte-exact,
`XAS-ECHO DONE: pass=4 fail=0`), and the only one with an image-side
feature: `net/renode-minimal` seeds a static IPv4 config at boot,
without which smoltcp never gains its `127.0.0.1/8` address (no DHCP
bind ever fires on the closed renode switch). That feature only
exists on xous-core branch `xas-integration-net` — point
`XOUS_CORE_DIR` there for this robot; the bundle fails loudly on a
tree without it.

**The machine (`xas-ci.resc`)** follows upstream xous-core's
`emulation/tests/pddb-ci.resc` CI pattern, not the interactive
`emulation/betrusted.resc`: headless SoC + EC pair (the EC is
mandatory — without it the first COM transaction null-derefs and PDDB
mount spin-waits), no gdb servers / socket terminals / analyzers (no
fixed listen ports), `emulation SetSeed 0x0` for determinism, a
**per-run, per-robot 0xFF-filled 128 MiB flash scratch file** (the
flash model writes through into its backing file; a shared or
zero-filled backing file leaks state across runs and stalls PDDB
boot), and file-backed UART logs under
`target/xas-ci/<robot>-{console,kernel}.log` — the robots grep the
console log for their end-of-test assertions, and the files are the
first stop for triage.

**Time bounds.** `Wait For Line On Uart` timeouts are virtual-time
seconds (host-speed independent); each robot also carries a
wall-clock `Test Timeout` of 10 minutes, and the wrapper wraps
`renode-test` in `timeout(1)` (`ROBOT_TIMEOUT_SECS`, default 900 s) —
a stalled machine cannot hang a run indefinitely. `PANIC in PID` is a
registered failing UART string, so a service death fails the pending
wait immediately.

**Environment.** `XOUS_CORE_DIR` — the xous-core checkout to bundle
into and boot (default: the `repos/xous-core` symlink from §1; must
register xas in `apps/manifest.json`). `RENODE_CI_MODE` — exported,
defaults to `YES`. `SKIP_BUNDLE=1` — boot the existing `xous.img`
as-is (only safe when it already matches the robot's features).
`RUSTUP_TOOLCHAIN` — honored if set; hosts whose stable rustc has a
stale rv32 sysroot need a pinned toolchain for all rv32 builds.

**Bundling by hand** (what the wrapper does, if you want to run
`renode-test` directly):

```sh
cd <xous-core>
cargo xtask app-image-xip \
    xas:<xas>/target/riscv32imac-unknown-xous-elf/release/xas \
    vault transientdisk --kernel-feature big-heap \
    --git-describe v0.9.21-0-g0000000
XOUS_CORE_DIR=<xous-core> renode-test tests/renode/<robot>.robot
```

Two load-bearing details:

- `app-image-xip`, **not** `app-image`: the all-in-RAM image
  (~12.9 MB `xous.img`) exceeds the Precursor's 16 MiB RAM once every
  service unpacks — under Renode the kernel OOM-panics ("Couldn't
  allocate new page: OutOfMemory" → KERNEL FAILURE → reboot loop)
  shortly after xas starts, before PDDB reaches the password prompt.
  XIP executes services from flash-mapped addresses and is the same
  bundle the hardware flash flow uses (§3.2).
- `--git-describe` is **mandatory on forks**: the xtask runs
  `git describe --tags` to embed a version string, and a fork branch
  generally has no reachable tag. Any v-prefixed semver parses;
  `v0.9.21-0-g0000000` is the value the harness pins against.

The three boot-gate robots (`xas-bulk-write-boot`,
`xas-selective-sync`, `xas-instrument-noise`) pass on the fresh 0xFF
flash without any keyboard injection: `Requesting login password`
prints *before* the first-boot REQFMT format prompt, so no
`pddb/autobasis` kernel feature is needed. (The former
`probe-pddb-real`/`probe-bulk-ab` features and
`xas-pddb-real-probe.robot` were removed 2026-05-14; bulk-write
benchmarking moved to the user-invoked shellchat `pddb bulk_probe`.)

---

## 3. Hardware path (Precursor PVT2)

If you skipped section 2 entirely, you still need the
`apps.rs` bootstrap from §2.1 — the standalone `cargo build`
in 3.1 has the same gam dependency and fails the same way
without it.

**Branch selection in xous-core matters.** Hardware builds need
the xous-core checkout on a branch whose `apps/manifest.json`
registers xas (`tunnell/xous-core@xas-v0.2` is the canonical one
for v0.2 builds; future releases will pin to `xas-v0.3`, etc. —
see RELEASING.md). Building against `dev` (or any
branch that doesn't register xas) will silently produce an
image that bundles the xas binary but where the launcher menu
doesn't list Signal — see the "Re-bootstrap on branch switches"
note in §2.1. Sanity-check with
`grep APP_NAME_XAS xous-core/services/gam/src/apps.rs` before
flashing.

### 3.1 Build the rv32 xas binary

The fastest path is the precursor test script, which does the
build + image-bundle for you (the equivalent of §3.1 + §3.2 below
in one shot — flashing in §3.3 is separate):

```sh
cd ~/code/xas/xous-app-signal
bash tests/precursor/build-and-bundle.sh
```

The script invokes `cargo xtask app-image-xip` with the same
flags documented in §3.2 (xas + vault, `--kernel-feature
big-heap`, `--gdb-stub`, SoC version pins). Override the SoC
version via `GIT_DESCRIBE`/`GIT_REV` env vars if your device
reports a different one (`lsusb -v | grep iSerial` while in
loader mode).

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
    transientdisk \
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
  `vault` and `transientdisk` are bundled as co-resident apps
  (xas's
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

Output: `xous-core/target/riscv32imac-unknown-xous-elf/release/xous.img`
(~12.89 MB signed kernel image). Note this is *xous-core's*
target dir, not `xous-app-signal/target/...` — the preceding
`cd` is into `xous-core`, so cargo writes there. Bundling step is
1–2 min.

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
| `error: component 'rust-std' for target 'riscv32imac-unknown-xous-elf' is unavailable for download` (any cargo command, before any crate compiles) | Modern rustup (≥ 1.28) hard-errors on the unavailable tier-3 std declared in `rust-toolchain.toml` | Install the betrusted-io std bundle first: `cd ../xous-core && cargo xtask install-toolkit`. See §0 "Order of operations" and §1.5. |
| `error: rustup could not choose a version of cargo to run, because one wasn't specified explicitly` (running anything from `xous-core/`) | `xous-core/` carries no `rust-toolchain.toml` and rustup has no default | `rustup default stable` (one-time), or prefix the command with `rustup run stable …` |
| `tests/hosted/test_link_qr.sh` reports `ERROR: Xous never booted within Ns` despite a long timeout | Often a misleading symptom of one of the two rustup pitfalls above — `cargo xtask run` exits before booting | `cat /tmp/xas-hosted-test.*/xous.log` and look for the rustup error before raising `BOOT_TIMEOUT` |
| `lsusb \| grep 1209` shows nothing | Precursor not in loader mode | Hold left-side button while plugging in USB; release after 2 seconds |
| `lsusb \| grep 1209` shows `1209:3613` not `1209:5bf0` | Precursor in running mode, not loader | Hold left-side button + paperclip-reset |
| `failed to read .../repos/xous-core/services/trng/Cargo.toml` (cargo build, very early) | `repos/xous-core` symlink missing or in the wrong place | See section 1 — symlink lives at `<workspace-parent>/repos/xous-core`, *not* inside `xous-app-signal/`. Run `ln -s ../xous-core repos/xous-core` from the workspace parent. |
| `error[E0583]: file not found for module 'apps'` in `services/gam/src/lib.rs` | `gam/src/apps.rs` not bootstrapped | See section 2.1 — write `apps.rs` by hand (with `APP_NAME_XAS`) before the standalone hosted build. |
| `usb_update.py` permission denied (Linux host) | udev rule missing | Add `tools/49-precursor.rules` to `/etc/udev/rules.d/` and `udevadm control --reload`, or run with sudo (not recommended) |
| Hosted xas shows "OOM during link" | Default heap cap too low | Run with `RUST_LOG=info` to see allocator messages; rebuild with `--features pddb-real,hosted` (the dist build is otherwise too lean) |
| Hardware link succeeds but no messages flow | Wi-Fi connected to 5 GHz, or DNS broken | Re-run the wlan recipe; verify `net ping chat.signal.org` works before opening xas |
| Send fails with "WebSocket closing" within 30s | Older xous-core without the encoding fix | Confirm you cloned the `xas-v0.2` branch of `tunnell/xous-core`. Relevant fixes: [#877](https://github.com/betrusted-io/xous-core/pull/877) (encoding fix — merged upstream 2026-06-02, so recent `betrusted-io/xous-core` also carries it, but only `xas-v0.2` adds the DNS + reaper + manifest deltas) and [tunnell/xous-core#26](https://github.com/tunnell/xous-core/pull/26) (services/net reaper fix shipped with v0.2). |
| Flash completes but device boots into the old image | Loader didn't validate the new signature | Re-flash; if it persists, check `tools/usb_update.py` log for verification errors |

---

## 5. Verifying your build matches mine

Quick checks before reporting issues:

```sh
# In xous-app-signal:
git rev-parse HEAD   # should match origin/main (or origin/dev if developing)
cargo --version      # should be 1.95.0 or newer

# Confirm the Signal-stack forks resolve to the pinned revs from
# docs/FORKS.md (cargo verifies the checkouts against these):
grep -A2 'name = "libsignal-service"' Cargo.lock   # expect: source = git+...tunnell/libsignal-service-rs?rev=86b9da7c...
grep -A2 'name = "presage"' Cargo.lock             # expect: source = git+...tunnell/presage?rev=7b63a451...

# Confirm the fork checkout cargo fetched carries the
# keepalive-tolerance fix (effective equivalent of upstream PR
# whisperfish/libsignal-service-rs#431):
grep -F 'MAX_OUTSTANDING_KEEPALIVES: usize = 3' \
    ~/.cargo/git/checkouts/libsignal-service-rs-*/86b9da7/src/websocket/mod.rs   # expect: 1 hit

# In xous-core:
git branch --show-current   # should be 'xas-v0.2' (the §1 pin)

# Confirm the byte-1 mirror is actually in respond_with_error
# (effective equivalent of upstream PR betrusted-io/xous-core#877):
grep -F 'Mirror code at byte 1' services/net/src/std_glue.rs   # expect: 1 hit

# Confirm xas is registered in apps/manifest.json so xtask
# generates apps.rs with APP_NAME_XAS:
grep -F '"xas":' apps/manifest.json   # expect: 1 hit
```

**(Hardware path only.)** A successful hardware build produces an
image of size ~12.89 MB (12,886,056 bytes give or take a few KB
across toolchain bumps). md5sum is non-deterministic (timestamp
embedded in the build) but the size should be within ~50 KB of
the baseline.
Hosted-mode builds don't produce a `xous.img` — they produce a
`target/release/xas` binary at ~58 MB.

---

## 6. Getting help

- File issues at <https://github.com/tunnell/xous-app-signal/issues>.
- This README's "Feature support" section tracks what works
  and what doesn't.
- The README's "Upstream patches" section documents the status
  of each upstream fix xas has tracked (merged, closed, or still
  carried in a pin/fork), with links to each PR; `docs/FORKS.md`
  has the fork pin matrix.
