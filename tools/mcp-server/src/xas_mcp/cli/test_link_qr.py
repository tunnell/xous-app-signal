"""``python -m xas_mcp.cli.test_link_qr`` — replaces ``tests/hosted/test_link_qr.sh``.

Runs the hosted-mode link-QR regression. The bash script is still
on disk and does the heavy lifting (X11, kernel boot, keystroke
injection); this CLI just calls `run_hosted_test('link_qr')`, which
adds the orphan-kernel pre-cleanup and the xvfb-auto-wrap that the
bash script doesn't include.
"""

from __future__ import annotations

import argparse
import os
import sys

from xas_mcp.config import load_config
from xas_mcp.tests_hosted import run_hosted_test

from ._common import (
    add_common_args,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-test-link-qr",
        description=(
            "Hosted-mode link-QR regression. Boots a Xous hosted kernel, "
            "drives xas through the link flow, asserts the link URL "
            "reaches the UI's modal-open path. Replaces "
            "tests/hosted/test_link_qr.sh; adds orphan-kernel pre-cleanup "
            "and xvfb-run auto-wrap on headless hosts."
        ),
    )
    parser.add_argument(
        "--no-cleanup",
        action="store_true",
        help="Skip the orphan-kernel pkill that runs before launch.",
    )
    parser.add_argument(
        "--timeout-sec",
        type=int,
        default=60 * 15,
        help="Hard timeout for the test (default: 900s).",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        cfg = load_config(env=dict(os.environ))
        result = run_hosted_test(
            "link_qr",
            config=cfg,
            cleanup_orphans=not args.no_cleanup,
            timeout_sec=args.timeout_sec,
        )

        def fmt(d: dict[str, object]) -> str:
            head = (
                f"==> link_qr {'PASS' if d.get('pass') else 'FAIL'} "
                f"(rc={d.get('returncode')}) in {d.get('duration_sec')}s"
            )
            log_lines = str(d.get("log", "")).splitlines()
            tail = "\n".join(log_lines[-20:])
            return head + "\n" + tail

        json_or_format(result, as_json=args.json, format_human=fmt)
        return int(result.get("returncode", 1) or 0) if not result.get("pass") else 0

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
