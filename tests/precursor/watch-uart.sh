#!/usr/bin/env bash
# Thin shim. Real logic in
# tools/mcp-server/src/xas_mcp/cli/watch_uart.py.
#
# Behavior matches the old standalone script. Default = stream live
# (Ctrl-C to stop). For the old `FOLLOW=0` one-shot mode, pass
# `--lines 200` (or any N). New: `--perf` parses iter-1
# instrumentation lines into structured rows.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Honor the old FOLLOW=0 env var convention.
extra=()
if [[ "${FOLLOW:-1}" == "0" ]]; then
    extra=("--lines" "200")
fi

exec env PYTHONPATH="$REPO_ROOT/tools/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 -m xas_mcp.cli.watch_uart "${extra[@]}" "$@"
