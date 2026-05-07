//! Build glue for the `xas` Signal client.
//!
//! Usage:
//!
//! ```sh
//! cargo xtask <subcommand>
//! ```
//!
//! Sub-commands:
//!
//! - **`build-rv32`** — cross-compile the `xas` binary for
//!   `riscv32imac-unknown-xous-elf` (release profile). Requires the
//!   rv32-xous rust-std component to be installed via the
//!   xous-core toolchain bootstrap; if missing, the underlying
//!   `cargo build` fails with a clear message.
//! - **`dist`** — runs `build-rv32` then copies the resulting ELF
//!   to a known dist location for downstream consumption (e.g.
//!   xous-core's image builder picking it up). Default destination
//!   is `dist/xas-rv32/xas`; override with `XAS_DIST_DIR=...`.
//! - **`renode-test`** — invokes `renode-test` against
//!   `tests/renode/xas-smoke.robot`. Requires Renode 1.16+ on
//!   PATH and a built Xous image at the location the `.resc`
//!   script expects. End-to-end this is what the ROADMAP
//!   verification step runs.
//! - **`help`** (default) — prints this list.
//!
//! No external dependencies. Each subcommand is one
//! `std::process::Command::spawn`+`wait` plus stdio plumbing.

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const TARGET_TRIPLE: &str = "riscv32imac-unknown-xous-elf";
const RELEASE_PROFILE: &str = "release";
const BIN_NAME: &str = "xas";
const PACKAGE_NAME: &str = "xous-app-signal";
const ROBOT_SCRIPT: &str = "tests/renode/xas-smoke.robot";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());

    let result = match cmd.as_str() {
        "build-rv32" => build_rv32(),
        "dist" => dist(),
        "renode-test" => renode_test(),
        "help" | "-h" | "--help" => {
            print_help();
            return ExitCode::SUCCESS;
        }
        unknown => {
            eprintln!("xtask: unknown subcommand `{unknown}`. Run `cargo xtask help`.");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("xtask — build glue for xas (xous-app-signal)\n");
    println!("Usage:");
    println!("  cargo xtask build-rv32     Cross-compile xas for rv32-xous");
    println!("  cargo xtask dist           build-rv32 + copy ELF to $XAS_DIST_DIR");
    println!("  cargo xtask renode-test    Run tests/renode/xas-smoke.robot");
    println!("  cargo xtask help           Print this message\n");
    println!("Environment:");
    println!("  XAS_DIST_DIR  — destination for `dist`. Default: dist/xas-rv32/");
    println!("  RENODE        — renode-test binary. Default: renode-test");
}

/// `cargo build --target=riscv32imac-unknown-xous-elf --release -p xous-app-signal`
/// `--features pddb-real,precursor`.
///
/// The features are non-negotiable for hardware deploy:
/// - `pddb-real`: real PDDB-backed store (link state survives across
///   power cycles). Without it the rv32 binary uses the in-memory mock
///   and forgets every link the moment the device sleeps.
/// - `precursor`: per-service feature cascade (blitstr2/precursor,
///   ux-api/precursor, gam/precursor, modals/precursor, graphics-
///   server/precursor, utralib/precursor*). Without it the UI crates
///   build for the wrong target ABI.
fn build_rv32() -> Result<(), String> {
    eprintln!("xtask: cross-compile xas for {TARGET_TRIPLE} (release)");
    let status = Command::new(cargo_bin())
        .args([
            "build",
            "--target",
            TARGET_TRIPLE,
            "--release",
            "-p",
            PACKAGE_NAME,
            "--features",
            "pddb-real,precursor",
        ])
        .status()
        .map_err(|e| format!("spawn cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build failed (exit {:?})", status.code()));
    }
    Ok(())
}

/// build-rv32 + copy the resulting ELF to `$XAS_DIST_DIR/xas`.
fn dist() -> Result<(), String> {
    build_rv32()?;
    let workspace_root = workspace_root()?;
    let elf_src = workspace_root
        .join("target")
        .join(TARGET_TRIPLE)
        .join(RELEASE_PROFILE)
        .join(BIN_NAME);
    if !elf_src.is_file() {
        return Err(format!(
            "expected ELF at {} not produced",
            elf_src.display()
        ));
    }
    let dst_dir = env::var("XAS_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("dist").join("xas-rv32"));
    std::fs::create_dir_all(&dst_dir).map_err(|e| format!("mkdir {}: {e}", dst_dir.display()))?;
    let elf_dst = dst_dir.join(BIN_NAME);
    std::fs::copy(&elf_src, &elf_dst)
        .map_err(|e| format!("copy {} -> {}: {e}", elf_src.display(), elf_dst.display()))?;
    eprintln!(
        "xtask: copied {} -> {} ({} bytes)",
        elf_src.display(),
        elf_dst.display(),
        std::fs::metadata(&elf_dst).map(|m| m.len()).unwrap_or(0)
    );
    Ok(())
}

/// `renode-test tests/renode/xas-smoke.robot`. Renode must be on
/// PATH (override with `RENODE` env var) and a Xous image
/// containing the xas ELF must already exist at the path the
/// .resc script expects (set inside the .resc; default
/// `dist/xas-rv32/xas`).
fn renode_test() -> Result<(), String> {
    let workspace_root = workspace_root()?;
    let robot = workspace_root.join(ROBOT_SCRIPT);
    if !robot.is_file() {
        return Err(format!("Robot script missing: {}", robot.display()));
    }
    let renode = env::var("RENODE").unwrap_or_else(|_| "renode-test".into());
    eprintln!("xtask: {} {}", renode, robot.display());
    let status = Command::new(&renode)
        .arg(&robot)
        .status()
        .map_err(|e| format!("spawn {renode}: {e}"))?;
    if !status.success() {
        return Err(format!("{renode} failed (exit {:?})", status.code()));
    }
    Ok(())
}

fn cargo_bin() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn workspace_root() -> Result<PathBuf, String> {
    // `CARGO_MANIFEST_DIR` for xtask is `<workspace>/xtask/`.
    let dir = env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set; run via `cargo xtask`".to_string())?;
    PathBuf::from(dir)
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "could not derive workspace root from CARGO_MANIFEST_DIR".to_string())
}
