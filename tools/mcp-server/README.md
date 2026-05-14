# xas-mcp — Python MCP server wrapping xas testing infrastructure

One Python package, two surfaces:

- **MCP tools** (stdio transport) — agents like Claude Code discover and call them via the [Model Context Protocol](https://modelcontextprotocol.io/).
- **CLI wrappers** — humans run the same operations from a shell. Each tool has a matching `python -m xas_mcp.cli.<name>` entry point and a console script.

Every existing bash script in `tests/precursor/`, `tests/hosted/`, and `tests/renode/` is now a one-line shim that execs the corresponding Python CLI. Behavior is intentionally identical; the env-var surface is preserved, and every flag the script exposed is also exposed via CLI argument.

## Why this exists

Three problems with the bash scripts that the MCP server fixes at once:

1. **Robustness gap on flash**: `tests/precursor/flash-via-pi.sh` ran `usb_update.py` over a bare ssh; an SSH disconnect mid-flash killed python3 on the Pi. The Python `flash_pi` tool wraps every long-running Pi-side operation in `screen -dmS … bash -c "nohup …"`, so the write keeps going even if the local process is killed.
2. **Env-var drift across scripts**: `build-and-bundle.sh`, `flash-via-pi.sh`, and `flash-direct.sh` parsed `XOUS_TARGET` independently with mismatched defaults. Now there is one `Config` dataclass with one set of defaults.
3. **Invisible to agents**: scripts couldn't be introspected by Claude Code or other MCP-aware tools. The MCP surface fixes that without removing the CLI surface humans rely on.

## MCP framework

This server uses [`FastMCP`](https://github.com/modelcontextprotocol/python-sdk) from the official Python MCP SDK (`mcp>=1.2`). Stdio transport for local agent use.

## Quickstart — agent

Register the server with Claude Code (or any MCP client):

```jsonc
// ~/.claude/settings.json (or equivalent)
{
  "mcpServers": {
    "xas": {
      "command": "python",
      "args": ["-m", "xas_mcp.server"],
      "env": {
        "PI_HOST": "pi@10.0.0.42",
        "XOUS_CORE_DIR": "/abs/path/to/xous-core"
      }
    }
  }
}
```

The server registers every tool listed in the [Tool catalog](#tool-catalog) below. Tool descriptions are taken verbatim from each function's docstring.

## Quickstart — human

Install in editable mode (Python ≥3.10):

```sh
cd tools/mcp-server
pip install -e '.[dev]'
cp .env.example .env       # edit as needed; gitignored
```

Run a tool directly from the CLI:

```sh
# Build xas + bundle a kernel image
python -m xas_mcp.cli.build_and_bundle

# Flash via Pi rig (robust — screen-detached on the Pi)
PI_HOST=pi@10.0.0.42 python -m xas_mcp.cli.flash_via_pi

# Poll a running flash
python -m xas_mcp.cli.flash_status /tmp/flash-1715712000.log

# Watch UART (follow mode by default)
python -m xas_mcp.cli.watch_uart
python -m xas_mcp.cli.watch_uart --lines 200    # one-shot snapshot

# Renode + hosted tests
python -m xas_mcp.cli.run_renode_tests xas-smoke.robot
python -m xas_mcp.cli.test_link_qr
```

Add `--json` to any CLI for machine-readable output.

## Script → Python mapping

| Existing script | Python CLI | Underlying tool |
|---|---|---|
| `tests/precursor/build-and-bundle.sh` | `xas_mcp.cli.build_and_bundle` | `build_xas` + `bundle_kernel_image` |
| `tests/precursor/flash-via-pi.sh` | `xas_mcp.cli.flash_via_pi` | `lsusb_pi` + `flash_pi` |
| `tests/precursor/flash-direct.sh` | `xas_mcp.cli.flash_direct` | `flash_direct` |
| `tests/precursor/watch-uart.sh` | `xas_mcp.cli.watch_uart` | `read_uart` / `tail_uart` |
| *(new)* | `xas_mcp.cli.flash_status` | `flash_status` |
| `tests/renode/run-renode-tests.sh` | `xas_mcp.cli.run_renode_tests` | `run_renode_test` |
| `tests/hosted/test_link_qr.sh` | `xas_mcp.cli.test_link_qr` | `run_hosted_test` |

## Tool catalog

| Tool | Purpose |
|---|---|
| `build_xas` | Cross-compile xas (release, hardware target). Returns `{path, size, sha256}`. |
| `bundle_kernel_image` | Bundle xous.img via `cargo xtask app-image-xip`. Returns `{path, size, sha256}`. |
| `lsusb_pi` | Check Precursor USB enumeration on the Pi. Returns `{visible, vid_pid, device_id}`. |
| `flash_pi` | scp xous.img + start screen-detached `usb_update.py` on the Pi. Returns immediately with `{pi_log_path, screen_session}`. |
| `flash_direct` | Same flash from the build host (no Pi rig). |
| `flash_status` | Poll a running flash. Returns `{running, percent, eta_sec, last_line}`. |
| `read_uart` | Hardcopy of the Pi's UART buffer (last N lines). |
| `tail_uart` | Stream UART live (callback / generator). |
| `parse_uart_perf` | Extract structured timings from instrumented-build UART logs. |
| `run_renode_test` | Build xas + run a Robot script under Renode. |
| `run_hosted_test` | Run a hosted-mode integration test (auto-wraps in xvfb-run if no $DISPLAY). |
| `cargo_test` | `cargo test` for a package with optional features. |
| `ssh_pi` / `scp_to_pi` / `scp_from_pi` | Generic escape hatches — prefer named tools above for new code. |
| `pi_screen_uart_status` | Is the persistent UART screen session alive on the Pi? |

Each tool returns a JSON-serializable dict (or a string for raw UART hardcopies). On error, exceptions surface as MCP error responses; the CLI converts them to a nonzero exit and a stderr line.

## Configuration

`xas_mcp.config.Config` loads from environment variables and an optional `.env` file (in `tools/mcp-server/.env`, gitignored). See `.env.example` for the full list. Key defaults:

| Field | Env var | Default |
|---|---|---|
| Pi user@host | `PI_HOST` | *(required for Pi tools)* |
| Pi flash dir | `PI_FLASH_DIR` | `~/xous-flash` |
| Pi UART log | `PI_UART_LOG` | `~/uart-logs/precursor-uart.log` |
| xous-core checkout | `XOUS_CORE_DIR` | `../xous-core` (relative to repo root) |
| xtask target | `XOUS_TARGET` | `riscv32imac-unknown-xous-elf` |
| SoC version pin | `GIT_DESCRIBE` | `v0.9.8-791-gc707f9d8` |
| SoC version rev | `GIT_REV` | `c707f9d8` |

`XOUS_TARGET` deliberately defaults to the cargo target triple in every entry point — `precursor-c809403e` is a legacy alias that causes `xous.img` to land in a different directory than the build step writes to. See `notes/chores/CHORES.md` for history.

## Robustness — screen-detached pattern

Every long-running Pi-side operation goes through `xas_mcp.ssh.screen_detached(cmd, session_name=, log_path=)`, which wraps the command in:

```sh
screen -dmS <session_name> bash -c 'nohup <cmd> > <log_path> 2>&1'
```

The local Python returns immediately with `{screen_session, log_path}`. The caller polls (e.g., `flash_status`) until the screen session is gone. Killing the local Python process, dropping the SSH connection, or rebooting the build host does **not** interrupt the Pi-side write.

If you see a flash hang halfway through, check `screen -ls` on the Pi — the session may still be alive even if your local console seems stuck.

## Testing

```sh
# Unit tests (no hardware required)
pytest tools/mcp-server/tests/

# Type check + lint
mypy --strict tools/mcp-server/src/xas_mcp
ruff check tools/mcp-server/src/xas_mcp

# Integration tests (require a reachable Pi + Precursor in loader mode)
XAS_MCP_INTEGRATION=1 pytest tools/mcp-server/tests/
```

CLI smoke check:

```sh
for m in build_and_bundle flash_via_pi flash_direct flash_status watch_uart \
         run_renode_tests test_link_qr; do
  python -m "xas_mcp.cli.$m" --help >/dev/null && echo "$m ok"
done
```

## Safety posture

- **No secrets in the repo.** `.env` is gitignored; only `.env.example` is committed.
- **No vendored crypto modifications.** This package wraps testing infrastructure; it does not touch `vendor/libsignal-service-rs/`, `vendor/presage/`, `vendor/curve25519-dalek/`, or any crypto crate.
- **Kernel-only flashes only.** Both `flash_pi` and `flash_direct` invoke `usb_update.py -k --bounce` — never `-l` (loader), `--soc`, or `--factory-reset`. See `tests/precursor/README.md` "Brick prevention" for why.
