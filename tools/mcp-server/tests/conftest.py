"""Shared pytest fixtures for xas_mcp unit tests."""

from __future__ import annotations

import os
from collections.abc import Iterator
from pathlib import Path

import pytest


@pytest.fixture
def env_isolated(monkeypatch: pytest.MonkeyPatch) -> pytest.MonkeyPatch:
    """Strip xas_mcp-relevant env vars so each test starts from a known state."""
    for var in (
        "PI_HOST",
        "PI_FLASH_DIR",
        "PI_UART_LOG",
        "PI_UART_SCREEN",
        "FLASH_LOG_DIR",
        "XOUS_CORE_DIR",
        "XOUS_TARGET",
        "GIT_DESCRIBE",
        "GIT_REV",
        "XAS_MCP_DOTENV",
    ):
        monkeypatch.delenv(var, raising=False)
    return monkeypatch


@pytest.fixture
def repo_root(tmp_path: Path) -> Path:
    """A throwaway directory that simulates the xous-app-signal checkout root."""
    (tmp_path / "tests" / "precursor").mkdir(parents=True)
    (tmp_path / "target" / "riscv32imac-unknown-xous-elf" / "release").mkdir(parents=True)
    return tmp_path


@pytest.fixture
def fake_xous_core(tmp_path: Path) -> Path:
    """A throwaway directory that simulates a xous-core checkout."""
    xc = tmp_path / "xous-core"
    (xc / "tools").mkdir(parents=True)
    (xc / "target" / "riscv32imac-unknown-xous-elf" / "release").mkdir(parents=True)
    (xc / "tools" / "usb_update.py").write_text("# fake usb_update\n")
    return xc


@pytest.fixture
def integration() -> Iterator[bool]:
    """Skip-or-run gate for tests that touch real hardware / network."""
    enabled = os.environ.get("XAS_MCP_INTEGRATION", "0") == "1"
    if not enabled:
        pytest.skip("set XAS_MCP_INTEGRATION=1 to run integration tests")
    yield enabled
