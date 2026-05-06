//! Xous Signal app entry point.
//!
//! Stage 8: first end-to-end glue. Construct a `PddbStore` (mock
//! backend in hosted mode; Stage 9 swaps in the real PDDB-backed
//! impl), spawn the manager worker thread, exchange a few commands
//! over the async-channel IPC pair, then shut down cleanly.
//!
//! Stage 1 was a smol-rs `LocalExecutor` smoke test — that work has
//! moved into the `xous-signal-bridge` worker, which uses the same
//! executor pattern. The hosted-mode test that drives this binary
//! prints "pong" and "whoami: <error string>" then exits — same
//! shape as the Renode smoke test ROADMAP §Stage 9 will assert on.
//!
//! See docs/ROADMAP.md Stage 8 for context.

use async_channel::bounded;
use presage_store_pddb::PddbStore;
use xous_signal_bridge::{Cmd, Event, run_signal_worker};

/// Channel capacity. 16 is plenty for the Stage 8 single-prompt
/// round-trip; production sizing (Stage 12+) will revisit.
const CHAN_CAP: usize = 16;

fn main() {
    println!("xas: starting");

    // Stage 8 uses the in-memory mock backend. Stage 9 will switch to
    // the real `pddb::Pddb`-backed impl behind the `pddb-backend`
    // feature flag.
    let store = PddbStore::with_mock_backend();

    let (cmd_tx, cmd_rx) = bounded::<Cmd>(CHAN_CAP);
    let (event_tx, event_rx) = bounded::<Event>(CHAN_CAP);

    // Worker thread owns the executor + manager state machine; main
    // thread drives the IPC commands.
    let worker = run_signal_worker(store, cmd_rx, event_tx);
    println!("xas: worker started");

    // 1. Channel-roundtrip ping. Confirms the worker is alive.
    cmd_tx
        .send_blocking(Cmd::Hello)
        .expect("worker accepts Hello");
    match event_rx.recv_blocking() {
        Ok(Event::Pong) => println!("xas: pong"),
        Ok(other) => panic!("expected Pong, got {other:?}"),
        Err(e) => panic!("event channel closed before Pong: {e}"),
    }

    // 2. GetWhoami against an empty store — expected to surface
    //    `Manager::load_registered`'s `NotYetRegisteredError` path.
    //    The point is to round-trip a Result<_, String> through the
    //    IPC, not to actually identify ourselves to Signal.
    cmd_tx
        .send_blocking(Cmd::GetWhoami)
        .expect("worker accepts GetWhoami");
    match event_rx.recv_blocking() {
        Ok(Event::Whoami(Ok(s))) => println!("xas: whoami ok: {s}"),
        Ok(Event::Whoami(Err(e))) => println!("xas: whoami err (expected): {e}"),
        Ok(other) => panic!("expected Whoami, got {other:?}"),
        Err(e) => panic!("event channel closed before Whoami: {e}"),
    }

    // 3. Clean shutdown — worker drains the cmd channel, emits a
    //    farewell event, and the thread joins.
    cmd_tx
        .send_blocking(Cmd::Shutdown)
        .expect("worker accepts Shutdown");
    match event_rx.recv_blocking() {
        Ok(Event::ShuttingDown) => println!("xas: worker shut down"),
        Ok(other) => panic!("expected ShuttingDown, got {other:?}"),
        Err(e) => panic!("event channel closed before ShuttingDown: {e}"),
    }
    drop(cmd_tx);

    worker.join().expect("worker thread joined cleanly");
    println!("xas: exiting");
}
