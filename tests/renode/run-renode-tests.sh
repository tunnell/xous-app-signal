#!/usr/bin/env bash
# Stage 9b — Renode test runner wrapper.
#
# Builds the xas rv32 ELF, copies it to a known dist location,
# then invokes `renode-test` against the Stage 9b smoke Robot
# script (or whichever .robot is named on the command line).
#
# Usage:
#   tests/renode/run-renode-tests.sh                  # default: xas-smoke.robot
#   tests/renode/run-renode-tests.sh xas-link-mock.robot
#
# Environment:
#   RENODE        — renode-test binary (default: renode-test)
#   XAS_DIST_DIR  — output dir for the built ELF (default: dist/xas-rv32/)
#
# Prerequisites (see tests/renode/xas-smoke.robot for the long list):
#   - Renode 1.16+ on PATH
#   - rv32-xous rust-std installed (via xous-core toolchain bootstrap)
#   - A Xous image with xas bundled, or path-A integration into
#     xous-core's apps/xas/ done

set -euo pipefail

# Resolve the workspace root from this script's location.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_root="$(cd "$script_dir/../.." && pwd)"

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

# 1. Cross-compile + dist.
cd "$workspace_root"
echo "==> cargo xtask dist"
cargo xtask dist

# 2. Run the Robot test. renode-test's working directory matters
#    for the .resc include resolution, so we cd into the renode dir.
cd "$script_dir"
echo "==> $renode_bin $robot_name"
exec "$renode_bin" "$robot_name"
