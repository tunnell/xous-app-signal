#!/usr/bin/env bash
# Thin shim. Real logic in
# tools/mcp-server/src/xas_mcp/cli/test_link_qr.py.
#
# Behavior matches the old standalone script; same structured exit
# codes (0=pass, 1=generic, 2=boot timeout, 3=window not found,
# 4=link URL never emitted). The Python entry point additionally
# kills orphan xous-kernel processes before launching (per the
# feedback_pretest_kernel_cleanup memory) and auto-wraps the run in
# xvfb-run when $DISPLAY is empty.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
exec env PYTHONPATH="$REPO_ROOT/tools/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 -m xas_mcp.cli.test_link_qr "$@"
