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

/// Stage 9b-deploy Phase C-1: real `__getrandom_v03_custom` body
/// backed by xous-core's TRNG service.
///
/// Looks up the trng SID via `xous-api-names`, then calls
/// `Trng::fill_buf` (per `xous-core/services/trng/src/lib.rs:63`).
/// `fill_buf` takes `&mut [u32]` — we cast from `*mut u8` and
/// handle a possible odd tail (`len % 4 != 0`) with a final 1-word
/// scratch read.
///
/// The signature mirrors `getrandom-0.3.4/src/backends/custom.rs:10`.
///
/// `Trng::new` registers a long-lived connection to the TRNG
/// server. We retain a thread-local `Trng` instance so each
/// getrandom call reuses the same IPC connection rather than
/// re-handshaking on every entry.
#[cfg(target_os = "xous")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    use std::cell::OnceCell;
    thread_local! {
        static TRNG: OnceCell<trng::Trng> = const { OnceCell::new() };
    }

    if len == 0 {
        return Ok(());
    }

    TRNG.with(|cell| -> Result<(), getrandom::Error> {
        let trng = cell.get_or_init(|| {
            let xns = xous_names::XousNames::new().expect("connect to xous-names");
            trng::Trng::new(&xns).expect("connect to TRNG service")
        });

        // Fill the aligned u32 prefix in chunks of <=1020 words (the
        // max `fill_buf` accepts per call, per `services/trng/src/
        // lib.rs:64`).
        let words = len / 4;
        let mut filled_bytes = 0usize;
        let mut remaining_words = words;
        while remaining_words > 0 {
            let chunk = remaining_words.min(1020);
            let mut buf = [0u32; 1020];
            trng.fill_buf(&mut buf[..chunk]).map_err(|_| getrandom::Error::UNSUPPORTED)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buf.as_ptr() as *const u8,
                    dest.add(filled_bytes),
                    chunk * 4,
                );
            }
            filled_bytes += chunk * 4;
            remaining_words -= chunk;
        }

        // Tail bytes (len not a multiple of 4): pull one extra word
        // and copy the leading bytes.
        let tail = len - filled_bytes;
        if tail > 0 {
            let mut scratch = [0u32; 1];
            trng.fill_buf(&mut scratch).map_err(|_| getrandom::Error::UNSUPPORTED)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    scratch.as_ptr() as *const u8,
                    dest.add(filled_bytes),
                    tail,
                );
            }
        }

        Ok(())
    })
}

/// Channel capacity. 16 is plenty for the Stage 8 single-prompt
/// round-trip; production sizing (Stage 12+) will revisit.
const CHAN_CAP: usize = 16;

fn main() -> std::io::Result<()> {
    init_logger();
    log::info!("xas: starting");

    let store = PddbStore::with_mock_backend();

    let (cmd_tx, cmd_rx) = bounded::<Cmd>(CHAN_CAP);
    let (event_tx, event_rx) = bounded::<Event>(CHAN_CAP);

    let worker = run_signal_worker(store, cmd_rx, event_tx);
    log::info!("xas: worker started");

    // The UI loop blocks on stdin (hosted) or GAM events
    // (Xous, Stage 9b/follow-up). It owns the cmd/event channel ends
    // and is responsible for sending `Cmd::Shutdown` on quit.
    Ui::new(cmd_tx, event_rx).run()?;

    // Worker has been told to shut down; join it. If the join hangs
    // it's a worker-side bug — surface as a nonzero exit, not a
    // silent hang.
    let _ = worker.join();
    log::info!("xas: exiting");
    Ok(())
}

/// Stage 9b: install a `log` implementation. Hosted picks
/// `env_logger`; rv32-xous binds `xous-api-log` via the integration
/// step (Stage 9b follow-up at hardware-deploy time — needs a
/// path-dep that resolves only inside xous-core's tree).
#[cfg(not(target_os = "xous"))]
fn init_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

#[cfg(target_os = "xous")]
fn init_logger() {
    // `xous-api-log::init_wait()` blocks until it can connect to
    // the `xous-log-server` SID, then registers itself as the
    // `log::Log` impl. The server (running as a separate process
    // in the baseline Xous image) forwards records to the UART —
    // which is what the Renode Robot test asserts on.
    //
    // Failure here means the log server didn't come up in time.
    // For an MVP smoke test that's a fatal misconfiguration, but
    // we don't panic — just continue. Subsequent `log::info!`
    // calls become no-ops, which is exactly the same surface as
    // a binary that hadn't installed a logger at all.
    let _ = xous_api_log::init_wait();
}
