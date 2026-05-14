"""Flash a built xous.img onto a Precursor PVT2.

Two paths:

- :func:`flash_pi` runs the flash from a Raspberry Pi rig (recommended:
  frees the build host during the ~25 minute write). Uses
  :func:`xas_mcp.ssh.screen_detached` so an SSH disconnect can't kill
  the write — this is the documented robustness gap in the bash
  ``flash-via-pi.sh`` script.
- :func:`flash_direct` runs the flash from the build host (no Pi rig).

Both invoke ``usb_update.py -k <img> --bounce`` exclusively. ``-k`` is
the kernel-only mode, which is recoverable via USB if anything goes
wrong. ``-l`` (loader) and ``--soc`` / ``--factory-reset`` (gateware)
are deliberately NOT supported by this module — flashing those without
explicit per-invocation authorization can brick the device and
require JTAG recovery. See ``tests/precursor/README.md`` "Brick
prevention".
"""

from __future__ import annotations

import re
import shlex
import subprocess
import time
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from .config import Config, load_config
from .ssh import screen_detached, scp_to_pi, ssh_pi

__all__ = [
    "VID_PID_LOADER",
    "VID_PID_NORMAL",
    "lsusb_pi",
    "flash_pi",
    "flash_direct",
    "flash_status",
    "pi_screen_uart_status",
]


# Precursor enumerates as 1209:5bf0 in loader mode (the mode flashing
# requires) and as 1209:3613 in normal mode (after a successful boot).
VID_PID_LOADER = "1209:5bf0"
VID_PID_NORMAL = "1209:3613"


def lsusb_pi(*, config: Config | None = None) -> dict[str, Any]:
    """Return the Precursor's USB enumeration state as seen by the Pi.

    Returns ``{visible, vid_pid, device_id, mode, raw}`` where ``mode``
    is one of ``loader`` / ``normal`` / ``unknown`` and ``device_id``
    is the integer USB device number lsusb prints (or ``None`` when
    not visible).
    """
    cfg = config or load_config()
    host = cfg.require_pi_host()
    res = ssh_pi(host, "lsusb", timeout_sec=15)
    if not res.ok:
        raise RuntimeError(f"lsusb on {host!r} failed: {res.stderr.strip()}")

    vid_pid: str | None = None
    device_id: int | None = None
    mode = "unknown"
    for line in res.stdout.splitlines():
        # Bus 001 Device 003: ID 1209:5bf0 ...
        m = re.match(
            r"Bus\s+\d+\s+Device\s+(\d+):\s+ID\s+([0-9a-fA-F:]+)\s",
            line,
        )
        if not m:
            continue
        candidate = m.group(2).lower()
        if candidate == VID_PID_LOADER:
            vid_pid, device_id, mode = candidate, int(m.group(1)), "loader"
            break
        if candidate == VID_PID_NORMAL:
            vid_pid, device_id, mode = candidate, int(m.group(1)), "normal"
            # keep looking — loader takes precedence if both present
    return {
        "visible": vid_pid is not None,
        "vid_pid": vid_pid,
        "device_id": device_id,
        "mode": mode,
        "raw": res.stdout,
    }


def _ensure_loader_mode(state: dict[str, Any]) -> None:
    if not state["visible"]:
        raise RuntimeError(
            "Precursor not visible to the Pi (lsusb didn't see 1209:5bf0). "
            "Hold the left-side button while paperclip-resetting to enter "
            "loader mode, then retry."
        )
    if state["mode"] != "loader":
        raise RuntimeError(
            f"Precursor is in {state['mode']!r} mode (1209:{state['vid_pid']}). "
            "Flashing requires loader mode (1209:5bf0). Hold the left-side "
            "button while paperclip-resetting and retry."
        )


def flash_pi(
    *,
    config: Config | None = None,
    image_path: Path | None = None,
    log_name: str | None = None,
    robust: bool = True,
    skip_lsusb: bool = False,
) -> dict[str, Any]:
    """Flash xous.img to a Precursor via the Pi rig.

    Returns immediately after kicking off a screen-detached
    ``usb_update.py`` invocation on the Pi. Poll :func:`flash_status`
    with the returned ``pi_log_path`` until the run completes
    (typically ~25 minutes for a full kernel write).

    ``robust=False`` runs without the screen/nohup wrapper — useful in
    tests but never in production, since an SSH disconnect or local
    process kill would interrupt the write.
    """
    cfg = config or load_config()
    host = cfg.require_pi_host()
    img = image_path or cfg.canonical_xous_img_path()
    if not Path(img).is_file():
        raise RuntimeError(
            f"image not found at {img}. Build it first via build_xas + "
            f"bundle_kernel_image (or `xas-build-and-bundle`)."
        )

    if not skip_lsusb:
        _ensure_loader_mode(lsusb_pi(config=cfg))

    # Verify usb_update.py is staged on the Pi.
    chk = ssh_pi(host, f"test -f {shlex.quote(cfg.pi_flash_dir)}/usb_update.py")
    if not chk.ok:
        raise RuntimeError(
            f"{cfg.pi_flash_dir}/usb_update.py not found on {host!r}. "
            f"Copy it once with:\n"
            f"  scp {cfg.xous_core_dir}/tools/usb_update.py "
            f"{host}:{cfg.pi_flash_dir}/"
        )

    # Stage the image.
    remote_img = f"{cfg.pi_flash_dir}/xous.img"
    upload = scp_to_pi(host, Path(img), remote_img)
    if not upload.ok:
        raise RuntimeError(f"scp xous.img to {host} failed: {upload.stderr.strip()}")

    epoch = int(time.time())
    session = f"flash_{epoch}"
    pi_log = f"{cfg.flash_log_dir}/{log_name or f'flash-{epoch}.log'}"

    # -k = kernel only (recoverable). NEVER -l or --soc here.
    cmd = "python3 usb_update.py -k xous.img --bounce"

    if robust:
        info = screen_detached(
            host,
            cmd,
            session_name=session,
            log_path=pi_log,
            cwd=cfg.pi_flash_dir,
        )
        return {
            "host": host,
            "image_path": str(img),
            "remote_image_path": remote_img,
            "screen_session": info["screen_session"],
            "pi_log_path": pi_log,
            "robust": True,
            "started_at": epoch,
        }
    # Non-robust path — blocks until the flash finishes (or the SSH
    # connection drops, which kills the write). Tests use this; humans
    # should not.
    res = ssh_pi(
        host,
        f"cd {shlex.quote(cfg.pi_flash_dir)} && {cmd} > {shlex.quote(pi_log)} 2>&1",
        timeout_sec=60 * 40,
    )
    return {
        "host": host,
        "image_path": str(img),
        "remote_image_path": remote_img,
        "screen_session": None,
        "pi_log_path": pi_log,
        "robust": False,
        "returncode": res.returncode,
        "started_at": epoch,
    }


def flash_direct(
    *,
    config: Config | None = None,
    image_path: Path | None = None,
    log_path: Path | None = None,
    usb_update_py: Path | None = None,
) -> dict[str, Any]:
    """Flash xous.img to a Precursor connected directly to this host.

    No Pi rig. Ties up the build host for ~25 minutes. Same brick-
    prevention rules: ``-k --bounce`` only, never ``-l`` or
    ``--soc``. Mirrors ``tests/precursor/flash-direct.sh``.
    """
    cfg = config or load_config()
    img = image_path or cfg.canonical_xous_img_path()
    if not Path(img).is_file():
        raise RuntimeError(f"image not found at {img}.")
    usb_update = usb_update_py or (cfg.xous_core_dir / "tools" / "usb_update.py")
    if not Path(usb_update).is_file():
        raise RuntimeError(
            f"usb_update.py not found at {usb_update}. "
            f"Set XOUS_CORE_DIR to your xous-core checkout."
        )

    # Local lsusb check (no Pi).
    try:
        lsusb_out = subprocess.run(
            ["lsusb"], capture_output=True, text=True, timeout=10, check=False
        )
    except FileNotFoundError as e:
        raise RuntimeError("`lsusb` not on PATH on this host") from e
    if VID_PID_LOADER not in lsusb_out.stdout:
        raise RuntimeError(
            f"Precursor not seen as {VID_PID_LOADER} on this host. "
            "Hold the left-side button while paperclip-resetting to enter "
            "loader mode, then retry."
        )

    epoch = int(time.time())
    log = log_path or Path(f"{cfg.flash_log_dir}/flash-{epoch}.log")
    log.parent.mkdir(parents=True, exist_ok=True)

    with log.open("w") as logf:
        proc = subprocess.run(
            ["python3", str(usb_update), "-k", str(img), "--bounce"],
            stdout=logf,
            stderr=subprocess.STDOUT,
            check=False,
        )

    return {
        "image_path": str(img),
        "log_path": str(log),
        "returncode": proc.returncode,
        "started_at": epoch,
    }


# usb_update.py progress lines look like:
#   "Writing kernel:  12% [#######........] eta 1234s"
# or (older builds):
#   "  62% complete"
# We grep both forms; the percentage is enough for callers to render a
# progress bar.
_PROGRESS_RE = re.compile(r"(\d{1,3})\s*%")
_ETA_RE = re.compile(r"eta\s+(\d+)\s*s", re.IGNORECASE)
_DONE_RE = re.compile(r"(flash complete|done|success|wrote)", re.IGNORECASE)


def _flash_status_from_text(text: str) -> dict[str, Any]:
    last_pct: int | None = None
    eta: int | None = None
    last_line = ""
    done = False
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        last_line = line
        m = _PROGRESS_RE.search(line)
        if m:
            try:
                v = int(m.group(1))
                if 0 <= v <= 100:
                    last_pct = v
            except ValueError:
                pass
        em = _ETA_RE.search(line)
        if em:
            try:
                eta = int(em.group(1))
            except ValueError:
                pass
        if _DONE_RE.search(line):
            done = True
    return {
        "percent": last_pct,
        "eta_sec": eta,
        "last_line": last_line,
        "done": done,
    }


def flash_status(
    log_path: str,
    *,
    config: Config | None = None,
    session: str | None = None,
) -> dict[str, Any]:
    """Poll a running flash via its Pi-side log file.

    Returns ``{running, percent, eta_sec, last_line, done, returncode,
    session, log_path}``. ``running`` is True when the screen session
    referenced by ``session`` is still alive on the Pi (if no session
    name is given, we infer "running" from ``done`` being false).
    """
    cfg = config or load_config()
    host = cfg.require_pi_host()
    # Pull the tail of the log; large logs are fine because tail is O(n)
    # over the trailing N lines.
    tail = ssh_pi(host, f"tail -n 200 {shlex.quote(log_path)}", timeout_sec=15)
    if not tail.ok:
        raise RuntimeError(
            f"could not read {log_path!r} on {host!r}: {tail.stderr.strip()}"
        )
    state = _flash_status_from_text(tail.stdout)

    running: bool
    if session:
        ls = ssh_pi(host, "screen -ls || true", timeout_sec=10)
        running = bool(re.search(rf"\.{re.escape(session)}\b", ls.stdout))
    else:
        running = not state["done"]

    return {
        "running": running,
        "session": session,
        "log_path": log_path,
        **state,
    }


def pi_screen_uart_status(*, config: Config | None = None) -> dict[str, Any]:
    """Is the persistent UART-capture screen session alive on the Pi?

    Returns ``{alive, session_id, log_file, raw}``. ``session_id`` is
    the full ``<pid>.<name>`` token from ``screen -ls``; ``log_file``
    echoes the configured ``PI_UART_LOG`` value for convenience.
    """
    cfg = config or load_config()
    host = cfg.require_pi_host()
    res = ssh_pi(host, "screen -ls || true", timeout_sec=10)
    sid: str | None = None
    needle = re.escape(cfg.pi_uart_screen)
    for line in res.stdout.splitlines():
        m = re.search(rf"\s+(\d+\.{needle})\b", line)
        if m:
            sid = m.group(1)
            break
    return {
        "alive": sid is not None,
        "session_id": sid,
        "log_file": cfg.pi_uart_log,
        "raw": res.stdout,
    }
