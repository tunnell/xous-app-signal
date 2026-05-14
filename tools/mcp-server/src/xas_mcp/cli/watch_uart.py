"""``python -m xas_mcp.cli.watch_uart`` — replaces ``tests/precursor/watch-uart.sh``.

Two modes (mirror the bash script):

- Default: ``tail -F`` the UART log on the Pi (Ctrl-C to stop).
- ``--lines N``: one-shot snapshot of the last N lines.

Add ``--perf`` to switch the output to structured perf/* parsing
of whatever's collected.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

from xas_mcp.config import load_config
from xas_mcp.uart import parse_uart_perf, read_uart, tail_uart

from ._common import add_common_args, env_default, run_with_error_handling


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-watch-uart",
        description=(
            "Watch the Pi's captured UART log. Default: stream live via "
            "ssh tail -F (Ctrl-C to stop). Use --lines N for a one-shot "
            "snapshot. Use --perf to parse iter-1 instrumented build "
            "output into structured key=value rows. Replaces "
            "tests/precursor/watch-uart.sh."
        ),
    )
    parser.add_argument(
        "--pi-host",
        default=env_default("PI_HOST", None),
        help="user@host of the Pi rig (env: PI_HOST).",
    )
    parser.add_argument(
        "--pi-uart-log",
        default=env_default("PI_UART_LOG", None),
        help="Pi-side path of the UART log (env: PI_UART_LOG).",
    )
    parser.add_argument(
        "--lines",
        type=int,
        default=None,
        metavar="N",
        help="Snapshot mode: print the last N lines and exit (no streaming).",
    )
    parser.add_argument(
        "--max-lines",
        type=int,
        default=None,
        metavar="N",
        help="Stop streaming after N lines (default: stream until Ctrl-C).",
    )
    parser.add_argument(
        "--until",
        default=None,
        metavar="REGEX",
        help="Stop streaming on the first line matching REGEX.",
    )
    parser.add_argument(
        "--perf",
        action="store_true",
        help="Parse perf/* lines into structured rows instead of raw text.",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        env_overrides: dict[str, str] = {}
        if args.pi_host:
            env_overrides["PI_HOST"] = args.pi_host
        if args.pi_uart_log:
            env_overrides["PI_UART_LOG"] = args.pi_uart_log
        cfg = load_config(env={**os.environ, **env_overrides})
        cfg.require_pi_host()

        if args.lines is not None:
            text = read_uart(config=cfg, lines=args.lines)
            if args.perf:
                parsed = parse_uart_perf(text)
                print(json.dumps(parsed, indent=2) if args.json else _format_perf(parsed))
            else:
                if args.json:
                    print(json.dumps({"text": text}, indent=2))
                else:
                    print(text, end="" if text.endswith("\n") else "\n")
            return 0

        # Streaming mode.
        if args.perf:
            collected: list[str] = []
            for line in tail_uart(
                config=cfg, until_pattern=args.until, max_lines=args.max_lines
            ):
                collected.append(line)
            parsed = parse_uart_perf("\n".join(collected))
            print(json.dumps(parsed, indent=2) if args.json else _format_perf(parsed))
        else:
            for line in tail_uart(
                config=cfg, until_pattern=args.until, max_lines=args.max_lines
            ):
                print(line)
        return 0

    return run_with_error_handling(body, debug=args.debug)


def _format_perf(parsed: dict[str, list[dict[str, object]]]) -> str:
    out: list[str] = []
    for topic, entries in parsed.items():
        out.append(f"==> {topic} ({len(entries)} entries)")
        for e in entries:
            fields = e.get("fields", {})
            kv = " ".join(f"{k}={v}" for k, v in fields.items())
            ts = e.get("ts") or ""
            out.append(f"  {ts} {e.get('payload', '').split(' ')[0]}: {kv}")
    return "\n".join(out) if out else "(no perf/* lines)"


if __name__ == "__main__":
    sys.exit(main())
