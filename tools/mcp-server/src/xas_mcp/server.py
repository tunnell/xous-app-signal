"""MCP server entrypoint — exposes every tool listed in README.md.

Uses ``FastMCP`` from the official Python MCP SDK (``pip install mcp``).
Stdio transport for local agent use; this server is intended to be
spawned by an MCP client (Claude Code etc.) and talk to it over the
client's stdin/stdout pipes.

The decision logic for each tool — argument shapes, return values,
robustness invariants — lives in the underlying modules
(:mod:`xas_mcp.build`, :mod:`xas_mcp.flash`, :mod:`xas_mcp.uart`,
:mod:`xas_mcp.tests_renode`, :mod:`xas_mcp.tests_hosted`,
:mod:`xas_mcp.cargo`). This file is the thinnest possible wiring layer:
it imports those tools, adapts a couple of types for the MCP boundary
(Paths become strings, generators become lists), and registers each
function with FastMCP.

Run directly::

    python -m xas_mcp.server
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from .build import build_xas as _build_xas
from .build import bundle_kernel_image as _bundle_kernel_image
from .cargo import cargo_test as _cargo_test
from .config import load_config
from .flash import (
    flash_direct as _flash_direct,
)
from .flash import (
    flash_pi as _flash_pi,
)
from .flash import (
    flash_status as _flash_status,
)
from .flash import (
    lsusb_pi as _lsusb_pi,
)
from .flash import (
    pi_screen_uart_status as _pi_screen_uart_status,
)
from .ssh import scp_from_pi as _scp_from_pi
from .ssh import scp_to_pi as _scp_to_pi
from .ssh import ssh_pi as _ssh_pi_raw
from .tests_hosted import run_hosted_test as _run_hosted_test
from .tests_renode import run_renode_test as _run_renode_test
from .uart import parse_uart_perf as _parse_uart_perf
from .uart import read_uart as _read_uart
from .uart import tail_uart as _tail_uart

__all__ = ["main", "build_app"]


def build_app() -> Any:
    """Construct and return a FastMCP instance with every xas tool registered.

    Importing ``mcp`` is deferred to this function so ``--help`` and
    other lightweight CLI use cases don't require the MCP SDK to be
    installed.
    """
    try:
        from mcp.server.fastmcp import FastMCP  # type: ignore[import-not-found]
    except ImportError as exc:  # pragma: no cover — exercised at runtime
        raise SystemExit(
            "xas_mcp.server requires the `mcp` package. "
            "Install with: pip install -e tools/mcp-server"
        ) from exc

    app = FastMCP("xas")

    # --- build ----------------------------------------------------------------

    @app.tool()
    def build_xas(
        features: list[str] | None = None,
        target: str | None = None,
        release: bool = True,
    ) -> dict[str, Any]:
        """Cross-compile xas for Precursor hardware.

        Defaults match BUILDING.md §3.2: release build, target
        ``riscv32imac-unknown-xous-elf``, features
        ``pddb-real,precursor``. Returns
        ``{path, size_bytes, sha256, log_path, returncode, command}``.
        """
        return _build_xas(
            features=tuple(features) if features is not None else ("pddb-real", "precursor"),
            target=target,
            release=release,
        )

    @app.tool()
    def bundle_kernel_image(
        xas_bin: str | None = None,
        extra_apps: list[str] | None = None,
        kernel_features: list[str] | None = None,
        gdb_stub: bool = True,
        git_describe: str | None = None,
        git_rev: str | None = None,
    ) -> dict[str, Any]:
        """Bundle xous.img via ``cargo xtask app-image-xip``.

        Mirrors BUILDING.md §3.2 step 2. Defaults: xas binary at the
        canonical build output, extra_apps ``vault transientdisk``,
        kernel_features ``big-heap``, gdb_stub on, git pins read from
        Config. Returns ``{path, size_bytes, sha256, ...}``.
        """
        return _bundle_kernel_image(
            xas_bin=Path(xas_bin) if xas_bin else None,
            extra_apps=tuple(extra_apps) if extra_apps is not None else ("vault", "transientdisk"),
            kernel_features=tuple(kernel_features) if kernel_features is not None else ("big-heap",),
            gdb_stub=gdb_stub,
            git_describe=git_describe,
            git_rev=git_rev,
        )

    # --- flash ----------------------------------------------------------------

    @app.tool()
    def lsusb_pi() -> dict[str, Any]:
        """Report the Precursor's USB enumeration state as seen by the Pi.

        Returns ``{visible, vid_pid, device_id, mode}`` — mode is
        ``loader`` / ``normal`` / ``unknown``. Flashing requires
        loader mode (1209:5bf0).
        """
        return _lsusb_pi()

    @app.tool()
    def flash_pi(
        image_path: str | None = None,
        log_name: str | None = None,
        robust: bool = True,
    ) -> dict[str, Any]:
        """Flash xous.img to a Precursor via the Pi rig.

        Robust=True wraps the Pi-side ``usb_update.py`` in a detached
        screen + nohup so an SSH disconnect can't kill the write —
        leave it on. Returns immediately with ``{screen_session,
        pi_log_path, host, image_path, ...}``; poll via flash_status.
        """
        return _flash_pi(
            image_path=Path(image_path) if image_path else None,
            log_name=log_name,
            robust=robust,
        )

    @app.tool()
    def flash_direct(image_path: str | None = None) -> dict[str, Any]:
        """Flash xous.img directly from this host (no Pi rig).

        Same brick-prevention rules as flash_pi: kernel-only
        ``-k --bounce``, never ``-l`` or ``--soc``. Blocks until the
        write finishes (~25 minutes).
        """
        return _flash_direct(image_path=Path(image_path) if image_path else None)

    @app.tool()
    def flash_status(log_path: str, session: str | None = None) -> dict[str, Any]:
        """Poll a running flash via its Pi-side log + screen session.

        Returns ``{running, percent, eta_sec, last_line, done, ...}``.
        """
        return _flash_status(log_path, session=session)

    @app.tool()
    def pi_screen_uart_status() -> dict[str, Any]:
        """Is the persistent UART-capture screen session alive on the Pi?

        Returns ``{alive, session_id, log_file}``.
        """
        return _pi_screen_uart_status()

    # --- uart -----------------------------------------------------------------

    @app.tool()
    def read_uart(lines: int = 200, timeout_sec: int = 30) -> str:
        """Hardcopy the tail of the Pi's UART capture log."""
        return _read_uart(lines=lines, timeout_sec=timeout_sec)

    @app.tool()
    def tail_uart(until_pattern: str | None = None, max_lines: int = 200) -> list[str]:
        """Stream UART live; return the lines collected until the stop condition.

        At least one of ``until_pattern`` or ``max_lines`` must
        terminate the stream — there is no infinite-tail MCP surface.
        """
        return list(_tail_uart(until_pattern=until_pattern, max_lines=max_lines))

    @app.tool()
    def parse_uart_perf(
        log_text: str,
        prefix: str = "perf/",
        topics: list[str] | None = None,
    ) -> dict[str, list[dict[str, Any]]]:
        """Extract structured ``perf/<topic>: key=value ...`` entries from UART text.

        Recognises the iter-1 instrumentation topics: ``net``,
        ``store``, ``cold-send``, ``send``. Returns
        ``{full_topic: [{ts, raw, payload, fields}]}``.
        """
        return _parse_uart_perf(log_text, prefix=prefix, topics=topics)

    # --- tests ----------------------------------------------------------------

    @app.tool()
    def run_renode_test(
        robot_file: str = "xas-smoke.robot",
        env: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """Build xas + run ``renode-test`` against a Robot script.

        Returns ``{pass, returncode, duration_sec, log, robot, ...}``.
        """
        return _run_renode_test(robot_file, env=env)

    @app.tool()
    def run_hosted_test(
        test_name: str = "link_qr",
        env: dict[str, str] | None = None,
        cleanup_orphans: bool = True,
    ) -> dict[str, Any]:
        """Run a hosted-mode integration test (auto-wraps in xvfb-run if no DISPLAY).

        ``test_name`` is one of: ``link_qr``, ``send_receive``,
        ``signal_cli_echo``. Returns ``{pass, returncode,
        duration_sec, log, test_name, script_path}``.
        """
        return _run_hosted_test(
            test_name, env=env, cleanup_orphans=cleanup_orphans
        )

    @app.tool()
    def cargo_test(
        package: str = "xous-app-signal",
        features: list[str] | None = None,
        target: str | None = None,
    ) -> dict[str, Any]:
        """Run ``cargo test`` for a package with the given features.

        Defaults to package=xous-app-signal, features=[hosted].
        Returns ``{pass, n_passed, n_failed, returncode,
        duration_sec, log, command}``.
        """
        return _cargo_test(
            package=package,
            features=tuple(features) if features is not None else ("hosted",),
            target=target,
        )

    # --- generic escape hatches (use named tools above where possible) -------

    @app.tool()
    def ssh_pi(cmd: str, timeout_sec: int = 30) -> dict[str, Any]:
        """Generic ``ssh pi-host <cmd>``. Prefer specific tools where they exist.

        Returns ``{returncode, stdout, stderr, command}``. Output has
        the post-quantum SSH warning filtered out.
        """
        cfg = load_config()
        res = _ssh_pi_raw(cfg.require_pi_host(), cmd, timeout_sec=timeout_sec)
        return {
            "returncode": res.returncode,
            "stdout": res.stdout,
            "stderr": res.stderr,
            "command": res.cmd,
        }

    @app.tool()
    def scp_to_pi(local_path: str, remote_path: str) -> dict[str, Any]:
        """``scp <local_path> pi-host:<remote_path>``."""
        cfg = load_config()
        res = _scp_to_pi(cfg.require_pi_host(), Path(local_path), remote_path)
        return {
            "returncode": res.returncode,
            "stdout": res.stdout,
            "stderr": res.stderr,
            "local": res.local,
            "remote": res.remote,
        }

    @app.tool()
    def scp_from_pi(remote_path: str, local_path: str) -> dict[str, Any]:
        """``scp pi-host:<remote_path> <local_path>``."""
        cfg = load_config()
        res = _scp_from_pi(cfg.require_pi_host(), remote_path, Path(local_path))
        return {
            "returncode": res.returncode,
            "stdout": res.stdout,
            "stderr": res.stderr,
            "local": res.local,
            "remote": res.remote,
        }

    # --- diagnostics ----------------------------------------------------------

    @app.tool()
    def config_dump() -> dict[str, Any]:
        """Dump the resolved Config (env + .env + defaults), JSON-serializable."""
        cfg = load_config()
        return {
            "repo_root": str(cfg.repo_root),
            "pi_host": cfg.pi_host,
            "pi_flash_dir": cfg.pi_flash_dir,
            "pi_uart_log": cfg.pi_uart_log,
            "pi_uart_screen": cfg.pi_uart_screen,
            "flash_log_dir": cfg.flash_log_dir,
            "xous_core_dir": str(cfg.xous_core_dir),
            "xous_target": cfg.xous_target,
            "git_describe": cfg.git_describe,
            "git_rev": cfg.git_rev,
            "canonical_xous_img_path": str(cfg.canonical_xous_img_path()),
            "xas_bin_path": str(cfg.xas_bin_path()),
        }

    return app


def main(argv: list[str] | None = None) -> int:
    """Entry point for ``python -m xas_mcp.server`` and the ``xas-mcp-server`` script.

    The MCP SDK does its own argument parsing for transports etc.;
    we accept ``--list-tools`` as a tiny non-MCP-protocol convenience
    so humans can introspect the registered surface without piping
    JSON-RPC into the process.
    """
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv == ["--list-tools"]:
        app = build_app()
        registry = getattr(app, "_tool_manager", None) or getattr(app, "tool_manager", None)
        names: list[str] = []
        if registry is not None and hasattr(registry, "list_tools"):
            tools = registry.list_tools()  # type: ignore[attr-defined]
            for t in tools:
                names.append(getattr(t, "name", str(t)))
        print(json.dumps({"tools": names}, indent=2))
        return 0
    app = build_app()
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
