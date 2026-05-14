"""``cargo test`` wrapper. Returns aggregate pass/fail counts + raw log.

Lives in its own module so the MCP surface can register a single
``cargo_test`` tool without having to depend on ``tests_renode``.
"""

from __future__ import annotations

import os
import subprocess
import time
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from .config import Config, load_config
from .tests_renode import parse_cargo_test_result

__all__ = ["cargo_test"]


def cargo_test(
    *,
    config: Config | None = None,
    package: str = "xous-app-signal",
    features: Iterable[str] = ("hosted",),
    target: str | None = None,
    extra_args: Iterable[str] = (),
    timeout_sec: int = 60 * 15,
    cargo: str = "cargo",
) -> dict[str, Any]:
    """Run ``cargo test`` for ``package`` with the given features.

    Returns ``{pass, n_passed, n_failed, returncode, duration_sec, log, command}``.
    ``n_passed`` / ``n_failed`` sum every ``test result:`` line in the
    output, so workspaces with multiple test binaries get a total.
    """
    cfg = config or load_config()
    argv = [cargo, "test", "-p", package]
    feat = ",".join(features)
    if feat:
        argv += ["--features", feat]
    if target:
        argv += ["--target", target]
    argv += list(extra_args)

    started = time.time()
    proc = subprocess.run(
        argv,
        cwd=str(cfg.repo_root),
        env=dict(os.environ),
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        check=False,
    )
    duration = time.time() - started
    combined = (proc.stdout or "") + (proc.stderr or "")
    n_passed, n_failed = parse_cargo_test_result(combined)
    return {
        "pass": proc.returncode == 0,
        "n_passed": n_passed,
        "n_failed": n_failed,
        "returncode": proc.returncode,
        "duration_sec": round(duration, 3),
        "log": combined,
        "command": argv,
    }
