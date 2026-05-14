#!/usr/bin/env bash
# Thin shim. Real logic in
# tools/mcp-server/src/xas_mcp/cli/build_and_bundle.py.
#
# Behavior is identical to the old standalone script: same env vars
# (XOUS_CORE_DIR / XOUS_TARGET / GIT_DESCRIBE / GIT_REV / BUILD_LOG),
# same exit codes (0 on success, nonzero on failure). The Python
# entry point also accepts flag forms — see `--help` for the full
# list.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
exec env PYTHONPATH="$REPO_ROOT/tools/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 -m xas_mcp.cli.build_and_bundle "$@"
