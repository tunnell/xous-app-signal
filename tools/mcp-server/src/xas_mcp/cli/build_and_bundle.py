"""``python -m xas_mcp.cli.build_and_bundle`` — replaces ``tests/precursor/build-and-bundle.sh``.

Builds xas for Precursor hardware, then bundles it into a xous.img
via ``cargo xtask app-image-xip``. Env-var compatible with the bash
script (XOUS_CORE_DIR / XOUS_TARGET / GIT_DESCRIBE / GIT_REV all
honoured). Flags override env which overrides .env which overrides
the baked-in defaults.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from xas_mcp.build import bundle_kernel_image, build_xas
from xas_mcp.config import load_config

from ._common import (
    add_common_args,
    env_default,
    format_artifact,
    json_or_format,
    run_with_error_handling,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="xas-build-and-bundle",
        description=(
            "Build xas + bundle a xous.img for Precursor PVT2 hardware. "
            "Output lands at <xous-core>/target/<target>/release/xous.img. "
            "Same behaviour as tests/precursor/build-and-bundle.sh; this "
            "is the canonical entry point now."
        ),
    )
    parser.add_argument(
        "--xous-core-dir",
        default=env_default("XOUS_CORE_DIR", None),
        help="Path to your xous-core checkout (env: XOUS_CORE_DIR; default: ../xous-core).",
    )
    parser.add_argument(
        "--xous-target",
        default=env_default("XOUS_TARGET", None),
        help="xtask target name (env: XOUS_TARGET; default: riscv32imac-unknown-xous-elf).",
    )
    parser.add_argument(
        "--git-describe",
        default=env_default("GIT_DESCRIBE", None),
        help="SoC version pin --git-describe (env: GIT_DESCRIBE; default per BUILDING.md §3.2).",
    )
    parser.add_argument(
        "--git-rev",
        default=env_default("GIT_REV", None),
        help="SoC version pin --git-rev (env: GIT_REV).",
    )
    parser.add_argument(
        "--features",
        default="pddb-real,precursor",
        help="Comma-separated cargo features for the xas build (default: pddb-real,precursor).",
    )
    parser.add_argument(
        "--no-gdb-stub",
        action="store_true",
        help="Disable --gdb-stub on the bundled image (default: enabled).",
    )
    parser.add_argument(
        "--build-log",
        default=None,
        help="Path for the build log (default: /tmp/xous-build-<epoch>.log).",
    )
    parser.add_argument(
        "--skip-bundle",
        action="store_true",
        help="Only build xas, don't bundle xous.img afterwards.",
    )
    add_common_args(parser)
    args = parser.parse_args(argv)

    def body() -> int:
        # Build a synthetic env so kwargs propagate through load_config.
        env_overrides: dict[str, str] = {}
        if args.xous_core_dir:
            env_overrides["XOUS_CORE_DIR"] = args.xous_core_dir
        if args.xous_target:
            env_overrides["XOUS_TARGET"] = args.xous_target
        if args.git_describe:
            env_overrides["GIT_DESCRIBE"] = args.git_describe
        if args.git_rev:
            env_overrides["GIT_REV"] = args.git_rev

        # Bring in the rest of the process environment so PI_HOST etc.
        # still resolve from the shell.
        import os

        cfg_env = {**os.environ, **env_overrides}
        cfg = load_config(env=cfg_env)

        features = tuple(f.strip() for f in args.features.split(",") if f.strip())
        build_log = Path(args.build_log) if args.build_log else None
        result_build = build_xas(config=cfg, features=features, build_log=build_log)
        if not args.skip_bundle:
            result_bundle = bundle_kernel_image(
                config=cfg, gdb_stub=not args.no_gdb_stub
            )
            combined = {"build": result_build, "bundle": result_bundle}

            def fmt(d: dict[str, dict[str, object]]) -> str:
                return (
                    "==> xas binary\n"
                    + format_artifact(d["build"])
                    + "\n\n==> xous.img\n"
                    + format_artifact(d["bundle"])
                    + f"\n\nNext: python -m xas_mcp.cli.flash_via_pi    # PI_HOST=... required"
                )

            json_or_format(combined, as_json=args.json, format_human=fmt)
        else:
            json_or_format(result_build, as_json=args.json, format_human=format_artifact)
        return 0

    return run_with_error_handling(body, debug=args.debug)


if __name__ == "__main__":
    sys.exit(main())
