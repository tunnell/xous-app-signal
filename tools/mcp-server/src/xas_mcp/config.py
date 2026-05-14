"""Configuration for the xas-mcp server and CLI.

A single ``Config`` dataclass is loaded from environment variables, with an
optional ``.env`` file at ``tools/mcp-server/.env`` providing defaults that
the shell environment can still override.

The defaults here are the canonical values; the existing bash scripts
historically drifted apart on ``XOUS_TARGET`` (build-and-bundle.sh used
the cargo target triple, flash-via-pi.sh used the legacy
``precursor-c809403e`` alias). Both forms now resolve to the same path
via ``Config.canonical_xous_img_path``; see ``notes/chores/CHORES.md``
for history.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path

__all__ = ["Config", "load_config", "load_dotenv"]


# Default values mirror BUILDING.md §3.2 and the env-var table in
# tests/precursor/README.md. Defaults that *differ* from any individual
# bash script default are deliberately the corrected form (see
# CHORES.md). Each entry is one source of truth.
_DEFAULTS = {
    "PI_HOST": None,  # required for Pi-side tools, else they raise
    "PI_FLASH_DIR": "~/xous-flash",
    "PI_UART_LOG": "~/uart-logs/precursor-uart.log",
    "PI_UART_SCREEN": "uart",
    "FLASH_LOG_DIR": "/tmp",
    "XOUS_CORE_DIR": "../xous-core",
    "XOUS_TARGET": "riscv32imac-unknown-xous-elf",
    "GIT_DESCRIBE": "v0.9.8-791-gc707f9d8",
    "GIT_REV": "c707f9d8",
}


def _repo_root_from(start: Path) -> Path:
    """Walk up from ``start`` until a directory containing ``Cargo.toml`` shows up.

    We anchor on Cargo.toml rather than ``.git`` because this package
    lives inside a worktree and the .git is a file, not a directory.
    """
    cur = start.resolve()
    for _ in range(20):
        if (cur / "Cargo.toml").is_file() and (cur / "tests").is_dir():
            return cur
        if cur.parent == cur:
            break
        cur = cur.parent
    # Fall back to start so callers still get a usable Path; downstream
    # tools will report a clearer error when paths don't resolve.
    return start.resolve()


def load_dotenv(path: Path) -> dict[str, str]:
    """Minimal ``.env`` parser: ``KEY=value`` lines, ``#`` comments, no exports.

    Quoting follows the shell convention only loosely: matching single or
    double quotes are stripped; everything else is treated literally. This
    keeps the parser dependency-free without pretending to be a full
    POSIX-shell substitute.
    """
    out: dict[str, str] = {}
    if not path.is_file():
        return out
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :].lstrip()
        m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$", line)
        if not m:
            continue
        key, val = m.group(1), m.group(2).strip()
        if len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
            val = val[1:-1]
        out[key] = val
    return out


@dataclass
class Config:
    """Resolved configuration for the xas-mcp server and CLIs.

    Construct via :func:`load_config` rather than directly so env + .env
    + defaults are merged consistently.
    """

    repo_root: Path
    pi_host: str | None
    pi_flash_dir: str
    pi_uart_log: str
    pi_uart_screen: str
    flash_log_dir: str
    xous_core_dir: Path
    xous_target: str
    git_describe: str
    git_rev: str
    # Raw dict carried for diagnostics (e.g., ``--config-dump``).
    raw: dict[str, str] = field(default_factory=dict)

    def require_pi_host(self) -> str:
        """Return ``pi_host`` or raise a friendly error pointing at the env var."""
        if not self.pi_host:
            raise RuntimeError(
                "PI_HOST is not set. Export it (e.g. PI_HOST=pi@10.0.0.42) "
                "or add it to tools/mcp-server/.env before invoking this tool."
            )
        return self.pi_host

    def xas_bin_path(self) -> Path:
        """Path the hardware build of xas lands at after ``cargo build --release``."""
        return (
            self.repo_root
            / "target"
            / "riscv32imac-unknown-xous-elf"
            / "release"
            / "xas"
        )

    def canonical_xous_img_path(self) -> Path:
        """Path xous.img lands at after ``cargo xtask app-image-xip``.

        Uses ``self.xous_target`` as the path component, matching what
        xtask actually writes to. The bash scripts used to mismatch on
        this — see CHORES.md.
        """
        return (
            self.xous_core_dir
            / "target"
            / self.xous_target
            / "release"
            / "xous.img"
        )


def load_config(
    *,
    env: dict[str, str] | None = None,
    dotenv_path: Path | None = None,
    repo_root: Path | None = None,
) -> Config:
    """Resolve ``Config`` from the process environment + an optional .env file.

    Resolution order (highest precedence first):

    1. ``env`` argument (only used for testing).
    2. ``os.environ`` at call time.
    3. Values parsed from ``dotenv_path`` (defaults to
       ``tools/mcp-server/.env`` next to this file; override with
       the ``XAS_MCP_DOTENV`` env var).
    4. ``_DEFAULTS`` baked into this module.

    Empty-string env values are treated as unset, so a stray
    ``PI_HOST=`` in the shell won't shadow the .env value.
    """
    real_env = os.environ if env is None else env

    pkg_root = Path(__file__).resolve().parent.parent.parent  # .../tools/mcp-server/
    if dotenv_path is None:
        override = real_env.get("XAS_MCP_DOTENV")
        dotenv_path = Path(override).expanduser() if override else pkg_root / ".env"
    dot = load_dotenv(dotenv_path)

    def pick(key: str) -> str | None:
        v = real_env.get(key)
        if v is not None and v != "":
            return v
        v = dot.get(key)
        if v is not None and v != "":
            return v
        default = _DEFAULTS.get(key)
        return default

    raw = {k: pick(k) or "" for k in _DEFAULTS}

    rr = repo_root if repo_root is not None else _repo_root_from(Path.cwd())
    xous_core = Path(raw["XOUS_CORE_DIR"])
    if not xous_core.is_absolute():
        xous_core = (rr / xous_core).resolve()

    return Config(
        repo_root=rr,
        pi_host=raw["PI_HOST"] or None,
        pi_flash_dir=raw["PI_FLASH_DIR"],
        pi_uart_log=raw["PI_UART_LOG"],
        pi_uart_screen=raw["PI_UART_SCREEN"],
        flash_log_dir=raw["FLASH_LOG_DIR"],
        xous_core_dir=xous_core,
        xous_target=raw["XOUS_TARGET"],
        git_describe=raw["GIT_DESCRIBE"],
        git_rev=raw["GIT_REV"],
        raw=raw,
    )
