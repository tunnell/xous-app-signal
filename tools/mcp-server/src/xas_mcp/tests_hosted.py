"""Hosted-mode test runner.

These tests launch a Xous kernel under Linux (no real Precursor) and
drive xas through a flow via X11 keystroke injection. The kernel
prints to a normal stdout log; the test scans that log for marker
lines and exits with a result-specific code.

The bash version (e.g., ``tests/hosted/test_link_qr.sh``) is a long
bespoke script; this module gives a thin Python wrapper that picks
the right script to invoke and auto-wraps the run in ``xvfb-run`` if
``$DISPLAY`` is empty (CI). Per the maintainer's
``feedback_pretest_kernel_cleanup`` memory, we also kill any orphan
xous-kernel processes left over from prior failed runs *before* the
test starts.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from .config import Config, load_config

__all__ = ["run_hosted_test", "kill_orphan_kernels", "KNOWN_HOSTED_TESTS"]


# Map test name → script in tests/hosted/. Adding a new hosted test
# means registering it here; the MCP surface intentionally lists the
# supported set rather than exposing a free-form path argument.
KNOWN_HOSTED_TESTS = {
    "link_qr": "test_link_qr.sh",
    "send_receive": "test_send_receive.sh",
    "signal_cli_echo": "test_signal_cli_echo.sh",
}


# Process patterns left lying around when a hosted test crashes
# midway. xous-kernel hangs on to UDP ports in the high range; if a
# second test boots while one of these is alive, DNS resolver
# initialisation panics and the kernel never gets to xas. See
# ``feedback_pretest_kernel_cleanup`` memory for the longer story.
_ORPHAN_PATTERNS: tuple[str, ...] = (
    "xous-core/target/release/xous-kernel",
    "target/debug/xtask run",
    "target/release/xas$",
)


def kill_orphan_kernels(*, wait_sec: float = 2.0) -> dict[str, int]:
    """Run pkill -KILL -f for each known leftover-kernel pattern.

    Returns ``{pattern: returncode}`` for diagnostics. A returncode of
    0 means pkill found and killed a match; 1 means no match (fine).
    """
    out: dict[str, int] = {}
    for pat in _ORPHAN_PATTERNS:
        rc = subprocess.run(
            ["pkill", "-KILL", "-f", pat], capture_output=True, check=False
        ).returncode
        out[pat] = rc
    if wait_sec > 0:
        time.sleep(wait_sec)
    return out


def _wrap_xvfb_if_needed(argv: list[str], env: dict[str, str]) -> list[str]:
    """Prepend ``xvfb-run --auto-servernum`` if ``$DISPLAY`` is empty.

    Most hosted tests need an X server; on a headless build host
    (e.g., CI) xvfb is the standard way to provide one. We only wrap
    when DISPLAY is unset/empty so a real local desktop session is
    left alone.
    """
    if env.get("DISPLAY"):
        return argv
    if shutil.which("xvfb-run") is None:
        raise RuntimeError(
            "DISPLAY is unset and xvfb-run is not on PATH. Either run "
            "this from a desktop session or `apt install xvfb`."
        )
    return ["xvfb-run", "--auto-servernum", "--server-args=-screen 0 1024x768x24", *argv]


def run_hosted_test(
    test_name: str = "link_qr",
    *,
    config: Config | None = None,
    env: dict[str, str] | None = None,
    cleanup_orphans: bool = True,
    timeout_sec: int = 60 * 15,
) -> dict[str, Any]:
    """Run a hosted-mode integration test.

    Returns ``{pass, returncode, duration_sec, log, test_name, script_path}``.
    The bash script's own structured exit codes (e.g., 2 = boot
    timeout, 3 = window not found, 4 = link URL not emitted for
    test_link_qr) are surfaced verbatim via ``returncode``.
    """
    cfg = config or load_config()
    if test_name not in KNOWN_HOSTED_TESTS:
        raise RuntimeError(
            f"unknown hosted test {test_name!r}. Known: {sorted(KNOWN_HOSTED_TESTS)}"
        )
    script = cfg.repo_root / "tests" / "hosted" / KNOWN_HOSTED_TESTS[test_name]
    if not script.is_file():
        raise RuntimeError(f"hosted test script not found: {script}")

    if cleanup_orphans:
        kill_orphan_kernels()

    test_env = dict(os.environ)
    if env:
        test_env.update(env)

    argv = ["bash", str(script)]
    argv = _wrap_xvfb_if_needed(argv, test_env)

    started = time.time()
    proc = subprocess.run(
        argv,
        cwd=str(cfg.repo_root),
        env=test_env,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        check=False,
    )
    duration = time.time() - started

    return {
        "pass": proc.returncode == 0,
        "returncode": proc.returncode,
        "duration_sec": round(duration, 3),
        "log": (proc.stdout or "") + (proc.stderr or ""),
        "test_name": test_name,
        "script_path": str(script),
    }
