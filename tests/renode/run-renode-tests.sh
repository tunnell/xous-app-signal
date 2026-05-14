#!/usr/bin/env bash
# Thin shim. Real logic in
# tools/mcp-server/src/xas_mcp/cli/run_renode_tests.py.
#
# Behavior matches the old standalone script. Default robot file is
# xas-smoke.robot; pass another .robot name as the first positional
# argument. Same env vars (RENODE, XAS_DIST_DIR).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
exec env PYTHONPATH="$REPO_ROOT/tools/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 -m xas_mcp.cli.run_renode_tests "$@"
