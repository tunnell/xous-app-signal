"""Tiny helpers shared by every CLI wrapper.

Three rules:
1. CLIs accept the same overrides as the bash script they replace,
   exposed as both flags and the same env var names (env var still
   works for muscle-memory).
2. ``--json`` swaps the human pretty-print for a single JSON dump
   on stdout.
3. Exceptions become a stderr line + nonzero exit, never a traceback,
   unless ``--debug`` is set.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import traceback
from collections.abc import Callable
from typing import Any

__all__ = [
    "format_artifact",
    "format_dict",
    "json_or_format",
    "run_with_error_handling",
    "add_common_args",
]


def add_common_args(parser: argparse.ArgumentParser) -> None:
    """Add ``--json`` and ``--debug`` flags every CLI accepts."""
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit a single JSON object on stdout (machine-readable).",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Print full Python traceback on unexpected errors.",
    )


def format_artifact(result: dict[str, Any]) -> str:
    """Human pretty-print for build/bundle results (path + size + sha)."""
    lines = []
    if "path" in result:
        lines.append(f"path:    {result['path']}")
    if "size_bytes" in result:
        size = result["size_bytes"]
        if size >= 1024 * 1024:
            sz = f"{size / 1024 / 1024:.1f} MiB"
        elif size >= 1024:
            sz = f"{size / 1024:.1f} KiB"
        else:
            sz = f"{size} B"
        lines.append(f"size:    {sz} ({size} bytes)")
    if "sha256" in result:
        lines.append(f"sha256:  {result['sha256'][:16]}…")
    if "log_path" in result:
        lines.append(f"log:     {result['log_path']}")
    return "\n".join(lines)


def format_dict(d: dict[str, Any], *, exclude: tuple[str, ...] = ("raw",)) -> str:
    """Human pretty-print for arbitrary dict results — one ``key: value`` per line."""
    out = []
    for k, v in d.items():
        if k in exclude:
            continue
        if isinstance(v, dict):
            out.append(f"{k}:")
            for sk, sv in v.items():
                out.append(f"  {sk}: {sv}")
        elif isinstance(v, list):
            out.append(f"{k}: [{len(v)} items]")
        else:
            out.append(f"{k}: {v}")
    return "\n".join(out)


def json_or_format(
    result: Any,
    *,
    as_json: bool,
    format_human: Callable[[Any], str],
) -> None:
    """Print ``result`` to stdout, JSON when ``as_json`` else via ``format_human``."""
    if as_json:
        print(json.dumps(result, default=str, indent=2))
    else:
        print(format_human(result))


def run_with_error_handling(
    body: Callable[[], int | None],
    *,
    debug: bool,
) -> int:
    """Run ``body``; on exception, print a short message + return 1 (or traceback on --debug)."""
    try:
        return body() or 0
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return 130
    except SystemExit as e:
        raise e
    except Exception as e:
        if debug:
            traceback.print_exc()
        print(f"error: {e}", file=sys.stderr)
        return 1


def env_default(name: str, fallback: str | None) -> str | None:
    """``argparse`` default that reads the env var ``name`` first."""
    v = os.environ.get(name)
    return v if (v is not None and v != "") else fallback
