"""Unit tests for renode/hosted/cargo test runners (mocked subprocess)."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from xas_mcp import cargo as cargo_mod
from xas_mcp import tests_hosted as hosted_mod
from xas_mcp import tests_renode as renode_mod
from xas_mcp.config import load_config


def _cp(rc: int = 0, stdout: str = "", stderr: str = "") -> MagicMock:
    cp = MagicMock(spec=subprocess.CompletedProcess)
    cp.returncode = rc
    cp.stdout = stdout
    cp.stderr = stderr
    return cp


# --- renode ------------------------------------------------------------------


def test_run_renode_test_argv_and_result(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "renode").mkdir(parents=True)
    (repo / "tests" / "renode" / "xas-smoke.robot").write_text("# stub robot\n")
    elf = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    elf.parent.mkdir(parents=True)

    calls: list[dict[str, Any]] = []

    def fake_run(argv: list[str], **kwargs: Any) -> Any:
        calls.append({"argv": argv, "cwd": kwargs.get("cwd")})
        if argv[:2] == ["cargo", "build"]:
            stdout = kwargs.get("stdout")
            if hasattr(stdout, "write"):
                stdout.write("cargo build ok\n")
            elf.write_bytes(b"xas-elf-stub")
            return _cp(0)
        if argv[0] == "renode-test":
            return _cp(0, stdout="11 tests, 11 passed\n")
        return _cp(0)

    monkeypatch.setattr(renode_mod.subprocess, "run", fake_run)
    monkeypatch.setattr(renode_mod.shutil, "which", lambda _x: "/usr/bin/renode-test")

    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    result = renode_mod.run_renode_test(config=cfg)
    assert result["pass"]
    assert result["robot"] == "xas-smoke.robot"
    assert "passed" in result["log"]
    # ELF must be staged in dist/xas-rv32/
    assert Path(result["dist_path"]) == repo / "dist" / "xas-rv32" / "xas"
    assert (repo / "dist" / "xas-rv32" / "xas").is_file()
    # cargo build invoked with the canonical hardware flags
    build_argv = calls[0]["argv"]
    assert build_argv[:2] == ["cargo", "build"]
    assert "--target" in build_argv
    assert build_argv[build_argv.index("--target") + 1] == "riscv32imac-unknown-xous-elf"
    assert build_argv[build_argv.index("--features") + 1] == "pddb-real,precursor"
    # renode-test invoked from the renode dir with the robot filename
    renode_call = next(c for c in calls if c["argv"][0] == "renode-test")
    assert renode_call["cwd"] == str(repo / "tests" / "renode")
    assert renode_call["argv"][1] == "xas-smoke.robot"


def test_run_renode_test_missing_robot(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "renode").mkdir(parents=True)
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match="not found"):
        renode_mod.run_renode_test(config=cfg, robot_file="does-not-exist.robot")


def test_run_renode_test_missing_renode_bin(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "renode").mkdir(parents=True)
    (repo / "tests" / "renode" / "xas-smoke.robot").write_text("# stub\n")
    monkeypatch.setattr(renode_mod.shutil, "which", lambda _x: None)
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match="not on PATH"):
        renode_mod.run_renode_test(config=cfg)


def test_parse_cargo_test_result_sums_all_binaries() -> None:
    log = (
        "running 5 tests\n"
        "test result: ok. 5 passed; 0 failed; 0 ignored\n"
        "running 3 tests\n"
        "test result: FAILED. 2 passed; 1 failed; 0 ignored\n"
    )
    passed, failed = renode_mod.parse_cargo_test_result(log)
    assert passed == 7
    assert failed == 1


# --- hosted ------------------------------------------------------------------


def test_kill_orphan_kernels_calls_pkill_per_pattern(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[list[str]] = []

    def fake_run(argv: list[str], **_kwargs: Any) -> Any:
        calls.append(argv)
        return _cp(0)

    monkeypatch.setattr(hosted_mod.subprocess, "run", fake_run)
    monkeypatch.setattr(hosted_mod.time, "sleep", lambda _: None)
    result = hosted_mod.kill_orphan_kernels()
    assert len(calls) == len(hosted_mod._ORPHAN_PATTERNS)
    for call in calls:
        assert call[:3] == ["pkill", "-KILL", "-f"]
    assert set(result.keys()) == set(hosted_mod._ORPHAN_PATTERNS)


def test_run_hosted_test_unknown_name(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match="unknown hosted test"):
        hosted_mod.run_hosted_test("not_a_test", config=cfg)


def test_run_hosted_test_invokes_bash_script_and_kills_orphans(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "hosted").mkdir(parents=True)
    (repo / "tests" / "hosted" / "test_link_qr.sh").write_text("#!/bin/bash\nexit 0\n")
    calls: list[list[str]] = []

    def fake_run(argv: list[str], **_kwargs: Any) -> Any:
        calls.append(list(argv))
        if argv[:3] == ["pkill", "-KILL", "-f"]:
            return _cp(0)
        return _cp(0, stdout="ok\n")

    monkeypatch.setattr(hosted_mod.subprocess, "run", fake_run)
    monkeypatch.setattr(hosted_mod.time, "sleep", lambda _: None)

    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    # Force DISPLAY so we skip the xvfb wrap path here.
    result = hosted_mod.run_hosted_test(
        "link_qr", config=cfg, env={"DISPLAY": ":10"}
    )
    assert result["pass"]
    # Orphan-cleanup pkills + bash invocation.
    pkill_calls = [c for c in calls if c[:3] == ["pkill", "-KILL", "-f"]]
    assert len(pkill_calls) == len(hosted_mod._ORPHAN_PATTERNS)
    bash_call = next(c for c in calls if c[0] == "bash")
    assert bash_call[1].endswith("test_link_qr.sh")


def test_run_hosted_test_xvfb_wrap_when_display_empty(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "hosted").mkdir(parents=True)
    (repo / "tests" / "hosted" / "test_link_qr.sh").write_text("#!/bin/bash\n")
    calls: list[list[str]] = []

    def fake_run(argv: list[str], **_kwargs: Any) -> Any:
        calls.append(list(argv))
        return _cp(0)

    monkeypatch.setattr(hosted_mod.subprocess, "run", fake_run)
    monkeypatch.setattr(hosted_mod.time, "sleep", lambda _: None)
    monkeypatch.setattr(hosted_mod.shutil, "which", lambda x: "/usr/bin/xvfb-run")
    monkeypatch.delenv("DISPLAY", raising=False)

    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    hosted_mod.run_hosted_test("link_qr", config=cfg, env={})
    bash_call = next(c for c in calls if "test_link_qr.sh" in " ".join(c))
    assert bash_call[0] == "xvfb-run"


def test_run_hosted_test_xvfb_missing_when_display_empty(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    (repo / "tests" / "hosted").mkdir(parents=True)
    (repo / "tests" / "hosted" / "test_link_qr.sh").write_text("#!/bin/bash\n")
    monkeypatch.setattr(hosted_mod.shutil, "which", lambda _x: None)
    monkeypatch.delenv("DISPLAY", raising=False)
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match="xvfb-run"):
        hosted_mod.run_hosted_test(
            "link_qr", config=cfg, env={}, cleanup_orphans=False
        )


# --- cargo -------------------------------------------------------------------


def test_cargo_test_argv_and_parse(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = (
        "running 5 tests\n"
        "test result: ok. 5 passed; 0 failed; 0 ignored\n"
    )
    calls: list[list[str]] = []

    def fake_run(argv: list[str], **_kwargs: Any) -> Any:
        calls.append(list(argv))
        return _cp(0, stdout=log)

    monkeypatch.setattr(cargo_mod.subprocess, "run", fake_run)
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    result = cargo_mod.cargo_test(config=cfg, package="my-pkg", features=["hosted"])
    assert result["pass"]
    assert result["n_passed"] == 5
    assert result["n_failed"] == 0
    argv = calls[0]
    assert argv[:3] == ["cargo", "test", "-p"]
    assert argv[3] == "my-pkg"
    assert "--features" in argv
    assert argv[argv.index("--features") + 1] == "hosted"


def test_cargo_test_propagates_failure(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    def fake_run(argv: list[str], **_kwargs: Any) -> Any:
        return _cp(101, stdout="test result: FAILED. 4 passed; 1 failed; 0 ignored\n")

    monkeypatch.setattr(cargo_mod.subprocess, "run", fake_run)
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    result = cargo_mod.cargo_test(config=cfg)
    assert not result["pass"]
    assert result["n_passed"] == 4
    assert result["n_failed"] == 1
