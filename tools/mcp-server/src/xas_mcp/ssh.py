"""SSH + SCP helpers for talking to the Pi rig.

Per the maintainer's longstanding preference, the underlying ``ssh``
binary is invoked plainly — no ``-o BatchMode=yes``, no ``-o
ConnectTimeout=...``. The host's normal ``~/.ssh/config`` + key agent
take care of authentication.

Every long-running Pi-side operation goes through
:func:`screen_detached` so an SSH disconnect can't kill the work. See
the module docstring of :mod:`xas_mcp.flash` for the failure mode this
prevents.
"""

from __future__ import annotations

import shlex
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

__all__ = [
    "SSHResult",
    "SCPResult",
    "filter_pq_warning",
    "ssh_pi",
    "scp_to_pi",
    "scp_from_pi",
    "screen_detached",
]


# Anchored substrings present in the post-quantum SSH warning OpenSSH 9.6+
# prints unconditionally when the server doesn't advertise an NTRU-PQ
# kex algorithm. The maintainer finds it noisy in flash logs.
_PQ_WARNING_FRAGMENTS = (
    "WARNING:",
    "post-quantum",
    "store now",
    "openssh.com",
)


@dataclass(frozen=True)
class SSHResult:
    """Outcome of a ``ssh`` invocation. ``stdout``/``stderr`` are PQ-warning-filtered."""

    cmd: list[str]
    returncode: int
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0


@dataclass(frozen=True)
class SCPResult:
    """Outcome of an ``scp`` invocation."""

    cmd: list[str]
    returncode: int
    stdout: str
    stderr: str
    local: str
    remote: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0


def filter_pq_warning(text: str) -> str:
    """Strip OpenSSH's post-quantum-warning preamble from captured output.

    The warning spans several lines and surfaces on every connection
    even though it's informational. We drop any line containing one of
    the known fragments rather than trying to detect the multi-line
    block precisely — false positives are unlikely in practice and the
    user has explicitly asked for these lines to disappear.
    """
    return "\n".join(
        line
        for line in text.splitlines()
        if not any(frag in line for frag in _PQ_WARNING_FRAGMENTS)
    )


def _run(cmd: list[str], *, timeout_sec: int | None) -> tuple[int, str, str]:
    """Wrapper around ``subprocess.run`` that returns text and a timeout-as-returncode."""
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            check=False,
        )
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or "")
        err = e.stderr.decode() if isinstance(e.stderr, bytes) else (e.stderr or "")
        return 124, out, err + f"\nxas_mcp.ssh: command timed out after {timeout_sec}s\n"
    return proc.returncode, proc.stdout or "", proc.stderr or ""


def ssh_pi(host: str, cmd: str, *, timeout_sec: int | None = 30) -> SSHResult:
    """Run ``cmd`` on the Pi via ssh. Returns :class:`SSHResult`.

    Plain ``ssh host cmd`` — no extra options. The caller is responsible
    for quoting if needed (this function does not wrap ``cmd`` in
    additional layers of shell-quoting). Long-running commands should
    use :func:`screen_detached` instead so a network blip doesn't kill
    the remote process.
    """
    argv = ["ssh", host, cmd]
    rc, out, err = _run(argv, timeout_sec=timeout_sec)
    return SSHResult(
        cmd=argv,
        returncode=rc,
        stdout=filter_pq_warning(out),
        stderr=filter_pq_warning(err),
    )


def scp_to_pi(host: str, local: Path, remote: str, *, timeout_sec: int | None = 600) -> SCPResult:
    """``scp <local> <host>:<remote>``. Default timeout is generous for image-sized files."""
    argv = ["scp", str(local), f"{host}:{remote}"]
    rc, out, err = _run(argv, timeout_sec=timeout_sec)
    return SCPResult(
        cmd=argv,
        returncode=rc,
        stdout=filter_pq_warning(out),
        stderr=filter_pq_warning(err),
        local=str(local),
        remote=f"{host}:{remote}",
    )


def scp_from_pi(host: str, remote: str, local: Path, *, timeout_sec: int | None = 600) -> SCPResult:
    """``scp <host>:<remote> <local>``."""
    argv = ["scp", f"{host}:{remote}", str(local)]
    rc, out, err = _run(argv, timeout_sec=timeout_sec)
    return SCPResult(
        cmd=argv,
        returncode=rc,
        stdout=filter_pq_warning(out),
        stderr=filter_pq_warning(err),
        local=str(local),
        remote=f"{host}:{remote}",
    )


def screen_detached(
    host: str,
    cmd: str,
    *,
    session_name: str | None = None,
    log_path: str,
    cwd: str | None = None,
    timeout_sec: int = 30,
) -> dict[str, str]:
    """Launch ``cmd`` on the Pi inside a detached ``screen`` + ``nohup`` wrapper.

    The remote shell runs::

        cd <cwd> && screen -dmS <session> bash -c 'nohup <cmd> > <log> 2>&1'

    so the work survives SSH disconnect, local-process kill, and even a
    build-host reboot. Returns a dict with ``screen_session`` and
    ``log_path`` for the caller to poll with :func:`xas_mcp.flash.flash_status`
    (or a generic ``ssh host "tail -f <log>"``).

    ``session_name`` defaults to ``xas_<epoch_ms>``; provide one
    explicitly when you need to refer to the session later.
    """
    if session_name is None:
        session_name = f"xas_{int(time.time() * 1000)}"

    inner = f"nohup {cmd} > {shlex.quote(log_path)} 2>&1"
    remote = f"screen -dmS {shlex.quote(session_name)} bash -c {shlex.quote(inner)}"
    if cwd:
        remote = f"cd {shlex.quote(cwd)} && {remote}"

    res = ssh_pi(host, remote, timeout_sec=timeout_sec)
    if not res.ok:
        raise RuntimeError(
            f"failed to launch screen-detached job on {host!r}: "
            f"exit={res.returncode} stderr={res.stderr.strip()!r}"
        )
    return {"screen_session": session_name, "log_path": log_path, "host": host}
