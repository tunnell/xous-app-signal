"""``python -m xas_mcp.cli.flash_via_pi`` — replaces ``tests/precursor/flash-via-pi.sh``.

scp's a built xous.img to the Pi and kicks off a screen-detached
``usb_update.py -k --bounce``. Returns immediately with the Pi-side
log path; poll progress via ``python -m xas_mcp.cli.flash_status``.

Brick prevention: ``-k --bounce`` only. Loader / SoC / factory-reset
flashes are intentionally not supported by this CLI.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from xas_mcp.config import load_config
from xas_mcp.flash import flash_pi

from ._common import (
    add_common_args,
    env_default,
    format_dict,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-flash-via-pi",
        description=(
            "Flash xous.img to a Precursor via a Raspberry Pi rig. "
            "scp's the image, then launches usb_update.py inside a "
            "screen-detached + nohup wrapper so an SSH disconnect can't "
            "kill the write. Returns immediately; poll progress via "
            "`python -m xas_mcp.cli.flash_status <log_path>`. "
            "Replaces tests/precursor/flash-via-pi.sh."
        ),
        epilog=(
            "SAFETY: kernel-only flash (-k --bounce). Recoverable via USB. "
            "Do not edit this tool to add -l or --soc / --factory-reset "
            "without reading tests/precursor/README.md 'Brick prevention'."
        ),
    )
    parser.add_argument(
        "image",
        nargs="?",
        default=None,
        help=(
            "Path to xous.img. Defaults to "
            "$XOUS_CORE_DIR/target/$XOUS_TARGET/release/xous.img."
        ),
    )
    parser.add_argument(
        "--pi-host",
        default=env_default("PI_HOST", None),
        help="user@host of the Pi rig (env: PI_HOST). Required.",
    )
    parser.add_argument(
        "--pi-flash-dir",
        default=env_default("PI_FLASH_DIR", None),
        help="Pi-side directory holding usb_update.py (env: PI_FLASH_DIR; default: ~/xous-flash).",
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
        "--log-name",
        default=env_default("FLASH_LOG", None),
        help="Filename for the Pi-side flash log (env: FLASH_LOG; default: flash-<epoch>.log).",
    )
    parser.add_argument(
        "--no-robust",
        action="store_true",
        help=(
            "Disable the screen-detached wrapper and run the flash in "
            "the foreground. Useful in tests; never in production — an "
            "SSH drop will kill the write."
        ),
    )
    parser.add_argument(
        "--skip-lsusb",
        action="store_true",
        help="Skip the pre-flash 1209:5bf0 visibility check on the Pi.",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        env_overrides: dict[str, str] = {}
        for arg_name, env_name in (
            ("pi_host", "PI_HOST"),
            ("pi_flash_dir", "PI_FLASH_DIR"),
            ("xous_core_dir", "XOUS_CORE_DIR"),
            ("xous_target", "XOUS_TARGET"),
        ):
            v = getattr(args, arg_name)
            if v:
                env_overrides[env_name] = v
        cfg = load_config(env={**os.environ, **env_overrides})
        cfg.require_pi_host()  # fail fast on missing PI_HOST

        image_path = Path(args.image) if args.image else None
        result = flash_pi(
            config=cfg,
            image_path=image_path,
            log_name=args.log_name,
            robust=not args.no_robust,
            skip_lsusb=args.skip_lsusb,
        )

        def fmt(d: dict[str, object]) -> str:
            session = d.get("screen_session")
            log = d.get("pi_log_path")
            lines = [
                f"==> Flash kicked off on {d.get('host')}",
                f"    image: {d.get('image_path')}",
                f"    log on Pi: {log}",
                f"    screen session: {session}",
                "",
                "Watch progress:",
                f"    python -m xas_mcp.cli.flash_status {log} --session {session}",
                "Watch UART (in another terminal):",
                "    python -m xas_mcp.cli.watch_uart",
            ]
            return "\n".join(lines)

        json_or_format(result, as_json=args.json, format_human=fmt)
        return 0

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
