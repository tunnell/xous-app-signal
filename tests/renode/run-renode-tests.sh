#!/usr/bin/env bash
# Renode test runner wrapper.
#
# Builds the xas rv32 ELF, BUNDLES it into a fresh xous.img in the
# xous-core the .resc boots, then invokes `renode-test` against the
# named Robot script (default: xas-smoke.robot).
#
# The bundle step matters: xas-smoke.resc boots
# `$xous_core_root/target/.../release/xous.img`, so without rebundling
# the freshly-built ELF the test would silently boot whatever stale
# image already sat in the tree (a false green). This wrapper now
# tests the ELF it just built.
#
# Usage:
#   tests/renode/run-renode-tests.sh                  # default: xas-smoke.robot
#   tests/renode/run-renode-tests.sh xas-probe.robot
#
# Environment (defaults shown):
#   RENODE=renode-test          — renode-test binary
#   XOUS_CORE_DIR=<workspace>/repos/xous-core
#                               — xous-core checkout to bundle into +
#                                 boot; MUST match xas-smoke.resc's
#                                 $xous_core_root (default resolves the
#                                 same repos/xous-core symlink)
#   XAS_FEATURES=pddb-real,precursor
#                               — cargo features for the ELF. Probe
#                                 robots need their own: e.g.
#                                 XAS_FEATURES=precursor,probe-flow for
#                                 xas-probe.robot,
#                                 precursor,probe-send-batch for
#                                 xas-send-batch.robot.
#   GIT_DESCRIBE / GIT_REV      — passed to xtask so xous-sign-image
#                                 doesn't fail on `git describe` in a
#                                 tagless fork checkout. Any v-prefixed
#                                 semver works for emulation.
#   SKIP_BUNDLE=1               — reuse the existing xous.img (skip the
#                                 ~2 min image build); only safe when
#                                 the ELF hasn't changed.

set -euo pipefail

# Resolve paths from this script's location.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"          # the xas repo
xous_core_dir="${XOUS_CORE_DIR:-$repo_root/../repos/xous-core}"
xas_features="${XAS_FEATURES:-pddb-real,precursor}"
git_describe="${GIT_DESCRIBE:-v0.9.21-0-g0000000}"
git_rev="${GIT_REV:-g0000000}"

robot_name="${1:-xas-smoke.robot}"
robot_path="$script_dir/$robot_name"

if [ ! -f "$robot_path" ]; then
    echo "error: Robot script not found: $robot_path" >&2
    exit 2
fi

renode_bin="${RENODE:-renode-test}"
if ! command -v "$renode_bin" >/dev/null 2>&1; then
    echo "error: $renode_bin not on PATH (override with RENODE=...)" >&2
    exit 2
fi

if [ ! -d "$xous_core_dir" ]; then
    echo "error: xous-core not found at $xous_core_dir" >&2
    echo "  Set XOUS_CORE_DIR, or create the repos/xous-core symlink per BUILDING.md §1." >&2
    exit 2
fi

# 1. Cross-compile xas for rv32.
cd "$repo_root"
echo "==> building xas (rv32, --features $xas_features)"
cargo build --target riscv32imac-unknown-xous-elf --release \
    -p xous-app-signal --features "$xas_features"
xas_bin="$repo_root/target/riscv32imac-unknown-xous-elf/release/xas"

# 2. Bundle the freshly-built ELF into a xous.img in the checkout the
#    .resc boots, unless SKIP_BUNDLE=1.
if [ "${SKIP_BUNDLE:-0}" != "1" ]; then
    # Preflight: gam's apps.rs must register APP_NAME_XAS or the launcher
    # won't see Signal (same trap as build-and-bundle.sh / BUILDING.md §2.1).
    apps_rs="$xous_core_dir/services/gam/src/apps.rs"
    if [ ! -f "$apps_rs" ] || ! grep -q '\bAPP_NAME_XAS\b' "$apps_rs"; then
        echo "error: $apps_rs missing or lacks APP_NAME_XAS." >&2
        echo "  xous-core is likely on a branch whose apps/manifest.json doesn't" >&2
        echo "  register xas (e.g. dev). Use a branch that does (tunnell/xous-core@xas*)." >&2
        exit 2
    fi
    echo "==> bundling xous.img into $xous_core_dir"
    ( cd "$xous_core_dir" && cargo xtask app-image-xip \
        "xas:$xas_bin" \
        vault \
        transientdisk \
        --kernel-feature big-heap \
        --git-describe "$git_describe" \
        --git-rev "$git_rev" )
else
    echo "==> SKIP_BUNDLE=1: booting the existing xous.img (ELF not rebundled)"
fi

# 3. Run the Robot test. renode-test's working directory matters for the
#    .resc include resolution, so cd into the renode dir.
cd "$script_dir"
echo "==> $renode_bin $robot_name"
exec "$renode_bin" "$robot_name"
