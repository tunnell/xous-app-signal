"""Unit tests for xas_mcp.flash."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from xas_mcp import flash as flash_mod
from xas_mcp import ssh as ssh_mod
from xas_mcp.config import load_config


def _stub(rc: int = 0, stdout: str = "", stderr: str = "") -> MagicMock:
    cp = MagicMock(spec=subprocess.CompletedProcess)
    cp.returncode = rc
    cp.stdout = stdout
    cp.stderr = stderr
    return cp


def _ssh_router(
    monkeypatch: pytest.MonkeyPatch,
    rules: list[tuple[str, MagicMock]],
) -> list[list[str]]:
    """Make subprocess.run match incoming argv against the first rule whose
    substring is found in the joined command. Records all argvs."""
    recorded: list[list[str]] = []

    def fake_run(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        recorded.append(list(cmd))
        joined = " ".join(cmd)
        for substring, response in rules:
            if substring in joined:
                return response
        return _stub(0)

    monkeypatch.setattr(ssh_mod.subprocess, "run", fake_run)
    monkeypatch.setattr(flash_mod.subprocess, "run", fake_run)
    return recorded


def test_lsusb_pi_loader_mode(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    lsusb_out = (
        "Bus 001 Device 002: ID 1d6b:0002 Linux Foundation 2.0 root hub\n"
        "Bus 001 Device 005: ID 1209:5bf0 Generic\n"
    )
    _ssh_router(monkeypatch, [("ssh pi@h lsusb", _stub(0, stdout=lsusb_out))])

    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    result = flash_mod.lsusb_pi(config=cfg)
    assert result["visible"]
    assert result["vid_pid"] == "1209:5bf0"
    assert result["mode"] == "loader"
    assert result["device_id"] == 5


def test_lsusb_pi_normal_mode(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    _ssh_router(
        monkeypatch,
        [("ssh pi@h lsusb", _stub(0, stdout="Bus 001 Device 9: ID 1209:3613 Precursor\n"))],
    )
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    res = flash_mod.lsusb_pi(config=cfg)
    assert res["visible"] and res["mode"] == "normal" and res["vid_pid"] == "1209:3613"


def test_lsusb_pi_loader_takes_precedence(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """If both vid_pids appear (transient state during reboot), prefer loader."""
    out = "Bus 1 Device 7: ID 1209:3613 X\nBus 1 Device 8: ID 1209:5bf0 Y\n"
    _ssh_router(monkeypatch, [("lsusb", _stub(0, stdout=out))])
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    assert flash_mod.lsusb_pi(config=cfg)["mode"] == "loader"


def test_lsusb_pi_invisible(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    _ssh_router(monkeypatch, [("lsusb", _stub(0, stdout="(no precursor here)\n"))])
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    res = flash_mod.lsusb_pi(config=cfg)
    assert not res["visible"]
    assert res["mode"] == "unknown"


def test_lsusb_pi_requires_pi_host(env_isolated: pytest.MonkeyPatch, tmp_path: Path) -> None:
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    with pytest.raises(RuntimeError, match="PI_HOST"):
        flash_mod.lsusb_pi(config=cfg)


def test_flash_pi_uses_screen_detached_robust_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    img = tmp_path / "xous.img"
    img.write_bytes(b"\x00" * 1024)
    recorded = _ssh_router(
        monkeypatch,
        [
            ("lsusb", _stub(0, stdout="Bus 1 Device 3: ID 1209:5bf0 X\n")),
            ("test -f", _stub(0)),  # usb_update.py exists
            ("scp ", _stub(0)),  # image upload
            ("screen -dmS", _stub(0)),  # screen-detached launch
        ],
    )
    cfg = load_config(
        env={"PI_HOST": "pi@h", "PI_FLASH_DIR": "~/xous-flash"},
        dotenv_path=tmp_path / "n",
        repo_root=tmp_path,
    )
    result = flash_mod.flash_pi(config=cfg, image_path=img)

    # Robust path returns screen_session + pi_log_path.
    assert result["robust"] is True
    assert result["screen_session"].startswith("flash_")
    assert result["pi_log_path"].startswith("/tmp/flash-")
    assert result["host"] == "pi@h"
    assert result["remote_image_path"] == "~/xous-flash/xous.img"

    # We see exactly: lsusb, test -f, scp, ssh screen -dmS … nohup …
    cmds = [" ".join(c) for c in recorded]
    assert any("lsusb" in c for c in cmds)
    assert any("test -f" in c and "usb_update.py" in c for c in cmds)
    assert any(c.startswith("scp ") for c in cmds)
    # The screen launch is wrapped in screen -dmS and contains nohup +
    # the kernel-only flash command.
    screen_cmds = [c for c in cmds if "screen -dmS" in c]
    assert len(screen_cmds) == 1
    sc = screen_cmds[0]
    assert "nohup python3 usb_update.py -k xous.img --bounce" in sc
    # Critical: -k only. NEVER -l or --soc.
    assert " -l " not in sc and "--soc" not in sc and "--factory-reset" not in sc


def test_flash_pi_rejects_non_loader_mode(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    img = tmp_path / "x.img"
    img.write_bytes(b"x")
    _ssh_router(monkeypatch, [("lsusb", _stub(0, stdout="Bus 1 Device 3: ID 1209:3613 X\n"))])
    cfg = load_config(
        env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path
    )
    with pytest.raises(RuntimeError, match="loader mode"):
        flash_mod.flash_pi(config=cfg, image_path=img)


def test_flash_pi_missing_image(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    with pytest.raises(RuntimeError, match="image not found"):
        flash_mod.flash_pi(config=cfg, image_path=tmp_path / "no.img")


def test_flash_pi_missing_usb_update_py(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    img = tmp_path / "x.img"
    img.write_bytes(b"x")
    _ssh_router(
        monkeypatch,
        [
            ("lsusb", _stub(0, stdout="Bus 1 Device 3: ID 1209:5bf0 X\n")),
            ("test -f", _stub(1)),  # missing
        ],
    )
    cfg = load_config(
        env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path
    )
    with pytest.raises(RuntimeError, match="usb_update.py not found"):
        flash_mod.flash_pi(config=cfg, image_path=img)


def test_flash_status_parses_percent_and_eta(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = (
        "Booting...\n"
        "Writing kernel: 0%\n"
        "Writing kernel: 42% eta 600s\n"
        "Writing kernel: 87% eta 120s\n"
    )
    _ssh_router(
        monkeypatch,
        [
            ("tail -n 200", _stub(0, stdout=log)),
            ("screen -ls", _stub(0, stdout="\t1234.flash_1715712000\t(Detached)\n")),
        ],
    )
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    s = flash_mod.flash_status("/tmp/flash-1.log", config=cfg, session="flash_1715712000")
    assert s["percent"] == 87
    assert s["eta_sec"] == 120
    assert "87%" in s["last_line"]
    assert s["running"] is True
    assert s["done"] is False


def test_flash_status_detects_done(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = "Writing kernel: 100%\nFlash complete.\n"
    _ssh_router(
        monkeypatch,
        [
            ("tail -n 200", _stub(0, stdout=log)),
            ("screen -ls", _stub(0, stdout="No Sockets found.\n")),
        ],
    )
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    s = flash_mod.flash_status("/tmp/x.log", config=cfg, session="flash_X")
    assert s["done"] is True
    assert s["running"] is False
    assert s["percent"] == 100


def test_pi_screen_uart_status_alive(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    _ssh_router(
        monkeypatch,
        [(
            "screen -ls",
            _stub(
                0,
                stdout=(
                    "There is a screen on:\n"
                    "\t1234.uart\t(05/14/2026 09:00:00)\t(Detached)\n"
                    "1 Socket in /run/screen.\n"
                ),
            ),
        )],
    )
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    s = flash_mod.pi_screen_uart_status(config=cfg)
    assert s["alive"]
    assert s["session_id"] == "1234.uart"
    assert s["log_file"] == "~/uart-logs/precursor-uart.log"


def test_pi_screen_uart_status_dead(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    _ssh_router(monkeypatch, [("screen -ls", _stub(0, stdout="No Sockets found.\n"))])
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    s = flash_mod.pi_screen_uart_status(config=cfg)
    assert s["alive"] is False
    assert s["session_id"] is None


def test_flash_direct_argv_kernel_only(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Direct flash MUST be -k --bounce only; no -l/--soc/--factory-reset."""
    img = tmp_path / "xous.img"
    img.write_bytes(b"\x00" * 4)
    xc = tmp_path / "xous-core"
    (xc / "tools").mkdir(parents=True)
    usb = xc / "tools" / "usb_update.py"
    usb.write_text("# stub\n")
    captured: list[list[str]] = []

    def fake_run(argv: list[str], **kwargs: Any) -> subprocess.CompletedProcess[Any]:
        captured.append(list(argv))
        # lsusb returns a loader-mode line
        if argv == ["lsusb"]:
            return _stub(0, stdout="Bus 1 Device 4: ID 1209:5bf0 X\n")
        # usb_update.py invocation: pretend success and produce log
        stdout = kwargs.get("stdout")
        if hasattr(stdout, "write"):
            stdout.write("done\n")
        return _stub(0)

    monkeypatch.setattr(flash_mod.subprocess, "run", fake_run)
    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=tmp_path,
    )
    result = flash_mod.flash_direct(config=cfg, image_path=img, log_path=tmp_path / "d.log")
    assert result["returncode"] == 0
    # The flash invocation argv is the second call (lsusb is first).
    flash_argv = captured[1]
    assert flash_argv[:2] == ["python3", str(usb)]
    assert "-k" in flash_argv
    assert "--bounce" in flash_argv
    assert "-l" not in flash_argv
    assert "--soc" not in flash_argv
    assert "--factory-reset" not in flash_argv


def test_flash_direct_requires_loader_visible(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    img = tmp_path / "x.img"
    img.write_bytes(b"x")
    xc = tmp_path / "xc"
    (xc / "tools").mkdir(parents=True)
    (xc / "tools" / "usb_update.py").write_text("# stub")

    def fake_run(argv: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[Any]:
        if argv == ["lsusb"]:
            return _stub(0, stdout="(empty)\n")
        return _stub(0)

    monkeypatch.setattr(flash_mod.subprocess, "run", fake_run)
    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=tmp_path,
    )
    with pytest.raises(RuntimeError, match=flash_mod.VID_PID_LOADER):
        flash_mod.flash_direct(config=cfg, image_path=img, log_path=tmp_path / "d.log")
