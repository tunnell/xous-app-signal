#!/usr/bin/env bash
# Build xas + bundle a kernel image for Precursor PVT2 hardware.
#
# Output: <xous-core>/target/<target>/release/xous.img
#
# Env vars (defaults shown):
#   XOUS_CORE_DIR=../xous-core
#   XOUS_TARGET=precursor-c809403e
#   BUILD_LOG=/tmp/xous-build-$(date +%s).log
#
# Safe to re-run; cargo will skip unchanged crates.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
XOUS_CORE_DIR="${XOUS_CORE_DIR:-$REPO_ROOT/../xous-core}"
XOUS_TARGET="${XOUS_TARGET:-precursor-c809403e}"
BUILD_LOG="${BUILD_LOG:-/tmp/xous-build-$(date +%s).log}"

if [[ ! -d "$XOUS_CORE_DIR" ]]; then
    echo "ERROR: xous-core not found at $XOUS_CORE_DIR" >&2
    echo "Set XOUS_CORE_DIR to your xous-core checkout." >&2
    exit 1
fi

XAS_BIN="$REPO_ROOT/target/riscv32imac-unknown-xous-elf/release/xas"

echo "==> Building xas (release, hardware target)"
echo "    repo:        $REPO_ROOT"
echo "    xous-core:   $XOUS_CORE_DIR"
echo "    target:      $XOUS_TARGET"
echo "    build log:   $BUILD_LOG"
echo

# Step 1: build xas itself with the hardware sysroot.
cd "$REPO_ROOT"
cargo build --release \
    --target riscv32imac-unknown-xous-elf \
    -p xous-app-signal \
    --features pddb-real \
    > "$BUILD_LOG" 2>&1 || {
        echo "xas build FAILED — see $BUILD_LOG" >&2
        tail -50 "$BUILD_LOG" >&2
        exit 1
    }

if [[ ! -f "$XAS_BIN" ]]; then
    echo "ERROR: expected xas binary not found at $XAS_BIN" >&2
    echo "Check $BUILD_LOG for build errors." >&2
    exit 1
fi

echo "==> xas built: $XAS_BIN ($(du -h "$XAS_BIN" | cut -f1))"

# Step 2: bundle into a xous.img alongside the standard apps.
echo "==> Bundling kernel image (cargo xtask app-image-xip)"
cd "$XOUS_CORE_DIR"
cargo xtask app-image-xip \
    --feature pddb \
    "$XAS_BIN" \
    >> "$BUILD_LOG" 2>&1 || {
        echo "image bundling FAILED — see $BUILD_LOG" >&2
        tail -50 "$BUILD_LOG" >&2
        exit 1
    }

XOUS_IMG="$XOUS_CORE_DIR/target/$XOUS_TARGET/release/xous.img"
if [[ ! -f "$XOUS_IMG" ]]; then
    echo "ERROR: expected xous.img not found at $XOUS_IMG" >&2
    echo "Check XOUS_TARGET (currently '$XOUS_TARGET')." >&2
    exit 1
fi

SHA="$(sha256sum "$XOUS_IMG" | cut -c1-12)"
SIZE="$(du -h "$XOUS_IMG" | cut -f1)"
echo
echo "==> Built kernel image:"
echo "    path:    $XOUS_IMG"
echo "    size:    $SIZE"
echo "    sha256:  ${SHA}…"
echo
echo "Next: bash tests/precursor/flash-via-pi.sh   (or flash-direct.sh)"
