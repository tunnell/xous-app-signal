"""Cross-compile xas and bundle a xous.img for Precursor PVT2 hardware.

Mirrors ``tests/precursor/build-and-bundle.sh`` step-for-step. The
canonical hardware flags from BUILDING.md §3.2 are hardcoded as
defaults — pass through kwargs only when you genuinely need to
override them (e.g., switching from kernel-only `big-heap` to a
slim build).
"""

from __future__ import annotations

import hashlib
import subprocess
import time
from collections.abc import Iterable, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .config import Config, load_config

__all__ = [
    "BuildArtifact",
    "build_xas",
    "bundle_kernel_image",
    "DEFAULT_XAS_FEATURES",
    "DEFAULT_KERNEL_FEATURES",
    "DEFAULT_EXTRA_APPS",
]


# Features the bash script bakes in. Kept as a constant so callers can
# diff against it before changing — every feature flip is a hardware
# behavior change.
DEFAULT_XAS_FEATURES: tuple[str, ...] = ("pddb-real", "precursor")

# Extra apps bundled alongside xas. ``vault`` + ``transientdisk`` are
# matched by manifest.json entries on the xous-core fork; without them
# the bundled image cannot unlock PDDB or hold a transient FAT image.
DEFAULT_EXTRA_APPS: tuple[str, ...] = ("vault", "transientdisk")

# Kernel-feature toggles for ``cargo xtask app-image-xip``. ``big-heap``
# is required for xas's working set; the bash script passes it
# unconditionally.
DEFAULT_KERNEL_FEATURES: tuple[str, ...] = ("big-heap",)


@dataclass(frozen=True)
class BuildArtifact:
    """A compiled binary or bundled image. Returned by both build tools."""

    path: str
    size_bytes: int
    sha256: str

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def _run_streaming(
    argv: Sequence[str],
    *,
    cwd: Path,
    log_path: Path,
) -> int:
    """Run ``argv`` capturing combined stdout/stderr to ``log_path``.

    We don't surface the live stream — the bash script also writes to a
    file and tails on failure. The caller decides how to surface logs
    (the CLI does ``tail -50`` on nonzero exit).
    """
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w") as logf:
        proc = subprocess.run(
            list(argv),
            cwd=str(cwd),
            stdout=logf,
            stderr=subprocess.STDOUT,
            check=False,
        )
    return proc.returncode


def _tail(path: Path, n: int = 50) -> str:
    try:
        with path.open() as f:
            lines = f.readlines()
    except FileNotFoundError:
        return ""
    return "".join(lines[-n:])


def build_xas(
    *,
    config: Config | None = None,
    features: Iterable[str] = DEFAULT_XAS_FEATURES,
    target: str | None = None,
    release: bool = True,
    package: str = "xous-app-signal",
    build_log: Path | None = None,
    cargo: str = "cargo",
) -> dict[str, Any]:
    """Cross-compile xas (release, hardware target) and return artefact metadata.

    The default invocation mirrors ``tests/precursor/build-and-bundle.sh``
    step 1::

        cargo build --release --target riscv32imac-unknown-xous-elf \\
            -p xous-app-signal --features pddb-real,precursor

    Returns ``{path, size_bytes, sha256, log_path, returncode, command}``.
    Raises ``RuntimeError`` if the build fails, with the last 50 lines
    of the build log embedded for triage.
    """
    cfg = config or load_config()
    triple = target or "riscv32imac-unknown-xous-elf"
    if build_log is None:
        build_log = Path(f"/tmp/xous-build-{int(time.time())}.log")

    argv: list[str] = [cargo, "build"]
    if release:
        argv.append("--release")
    argv += ["--target", triple, "-p", package]
    feat_list = list(features)
    if feat_list:
        argv += ["--features", ",".join(feat_list)]

    rc = _run_streaming(argv, cwd=cfg.repo_root, log_path=build_log)
    if rc != 0:
        raise RuntimeError(
            f"xas build failed (exit {rc}). Last lines of {build_log}:\n{_tail(build_log)}"
        )

    bin_path = cfg.repo_root / "target" / triple / ("release" if release else "debug") / "xas"
    if not bin_path.is_file():
        raise RuntimeError(
            f"xas build reported success but binary not found at {bin_path}. "
            f"See {build_log}."
        )

    art = BuildArtifact(
        path=str(bin_path),
        size_bytes=bin_path.stat().st_size,
        sha256=_sha256(bin_path),
    )
    return {
        **art.to_dict(),
        "log_path": str(build_log),
        "returncode": rc,
        "command": argv,
    }


def bundle_kernel_image(
    *,
    config: Config | None = None,
    xas_bin: Path | None = None,
    extra_apps: Iterable[str] = DEFAULT_EXTRA_APPS,
    kernel_features: Iterable[str] = DEFAULT_KERNEL_FEATURES,
    gdb_stub: bool = True,
    git_describe: str | None = None,
    git_rev: str | None = None,
    build_log: Path | None = None,
    cargo: str = "cargo",
) -> dict[str, Any]:
    """Bundle xous.img via ``cargo xtask app-image-xip``.

    Mirrors ``tests/precursor/build-and-bundle.sh`` step 2. The
    ``xas:`` prefix on the xas binary is *required* — without it,
    xtask's CrateSpec parser silently records ``name = None`` and
    skips xas. The default ``extra_apps`` (vault, transientdisk)
    match manifest.json entries in the xous-core fork.

    Returns ``{path, size_bytes, sha256, log_path, returncode, command}``.
    """
    cfg = config or load_config()
    if xas_bin is None:
        xas_bin = cfg.xas_bin_path()
    if not xas_bin.is_file():
        raise RuntimeError(
            f"xas binary not found at {xas_bin}. Build it first via build_xas()."
        )
    if not cfg.xous_core_dir.is_dir():
        raise RuntimeError(
            f"xous-core directory not found at {cfg.xous_core_dir}. "
            f"Set XOUS_CORE_DIR or pass --xous-core-dir."
        )
    if build_log is None:
        build_log = Path(f"/tmp/xous-image-{int(time.time())}.log")

    argv: list[str] = [
        cargo,
        "xtask",
        "app-image-xip",
        f"xas:{xas_bin}",
    ]
    argv += list(extra_apps)
    for feat in kernel_features:
        argv += ["--kernel-feature", feat]
    if gdb_stub:
        argv.append("--gdb-stub")
    argv += [
        "--git-describe",
        git_describe or cfg.git_describe,
        "--git-rev",
        git_rev or cfg.git_rev,
    ]

    rc = _run_streaming(argv, cwd=cfg.xous_core_dir, log_path=build_log)
    if rc != 0:
        raise RuntimeError(
            f"app-image-xip failed (exit {rc}). Last lines of {build_log}:\n{_tail(build_log)}"
        )

    img_path = cfg.canonical_xous_img_path()
    if not img_path.is_file():
        raise RuntimeError(
            f"app-image-xip reported success but xous.img not found at {img_path}. "
            f"Check XOUS_TARGET (currently {cfg.xous_target!r}). See {build_log}."
        )

    art = BuildArtifact(
        path=str(img_path),
        size_bytes=img_path.stat().st_size,
        sha256=_sha256(img_path),
    )
    return {
        **art.to_dict(),
        "log_path": str(build_log),
        "returncode": rc,
        "command": argv,
    }
