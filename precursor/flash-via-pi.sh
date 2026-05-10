#!/usr/bin/env bash
# Flash a built xous.img to a Precursor PVT2 via a Raspberry Pi rig.
#
# Prerequisites:
#   - PI_HOST set (e.g., pi@10.0.0.42)
#   - usb_update.py already copied to $PI_FLASH_DIR on the Pi (see precursor/README.md "Setup once")
#   - Precursor in loader window: lsusb on the Pi shows 1209:5bf0
#   - bash precursor/build-and-bundle.sh has produced xous.img
#
# This script:
#   1. scp's xous.img to the Pi
#   2. Runs `python3 usb_update.py -k xous.img --bounce` over SSH
#      with output redirected to /tmp/flash-*.log on the Pi (stdin
#      stays a TTY so usb_update.py can drive the device)
#   3. Tails the flash log
#
# SAFETY:
#   - This script ONLY uses -k (kernel-only). Kernel-only is recoverable
#     via USB. Never edit this script to add -l or --soc/--factory-reset
#     without reading precursor/AGENT-USAGE.md "Brick prevention" first.
#   - The flash takes ~25 minutes. Don't unplug the Precursor.
#
# Env vars (defaults shown):
#   PI_HOST=         (required)
#   PI_FLASH_DIR=~/xous-flash
#   XOUS_CORE_DIR=../xous-core
#   XOUS_TARGET=precursor-c809403e
#   FLASH_LOG=/tmp/flash-$(date +%s).log   (path on the Pi)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
XOUS_CORE_DIR="${XOUS_CORE_DIR:-$REPO_ROOT/../xous-core}"
XOUS_TARGET="${XOUS_TARGET:-precursor-c809403e}"
PI_FLASH_DIR="${PI_FLASH_DIR:-~/xous-flash}"
FLASH_LOG="${FLASH_LOG:-/tmp/flash-$(date +%s).log}"

if [[ -z "${PI_HOST:-}" ]]; then
    echo "ERROR: PI_HOST not set." >&2
    echo "  export PI_HOST=pi@10.0.0.42" >&2
    exit 1
fi

XOUS_IMG="$XOUS_CORE_DIR/target/$XOUS_TARGET/release/xous.img"
if [[ ! -f "$XOUS_IMG" ]]; then
    echo "ERROR: $XOUS_IMG not found. Run precursor/build-and-bundle.sh first." >&2
    exit 1
fi

# Confirm the Precursor is visible on the Pi BEFORE we copy anything.
echo "==> Checking Precursor visibility on $PI_HOST"
if ! ssh "$PI_HOST" 'lsusb | grep -q "1209:5bf0"'; then
    echo "ERROR: Precursor not seen as 1209:5bf0 on the Pi." >&2
    echo "  - Check the device is in the loader window (hold power for 5s during boot)" >&2
    echo "  - Check USB cable is data-capable, not power-only" >&2
    echo "  - Check it's plugged into the Pi, not the laptop" >&2
    exit 1
fi
echo "    OK — 1209:5bf0 present"

# Check usb_update.py is on the Pi.
echo "==> Checking usb_update.py is on $PI_HOST:$PI_FLASH_DIR"
if ! ssh "$PI_HOST" "test -f $PI_FLASH_DIR/usb_update.py"; then
    echo "ERROR: $PI_FLASH_DIR/usb_update.py not found on Pi." >&2
    echo "  Copy it once with:" >&2
    echo "    scp $XOUS_CORE_DIR/tools/usb_update.py $PI_HOST:$PI_FLASH_DIR/" >&2
    exit 1
fi
echo "    OK"

echo "==> scp'ing xous.img to $PI_HOST:$PI_FLASH_DIR/"
scp "$XOUS_IMG" "$PI_HOST:$PI_FLASH_DIR/xous.img"

echo "==> Flashing kernel-only (-k --bounce)"
echo "    log on Pi: $FLASH_LOG"
echo "    expect ~25 min; do not unplug the Precursor"
echo

# Run flash on the Pi. Important:
#   - Redirect stdout+stderr to a file so a flaky SSH connection
#     doesn't kill the flash mid-write.
#   - Do NOT pipe through tee/head (would close stdin).
#   - The script uses -k (kernel-only, recoverable). It does NOT
#     pass -l (loader) or --soc/--factory-reset (gateware).
ssh "$PI_HOST" "cd $PI_FLASH_DIR && python3 usb_update.py -k xous.img --bounce > $FLASH_LOG 2>&1"
RC=$?

echo "==> Flash command exited with code $RC"
echo "    Last 30 lines of flash log:"
ssh "$PI_HOST" "tail -30 $FLASH_LOG"

if [[ $RC -ne 0 ]]; then
    echo
    echo "Flash FAILED. Full log:" >&2
    echo "  ssh $PI_HOST 'cat $FLASH_LOG'" >&2
    exit $RC
fi

echo
echo "==> Flash complete. Precursor should reboot into the new kernel."
echo "    Watch UART:  bash precursor/watch-uart.sh"
