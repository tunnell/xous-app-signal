"""Renode Robot-test runner.

Mirrors ``tests/renode/run-renode-tests.sh``: builds xas for rv32,
copies the ELF into the dist dir the .resc script expects, then
invokes ``renode-test`` against the requested Robot file.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from .config import Config, load_config

__all__ = ["run_renode_test"]


def run_renode_test(
    robot_file: str = "xas-smoke.robot",
    *,
    config: Config | None = None,
    env: dict[str, str] | None = None,
    renode_bin: str | None = None,
    dist_dir: Path | None = None,
    timeout_sec: int = 60 * 30,
) -> dict[str, Any]:
    """Build xas for rv32 + run ``renode-test`` against ``robot_file``.

    Returns ``{pass, returncode, duration_sec, log, robot, dist_path}``.
    ``log`` is the combined stdout/stderr of the renode-test run, with
    earlier build output omitted (the build is captured in its own
    file under ``/tmp/xous-build-*.log``).
    """
    cfg = config or load_config()
    repo_root = cfg.repo_root
    script_dir = repo_root / "tests" / "renode"
    robot_path = script_dir / robot_file
    if not robot_path.is_file():
        raise RuntimeError(f"Robot script not found: {robot_path}")

    renode = renode_bin or os.environ.get("RENODE", "renode-test")
    if shutil.which(renode) is None:
        raise RuntimeError(
            f"{renode!r} is not on PATH. Install Renode 1.16+ or override "
            f"via the RENODE env var / renode_bin= kwarg."
        )

    # 1. Cross-compile xas for rv32 release (same flags as the bash script).
    triple = "riscv32imac-unknown-xous-elf"
    build_log = Path(f"/tmp/xous-build-renode-{int(time.time())}.log")
    build_argv = [
        "cargo",
        "build",
        "--target",
        triple,
        "--release",
        "-p",
        "xous-app-signal",
        "--features",
        "pddb-real,precursor",
    ]
    with build_log.open("w") as logf:
        bp = subprocess.run(
            build_argv,
            cwd=str(repo_root),
            stdout=logf,
            stderr=subprocess.STDOUT,
            check=False,
        )
    if bp.returncode != 0:
        raise RuntimeError(
            f"xas rv32 build failed (exit {bp.returncode}). See {build_log}."
        )

    # 2. Stage the ELF where the .resc script looks for it.
    dist = dist_dir or (repo_root / "dist" / "xas-rv32")
    dist.mkdir(parents=True, exist_ok=True)
    built = repo_root / "target" / triple / "release" / "xas"
    if not built.is_file():
        raise RuntimeError(f"expected xas ELF at {built} after build")
    target_path = dist / "xas"
    shutil.copyfile(built, target_path)

    # 3. Run renode-test from the renode test dir (matters for .resc includes).
    test_env = dict(os.environ)
    if env:
        test_env.update(env)

    started = time.time()
    proc = subprocess.run(
        [renode, robot_file],
        cwd=str(script_dir),
        env=test_env,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        check=False,
    )
    duration = time.time() - started
    log = (proc.stdout or "") + (proc.stderr or "")

    return {
        "pass": proc.returncode == 0,
        "returncode": proc.returncode,
        "duration_sec": round(duration, 3),
        "log": log,
        "robot": robot_file,
        "dist_path": str(target_path),
        "build_log": str(build_log),
    }


_CARGO_TEST_RESULT = re.compile(
    r"test result:\s+(?:ok|FAILED)\.\s+(?P<passed>\d+)\s+passed;\s+(?P<failed>\d+)\s+failed"
)


def parse_cargo_test_result(stdout: str) -> tuple[int, int]:
    """Sum ``test result: ok. N passed; M failed`` lines from a cargo test log."""
    total_p = total_f = 0
    for m in _CARGO_TEST_RESULT.finditer(stdout):
        total_p += int(m.group("passed"))
        total_f += int(m.group("failed"))
    return total_p, total_f
