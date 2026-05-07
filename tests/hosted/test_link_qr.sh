#!/usr/bin/env bash
# Headless hosted-mode regression test: drive xas through the link
# flow until either Event::LinkUrl is emitted (PASS) or a timeout
# fires (FAIL).
#
# Pass criterion: the worker logs `bridge/link: URL received from
# libsignal: sgnl://linkdevice?...` AND gam_app logs
# `xas/gam_app: link URL = sgnl://linkdevice?...`. Both lines mean
# the URL was generated and routed all the way to the UI's modal-
# open path.
#
# This test does NOT scan the QR (no real phone in headless CI),
# so a successful `LinkComplete` is not expected. The test
# specifically checks that the QR data made it to the UI; what
# happens after (envelope arrival or timeout) is outside scope.
#
# Prerequisites:
# - Xous-core checkout at $XOUS_CORE_DIR (default
#   ~/precursor-signal/repos/xous-core), `xas` branch checked out.
# - xas binary built for hosted at
#   target/release/xas (default $XAS_BIN_PATH).
# - X11 display reachable as $DISPLAY (default localhost:10.0;
#   the test launches Xous with this DISPLAY exported and uses
#   XSendEvent via Python ctypes for keystroke injection).
# - python3 + libX11.so.6 available.
#
# Exit codes:
#   0 — pass
#   1 — generic failure
#   2 — Xous never finished booting
#   3 — could not find Precursor X11 window
#   4 — link URL never emitted within $LINK_TIMEOUT seconds

set -u
set -o pipefail

XOUS_CORE_DIR="${XOUS_CORE_DIR:-$HOME/precursor-signal/repos/xous-core}"
XAS_BIN_PATH="${XAS_BIN_PATH:-$HOME/precursor-signal/xous-app-signal/target/release/xas}"
export DISPLAY="${DISPLAY:-localhost:10.0}"
LINK_TIMEOUT="${LINK_TIMEOUT:-90}"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-180}"

LOG_DIR="$(mktemp -d -t xas-hosted-test.XXXXXX)"
KERNEL_LOG="$LOG_DIR/xous-kernel.log"
DRIVE_LOG="$LOG_DIR/drive.log"

cleanup() {
    pkill -f "xous-kernel" 2>/dev/null || true
    pkill -f "cargo xtask run" 2>/dev/null || true
    if [ "${KEEP_LOGS:-0}" = "1" ]; then
        echo "Logs preserved: $LOG_DIR" >&2
    else
        rm -rf "$LOG_DIR"
    fi
}
trap cleanup EXIT

echo "==> tests/hosted/test_link_qr.sh"
echo "    XOUS_CORE_DIR=$XOUS_CORE_DIR"
echo "    XAS_BIN_PATH=$XAS_BIN_PATH"
echo "    DISPLAY=$DISPLAY"
echo "    LOG_DIR=$LOG_DIR"

# Sanity: binary exists and X11 reachable.
if [ ! -x "$XAS_BIN_PATH" ]; then
    echo "ERROR: xas binary not found at $XAS_BIN_PATH" >&2
    exit 1
fi
if ! xset q >/dev/null 2>&1; then
    echo "ERROR: X server not reachable on $DISPLAY" >&2
    exit 1
fi

# Step 1: kill any in-flight Xous, then launch fresh.
pkill -f "xous-kernel" 2>/dev/null || true
pkill -f "cargo xtask run" 2>/dev/null || true
sleep 1

echo "==> launching hosted Xous"
(
    cd "$XOUS_CORE_DIR"
    cargo xtask run "xas:$XAS_BIN_PATH" >"$KERNEL_LOG" 2>&1
) &
XOUS_BG_PID=$!

echo "    Xous bg PID=$XOUS_BG_PID"

# Step 2: wait for boot — services finish coming up when status
# logs "starting main loop". Generous timeout because cold builds
# can take a couple of minutes.
echo "==> waiting up to ${BOOT_TIMEOUT}s for Xous boot"
WAIT=0
while [ $WAIT -lt $BOOT_TIMEOUT ]; do
    if grep -q "starting main loop" "$KERNEL_LOG" 2>/dev/null; then
        echo "    boot signal seen at t=${WAIT}s"
        break
    fi
    sleep 2
    WAIT=$((WAIT + 2))
done
if ! grep -q "starting main loop" "$KERNEL_LOG" 2>/dev/null; then
    echo "ERROR: Xous never booted within ${BOOT_TIMEOUT}s" >&2
    tail -20 "$KERNEL_LOG" >&2
    exit 2
fi

# Wait an extra few seconds for shellchat to register the launcher
# manifest and gam to settle.
sleep 4

# Step 3: confirm Precursor X11 window is up.
WIN_HEX="$(xdotool search --name "Precursor" 2>/dev/null | head -1)"
if [ -z "$WIN_HEX" ]; then
    echo "ERROR: Precursor X11 window not found" >&2
    exit 3
fi
echo "==> Precursor window=$WIN_HEX"

# Step 4: drive the link flow via XSendEvent. Reuses the proven
# pattern from agent_notes/drive_to_signal.py.
echo "==> driving keystrokes to xas"
python3 "$(dirname "$0")/drive_link.py" "$WIN_HEX" >"$DRIVE_LOG" 2>&1
DRIVE_EC=$?
echo "    drive exit code=$DRIVE_EC"
if [ $DRIVE_EC -ne 0 ]; then
    echo "ERROR: keystroke driver failed" >&2
    cat "$DRIVE_LOG" >&2
    exit 1
fi

# Step 5: poll the kernel log for the URL emission. PASS as soon
# as both bridge and gam_app log it; FAIL on timeout.
echo "==> waiting up to ${LINK_TIMEOUT}s for link URL emission"
WAIT=0
SAW_BRIDGE=0
SAW_GAMAPP=0
while [ $WAIT -lt $LINK_TIMEOUT ]; do
    if grep -q "bridge/link: URL received from libsignal:" "$KERNEL_LOG" 2>/dev/null; then
        SAW_BRIDGE=1
    fi
    if grep -q "xas/gam_app: link URL = sgnl://linkdevice" "$KERNEL_LOG" 2>/dev/null; then
        SAW_GAMAPP=1
    fi
    if [ $SAW_BRIDGE -eq 1 ] && [ $SAW_GAMAPP -eq 1 ]; then
        break
    fi
    sleep 1
    WAIT=$((WAIT + 1))
done

echo
echo "==> result"
echo "    bridge URL log: $SAW_BRIDGE (expect 1)"
echo "    gam_app URL log: $SAW_GAMAPP (expect 1)"
echo
echo "==> link-related lines in kernel log:"
grep -E "bridge/link|gam_app: link URL|gam_app: starting|generating qrcode|provisioning|LinkUrl|LinkComplete|LinkError|Sink closed" "$KERNEL_LOG" || echo "(no link lines found)"

if [ $SAW_BRIDGE -eq 1 ] && [ $SAW_GAMAPP -eq 1 ]; then
    echo
    echo "PASS: link URL reached the UI."
    exit 0
else
    echo
    echo "FAIL: link URL did not reach the UI within ${LINK_TIMEOUT}s." >&2
    echo "(set KEEP_LOGS=1 to keep $LOG_DIR for triage)"
    exit 4
fi
