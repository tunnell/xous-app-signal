"""``python -m xas_mcp.cli.flash_status`` — poll a running Pi-side flash.

No bash predecessor; new with the MCP work. Reads the Pi-side flash
log + ``screen -ls`` and reports {running, percent, eta_sec,
last_line, done}.
"""

from __future__ import annotations

import argparse
import os
import sys
import time

from xas_mcp.config import load_config
from xas_mcp.flash import flash_status

from ._common import (
    add_common_args,
    env_default,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-flash-status",
        description=(
            "Poll a running Pi-side flash. Reads the flash log and "
            "screen -ls; prints percent / ETA / last log line. "
            "Run after `flash_via_pi` to watch progress."
        ),
    )
    parser.add_argument(
        "log_path",
        help="Pi-side flash log path (printed by flash_via_pi).",
    )
    parser.add_argument(
        "--session",
        default=None,
        help="Screen session name (printed by flash_via_pi). Used for liveness check.",
    )
    parser.add_argument(
        "--pi-host",
        default=env_default("PI_HOST", None),
        help="user@host of the Pi rig (env: PI_HOST).",
    )
    parser.add_argument(
        "--watch",
        type=int,
        default=0,
        metavar="N",
        help="Poll every N seconds and re-print until done. 0 = single shot (default).",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        env_overrides: dict[str, str] = {}
        if args.pi_host:
            env_overrides["PI_HOST"] = args.pi_host
        cfg = load_config(env={**os.environ, **env_overrides})
        cfg.require_pi_host()

        def fmt(d: dict[str, object]) -> str:
            pct = d.get("percent")
            pct_str = f"{pct}%" if pct is not None else "--"
            eta = d.get("eta_sec")
            eta_str = f"{eta}s" if eta is not None else "--"
            return (
                f"running={d.get('running')} done={d.get('done')} "
                f"percent={pct_str} eta={eta_str} session={d.get('session')}\n"
                f"last: {d.get('last_line', '')}"
            )

        while True:
            result = flash_status(args.log_path, config=cfg, session=args.session)
            json_or_format(result, as_json=args.json, format_human=fmt)
            if args.watch <= 0 or result.get("done"):
                break
            time.sleep(args.watch)
            if not args.json:
                print("---")
        return 0

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
