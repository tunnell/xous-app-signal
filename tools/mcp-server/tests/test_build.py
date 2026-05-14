"""Unit tests for xas_mcp.build."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

import pytest

from xas_mcp import build as build_mod
from xas_mcp.config import load_config


def _fake_runner(
    *,
    rc: int = 0,
    side_effect_writes: dict[str, bytes] | None = None,
    log_contents: str = "",
):
    """Return a fake subprocess.run that writes ``side_effect_writes`` paths after success.

    ``side_effect_writes`` maps path → bytes; those paths are created
    so the post-build "did the binary appear" check can succeed.
    """
    side_effect_writes = side_effect_writes or {}
    captured: list[dict[str, Any]] = []

    def fake_run(argv: list[str], **kwargs: Any) -> subprocess.CompletedProcess[Any]:
        # Honour stdout=<file>, stderr=STDOUT contract from build._run_streaming.
        stdout = kwargs.get("stdout")
        if hasattr(stdout, "write"):
            stdout.write(log_contents)
        captured.append({"argv": argv, "cwd": kwargs.get("cwd")})
        if rc == 0:
            for p, blob in side_effect_writes.items():
                pp = Path(p)
                pp.parent.mkdir(parents=True, exist_ok=True)
                pp.write_bytes(blob)
        proc = subprocess.CompletedProcess(args=argv, returncode=rc)
        return proc

    return fake_run, captured


def test_build_xas_argv_and_artifact(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    bin_path = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    fake, captured = _fake_runner(side_effect_writes={str(bin_path): b"\x7fELF stub xas"})
    monkeypatch.setattr(build_mod.subprocess, "run", fake)

    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    result = build_mod.build_xas(
        config=cfg, build_log=tmp_path / "build.log"
    )

    # argv shape mirrors the bash script.
    argv = captured[0]["argv"]
    assert argv[:3] == ["cargo", "build", "--release"]
    assert "--target" in argv
    assert argv[argv.index("--target") + 1] == "riscv32imac-unknown-xous-elf"
    assert argv[argv.index("-p") + 1] == "xous-app-signal"
    assert argv[argv.index("--features") + 1] == "pddb-real,precursor"
    # cwd is the repo root.
    assert captured[0]["cwd"] == str(repo)

    # Result shape.
    assert result["returncode"] == 0
    assert result["path"] == str(bin_path)
    assert result["size_bytes"] == len(b"\x7fELF stub xas")
    assert len(result["sha256"]) == 64
    assert result["log_path"] == str(tmp_path / "build.log")


def test_build_xas_features_overridable(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    bin_path = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    fake, captured = _fake_runner(side_effect_writes={str(bin_path): b"x"})
    monkeypatch.setattr(build_mod.subprocess, "run", fake)

    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    build_mod.build_xas(
        config=cfg,
        features=["hosted", "debug-logging"],
        build_log=tmp_path / "build.log",
    )
    argv = captured[0]["argv"]
    assert argv[argv.index("--features") + 1] == "hosted,debug-logging"


def test_build_xas_propagates_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    fake, _ = _fake_runner(rc=101, log_contents="cargo: missing rustup target\n")
    monkeypatch.setattr(build_mod.subprocess, "run", fake)
    repo = tmp_path / "repo"
    repo.mkdir()
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match=r"exit 101"):
        build_mod.build_xas(config=cfg, build_log=tmp_path / "build.log")


def test_build_xas_missing_binary_after_success(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    fake, _ = _fake_runner(side_effect_writes={})  # success rc but no output
    monkeypatch.setattr(build_mod.subprocess, "run", fake)
    repo = tmp_path / "repo"
    repo.mkdir()
    cfg = load_config(env={}, dotenv_path=tmp_path / "n", repo_root=repo)
    with pytest.raises(RuntimeError, match="binary not found"):
        build_mod.build_xas(config=cfg, build_log=tmp_path / "build.log")


def test_bundle_kernel_image_argv_and_artifact(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    xc = tmp_path / "xc"
    xc.mkdir()
    xas_bin = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    xas_bin.parent.mkdir(parents=True)
    xas_bin.write_bytes(b"xas-elf-stub")
    img_path = xc / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xous.img"

    fake, captured = _fake_runner(
        side_effect_writes={str(img_path): b"xous-img-bytes-..." * 100}
    )
    monkeypatch.setattr(build_mod.subprocess, "run", fake)

    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=repo,
    )
    result = build_mod.bundle_kernel_image(config=cfg, build_log=tmp_path / "bundle.log")

    argv = captured[0]["argv"]
    assert argv[:4] == ["cargo", "xtask", "app-image-xip", f"xas:{xas_bin}"]
    # extra apps preserved.
    assert "vault" in argv and "transientdisk" in argv
    # canonical flags.
    assert "--kernel-feature" in argv
    assert argv[argv.index("--kernel-feature") + 1] == "big-heap"
    assert "--gdb-stub" in argv
    assert "--git-describe" in argv
    assert argv[argv.index("--git-describe") + 1] == "v0.9.8-791-gc707f9d8"
    assert "--git-rev" in argv
    assert argv[argv.index("--git-rev") + 1] == "c707f9d8"
    # cwd is xous-core, not the repo.
    assert captured[0]["cwd"] == str(xc)

    # Result.
    assert result["returncode"] == 0
    assert result["path"] == str(img_path)
    assert result["size_bytes"] > 0


def test_bundle_kernel_image_xas_prefix_required(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Regression: the `xas:` prefix on the binary is load-bearing."""
    repo = tmp_path / "repo"
    repo.mkdir()
    xc = tmp_path / "xc"
    xc.mkdir()
    xas_bin = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    xas_bin.parent.mkdir(parents=True)
    xas_bin.write_bytes(b"x")
    img_path = xc / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xous.img"
    fake, captured = _fake_runner(side_effect_writes={str(img_path): b"img"})
    monkeypatch.setattr(build_mod.subprocess, "run", fake)

    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=repo,
    )
    build_mod.bundle_kernel_image(config=cfg, build_log=tmp_path / "b.log")
    # the binary entry MUST start with the "xas:" prefix; without it
    # xtask's CrateSpec records name=None.
    bin_token = captured[0]["argv"][3]
    assert bin_token.startswith("xas:")


def test_bundle_kernel_image_missing_xas_binary(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    xc = tmp_path / "xc"
    xc.mkdir()
    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=repo,
    )
    with pytest.raises(RuntimeError, match="xas binary not found"):
        build_mod.bundle_kernel_image(config=cfg, build_log=tmp_path / "b.log")


def test_bundle_kernel_image_missing_xous_core(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    xas_bin = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    xas_bin.parent.mkdir(parents=True)
    xas_bin.write_bytes(b"x")
    cfg = load_config(
        env={"XOUS_CORE_DIR": str(tmp_path / "missing")},
        dotenv_path=tmp_path / "n",
        repo_root=repo,
    )
    with pytest.raises(RuntimeError, match="xous-core directory not found"):
        build_mod.bundle_kernel_image(config=cfg, build_log=tmp_path / "b.log")


def test_bundle_kernel_image_gdb_stub_toggle(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    xc = tmp_path / "xc"
    xc.mkdir()
    xas_bin = repo / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xas"
    xas_bin.parent.mkdir(parents=True)
    xas_bin.write_bytes(b"x")
    img_path = xc / "target" / "riscv32imac-unknown-xous-elf" / "release" / "xous.img"
    fake, captured = _fake_runner(side_effect_writes={str(img_path): b"img"})
    monkeypatch.setattr(build_mod.subprocess, "run", fake)

    cfg = load_config(
        env={"XOUS_CORE_DIR": str(xc)},
        dotenv_path=tmp_path / "n",
        repo_root=repo,
    )
    build_mod.bundle_kernel_image(
        config=cfg, gdb_stub=False, build_log=tmp_path / "b.log"
    )
    assert "--gdb-stub" not in captured[0]["argv"]
