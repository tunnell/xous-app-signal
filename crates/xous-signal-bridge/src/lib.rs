//! Worker-thread harness for the `presage::Manager` state machine.
//!
//! Per `docs/REPORT.md` Decision 2 the manager runs on a dedicated
//! Xous worker thread driven by a `smol-rs` `LocalExecutor`. Stage 8
//! stands up the harness without yet wiring real network transport:
//! the worker takes a `PddbStore` and an async-channel pair, runs the
//! executor, dispatches `Cmd` values, and emits `Event` replies. The
//! Stage 6 `HttpClient` thread-locals (set inside the worker thread
//! at Stage 9+) and the actual `Manager` operations bolt on later
//! without changing this surface.
//!
//! Why a *local* executor rather than a multi-thread one: presage's
//! traits use `#[async_trait(?Send)]` (per `libsignal/protocol/src/
//! storage/traits.rs:48`), and many of our store-cache pointers are
//! `!Send` once the real PDDB backend lands. A `LocalExecutor` keeps
//! all spawned tasks on this single thread, which is also what
//! Whisperfish-Qt does on the linux build.

mod cmd;

pub use cmd::{Cmd, Event};

use std::thread::{self, JoinHandle};

use async_channel::{Receiver, Sender};
use async_executor::LocalExecutor;
use futures_lite::future::block_on;
use presage::Manager;
use presage_store_pddb::PddbStore;

/// Initial worker-thread stack size. Comfortable headroom for
/// zkgroup batch ops (the heaviest compute presage triggers) and
/// the smol-rs executor's recursive task graph. Stage 8 doesn't
/// exercise zkgroup; bump this if a real-flow stage finds it short.
const WORKER_STACK_BYTES: usize = 4 * 1024 * 1024;

/// Spawn the Manager worker thread.
///
/// Returns a `JoinHandle` for the worker thread itself. The worker
/// terminates when either:
///
/// - the `cmd_rx` channel returns `Err(RecvError)` (sender dropped), or
/// - it processes a `Cmd::Shutdown` and emits `Event::ShuttingDown`.
///
/// `event_tx` is held across `await` points so the executor parks on
/// I/O rather than busy-waiting; cloning is cheap (it's an
/// `Arc`-shaped channel handle).
///
/// The worker never panics on a `event_tx.send` failure — if the main
/// thread has dropped its receiver, the right thing is to exit
/// cleanly rather than abort the executor.
pub fn run_signal_worker(
    store: PddbStore,
    cmd_rx: Receiver<Cmd>,
    event_tx: Sender<Event>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("signal-worker".into())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || worker_main(store, cmd_rx, event_tx))
        .expect("spawn signal-worker thread")
}

fn worker_main(store: PddbStore, cmd_rx: Receiver<Cmd>, event_tx: Sender<Event>) {
    // Box::leak the executor so async tasks spawned on it can borrow
    // it for `'static`. `LocalExecutor` itself is `!Send` so it stays
    // pinned to this thread for life. Same pattern Stage 1's smoke
    // test established.
    let executor: &'static LocalExecutor<'static> = Box::leak(Box::new(LocalExecutor::new()));

    block_on(executor.run(async move {
        tracing::debug!("signal-worker: ready");
        loop {
            match cmd_rx.recv().await {
                Ok(Cmd::Hello) => {
                    if event_tx.send(Event::Pong).await.is_err() {
                        tracing::warn!("event channel dropped while sending Pong; shutting down");
                        break;
                    }
                }
                Ok(Cmd::GetWhoami) => {
                    let outcome = handle_whoami(store.clone()).await;
                    if event_tx.send(Event::Whoami(outcome)).await.is_err() {
                        tracing::warn!("event channel dropped while sending Whoami; shutting down");
                        break;
                    }
                }
                Ok(Cmd::Shutdown) => {
                    let _ = event_tx.send(Event::ShuttingDown).await;
                    break;
                }
                Err(_) => {
                    // Sender dropped. Treat as implicit shutdown — no
                    // farewell event since nobody's listening anyway.
                    tracing::debug!("signal-worker: cmd channel closed, exiting");
                    break;
                }
            }
        }
    }));
}

/// Run `Manager::load_registered` and stringify the result. Stage 8
/// always sees `Err(Error::NotYetRegisteredError)` here because the
/// store starts empty — that's the path we want to exercise (the
/// channel round-trip; that the error type round-trips cleanly; that
/// the executor doesn't deadlock when a future returns an error).
///
/// `Manager::load_registered` takes `S: Store` by value and returns a
/// `Manager<S, Registered>` if registration data exists. We only need
/// the `Ok` arm to format the data we'd send; for now the
/// always-error path is what we test.
async fn handle_whoami(store: PddbStore) -> Result<String, String> {
    match Manager::load_registered(store).await {
        Ok(manager) => {
            // Future stages: this is where a real `whoami` over the
            // websocket would go. For now reach into the cached
            // registration data so the type-checker proves the store
            // round-trips through `Manager`.
            let data = manager.registration_data();
            Ok(format!(
                "phone={} aci={} pni={}",
                data.phone_number, data.service_ids.aci, data.service_ids.pni
            ))
        }
        Err(e) => Err(format!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    //! Integration-style tests: spawn the worker, exercise the
    //! channel round-trip end-to-end. We use the host-mode
    //! `PddbStore::with_mock_backend` so no Xous services are needed.

    use super::*;

    /// Channel capacity. 16 is plenty for these tests; the
    /// production app sizes this against the IPC fan-in/fan-out.
    const CHAN_CAP: usize = 16;

    fn spawn() -> (Sender<Cmd>, Receiver<Event>, JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = async_channel::bounded(CHAN_CAP);
        let (event_tx, event_rx) = async_channel::bounded(CHAN_CAP);
        let store = PddbStore::with_mock_backend();
        let handle = run_signal_worker(store, cmd_rx, event_tx);
        (cmd_tx, event_rx, handle)
    }

    #[test]
    fn hello_pong_round_trip() {
        let (cmd_tx, event_rx, handle) = spawn();
        cmd_tx.send_blocking(Cmd::Hello).unwrap();
        let evt = event_rx.recv_blocking().unwrap();
        assert!(matches!(evt, Event::Pong));

        cmd_tx.send_blocking(Cmd::Shutdown).unwrap();
        let evt = event_rx.recv_blocking().unwrap();
        assert!(matches!(evt, Event::ShuttingDown));
        handle.join().unwrap();
    }

    #[test]
    fn whoami_returns_error_on_empty_store() {
        let (cmd_tx, event_rx, handle) = spawn();
        cmd_tx.send_blocking(Cmd::GetWhoami).unwrap();
        let evt = event_rx.recv_blocking().unwrap();
        match evt {
            Event::Whoami(Err(_)) => { /* expected: not registered */ }
            other => panic!("unexpected event: {other:?}"),
        }
        cmd_tx.send_blocking(Cmd::Shutdown).unwrap();
        let _ = event_rx.recv_blocking();
        handle.join().unwrap();
    }

    /// Dropping the cmd-channel sender (without sending Shutdown) is
    /// the implicit teardown path. The worker exits when its `recv`
    /// returns `Err(RecvError)`.
    #[test]
    fn dropping_cmd_channel_shuts_down_cleanly() {
        let (cmd_tx, event_rx, handle) = spawn();
        cmd_tx.send_blocking(Cmd::Hello).unwrap();
        assert!(matches!(event_rx.recv_blocking().unwrap(), Event::Pong));
        drop(cmd_tx);
        // event_rx will return Err once the worker exits and event_tx
        // drops. Don't assert on the value — just confirm the join.
        let _ = event_rx.recv_blocking();
        handle.join().unwrap();
    }
}
