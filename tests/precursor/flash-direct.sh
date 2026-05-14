#!/usr/bin/env bash
# Thin shim. Real logic in
# tools/mcp-server/src/xas_mcp/cli/flash_direct.py.
#
# Behavior is identical to the old standalone script: same env vars
# (XOUS_CORE_DIR, XOUS_TARGET, FLASH_LOG), same kernel-only safety
# posture (-k --bounce only).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
exec env PYTHONPATH="$REPO_ROOT/tools/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 -m xas_mcp.cli.flash_direct "$@"
