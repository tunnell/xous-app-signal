//! Worker-thread harness for the `presage::Manager` state machine.
//!
//! The manager runs on a dedicated Xous worker thread driven by a
//! `smol-rs` `LocalExecutor`. The worker takes a `PddbStore` and an
//! async-channel pair, runs the executor, dispatches `Cmd` values,
//! and emits `Event` replies. The `HttpClient` thread-locals are set
//! inside the worker thread, and the actual `Manager` operations
//! bolt onto this surface.
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

/// Initial worker-thread stack size.
///
/// Sized at 2 MiB. History:
///
/// - Original value: 4 MiB. Set by the xas authors as "comfortable
///   headroom for zkgroup batch ops + smol's recursive task graph."
/// - Dropped to 1 MiB in 2026-05-08 because the 4 MiB reservation
///   was eating 4 MiB of RAM (Xous commits stack pages eagerly via
///   `map_memory`), which combined with large per-process state to
///   push xas over the kernel's then-default 512 KiB heap cap.
/// - Restored to 2 MiB shortly after when hosted-mode emulation
///   showed `thread 'signal-worker' has overflowed its stack`
///   during `PUT /v1/accounts/attributes` on post-link auto-reload.
///   That path runs zkgroup credential batch + serde JSON build +
///   rustls TLS write + tungstenite WS framing, deep enough to
///   blow 1 MiB. The kernel-level `big-heap` feature now in use
///   makes the heap-pressure rationale for the 1 MiB cut obsolete.
///
/// 2 MiB is the empirical floor for the link path; if a future
/// flow stage stack-overflows again, bump to 4 MiB.
const WORKER_STACK_BYTES: usize = 2 * 1024 * 1024;

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
    // pinned to this thread for life. Same pattern as the smoke
    // test established.
    let executor: &'static LocalExecutor<'static> = Box::leak(Box::new(LocalExecutor::new()));

    // Register two thread-locals that libsignal-service-rs expects on
    // every thread that touches its API. Both are set here because
    // the worker is the only thread that ever does.
    //
    // 1. `HttpClient` — the sync HTTP/1.1 + WebSocket transport we
    //    forked. Cloning is cheap (Arc-shaped).
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
    // presage's `receive_messages` and other long-running Manager
    // ops spawn their own background tasks via
    // `presage::runtime::spawn_detached`. That goes through a separate
    // thread-local from libsignal-service's `task_spawner` and panics
    // if not configured. Wire to the same LocalExecutor.
    presage::set_executor(executor);

    block_on(executor.run(async move {
        tracing::debug!("signal-worker: ready");

        // When `LinkDevice` succeeds the resulting
        // `Manager<S, Registered>` is retained here so subsequent
        // Cmds (StartReceive, SendMessage) reuse the same
        // session state instead of re-running `load_registered`.
        // `None` means we haven't linked (or failed to).
        let mut linked: Option<Manager<PddbStore, Registered>> = None;

        // Cached identity fields. Populated whenever the bridge sees
        // a successful Manager (load_registered or link_secondary_device);
        // served back via Cmd::GetAccountInfo for the UI Profile screen
        // on cold-start where Event::LinkComplete may have fired before
        // the user navigated to Profile (or never fired because PDDB
        // wasn't unlocked within the load_registered retry budget).
        let mut cached_account_info: Option<crate::cmd::AccountInfoData> = None;

        // Auto-load existing registration so the user doesn't re-link
        // on every boot. PDDB may not be mounted at worker spawn time
        // (mount fires lazily on first IPC, racing with us); retry on
        // transient errors until the store either returns a Manager or
        // a definitive NotYetRegistered.
        log::info!("bridge: attempting load_registered from PDDB");
        let mut linked_attempts = 0;
        loop {
            match Manager::load_registered(store.clone()).await {
                Ok(manager) => {
                    let data = manager.registration_data();
                    let device_name = data.device_name.clone().unwrap_or_default();
                    let aci = data.service_ids.aci.to_string();
                    let phone = data.phone_number.to_string();
                    log::info!(
                        "bridge: load_registered OK — device={} aci={} phone={}",
                        device_name, aci, phone
                    );
                    cached_account_info = Some(crate::cmd::AccountInfoData {
                        device_name: device_name.clone(),
                        aci: aci.clone(),
                        phone: phone.clone(),
                    });
                    let _ = event_tx
                        .send(Event::LinkComplete { device_name, aci, phone })
                        .await;
                    linked = Some(manager);
                    break;
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    if msg.contains("not yet registered") || msg.contains("NotYetRegistered") {
                        log::info!("bridge: load_registered: not registered yet (first boot)");
                        break;
                    }
                    linked_attempts += 1;
                    if linked_attempts >= 10 {
                        log::warn!("bridge: load_registered gave up after 10 retries: {}", e);
                        break;
                    }
                    log::info!(
                        "bridge: load_registered transient err (attempt {}/10): {}",
                        linked_attempts, e
                    );
                    futures_lite::future::yield_now().await;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }

        // Handle to the per-receive "manager task"'s
        // internal send channel. `Some(tx)` means the manager task
        // is running and `Cmd::SendMessage` should be forwarded;
        // `None` means receive isn't started yet (or the task died).
        let mut send_to_manager: Option<Sender<InnerSend>> = None;

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
                            // Manager so subsequent Cmds can use it.
                            // Cache the account info too so
                            // Cmd::GetAccountInfo can serve it after
                            // the manager is moved into manager_task.
                            let data = manager.registration_data();
                            cached_account_info = Some(crate::cmd::AccountInfoData {
                                device_name: data.device_name.clone().unwrap_or_default(),
                                aci: data.service_ids.aci.to_string(),
                                phone: data.phone_number.to_string(),
                            });
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
                    // No-op. presage's link_secondary_device
                    // doesn't expose a cancel handle; the in-flight task
                    // runs to completion or HTTP timeout. The UI side
                    // already navigates away on Cancel; a stale
                    // LinkComplete/LinkError eventually arriving is
                    // ignored by the UI (it's no longer on a Link*
                    // screen).
                    tracing::debug!("LinkCancel received; in-flight link runs to completion");
                }
                Ok(Cmd::GetAccountInfo) => {
                    log::info!("bridge: Cmd::GetAccountInfo received");
                    // Try the cache first (fastest path: this is
                    // populated after any successful load_registered
                    // or link_secondary_device).
                    let outcome = if let Some(info) = cached_account_info.as_ref() {
                        log::info!("bridge: GetAccountInfo — serving from cache");
                        Ok(info.clone())
                    } else {
                        // No cache. Try a fresh load_registered —
                        // PDDB may have unlocked since the worker's
                        // startup retry budget expired.
                        log::info!("bridge: GetAccountInfo — cache miss, retrying load_registered");
                        match Manager::load_registered(store.clone()).await {
                            Ok(manager) => {
                                let data = manager.registration_data();
                                let info = crate::cmd::AccountInfoData {
                                    device_name: data.device_name.clone().unwrap_or_default(),
                                    aci: data.service_ids.aci.to_string(),
                                    phone: data.phone_number.to_string(),
                                };
                                cached_account_info = Some(info.clone());
                                // If we don't already have a manager
                                // (e.g., load failed earlier), keep
                                // this one — saves a re-load later.
                                if linked.is_none() && send_to_manager.is_none() {
                                    linked = Some(manager);
                                }
                                Ok(info)
                            }
                            Err(e) => {
                                let msg = format!("{}", e);
                                log::warn!("bridge: GetAccountInfo — load_registered err: {}", msg);
                                Err(msg)
                            }
                        }
                    };
                    if event_tx.send(Event::AccountInfo(outcome)).await.is_err() {
                        tracing::warn!("event channel dropped while sending AccountInfo");
                        break;
                    }
                }
                Ok(Cmd::StartReceive) => {
                    log::info!("bridge: Cmd::StartReceive received");
                    if send_to_manager.is_some() {
                        log::info!("bridge: StartReceive — already running, idempotent drop");
                        continue;
                    }
                    let Some(manager) = linked.take() else {
                        log::warn!("bridge: StartReceive — not linked");
                        let _ = event_tx
                            .send(Event::ReceiveError(
                                "not linked yet — send Cmd::LinkDevice first".to_string(),
                            ))
                            .await;
                        continue;
                    };
                    log::info!("bridge: StartReceive — spawning manager_task");
                    // Spawn a single "manager task" that owns the
                    // Manager for life and multiplexes receive-stream
                    // items with inbound send requests via an internal
                    // channel.
                    let (inner_tx, inner_rx) = async_channel::bounded::<InnerSend>(8);
                    send_to_manager = Some(inner_tx);
                    let store_for_flush = store.clone();
                    let event_tx_for_recv = event_tx.clone();
                    executor
                        .spawn(manager_task(
                            manager,
                            store_for_flush,
                            event_tx_for_recv,
                            inner_rx,
                        ))
                        .detach();
                }
                Ok(Cmd::SendMessage { recipient, body, timestamp }) => {
                    let Some(send_tx) = send_to_manager.as_ref() else {
                        let _ = event_tx
                            .send(Event::SendError {
                                reason: "not receiving; send Cmd::StartReceive first"
                                    .to_string(),
                                timestamp: Some(timestamp),
                            })
                            .await;
                        continue;
                    };
                    // Forward to the manager task. If the channel is
                    // closed (manager task exited) we drop the
                    // send_to_manager handle and surface the error.
                    if send_tx.send(InnerSend { recipient, body, timestamp }).await.is_err() {
                        send_to_manager = None;
                        let _ = event_tx
                            .send(Event::SendError {
                                reason: "manager task died".to_string(),
                                timestamp: Some(timestamp),
                            })
                            .await;
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

        // `linked` drops here. Pre-Stage-11 we may want a graceful
        // Manager teardown (close WS, flush sessions); for now drop
        // semantics are sufficient.
        drop(linked);
    }));
}

/// Run `Manager::link_secondary_device`. Forwards the
/// provisioning URL to the UI as `Event::LinkUrl(...)` as soon as it
/// arrives, then awaits the user-confirmation step. Returns the linked
/// `Manager` on success (worker keeps it for subsequent Cmds) or `Err`
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

    log::info!("bridge/link: begin (device_name={:?})", device_name);

    let (url_tx, url_rx) = oneshot::channel::<url::Url>();
    let event_tx_for_url = event_tx.clone();

    let forwarder = async move {
        match url_rx.await {
            Ok(url) => {
                log::info!("bridge/link: URL received from libsignal: {}", url);
                if let Err(e) = event_tx_for_url.send(Event::LinkUrl(url.to_string())).await {
                    log::warn!("bridge/link: event_tx send LinkUrl failed: {:?}", e);
                }
            }
            Err(e) => {
                log::warn!("bridge/link: url_rx closed before URL: {:?}", e);
            }
        }
    };

    log::info!("bridge/link: calling Manager::link_secondary_device");
    let (link_result, _) = future::join(
        Manager::link_secondary_device(store, SignalServers::Production, device_name, url_tx),
        forwarder,
    )
    .await;
    log::info!("bridge/link: link_secondary_device returned");

    match link_result {
        Ok(mut manager) => {
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
            // SKIP request_contacts on link.
            //
            // Previously: this called manager.request_contacts() to
            // populate the ContentsStore with (uuid, phone_number,
            // name) entries from the linked phone, so xas could send
            // to e164 phone numbers. The call did multiple WS
            // roundtrips (establish self-session, fetch own prekey
            // bundle, send self-targeted contact-sync request) and
            // BLOCKED the worker for 30-90s on rv32 — preventing
            // Cmd::StartReceive from being processed in that window
            // and racing the Signal-server WS-rotation problem that
            // intermittently kills user sends.
            //
            // For now, dropping the call entirely. xas can already
            // send to UUIDs (the working path), and the explicit
            // F2 "Sync" UI item (currently a stub) is the
            // user-triggered way to populate contacts on demand.
            // Phone-number recipient resolution stays broken until
            // that lands — an acceptable trade for unblocking
            // send entirely.
            log::info!("bridge/link: skipping request_contacts (deferred to user-triggered Sync)");
            Ok(manager)
        }
        Err(e) => {
            let _ = event_tx.send(Event::LinkError(format!("{e}"))).await;
            Err(())
        }
    }
}

/// Inner-channel payload from the worker dispatcher to the
/// `manager_task`. `Cmd::SendMessage` decomposes into this struct
/// before being forwarded.
struct InnerSend {
    recipient: String,
    body: String,
    /// Client-generated unix-ms timestamp; carried through to
    /// `manager.send_message` and echoed back in `Event::Send*`
    /// so the UI can correlate the optimistic-render row with
    /// the eventual outcome event.
    timestamp: u64,
}

/// The long-running task that owns the linked Manager
/// and multiplexes the receive stream with inbound send requests.
///
/// presage's API forces a difficult shape: both `receive_messages`
/// and `send_message` take `&mut self`, and `receive_messages`
/// returns a stream that holds the `&mut self` borrow for its
/// lifetime. We can't simultaneously poll the stream and call
/// `send_message`. The workaround: open the stream, `futures::select`
/// between stream items and the inner send-cmd channel; when a send
/// arrives, drop the stream (release the borrow), call
/// `send_message`, then re-open the stream and loop. Re-opening the
/// stream means presage re-establishes the WS read and replays
/// pending Signal-server-side messages from the next batch — no
/// data loss, just a brief reconnect cost.
async fn manager_task(
    mut manager: Manager<PddbStore, Registered>,
    store: PddbStore,
    event_tx: Sender<Event>,
    send_rx: Receiver<InnerSend>,
) {
    use futures::FutureExt;
    use futures::StreamExt;
    use futures::select;

    // Why this exits the inner pump loop. The pump runs inside a
    // single `loop { select! { ... } }`; we need to surface to the
    // outer 'outer loop both "user sent something, drop the stream
    // and call handle_send" and "stream died, drop everything and
    // re-open with backoff" cases without using `continue 'outer`
    // from inside the select! macro (which works syntactically but
    // is awkward to read at this level of nesting).
    enum InnerExit {
        Send(InnerSend),
        Reopen { reason: String },
        Shutdown,
    }

    // Tell the UI we're listening (only on the first stream open).
    let mut announced = false;

    // Consecutive failures since the last successful event. Used to
    // (a) decide backoff before re-opening the stream and
    // (b) deduplicate the "receive error" event so the UI sees one
    // banner per outage, not one per retry. Reset to 0 after a
    // successful `manager.receive_messages()` AND after we observe
    // an actual stream item — opening Ok then immediately seeing
    // None counts as a failure (we made no real progress).
    let mut consecutive_failures: u32 = 0;
    // Cap on how many consecutive failures we tolerate before giving
    // up entirely. With backoff capped at 30s, this is roughly an
    // 8-minute window of constant retries before we surface a fatal
    // ReceiveError and exit. Empirically, the rv32 net stack
    // recovers from idle-WS death in well under a minute, so any
    // failure that persists past this is something we genuinely
    // can't paper over.
    const MAX_CONSECUTIVE_FAILURES: u32 = 20;

    log::info!("bridge: manager_task entered");

    'outer: loop {
        // Backoff before reopening if this isn't the first attempt.
        // 1s * 1.5^n capped at 30s. Sleep happens BEFORE the open
        // attempt so a failed open also gets the delay without
        // double-sleeping.
        if consecutive_failures > 0 {
            // 1500 = 1s in ms with 1.5^0; the f64 path is fine in a
            // backoff context — we don't need millisecond accuracy.
            let backoff_ms =
                (1000.0 * 1.5_f64.powi(consecutive_failures as i32 - 1)).min(30_000.0) as u64;
            log::info!(
                "bridge: manager_task — backoff {}ms before re-open (consecutive_failures={})",
                backoff_ms, consecutive_failures,
            );
            futures_timer::Delay::new(std::time::Duration::from_millis(backoff_ms)).await;
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                log::error!(
                    "bridge: manager_task — gave up after {} consecutive failures",
                    consecutive_failures,
                );
                let _ = event_tx
                    .send(Event::ReceiveError(format!(
                        "receive failed {} times; restart xas to retry",
                        consecutive_failures,
                    )))
                    .await;
                return;
            }
        }

        // Open the stream for this iteration.
        log::info!("bridge: manager_task — opening receive_messages stream");
        let mut stream = match manager.receive_messages().await {
            Ok(s) => {
                log::info!("bridge: manager_task — receive_messages OK");
                Box::pin(s)
            }
            Err(e) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                log::warn!(
                    "bridge: manager_task — receive_messages err (failure #{}): {}",
                    consecutive_failures, e,
                );
                // Surface only the first error in a streak so the UI
                // shows one banner per outage, not N.
                if consecutive_failures == 1 {
                    let _ = event_tx
                        .send(Event::ReceiveError(format!("receive_messages: {e}")))
                        .await;
                }
                continue 'outer;
            }
        };

        if !announced {
            log::info!("bridge: manager_task — sending ReceiveStarted");
            if event_tx.send(Event::ReceiveStarted).await.is_err() {
                return;
            }
            announced = true;
        }

        // Pump items + send-cmds.
        let exit: InnerExit = loop {
            select! {
                item = stream.next().fuse() => {
                    match item {
                        Some(item) => {
                            log::info!("bridge: stream item received");
                            // Real progress on this stream — clear
                            // the failure counter so the next reopen
                            // (when handle_send forces one) doesn't
                            // sleep at all.
                            consecutive_failures = 0;
                            if !process_received(item, &store, &event_tx).await {
                                // event_tx closed — bail.
                                return;
                            }
                        }
                        None => {
                            // Stream ended — typically because the
                            // identified WS died (rv32: keepalive
                            // responses not coming back, libsignal
                            // closes after 2 outstanding). Don't
                            // exit the task — re-open after backoff.
                            break InnerExit::Reopen {
                                reason: "stream ended".to_string(),
                            };
                        }
                    }
                }
                send = send_rx.recv().fuse() => {
                    match send {
                        Ok(s) => break InnerExit::Send(s),
                        Err(_) => {
                            // Sender side dropped — worker dispatcher
                            // ended; shutting down.
                            break InnerExit::Shutdown;
                        }
                    }
                }
            }
        };

        // Drop the stream so the &mut borrow on `manager` is
        // released before we call anything else on `manager`.
        log::info!("bridge: manager_task — dropping receive stream to free &mut manager borrow");
        drop(stream);

        match exit {
            InnerExit::Send(send) => {
                log::info!("bridge: manager_task — invoking handle_send");
                handle_send(&mut manager, send, &event_tx).await;
                log::info!("bridge: manager_task — handle_send returned, re-opening stream");
                // Keep consecutive_failures as is. If handle_send
                // failed because the WS was already dying, the
                // upcoming receive_messages() call may also fail —
                // and we want backoff to kick in then.
            }
            InnerExit::Reopen { reason } => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                log::warn!(
                    "bridge: manager_task — stream closed (failure #{}, reason={}), re-opening with backoff",
                    consecutive_failures, reason,
                );
                if consecutive_failures == 1 {
                    let _ = event_tx
                        .send(Event::ReceiveError(format!(
                            "receive {} (auto-retrying)",
                            reason
                        )))
                        .await;
                }
            }
            InnerExit::Shutdown => break 'outer,
        }
        // Loop back: re-open the stream and continue.
    }
    log::info!("bridge: manager_task exiting (outer break)");
}

/// Process one item from `Manager::receive_messages`. Returns
/// `false` if the event channel is closed (caller should exit);
/// `true` to keep going.
async fn process_received(
    item: presage::model::messages::Received,
    store: &PddbStore,
    event_tx: &Sender<Event>,
) -> bool {
    use presage::libsignal_service::content::ContentBody;
    use presage::model::messages::Received;

    log::info!("bridge: process_received variant={}", match &item {
        Received::Content(_) => "Content",
        Received::QueueEmpty => "QueueEmpty",
        Received::Contacts => "Contacts",
    });
    match item {
        Received::Content(content) => {
            log::info!(
                "bridge: process_received Content body_kind={}",
                match &content.body {
                    presage::libsignal_service::content::ContentBody::NullMessage(_) => "NullMessage",
                    presage::libsignal_service::content::ContentBody::DataMessage(_) => "DataMessage",
                    presage::libsignal_service::content::ContentBody::SynchronizeMessage(_) => "SynchronizeMessage",
                    presage::libsignal_service::content::ContentBody::CallMessage(_) => "CallMessage",
                    presage::libsignal_service::content::ContentBody::ReceiptMessage(_) => "ReceiptMessage",
                    presage::libsignal_service::content::ContentBody::TypingMessage(_) => "TypingMessage",
                    presage::libsignal_service::content::ContentBody::DecryptionErrorMessage(_) => "DecryptionErrorMessage",
                    presage::libsignal_service::content::ContentBody::StoryMessage(_) => "StoryMessage",
                    presage::libsignal_service::content::ContentBody::PniSignatureMessage(_) => "PniSignatureMessage",
                    presage::libsignal_service::content::ContentBody::EditMessage(_) => "EditMessage",
                }
            );
            // Only surface text-bearing DataMessages for MVP.
            // Sync/receipt/typing messages have already been
            // absorbed by the store machinery presage runs
            // internally; they don't need UI display.
            let body = match &content.body {
                ContentBody::DataMessage(dm) => dm.body.clone().unwrap_or_default(),
                ContentBody::SynchronizeMessage(sm) => sm
                    .sent
                    .as_ref()
                    .and_then(|s| s.message.as_ref())
                    .and_then(|dm| dm.body.clone())
                    .unwrap_or_default(),
                _ => return true, // EditMessage / Receipt / Typing / Call — skip
            };
            if body.is_empty() {
                return true; // attachment- or reaction-only
            }
            let sender_sid = content.metadata.sender.clone();
            let sender = sender_sid.service_id_string();
            let timestamp = content.metadata.timestamp;

            // Resolve display info from the contacts store. Empty if the
            // sender isn't yet known (peer hasn't been synced from the
            // linked phone). UI falls back to the UUID.
            use presage::libsignal_service::prelude::phonenumber::Mode;
            use presage::store::ContentsStore;
            let (sender_phone, sender_name) = match store.contact_by_id(&sender_sid).await {
                Ok(Some(c)) => {
                    let phone = c
                        .phone_number
                        .as_ref()
                        .map(|pn| pn.format().mode(Mode::E164).to_string());
                    let name = if c.name.is_empty() { None } else { Some(c.name) };
                    (phone, name)
                }
                _ => (None, None),
            };

            event_tx
                .send(Event::Message {
                    sender,
                    sender_phone,
                    sender_name,
                    body,
                    timestamp,
                })
                .await
                .is_ok()
        }
        Received::QueueEmpty => {
            // Flush dirty sessions in batched chunks at
            // quiescence. Errors are non-fatal — next QueueEmpty
            // retries.
            if let Err(e) = store.flush_sessions() {
                tracing::warn!("flush_sessions on QueueEmpty failed: {e}");
            }
            true
        }
        Received::Contacts => {
            tracing::debug!("contact-sync batch absorbed by store");
            true
        }
    }
}

/// Parse the recipient UUID, build a text-only DataMessage
/// at the current wall-clock timestamp, and call
/// `Manager::send_message`. Always emits exactly one event:
/// `SendComplete{timestamp}` on success, `SendError(reason)` on
/// failure.
async fn handle_send(
    manager: &mut Manager<PddbStore, Registered>,
    send: InnerSend,
    event_tx: &Sender<Event>,
) {
    use futures::FutureExt;
    use presage::libsignal_service::content::ContentBody;
    use presage::libsignal_service::prelude::Uuid;
    use presage::libsignal_service::proto::DataMessage;
    use presage::libsignal_service::protocol::{Aci, ServiceId};
    use presage::store::ContentsStore;

    let timestamp = send.timestamp;
    log::info!(
        "bridge/send: handle_send entered; recipient_raw={:?} body_len={} ts={}",
        send.recipient,
        send.body.len(),
        timestamp,
    );

    // Recipient may be either a UUID/ACI (36 chars, dashed) or a
    // phone number in e164 form (`+<digits>`). UUIDs go straight through;
    // phone numbers are looked up against the contacts store, which is
    // populated by phone-sourced sync messages (and `presage` auto-
    // saves contacts on first received message).
    let recipient = if let Ok(uuid) = Uuid::parse_str(send.recipient.trim()) {
        log::info!("bridge/send: recipient parsed as UUID={}", uuid);
        ServiceId::Aci(Aci::from_uuid_bytes(uuid.into_bytes()))
    } else if send.recipient.trim().starts_with('+') {
        log::info!("bridge/send: recipient is e164; consulting contacts");
        let target = send.recipient.trim();
        let Ok(contacts_iter) = manager.store().contacts().await else {
            log::warn!("bridge/send: contacts() returned Err");
            let _ = event_tx
                .send(Event::SendError {
                    reason: "couldn't read contacts store".to_string(),
                    timestamp: Some(timestamp),
                })
                .await;
            return;
        };
        use presage::libsignal_service::prelude::phonenumber::Mode;
        let mut matched: Option<Uuid> = None;
        for contact_res in contacts_iter {
            let Ok(contact) = contact_res else { continue };
            let Some(pn) = contact.phone_number.as_ref() else { continue };
            let pn_str = pn.format().mode(Mode::E164).to_string();
            if pn_str == target {
                matched = Some(contact.uuid);
                break;
            }
        }
        let Some(uuid) = matched else {
            log::warn!("bridge/send: no contact matched e164={}", target);
            let _ = event_tx
                .send(Event::SendError {
                    reason: format!(
                        "phone {} not in contacts (receive a message from them first, or use their ACI UUID)",
                        target
                    ),
                    timestamp: Some(timestamp),
                })
                .await;
            return;
        };
        log::info!("bridge/send: e164 resolved -> uuid={}", uuid);
        ServiceId::Aci(Aci::from_uuid_bytes(uuid.into_bytes()))
    } else {
        log::warn!("bridge/send: recipient parse failed: {:?}", send.recipient);
        let _ = event_tx
            .send(Event::SendError {
                reason: format!(
                    "recipient must be ACI UUID or +e164 phone number; got {:?}",
                    send.recipient
                ),
                timestamp: Some(timestamp),
            })
            .await;
        return;
    };

    let content_body = ContentBody::DataMessage(DataMessage {
        body: Some(send.body),
        timestamp: Some(timestamp),
        ..Default::default()
    });

    log::info!(
        "bridge/send: calling manager.send_message ts={} (heavy path on first-send: prekey bundle fetch + PQXDH + double-ratchet init + PDDB write)",
        timestamp
    );

    // Retry-on-WsClosing: on rv32 the auth WS dies after ~60-90s
    // when libsignal-service-rs's keepalive responses don't make it
    // back from the server. libsignal-service spawns a fresh WS
    // automatically (we see `ka_count=1` for replacements in UART)
    // but the in-flight send is dropped. A brief sleep + retry lets
    // the next attempt land on the new WS instead of bubbling
    // "WebSocket closing" up to the user.
    //
    // Detection is by string match because the error type passes
    // through several wrapping layers (presage::Error ->
    // ServiceError -> SignalProtocolError) before reaching us.
    // The substring "websocket closing" appears in both surfaced
    // shapes ("WebSocket closing while sending request" and "...
    // while waiting for a response").
    // Signal's edge servers close WSes aggressively on rv32
    // (multiple `code=1001 "Connection Idle Timeout"` events
    // within seconds of each other). A 3-retry × 2s window wasn't
    // enough to reliably land on a WS that survived a full
    // request-response. 6 retries with exponential gaps span ~62s
    // total, covering ~10 server-rotation cycles statistically.
    // Until the transport is reworked to open a fresh WS per send,
    // this aggressive retry loop is the workaround.
    const SEND_MAX_ATTEMPTS: u32 = 6;

    /// Sleep before attempt N (zero-indexed in the call below):
    /// attempt 2 -> 2s, attempt 3 -> 4s, 4 -> 8s, 5 -> 16s, 6 -> 32s.
    /// Total worst-case wait between user-press and giving up: 62s.
    fn backoff_for(next_attempt: u32) -> std::time::Duration {
        let secs = 1_u64 << (next_attempt as u64).min(5);
        std::time::Duration::from_secs(secs)
    }

    let mut last_err: Option<String> = None;
    for attempt in 1..=SEND_MAX_ATTEMPTS {
        log::info!(
            "bridge/send: attempt {}/{} ts={}",
            attempt, SEND_MAX_ATTEMPTS, timestamp,
        );

        // catch_unwind so a panic inside libsignal/presage's send path
        // doesn't kill manager_task. AssertUnwindSafe is needed because
        // `&mut manager` is not UnwindSafe by default; we accept the risk
        // — a panic mid-send may leave the Manager's session state
        // inconsistent, but the alternative (manager_task dies, every
        // subsequent send returns "manager task died") is worse. The
        // surfaced panic message goes to the UI as a normal SendError.
        let send_fut = std::panic::AssertUnwindSafe(
            manager.send_message(recipient.clone(), content_body.clone(), timestamp),
        );
        let outcome = send_fut.catch_unwind().await;
        log::info!("bridge/send: attempt {} returned", attempt);

        match outcome {
            Ok(Ok(())) => {
                log::info!("bridge/send: SendComplete ts={} (attempt {})", timestamp, attempt);
                let _ = event_tx.send(Event::SendComplete { timestamp }).await;
                return;
            }
            Ok(Err(e)) => {
                let msg = format!("{e}");
                log::warn!("bridge/send: attempt {} Err: {}", attempt, msg);
                let retryable = msg.to_lowercase().contains("websocket closing");
                if retryable && attempt < SEND_MAX_ATTEMPTS {
                    let delay = backoff_for(attempt);
                    log::info!(
                        "bridge/send: WsClosing-shaped error; sleeping {:?} then retrying (attempt {}->{} of {})",
                        delay, attempt, attempt + 1, SEND_MAX_ATTEMPTS,
                    );
                    futures_timer::Delay::new(delay).await;
                    last_err = Some(msg);
                    continue;
                }
                let _ = event_tx
                    .send(Event::SendError {
                        reason: msg,
                        timestamp: Some(timestamp),
                    })
                    .await;
                return;
            }
            Err(panic_payload) => {
                let msg = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic payload".to_string()
                };
                log::error!(
                    "bridge/send: PANIC inside manager.send_message attempt {}: {}",
                    attempt, msg,
                );
                let _ = event_tx
                    .send(Event::SendError {
                        reason: format!("panic in send: {msg}"),
                        timestamp: Some(timestamp),
                    })
                    .await;
                return;
            }
        }
    }

    // Loop ran out of attempts without an early return.
    let reason = last_err.unwrap_or_else(|| "send retry loop exhausted".to_string());
    log::warn!("bridge/send: all {} attempts failed; last={}", SEND_MAX_ATTEMPTS, reason);
    let _ = event_tx
        .send(Event::SendError {
            reason: format!("retried {} times: {}", SEND_MAX_ATTEMPTS, reason),
            timestamp: Some(timestamp),
        })
        .await;
}

/// Run `Manager::load_registered` and stringify the result. On a
/// fresh install this always sees `Err(Error::NotYetRegisteredError)`
/// here because the store starts empty — that's the path we want to
/// exercise (the channel round-trip; that the error type round-trips
/// cleanly; that the executor doesn't deadlock when a future returns
/// an error).
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
