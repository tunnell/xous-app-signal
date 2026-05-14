"""Unit tests for xas_mcp.config."""

from __future__ import annotations

from pathlib import Path

import pytest

from xas_mcp.config import Config, load_config, load_dotenv


def test_defaults_present(env_isolated: pytest.MonkeyPatch, tmp_path: Path) -> None:
    cfg = load_config(env={}, dotenv_path=tmp_path / "no-such.env", repo_root=tmp_path)
    assert cfg.xous_target == "riscv32imac-unknown-xous-elf"
    assert cfg.pi_flash_dir == "~/xous-flash"
    assert cfg.pi_uart_log == "~/uart-logs/precursor-uart.log"
    assert cfg.pi_uart_screen == "uart"
    assert cfg.git_describe == "v0.9.8-791-gc707f9d8"
    assert cfg.git_rev == "c707f9d8"
    assert cfg.pi_host is None


def test_env_overrides_dotenv_overrides_defaults(tmp_path: Path) -> None:
    dot = tmp_path / ".env"
    dot.write_text(
        "PI_HOST=pi@from-dotenv\n"
        "XOUS_TARGET=triple-from-dotenv\n"
        "# comment ignored\n"
        'PI_FLASH_DIR="~/quoted-path"\n'
    )
    # Env wins where set; dotenv fills in elsewhere.
    env = {"XOUS_TARGET": "triple-from-env"}
    cfg = load_config(env=env, dotenv_path=dot, repo_root=tmp_path)
    assert cfg.pi_host == "pi@from-dotenv"  # only in .env
    assert cfg.xous_target == "triple-from-env"  # env beats .env
    assert cfg.pi_flash_dir == "~/quoted-path"  # quoting honoured


def test_empty_env_treated_as_unset(tmp_path: Path) -> None:
    """An empty PI_HOST= in the shell should not shadow a real .env value."""
    dot = tmp_path / ".env"
    dot.write_text("PI_HOST=pi@from-dotenv\n")
    cfg = load_config(env={"PI_HOST": ""}, dotenv_path=dot, repo_root=tmp_path)
    assert cfg.pi_host == "pi@from-dotenv"


def test_require_pi_host_raises_when_unset(tmp_path: Path) -> None:
    cfg = load_config(env={}, dotenv_path=tmp_path / "noop.env", repo_root=tmp_path)
    with pytest.raises(RuntimeError, match="PI_HOST"):
        cfg.require_pi_host()


def test_require_pi_host_returns_when_set(tmp_path: Path) -> None:
    cfg = load_config(env={"PI_HOST": "pi@x"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    assert cfg.require_pi_host() == "pi@x"


def test_canonical_xous_img_path_uses_target(tmp_path: Path) -> None:
    """The image path component must match the configured XOUS_TARGET — the
    bug the unified config fixes is having build write to triple/ and
    flash read from precursor-c809403e/ (or vice versa)."""
    env = {"XOUS_CORE_DIR": str(tmp_path / "xc"), "XOUS_TARGET": "foo-bar"}
    cfg = load_config(env=env, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    assert cfg.canonical_xous_img_path() == tmp_path / "xc" / "target" / "foo-bar" / "release" / "xous.img"


def test_xas_bin_path_is_hardware_release(tmp_path: Path) -> None:
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    assert cfg.xas_bin_path() == tmp_path / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"


def test_load_dotenv_strips_quotes_and_comments(tmp_path: Path) -> None:
    p = tmp_path / "a.env"
    p.write_text(
        "# header comment\n"
        "\n"
        "FOO=bare\n"
        'BAR="double"\n'
        "BAZ='single'\n"
        "export QUX=after-export\n"
        "WEIRD = spaced=value\n"
        "not a kv line\n"
    )
    got = load_dotenv(p)
    assert got == {
        "FOO": "bare",
        "BAR": "double",
        "BAZ": "single",
        "QUX": "after-export",
        "WEIRD": "spaced=value",
    }


def test_load_dotenv_missing_file_is_empty(tmp_path: Path) -> None:
    assert load_dotenv(tmp_path / "nope.env") == {}


def test_xous_core_dir_resolution_relative_to_repo_root(tmp_path: Path) -> None:
    """XOUS_CORE_DIR=../xous-core must resolve against the repo root, not cwd."""
    repo = tmp_path / "repo"
    repo.mkdir()
    cfg = load_config(env={"XOUS_CORE_DIR": "../sibling"}, dotenv_path=tmp_path / "n", repo_root=repo)
    assert cfg.xous_core_dir == (tmp_path / "sibling").resolve()


def test_xous_core_dir_absolute_is_left_alone(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    abs_path = tmp_path / "elsewhere"
    cfg = load_config(env={"XOUS_CORE_DIR": str(abs_path)}, dotenv_path=tmp_path / "n", repo_root=repo)
    assert cfg.xous_core_dir == abs_path


def test_dotenv_path_override_env_var(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    custom = tmp_path / "alt.env"
    custom.write_text("PI_HOST=pi@alt\n")
    monkeypatch.setenv("XAS_MCP_DOTENV", str(custom))
    cfg = load_config(env={"XAS_MCP_DOTENV": str(custom)}, repo_root=tmp_path)
    assert cfg.pi_host == "pi@alt"


def test_config_is_dataclass_safe_for_json(tmp_path: Path) -> None:
    """The raw dict must be JSON-serializable so MCP tools can return diagnostic dumps."""
    import json

    cfg = load_config(env={"PI_HOST": "pi@x"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    assert isinstance(cfg, Config)
    json.dumps(cfg.raw)  # would raise on non-serializable values
