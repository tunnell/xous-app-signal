# precursor/ — running a hardware test on a Precursor PVT2

This folder is the manual hardware-testing path: build a kernel
image, flash it to a Precursor over USB (optionally via a
Raspberry Pi rig), and watch UART for what the device does. For
the faster hosted-mode emulator and unit tests, see
[`../tests/README.md`](../tests/README.md). For everything-from-
scratch toolchain setup, see [`../BUILDING.md`](../BUILDING.md).

The instructions below are written so that a human or an
automated coding session can follow them end-to-end. Read the
**Brick prevention** section before running any flash command.

```
precursor/
├── README.md            ← this file
├── build-and-bundle.sh  ← rebuild xas + bundle a kernel image
├── flash-via-pi.sh      ← scp the image to a Pi and run usb_update.py
├── flash-direct.sh      ← flash directly from your build host (no Pi)
└── watch-uart.sh        ← tail the captured UART log on the Pi
```

All scripts are environment-variable driven. Defaults are at the
top of each script. None of them write outside their own target
directory or the Pi's `~/xous-flash/` folder.

---

## Brick prevention — read before flashing

A careless flash can require JTAG to recover, which is a
multi-hour user-assisted operation. The rules:

- **NEVER flash gateware (`--soc` / `--factory-reset`) without
  explicit per-invocation user authorization.** Mid-write
  interrupts require JTAG.
- **NEVER flash the loader (`-l`) without explicit per-invocation
  user authorization.** A broken loader cannot be fixed via USB.
- **Default to `-k` (kernel-only).** Kernel-only flashes ARE
  recoverable via USB. Every script in this folder uses `-k`
  and only `-k`. Don't edit them to add `-l` or `--soc`.
- **Verify Precursor is on USB power before flashing.** A flash
  that runs out of power mid-write requires JTAG.
- **Never pipe `usb_update.py` through `head`/`tee`/anything
  that closes stdin.** It needs a stable TTY to drive the
  device. The scripts use `> file 2>&1` redirection (which
  keeps stdin open).
- **Save flash output to a file** (`/tmp/flash-*.log`). The
  flash takes ~25 minutes and the SSH session may drop in the
  meantime — file output survives.
- If the device gets bricked despite precautions, **stop and
  ask** — don't try to recover via JTAG without an explicit
  separate authorization.

---

## What you need

**Always:**
- A working `dev`-branch checkout of this repo
  ([`../tests/README.md`](../tests/README.md) for branch
  conventions; [`../BUILDING.md`](../BUILDING.md) for toolchain
  setup)
- A xous-core checkout adjacent to this repo (default
  `../xous-core`; override via `XOUS_CORE_DIR`)
- A Precursor PVT2 in the loader window (hold power 5s during
  boot until the loader window appears)

**For the Pi rig path (recommended):**
- A Raspberry Pi 4B with the betrusted debug HAT, wired to your
  Precursor's USB and UART
- SSH access to the Pi
- `usb_update.py` already copied to `~/xous-flash/` on the Pi
  (one-time setup below)
- A long-running screen session on the Pi capturing UART to
  `~/uart-logs/precursor-uart.log` (one-time setup below)

**For the direct-from-host path:**
- The Precursor connected by USB to your build host
- Permission to talk to USB device `1209:5bf0` (a udev rule is
  the right answer; `sudo` is the short-term escape hatch)

---

## One-time Pi rig setup

Set the SSH target:

```sh
export PI_HOST=pi@10.0.0.42        # your Pi's user@ip
export PI_FLASH_DIR='~/xous-flash' # Pi-side directory for xous.img + usb_update.py
```

Copy `usb_update.py` to the Pi once:

```sh
ssh "$PI_HOST" 'mkdir -p ~/xous-flash ~/uart-logs'
scp /path/to/xous-core/tools/usb_update.py "$PI_HOST":"$PI_FLASH_DIR/usb_update.py"
```

Start the long-running UART capture (re-run after Pi reboots):

```sh
ssh "$PI_HOST" 'screen -dmS uart -L -Logfile ~/uart-logs/precursor-uart.log /dev/ttyAMA0 115200'
```

(Some Pi/HAT combinations route UART to `/dev/ttyS0` or
`/dev/serial0` instead — check `dmesg | grep tty` if the log
stays empty.)

Confirm everything:

```sh
ssh "$PI_HOST" 'lsusb | grep 1209 && ls $PI_FLASH_DIR/usb_update.py && screen -ls | grep uart'
```

You should see `1209:5bf0` (Precursor in loader window), the
script path, and an attached `uart` screen session.

---

## Running a hardware test (the dev cycle)

1. **Edit code** on the `dev` branch (see
   [`../tests/README.md`](../tests/README.md) for the branch
   convention).

2. **Run unit tests** (a few seconds — catches obvious breakage
   before the 30-minute hardware loop):
   ```sh
   cargo test --features hosted -p xous-app-signal --bins
   ```

3. **Optionally run the hosted-mode link smoke test** (a few
   minutes — catches link-flow regressions without burning a
   flash cycle):
   ```sh
   bash tests/hosted/test_link_qr.sh
   ```

4. **Build and bundle a kernel image** (~3-5 minutes):
   ```sh
   bash precursor/build-and-bundle.sh
   ```
   Output: `<xous-core>/target/precursor-c809403e/release/xous.img`.
   Override `XOUS_CORE_DIR` / `XOUS_TARGET` if your layout
   differs.

5. **Flash** (~25 minutes; do not unplug the Precursor):

   With the Pi rig (laptop is free during the flash):
   ```sh
   bash precursor/flash-via-pi.sh
   ```

   Direct from this host (ties up the laptop for the flash):
   ```sh
   bash precursor/flash-direct.sh
   ```

   Both scripts only use `-k` (kernel-only, recoverable). Both
   confirm `1209:5bf0` is visible before doing anything. Both
   redirect output to a `/tmp/flash-*.log` file.

6. **Watch UART** (in another terminal, during or after the
   flash):
   ```sh
   bash precursor/watch-uart.sh
   ```
   The script tails `~/uart-logs/precursor-uart.log` on the Pi.
   Set `FOLLOW=0` for a one-shot last-200-lines snapshot.

7. **On the device:** unlock PDDB → join Wi-Fi (`wlan setup` in
   shellchat if needed) → open xas → exercise the feature you're
   testing.

8. **Analyze.** If reproducing a bug, diff against a known-good
   baseline (see "Capturing a baseline" below). If iterating,
   loop back to step 1.

---

## Capturing a baseline

Before debugging a broken case, capture a "boots cleanly, xas
not opened" UART log. Future runs can diff against it:

```sh
bash precursor/build-and-bundle.sh
bash precursor/flash-via-pi.sh
# Let Precursor boot to launcher; don't open xas. Wait ~10s.
ssh "$PI_HOST" 'cp ~/uart-logs/precursor-uart.log ~/uart-logs/baseline.log'

# Later:
ssh "$PI_HOST" 'diff ~/uart-logs/baseline.log ~/uart-logs/precursor-uart.log'
```

---

## What you can and can't see from UART

**You CAN:**

- Reproduce a bug, capture UART, analyze it
- Identify panic locations from `panicked at file:line` lines
- Identify watchdog resets (loader banner appearing without a
  preceding panic)
- Trace protocol state from `log::info!` calls in xas
- Compare across runs (baseline vs broken)

**You CANNOT:**

- Set breakpoints or step through code (that's JTAG/GDB —
  separate session, separate authorization)
- Modify the running kernel (UART is read-only)
- Test changes without rebuilding and reflashing
- See output from before the kernel started running (loader
  output yes; pre-loader FPGA gateware no)
- See WF200 SPI traffic or smoltcp internal queues without
  explicit instrumentation

---

## Gotchas

- **UART output stops if Precursor sleeps/suspends.** Disable
  auto-suspend in shellchat before long debug sessions:
  `susres autosuspend off`.

- **The serial mux is exclusive:** `gdbserver` in shellchat
  routes logs to the debug header, but it's the same UART GDB
  would use. You can't have GDB and live logging at the same
  time.

- **WF200 errors don't auto-log.** They surface as smoltcp /
  `std::io` errors in xas only if the code path explicitly
  logs them.

- **Pi heat:** the Pi 4B under sustained load can throttle on
  long sessions — `ssh "$PI_HOST" 'vcgencmd measure_temp'` to
  check.

- **Stale screen session:** if a script crashes and leaves a
  broken `uart` screen session, kill it manually:
  ```sh
  ssh "$PI_HOST" 'screen -S uart -X quit'
  ```
  Then re-run the screen-session command from "One-time Pi rig
  setup" above.

---

## Environment variables

| Var | Default | Purpose |
|---|---|---|
| `PI_HOST` | (required for `flash-via-pi.sh` / `watch-uart.sh`) | `user@host` for SSH/SCP |
| `PI_FLASH_DIR` | `~/xous-flash` | Pi-side directory for `xous.img` + `usb_update.py` |
| `PI_UART_LOG` | `~/uart-logs/precursor-uart.log` | Pi-side path of the UART log |
| `XOUS_CORE_DIR` | `../xous-core` | Path to your xous-core checkout |
| `XOUS_TARGET` | `precursor-c809403e` | xtask target for `app-image-xip` |
| `BUILD_LOG` | `/tmp/xous-build-$(date +%s).log` | Build stdout/stderr |
| `FLASH_LOG` | `/tmp/flash-$(date +%s).log` | Flash stdout/stderr (Pi-side for `flash-via-pi.sh`) |
| `FOLLOW` | `1` | `watch-uart.sh`: 1 = `tail -F`, 0 = last 200 lines |

Override at the call site:

```sh
PI_HOST=pi@192.168.1.50 bash precursor/flash-via-pi.sh
```
