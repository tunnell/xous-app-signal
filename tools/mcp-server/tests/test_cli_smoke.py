"""Smoke tests: every CLI module exits 0 on ``--help`` and prints usage.

These run without any subprocess mocking — argparse handles --help in
a side-effect-free way (just prints + sys.exit(0)). The tests catch
the SystemExit and assert it's 0.
"""

from __future__ import annotations

import importlib
import sys

import pytest

CLIS = [
    "xas_mcp.cli.build_and_bundle",
    "xas_mcp.cli.flash_via_pi",
    "xas_mcp.cli.flash_direct",
    "xas_mcp.cli.flash_status",
    "xas_mcp.cli.watch_uart",
    "xas_mcp.cli.run_renode_tests",
    "xas_mcp.cli.test_link_qr",
]


@pytest.mark.parametrize("module_name", CLIS)
def test_cli_help_exits_zero(
    module_name: str, capsys: pytest.CaptureFixture[str]
) -> None:
    """``python -m <cli> --help`` must exit 0 and print non-empty usage."""
    module = importlib.import_module(module_name)
    saved_argv = sys.argv
    try:
        sys.argv = [module_name, "--help"]
        with pytest.raises(SystemExit) as excinfo:
            module.main()
        assert excinfo.value.code == 0
    finally:
        sys.argv = saved_argv

    captured = capsys.readouterr()
    # Help output goes to stdout via argparse's print_help.
    assert "usage" in captured.out.lower() or "Usage" in captured.out
    # Each CLI's prog name should appear at least once.
    short_name = module_name.rsplit(".", 1)[-1]
    assert short_name.replace("_", "-") in captured.out or short_name in captured.out
