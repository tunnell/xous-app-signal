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

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use async_channel::{Receiver, Sender};
use async_executor::LocalExecutor;
use futures_lite::future::block_on;
use presage::Manager;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::transport;
use presage::manager::Registered;
use presage_store_pddb::PddbStore;
use xous_net_bridge::{SyncHttpClient, signal_production_roots};

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

    // Stage 10: register two thread-locals that libsignal-service-rs
    // expects on every thread that touches its API. Both are set
    // here because the worker is the only thread that ever does.
    //
    // 1. `HttpClient` — the sync HTTP/1.1 + WebSocket transport we
    //    forked at Stage 6. Cloning is cheap (Arc-shaped).
    // 2. `TaskSpawner` — a closure used by libsignal-service-rs's
    //    internals (`provisioning::link_device`, WS handlers) to
    //    fire-and-forget detached tasks onto our local executor.
    //
    // Without either, `Manager::link_secondary_device` panics on its
    // first internal `PushService` construction or `ws()` call.
    transport::set_http_client(Arc::new(SyncHttpClient::new(
        signal_production_roots(),
        format!("xas/{}", env!("CARGO_PKG_VERSION")),
    )));
    transport::set_task_spawner(Box::new(|task| {
        executor.spawn(task).detach();
    }));

    block_on(executor.run(async move {
        tracing::debug!("signal-worker: ready");

        // Stage 10: when `LinkDevice` succeeds the resulting
        // `Manager<S, Registered>` is retained here so subsequent
        // Stage 11+ Cmds (StartReceive, SendMessage) reuse the same
        // session state instead of re-running `load_registered`.
        // `None` means we haven't linked (or failed to).
        let mut linked: Option<Manager<PddbStore, Registered>> = None;

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
                Ok(Cmd::LinkDevice { device_name }) => {
                    let outcome =
                        handle_link_device(store.clone(), event_tx.clone(), device_name).await;
                    match outcome {
                        Ok(manager) => {
                            // We already sent Event::LinkComplete from
                            // inside handle_link_device. Retain the
                            // Manager so Stage 11+ Cmds can use it.
                            linked = Some(manager);
                        }
                        Err(()) => {
                            // handle_link_device already sent
                            // Event::LinkError. Drop any half-linked
                            // state on the floor; retry by sending
                            // another Cmd::LinkDevice.
                        }
                    }
                }
                Ok(Cmd::LinkCancel) => {
                    // No-op for Stage 10. presage's link_secondary_device
                    // doesn't expose a cancel handle; the in-flight task
                    // runs to completion or HTTP timeout. The UI side
                    // already navigates away on Cancel; a stale
                    // LinkComplete/LinkError eventually arriving is
                    // ignored by the UI (it's no longer on a Link*
                    // screen).
                    tracing::debug!("LinkCancel received; in-flight link runs to completion");
                }
                Ok(Cmd::StartReceive) => {
                    let Some(manager) = linked.take() else {
                        let _ = event_tx
                            .send(Event::ReceiveError(
                                "not linked yet — send Cmd::LinkDevice first".to_string(),
                            ))
                            .await;
                        continue;
                    };
                    // Move the Manager into a long-running receive
                    // task. We `take` it from `linked` (so other Cmds
                    // can't try to use it concurrently — Stage 11
                    // simplification; Stage 12 will refactor to
                    // share). The task lives until the stream ends
                    // or errors; afterward the worker has no Manager.
                    let store_for_flush = store.clone();
                    let event_tx_for_recv = event_tx.clone();
                    executor
                        .spawn(handle_receive(manager, store_for_flush, event_tx_for_recv))
                        .detach();
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

        // `linked` drops here. Pre-Stage-11 we may want a graceful
        // Manager teardown (close WS, flush sessions); for now drop
        // semantics are sufficient.
        drop(linked);
    }));
}

/// Stage 10: run `Manager::link_secondary_device`. Forwards the
/// provisioning URL to the UI as `Event::LinkUrl(...)` as soon as it
/// arrives, then awaits the user-confirmation step. Returns the linked
/// `Manager` on success (worker keeps it for Stage 11+ Cmds) or `Err`
/// after sending `Event::LinkError`.
///
/// We use `futures::channel::oneshot` for the URL handoff (same type
/// presage's API expects) and `futures::future::join` to drive the
/// link future and the URL forwarder concurrently. The store is
/// cloned for the call — `link_secondary_device` consumes its `S`
/// argument by clearing the registration on entry, then writing fresh
/// keys; cloning means the worker's outer `store` (still owned, used
/// for `whoami`/etc. paths) stays usable if linking fails.
async fn handle_link_device(
    store: PddbStore,
    event_tx: Sender<Event>,
    device_name: String,
) -> Result<Manager<PddbStore, Registered>, ()> {
    use futures::channel::oneshot;
    use futures::future;

    let (url_tx, url_rx) = oneshot::channel::<url::Url>();
    let event_tx_for_url = event_tx.clone();

    // Two concurrent futures:
    //  - `link_secondary_device` runs the linking flow, writes its
    //    URL to `url_tx`, and resolves with `Manager<S, Registered>`.
    //  - The forwarder awaits `url_rx`, sends `Event::LinkUrl`. Stays
    //    alive after sending; that's intentional — `future::join`
    //    requires both futures to resolve before returning.
    let forwarder = async move {
        match url_rx.await {
            Ok(url) => {
                let _ = event_tx_for_url.send(Event::LinkUrl(url.to_string())).await;
            }
            Err(_) => {
                // Sender dropped (the link future completed without
                // emitting a URL — usually an early HTTP error).
                // No URL to forward; the link future's own error will
                // come back via the outer match.
            }
        }
    };

    let (link_result, _) = future::join(
        Manager::link_secondary_device(store, SignalServers::Production, device_name, url_tx),
        forwarder,
    )
    .await;

    match link_result {
        Ok(manager) => {
            let data = manager.registration_data();
            let device_name = data.device_name.clone().unwrap_or_default();
            let aci = data.service_ids.aci.to_string();
            let phone = data.phone_number.to_string();
            let _ = event_tx
                .send(Event::LinkComplete {
                    device_name,
                    aci,
                    phone,
                })
                .await;
            Ok(manager)
        }
        Err(e) => {
            let _ = event_tx.send(Event::LinkError(format!("{e}"))).await;
            Err(())
        }
    }
}

/// Stage 11: long-running receive loop.
///
/// Owns the linked `Manager` for the lifetime of the task, calls
/// `receive_messages` to obtain a `Stream<Item = Received>`, and
/// translates each item into either an `Event::Message` (for
/// `Received::Content` carrying a `DataMessage` body) or a
/// `flush_sessions()` side-effect (for `Received::QueueEmpty`, per
/// docs/REPORT.md Decision 5). `Received::Contacts` is logged but
/// not surfaced to the UI in MVP (the store has already absorbed
/// the contact-sync results).
///
/// On terminal error the Manager is dropped and `Event::ReceiveError`
/// is sent. The worker has no way to recover from this without a
/// fresh `Cmd::LinkDevice` — that's noted in the receive-error
/// path.
async fn handle_receive(
    mut manager: Manager<PddbStore, Registered>,
    store: PddbStore,
    event_tx: Sender<Event>,
) {
    use futures::StreamExt;
    use presage::libsignal_service::content::ContentBody;
    use presage::model::messages::Received;

    let mut stream = match manager.receive_messages().await {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx
                .send(Event::ReceiveError(format!("receive_messages: {e}")))
                .await;
            return;
        }
    };

    if event_tx.send(Event::ReceiveStarted).await.is_err() {
        // UI already gone; no point continuing.
        return;
    }

    while let Some(item) = stream.next().await {
        match item {
            Received::Content(content) => {
                // Only surface text-bearing DataMessages for MVP.
                // Sync/receipt/typing messages have already been
                // absorbed by the store machinery presage runs
                // internally; they don't need UI display.
                let body = match &content.body {
                    ContentBody::DataMessage(dm) => dm.body.clone().unwrap_or_default(),
                    ContentBody::SynchronizeMessage(sm) => {
                        // Outgoing message we sent from another
                        // device, mirrored here. Display the body
                        // if present so the user sees their own
                        // sends in the conversation list.
                        sm.sent
                            .as_ref()
                            .and_then(|s| s.message.as_ref())
                            .and_then(|dm| dm.body.clone())
                            .unwrap_or_default()
                    }
                    _ => {
                        // EditMessage, ReceiptMessage, TypingMessage,
                        // CallMessage — not displayed in MVP.
                        continue;
                    }
                };
                if body.is_empty() {
                    // Attachment-only or reaction-only — skip for MVP.
                    continue;
                }
                let sender = content.metadata.sender.service_id_string();
                let timestamp = content.metadata.timestamp;
                if event_tx
                    .send(Event::Message {
                        sender,
                        body,
                        timestamp,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Received::QueueEmpty => {
                // Decision 5: flush dirty sessions in batched chunks
                // at quiescence. Errors are non-fatal — the next
                // QueueEmpty will retry; if PDDB is genuinely broken
                // the next session-store write fails too and surfaces
                // there.
                if let Err(e) = store.flush_sessions() {
                    tracing::warn!("flush_sessions on QueueEmpty failed: {e}");
                }
            }
            Received::Contacts => {
                tracing::debug!("contact-sync batch absorbed by store");
            }
        }
    }

    // Stream ended cleanly — usually means WS closed or shutdown.
    let _ = event_tx
        .send(Event::ReceiveError("receive stream ended".to_string()))
        .await;
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
