"""Unit tests for xas_mcp.ssh."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from xas_mcp import ssh as ssh_mod


def _stub_run(rc: int, stdout: str = "", stderr: str = "") -> MagicMock:
    """Build a MagicMock that mimics subprocess.CompletedProcess(rc, out, err)."""
    completed = MagicMock(spec=subprocess.CompletedProcess)
    completed.returncode = rc
    completed.stdout = stdout
    completed.stderr = stderr
    return completed


def test_filter_pq_warning_drops_known_fragments() -> None:
    raw = (
        "WARNING: hash algorithm SHA1 is disabled\n"
        "post-quantum hybrid kex unsupported by server\n"
        "store now decrypt later applies\n"
        "via openssh.com OpenSSH 9.6\n"
        "Hello from the Pi\n"
        "uptime 5 days\n"
    )
    out = ssh_mod.filter_pq_warning(raw)
    assert "Hello from the Pi" in out
    assert "uptime 5 days" in out
    assert "WARNING" not in out
    assert "post-quantum" not in out
    assert "store now" not in out
    assert "openssh.com" not in out


def test_ssh_pi_invokes_plain_ssh(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        calls.append(cmd)
        return _stub_run(0, stdout="ok\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    res = ssh_mod.ssh_pi("pi@10.0.0.42", "uptime")
    assert res.ok
    assert res.returncode == 0
    assert res.stdout.strip() == "ok"
    # Plain ssh — no -o BatchMode, no -o ConnectTimeout, etc.
    assert calls == [["ssh", "pi@10.0.0.42", "uptime"]]


def test_ssh_pi_filters_pq_warning_from_stdout_and_stderr(monkeypatch: pytest.MonkeyPatch) -> None:
    noisy = "WARNING: post-quantum stuff\nreal output\n"

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        return _stub_run(0, stdout=noisy, stderr=noisy)

    monkeypatch.setattr(subprocess, "run", fake_run)
    res = ssh_mod.ssh_pi("h", "echo")
    assert "WARNING" not in res.stdout
    assert "post-quantum" not in res.stderr
    assert "real output" in res.stdout


def test_ssh_pi_returns_nonzero_on_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        return _stub_run(2, stderr="connection refused\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    res = ssh_mod.ssh_pi("h", "true")
    assert not res.ok
    assert res.returncode == 2
    assert "refused" in res.stderr


def test_ssh_pi_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        raise subprocess.TimeoutExpired(cmd, timeout=5, stderr=b"")

    monkeypatch.setattr(subprocess, "run", fake_run)
    res = ssh_mod.ssh_pi("h", "sleep 100", timeout_sec=5)
    assert res.returncode == 124
    assert "timed out" in res.stderr


def test_scp_to_pi_argv_shape(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        calls.append(cmd)
        return _stub_run(0)

    monkeypatch.setattr(subprocess, "run", fake_run)
    img = tmp_path / "xous.img"
    img.write_bytes(b"\x00" * 16)
    res = ssh_mod.scp_to_pi("pi@host", img, "~/xous-flash/xous.img")
    assert res.ok
    assert calls == [["scp", str(img), "pi@host:~/xous-flash/xous.img"]]


def test_scp_from_pi_argv_shape(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        calls.append(cmd)
        return _stub_run(0)

    monkeypatch.setattr(subprocess, "run", fake_run)
    local = tmp_path / "out.log"
    res = ssh_mod.scp_from_pi("pi@host", "/tmp/flash.log", local)
    assert res.ok
    assert calls == [["scp", "pi@host:/tmp/flash.log", str(local)]]


def test_screen_detached_wraps_in_screen_nohup(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[list[str]] = []

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        calls.append(cmd)
        return _stub_run(0)

    monkeypatch.setattr(subprocess, "run", fake_run)
    out = ssh_mod.screen_detached(
        "pi@h",
        "python3 usb_update.py -k xous.img --bounce",
        session_name="flash_1",
        log_path="/tmp/flash_1.log",
        cwd="~/xous-flash",
    )
    assert out == {
        "screen_session": "flash_1",
        "log_path": "/tmp/flash_1.log",
        "host": "pi@h",
    }
    # Exactly one ssh invocation; argv is ["ssh", host, remote_cmd].
    assert len(calls) == 1
    argv = calls[0]
    assert argv[0] == "ssh"
    assert argv[1] == "pi@h"
    remote = argv[2]
    # The remote command must cd, then launch a detached screen, then
    # the inner command must be wrapped in nohup with redirection.
    assert "cd " in remote and "xous-flash" in remote
    assert "screen -dmS" in remote
    assert "flash_1" in remote
    assert "nohup python3 usb_update.py -k xous.img --bounce" in remote
    assert "/tmp/flash_1.log" in remote
    assert "2>&1" in remote


def test_screen_detached_default_session_name_format(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        return _stub_run(0)

    monkeypatch.setattr(subprocess, "run", fake_run)
    out = ssh_mod.screen_detached("pi@h", "true", log_path="/tmp/t.log")
    assert out["screen_session"].startswith("xas_")
    assert out["screen_session"][4:].isdigit()


def test_screen_detached_raises_when_ssh_fails(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        return _stub_run(1, stderr="ssh: connect failed\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    with pytest.raises(RuntimeError, match="screen-detached"):
        ssh_mod.screen_detached("pi@h", "true", log_path="/tmp/t.log")
