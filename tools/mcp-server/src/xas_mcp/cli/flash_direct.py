"""``python -m xas_mcp.cli.flash_direct`` — replaces ``tests/precursor/flash-direct.sh``.

Flash directly from this host (no Pi rig). Blocks until the write
finishes. Same brick-prevention rules as flash_via_pi: ``-k --bounce``
only.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from xas_mcp.config import load_config
from xas_mcp.flash import flash_direct

from ._common import (
    add_common_args,
    env_default,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-flash-direct",
        description=(
            "Flash xous.img to a Precursor connected directly to this host "
            "(no Pi rig). Ties up the build host for ~25 minutes. "
            "Replaces tests/precursor/flash-direct.sh."
        ),
        epilog=(
            "SAFETY: kernel-only flash (-k --bounce). Recoverable via USB. "
            "Do not edit this tool to add -l or --soc / --factory-reset."
        ),
    )
    parser.add_argument(
        "image",
        nargs="?",
        default=None,
        help="Path to xous.img. Defaults to the canonical build output.",
    )
    parser.add_argument(
        "--xous-core-dir",
        default=env_default("XOUS_CORE_DIR", None),
        help="Path to your xous-core checkout (env: XOUS_CORE_DIR).",
    )
    parser.add_argument(
        "--xous-target",
        default=env_default("XOUS_TARGET", None),
        help="xtask target name (env: XOUS_TARGET).",
    )
    parser.add_argument(
        "--log-path",
        default=env_default("FLASH_LOG", None),
        help="Path for the flash log (env: FLASH_LOG; default: /tmp/flash-<epoch>.log).",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        env_overrides: dict[str, str] = {}
        for arg_name, env_name in (
            ("xous_core_dir", "XOUS_CORE_DIR"),
            ("xous_target", "XOUS_TARGET"),
        ):
            v = getattr(args, arg_name)
            if v:
                env_overrides[env_name] = v
        cfg = load_config(env={**os.environ, **env_overrides})

        result = flash_direct(
            config=cfg,
            image_path=Path(args.image) if args.image else None,
            log_path=Path(args.log_path) if args.log_path else None,
        )

        def fmt(d: dict[str, object]) -> str:
            rc = d.get("returncode")
            log = d.get("log_path")
            head = f"==> Flash exited with code {rc}"
            tail_hint = (
                f"    full log: {log}"
                if rc != 0
                else "==> Flash complete. Precursor should reboot into the new kernel."
            )
            return head + "\n" + tail_hint

        json_or_format(result, as_json=args.json, format_human=fmt)
        return int(result.get("returncode", 1) or 0)

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
