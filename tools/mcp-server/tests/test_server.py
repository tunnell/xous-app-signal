"""Tests for the server entrypoint that don't require the mcp package installed."""

from __future__ import annotations

import pytest


def test_server_module_imports() -> None:
    """Importing the module must not require the mcp package."""
    from xas_mcp import server  # noqa: F401

    # build_app is the gated import boundary; it should not trigger
    # the mcp import at module load time.
    assert callable(server.build_app)
    assert callable(server.main)


def test_build_app_errors_without_mcp(monkeypatch: pytest.MonkeyPatch) -> None:
    """If the mcp package isn't installed, build_app() raises a clear SystemExit."""
    import builtins
    import importlib

    from xas_mcp import server

    # Force ImportError on `from mcp.server.fastmcp import FastMCP`.
    real_import = builtins.__import__

    def fake_import(name: str, *args: object, **kwargs: object) -> object:
        if name.startswith("mcp.server.fastmcp"):
            raise ImportError("simulated missing mcp")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    importlib.reload(server)
    with pytest.raises(SystemExit, match="mcp"):
        server.build_app()
