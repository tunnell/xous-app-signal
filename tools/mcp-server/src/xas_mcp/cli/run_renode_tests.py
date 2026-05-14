"""``python -m xas_mcp.cli.run_renode_tests`` — replaces ``tests/renode/run-renode-tests.sh``."""

from __future__ import annotations

import argparse
import os
import sys

from xas_mcp.config import load_config
from xas_mcp.tests_renode import run_renode_test

from ._common import (
    add_common_args,
    env_default,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-run-renode-tests",
        description=(
            "Build xas for rv32 and run renode-test against a Robot script. "
            "Replaces tests/renode/run-renode-tests.sh. Default robot file: "
            "xas-smoke.robot."
        ),
    )
    parser.add_argument(
        "robot_file",
        nargs="?",
        default="xas-smoke.robot",
        help="Robot script under tests/renode/ to run.",
    )
    parser.add_argument(
        "--renode",
        default=env_default("RENODE", None),
        help="renode-test binary (env: RENODE).",
    )
    parser.add_argument(
        "--dist-dir",
        default=env_default("XAS_DIST_DIR", None),
        help="Output dir for the built ELF (env: XAS_DIST_DIR; default: dist/xas-rv32).",
    )
    parser.add_argument(
        "--timeout-sec",
        type=int,
        default=60 * 30,
        help="Hard timeout for the renode-test run (default: 1800s).",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        from pathlib import Path

        cfg = load_config(env=dict(os.environ))
        dist = Path(args.dist_dir) if args.dist_dir else None
        result = run_renode_test(
            args.robot_file,
            config=cfg,
            renode_bin=args.renode,
            dist_dir=dist,
            timeout_sec=args.timeout_sec,
        )

        def fmt(d: dict[str, object]) -> str:
            head = (
                f"==> {d.get('robot')} "
                f"{'PASS' if d.get('pass') else 'FAIL'} "
                f"in {d.get('duration_sec')}s"
            )
            log = str(d.get("log", "")).splitlines()
            tail = "\n".join(log[-20:])
            return head + "\n" + tail

        json_or_format(result, as_json=args.json, format_human=fmt)
        return 0 if result.get("pass") else 1

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
