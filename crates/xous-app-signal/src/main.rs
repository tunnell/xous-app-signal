//! Xous Signal app entry point.
//!
//! Stage 9c: replaces the Stage 8 sequential Hello/Whoami probe with
//! a real UI loop. The probe lives on as the menu's "Test worker"
//! item — useful for verifying the worker thread + IPC channels are
//! still alive after a code change. The shape of the binary is now:
//!
//! 1. Construct a `PddbStore` (mock backend in hosted; real PDDB
//!    behind a `pddb-backend` feature flag at Stage 9b).
//! 2. Spawn the manager worker thread (`xous-signal-bridge`).
//! 3. Hand the cmd/event channels to `Ui::new` and call `Ui::run`.
//! 4. Worker shutdown is the responsibility of the UI driver — it
//!    sends `Cmd::Shutdown` on Quit.
//!
//! See docs/ROADMAP.md Stage 9c and docs/UI.md for the design.

use async_channel::bounded;
use presage_store_pddb::PddbStore;
use xous_app_signal_ui::Ui;
use xous_signal_bridge::{Cmd, Event, run_signal_worker};

/// Stage 9a: provide the `__getrandom_v03_custom` symbol the
/// `--cfg getrandom_backend="custom"` rv32-xous build requires.
///
/// Body is a panic for now — Stage 9b replaces it with a real call
/// to xous-core's `trng::Trng` client (`services/trng/src/lib.rs`,
/// see `Trng::get_u64` and `Trng::fill_buf`). Until then the symbol
/// just needs to exist so the linker resolves; any code path that
/// actually consumes randomness will panic, which is exactly what
/// we want — Stage 9b's Renode boot test will catch any missing
/// wiring before MVP flows ship.
///
/// The signature mirrors `getrandom-0.3.4/src/backends/custom.rs:10`.
#[cfg(target_os = "xous")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    _dest: *mut u8,
    _len: usize,
) -> Result<(), getrandom::Error> {
    panic!(
        "__getrandom_v03_custom: Stage 9b wires xous-core's trng client; \
         hit before that landed"
    );
}

/// Channel capacity. 16 is plenty for the Stage 8 single-prompt
/// round-trip; production sizing (Stage 12+) will revisit.
const CHAN_CAP: usize = 16;

fn main() -> std::io::Result<()> {
    let store = PddbStore::with_mock_backend();

    let (cmd_tx, cmd_rx) = bounded::<Cmd>(CHAN_CAP);
    let (event_tx, event_rx) = bounded::<Event>(CHAN_CAP);

    let worker = run_signal_worker(store, cmd_rx, event_tx);

    // The UI loop blocks on stdin (hosted) or GAM events
    // (Xous, Stage 9b/follow-up). It owns the cmd/event channel ends
    // and is responsible for sending `Cmd::Shutdown` on quit.
    Ui::new(cmd_tx, event_rx).run()?;

    // Worker has been told to shut down; join it. If the join hangs
    // it's a worker-side bug — surface as a nonzero exit, not a
    // silent hang.
    let _ = worker.join();
    Ok(())
}
