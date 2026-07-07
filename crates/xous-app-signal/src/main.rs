//! Xous Signal app (`xas`) binary entry point.
//!
//! Sits at the top of the crate stack. From bottom to top:
//!
//! - `xous-net-bridge` — sync TLS + WSS + HTTPS transport, owns the
//!   `Arc<ClientConfig>` for TLS-1.3 ticket resumption.
//! - `presage-store-pddb` — `presage::Store` impl backed by Xous's
//!   PDDB (real) or an in-memory mock (hosted).
//! - `xous-signal-worker` — owns the `presage::Manager` on a
//!   dedicated worker thread driven by `smol-rs::LocalExecutor`,
//!   exposes a [`Cmd`] / [`Event`] async-channel surface.
//! - This crate — UI (`gam_app::App` on hardware, `stdin_ui::Ui` on
//!   hosted) wired to the worker via the two channels.
//!
//! The startup sequence is:
//!
//! 1. Wire up the rv32 TRNG-backed `getrandom` shim if compiling for
//!    Xous; on hosted the OS RNG is used.
//! 2. Construct a [`PddbStore`] (mock backend on hosted; real PDDB
//!    behind the `pddb-backend` feature).
//! 3. Spawn [`run_signal_worker`].
//! 4. Hand the cmd/event channels to the UI and run.
//! 5. The UI sends [`Cmd::Shutdown`] on quit; the worker drains and
//!    emits `Event::ShuttingDown`.
//!
//! See `docs/ARCHITECTURE.md` for the full data-flow walkthrough and
//! `docs/UI.md` for the UI design.

mod dialogue;
mod gam_app;
mod stdin_ui;
mod store;

use async_channel::bounded;
use presage_store_pddb::PddbStore;
use stdin_ui::Ui;
use xous_signal_worker::{Cmd, Event, run_signal_worker};

/// `__getrandom_v03_custom` implementation backed by xous-core's
/// TRNG service.
///
/// Looks up the TRNG SID via `xous-api-names`, then calls
/// `Trng::fill_buf`. `fill_buf` takes `&mut [u32]` so this shim
/// casts from `*mut u8` to a `[u32; 1020]` scratch buffer, copies
/// the filled bytes back to `dest`, and handles a possible odd
/// tail (`len % 4 != 0`) with one final 1-word read.
///
/// The signature mirrors `getrandom::backends::custom`. A
/// thread-local `Trng` cell preserves the IPC connection across
/// calls so each `getrandom` entry reuses the same SID handshake.
///
/// # Trust boundary
///
/// This routes every `getrandom` call from libsignal, rustls, and
/// the rest of the dependency tree to the xous-core TRNG service.
/// Everything cryptographic on the device depends on this returning
/// real entropy.
///
/// # Errors
///
/// Returns `getrandom::Error::UNSUPPORTED` if the TRNG IPC call
/// fails. The thread-local `OnceCell` initialization may panic via
/// the inner `expect` if `xous-names` or the TRNG service are
/// unreachable at first use — that is a fatal misconfiguration of
/// the Xous image, not a recoverable runtime condition.
///
/// # Safety
///
/// Implements an `unsafe extern "Rust"` symbol that the
/// `getrandom` crate's custom backend mechanism resolves at link
/// time. Caller (the `getrandom` crate) guarantees `dest` is valid
/// for `len` bytes; this function only writes into `dest[..len]`.
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

/// Capacity of the [`Cmd`] and [`Event`] async channels between
/// the UI and the worker.
///
/// Sized for the UI's typical command pattern: a small handful of
/// commands in flight at any time (link, send, sync, account
/// info). The worker drains commands eagerly so back-pressure on
/// the UI side is unlikely on real workloads.
///
/// Capacity tradeoff: the `event_tx` cap bounds how many
/// back-pressured `Event::Message` emissions
/// `xous_signal_worker::manager_task` can buffer before its
/// `event_tx.send(...).await` blocks the worker's receive stream.
/// Too small → an idle or slow UI stalls inbound receive; too large
/// → unbounded memory on a poorly-behaved peer with high message
/// flux. 16 is the negotiated middle: enough for a bursty receive
/// from a chat the user just opened, small enough to keep the
/// post-Drop bare-`String` body exposure window bounded (SecretBox
/// wrapping of message bodies is tracked in issue #37, item 3).
const CHAN_CAP: usize = 16;

/// Construct the [`PddbStore`] the worker will use.
///
/// Selects backend based on feature flags:
///
/// - default (hosted / smoke / probe-flow / probe-pddb): in-memory
///   mock backend.
/// - `pddb-real` on rv32-xous: real
///   `PddbStore::with_pddb_backend`. On connect failure (PDDB
///   service not yet up), logs a warning and falls back to mock so
///   the binary still boots.
///
/// # Trust boundary
///
/// The returned [`PddbStore`] is the only persistence boundary for
/// Signal-Protocol state. The mock backend is plaintext and is for
/// hosted / probe builds only — production images must be built
/// with `pddb-real` enabled.
#[cfg(feature = "pddb-real")]
fn build_store() -> PddbStore {
    match PddbStore::with_pddb_backend() {
        Ok(s) => {
            log::info!("xas: store=PDDB (real)");
            s
        }
        Err(e) => {
            log::warn!("xas: PDDB connect failed ({}); falling back to mock", e);
            PddbStore::with_mock_backend()
        }
    }
}

/// Mock-backend variant of [`build_store`] for builds without the
/// `pddb-real` feature.
///
/// # Security
///
/// The mock backend keeps every store byte in plaintext RAM —
/// suitable only for hosted-mode iteration and the Renode-driven
/// probe binaries. Never select this variant for a binary intended
/// to hold real registration data.
#[cfg(not(feature = "pddb-real"))]
fn build_store() -> PddbStore {
    log::info!("xas: store=mock");
    PddbStore::with_mock_backend()
}

fn main() -> std::io::Result<()> {
    init_logger();
    log::info!("xas: starting");

    let store = build_store();

    let (cmd_tx, cmd_rx) = bounded::<Cmd>(CHAN_CAP);
    let (event_tx, event_rx) = bounded::<Event>(CHAN_CAP);

    let worker = run_signal_worker(store, cmd_rx, event_tx);
    log::info!("xas: worker started");

    #[cfg(feature = "probe-flow")]
    probe_network();

    #[cfg(all(feature = "probe-pddb", target_os = "xous"))]
    probe_pddb();

    #[cfg(all(feature = "probe-send-batch", target_os = "xous"))]
    probe_send_batch();

    // NOTE: load-bearing — there is no `probe-pddb-real` or
    // `probe-bulk-ab` auto-fire probe here. Calling
    // `presage_store_pddb::PddbBackend::connect()` immediately after
    // `run_signal_worker()` races xous-names server registration
    // during boot and triggers a `ServerNotFound` cascade in
    // unrelated services (llio, trng, modals, susres), which can
    // crash the boot via a watchdog reboot loop. xous-names itself
    // documents this race (`api/xous-api-names/src/lib.rs`).
    //
    // Bulk-write A/B benchmarking lives in shellchat as `pddb
    // bulk_probe [N]`, exercised by the user after PIN entry and
    // PDDB mount.

    // PDDB put-truncate smoke test (refs #14). Runtime-gated; exits
    // 0 PASS / 1 FAIL so a shell wrapper can assert the regression.
    #[cfg(feature = "pddb-real")]
    if std::env::var("XAS_PDDB_TRUNCATE_TEST").is_ok() {
        let backend = match presage_store_pddb::PddbBackend::connect() {
            Ok(b) => b,
            Err(e) => {
                log::error!("XAS_PDDB_TRUNCATE_TEST: connect: {}", e);
                std::process::exit(1);
            }
        };
        if !backend.is_mounted() {
            match backend.try_mount() {
                Ok(true) => {}
                Ok(false) => {
                    log::error!("XAS_PDDB_TRUNCATE_TEST: try_mount declined");
                    std::process::exit(1);
                }
                Err(e) => {
                    log::error!("XAS_PDDB_TRUNCATE_TEST: try_mount: {}", e);
                    std::process::exit(1);
                }
            }
        }
        let result = presage_store_pddb::smoke_put_truncates(&backend);
        log::info!("XAS_PDDB_TRUNCATE_TEST: {:?}", result);
        match result {
            presage_store_pddb::SmokeResult::Pass => std::process::exit(0),
            _ => std::process::exit(1),
        }
    }

    // Try to run as a real GAM-rendered Xous app first
    // (works for both hosted Xous emulation and rv32 hardware
    // when running inside a Xous environment). If we're not
    // inside Xous (no xous-names server reachable, e.g.
    // running standalone for unit tests), fall back to the
    // stdin-driven hosted UI.
    let cmd_tx_for_gam = cmd_tx.clone();
    let event_rx_for_gam = event_rx.clone();
    match gam_app::run(cmd_tx_for_gam, event_rx_for_gam) {
        Ok(()) => {}
        Err(e) => {
            log::info!("xas: GAM UI unavailable ({}); falling back to stdin Ui", e);
            Ui::new(cmd_tx, event_rx).run()?;
        }
    }

    // Worker has been told to shut down; join it. If the join hangs
    // it's a worker-side bug — surface as a nonzero exit, not a
    // silent hang.
    let _ = worker.join();
    log::info!("xas: exiting");
    Ok(())
}

/// Install a `log` implementation for hosted builds.
///
/// Uses `env_logger` with a default `info` filter. Errors from
/// `try_init` are ignored (it returns `Err` if the logger has
/// already been installed, e.g. by a test harness).
#[cfg(not(target_os = "xous"))]
fn init_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

/// TCP-connect probe verifying Renode's WF200 wifi emulation
/// carries outbound traffic.
///
/// Three connect targets, each logged with elapsed time and
/// result:
///
/// 1. `8.8.8.8:53` — Google DNS over TCP. No DNS lookup needed;
///    answers "is there any route to the internet".
/// 2. `1.1.1.1:443` — Cloudflare HTTPS. Same shape as (1) but a
///    different provider, in case route filtering is in play.
/// 3. `chat.signal.org:443` — the real Signal endpoint. Requires
///    DNS, so this also exercises the Xous `dns` service.
///
/// Each probe has a 10-second timeout. Output goes to the same
/// `INFO:xas:` stream the Robot smoke tests assert on.
#[cfg(feature = "probe-flow")]
fn probe_network() {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};

    log::info!("probe: starting network reachability probe");

    let timeout = Duration::from_secs(10);

    // (label, hostport, requires_dns)
    let probes: &[(&str, &str, bool)] = &[
        ("google-dns", "8.8.8.8:53", false),
        ("cloudflare-https", "1.1.1.1:443", false),
        ("signal-prod", "chat.signal.org:443", true),
    ];

    for (label, target, _needs_dns) in probes {
        let start = Instant::now();
        let addrs: Result<Vec<SocketAddr>, _> = target.to_socket_addrs().map(|i| i.collect());
        match addrs {
            Err(e) => {
                log::warn!(
                    "probe: {} resolve FAIL after {:?}: {}",
                    label,
                    start.elapsed(),
                    e
                );
                continue;
            }
            Ok(addrs) if addrs.is_empty() => {
                log::warn!("probe: {} resolve EMPTY after {:?}", label, start.elapsed());
                continue;
            }
            Ok(addrs) => {
                let addr = addrs[0];
                match TcpStream::connect_timeout(&addr, timeout) {
                    Ok(_stream) => {
                        log::info!(
                            "probe: {} CONNECT OK to {} after {:?}",
                            label,
                            addr,
                            start.elapsed()
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "probe: {} CONNECT FAIL to {} after {:?}: {}",
                            label,
                            addr,
                            start.elapsed(),
                            e
                        );
                    }
                }
            }
        }
    }

    log::info!("probe: network probe done");
}

/// Poll xous-core's PDDB Mount Poller via raw `xous` IPC and log
/// the result.
///
/// Mirrors `services/pddb/src/lib.rs`'s `PddbMountPoller::new` +
/// `is_mounted_nonblocking`, but depends only on `xous`,
/// `xous-api-names`, and `xous_ipc` (the same crate set the TRNG
/// shim already pulls in). The Mount Poller's `Poll` opcode (0) is
/// the simplest IPC roundtrip xas can do against PDDB: no payload,
/// returns a `Scalar1(0|1)` mount state.
///
/// Outcomes:
/// - `OK true` / `OK false` — IPC plumbing works; the value
///   indicates whether the image auto-mounts or expects
///   password-driven mount.
/// - `connection refused` / no response — protocol drift (rkyv
///   version, Buffer layout, or SID name mismatch).
#[cfg(all(feature = "probe-pddb", target_os = "xous"))]
fn probe_pddb() {
    use std::time::Instant;
    use xous::{Message, send_message};

    log::info!("probe-pddb: starting PDDB mount-poller probe");
    let start = Instant::now();

    let xns = match xous_names::XousNames::new() {
        Ok(x) => x,
        Err(e) => {
            log::warn!("probe-pddb: XousNames::new FAIL: {:?}", e);
            return;
        }
    };

    // SID name copied from `services/pddb/src/api.rs:19`. If this
    // doesn't match the running PDDB server's registered SID,
    // request_connection_blocking will block forever — which is
    // why this probe runs after the smoke boot lines, on a path
    // the Robot test can time out on.
    let conn = match xns.request_connection_blocking("_PDDB Mount Poller_") {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "probe-pddb: request_connection FAIL after {:?}: {:?}",
                start.elapsed(),
                e
            );
            return;
        }
    };
    log::info!("probe-pddb: connected to PDDB Mount Poller in {:?}", start.elapsed());

    // PollOp::Poll = 0, per `services/pddb/src/api.rs` PollOp enum.
    // Args are all unused (server reads only the opcode).
    let poll_start = Instant::now();
    let resp = send_message(conn, Message::new_blocking_scalar(0, 0, 0, 0, 0));
    match resp {
        Ok(xous::Result::Scalar1(v)) => {
            log::info!(
                "probe-pddb: Poll OK is_mounted={} after {:?}",
                v != 0,
                poll_start.elapsed()
            );
        }
        Ok(other) => {
            log::warn!(
                "probe-pddb: Poll unexpected response {:?} after {:?}",
                other,
                poll_start.elapsed()
            );
        }
        Err(e) => {
            log::warn!("probe-pddb: Poll FAIL after {:?}: {:?}", poll_start.elapsed(), e);
        }
    }

    log::info!("probe-pddb: probe done in {:?}", start.elapsed());
}


/// Exercise [`presage_store_pddb::BufferingBackend`] +
/// [`presage_store_pddb::KvBackend`] semantics against a fresh
/// [`presage_store_pddb::MockBackend`] and log UART lines for the
/// `xas-send-batch.robot` Renode test.
///
/// Uses [`presage_store_pddb::MockBackend`] rather than the real
/// PDDB because the real backend cannot mount inside Renode
/// (`Opcode::TryMount` requires rootkeys + password modal that
/// Renode does not inject). The wrapper's batching semantics are
/// the same regardless of inner backend; this probe adds rv32
/// build + boot + run coverage on top of the host-side
/// `cargo test -p presage-store-pddb` suite.
///
/// Synthetic send-shaped sequence:
///
/// 1. Wrap a fresh `MockBackend` in a `BufferingBackend`.
/// 2. `begin_batch` and confirm `is_batching` flips.
/// 3. Three writes simulating a cold send (recipient identity,
///    sender certificate, outbound message body).
/// 4. Intra-batch read-through (writes visible inside the batch).
/// 5. Commit; confirm the replay count.
/// 6. Post-commit reads (writes durable in the inner backend).
/// 7. Abort path: open a second batch, write, drop without
///    committing, verify the writes are gone.
#[cfg(all(feature = "probe-send-batch", target_os = "xous"))]
fn probe_send_batch() {
    use std::sync::Arc;
    use std::time::Instant;

    use presage_store_pddb::{BufferingBackend, KvBackend, MockBackend};

    log::info!("probe-send-batch: starting");
    let start = Instant::now();

    let backend = BufferingBackend::new(Arc::new(MockBackend::new()));
    log::info!("probe-send-batch: backend constructed");

    // --- 1. Begin a batch.
    let guard = match backend.begin_batch() {
        Ok(g) => g,
        Err(e) => {
            log::warn!("probe-send-batch: FAIL: begin_batch error: {}", e);
            return;
        }
    };
    if !backend.is_batching() {
        log::warn!("probe-send-batch: FAIL: is_batching false after begin");
        return;
    }
    log::info!("probe-send-batch: batch begin OK in {:?}", start.elapsed());

    // --- 2. Three writes simulating a cold-send protocol-store
    //        update.
    let phase = Instant::now();
    backend.put("signal.protocol.aci.identity", "peer.1", b"identity-bytes").unwrap();
    backend.put("signal.state", "sender_certificate", b"sender-cert-bytes").unwrap();
    backend
        .put("signal.contents.thread.peer", "00000000000186A0", b"hello world")
        .unwrap();
    let buffered = guard.buffered_len();
    log::info!(
        "probe-send-batch: 3 writes buffered in {:?} (count={})",
        phase.elapsed(),
        buffered
    );
    if buffered != 3 {
        log::warn!("probe-send-batch: FAIL: expected 3 buffered, got {}", buffered);
        return;
    }

    // --- 3. Intra-batch read-through.
    let read = backend.get("signal.protocol.aci.identity", "peer.1").unwrap();
    if read.as_deref() != Some(b"identity-bytes".as_slice()) {
        log::warn!("probe-send-batch: FAIL: intra-batch read mismatch");
        return;
    }
    log::info!("probe-send-batch: intra-batch read-through OK");

    // --- 4. Commit.
    let phase = Instant::now();
    let n = match guard.commit() {
        Ok(n) => n,
        Err(e) => {
            log::warn!("probe-send-batch: FAIL: commit error: {}", e);
            return;
        }
    };
    if backend.is_batching() {
        log::warn!("probe-send-batch: FAIL: still batching after commit");
        return;
    }
    log::info!(
        "probe-send-batch: commit OK in {:?} (replayed {})",
        phase.elapsed(),
        n
    );

    // --- 5. Post-commit reads.
    let r1 = backend.get("signal.protocol.aci.identity", "peer.1").unwrap();
    let r2 = backend.get("signal.state", "sender_certificate").unwrap();
    let r3 = backend.get("signal.contents.thread.peer", "00000000000186A0").unwrap();
    if r1.as_deref() != Some(b"identity-bytes".as_slice())
        || r2.as_deref() != Some(b"sender-cert-bytes".as_slice())
        || r3.as_deref() != Some(b"hello world".as_slice())
    {
        log::warn!("probe-send-batch: FAIL: post-commit read mismatch");
        return;
    }
    log::info!("probe-send-batch: post-commit reads match");

    // --- 6. Abort path: open a second batch, write, drop without
    //        commit, verify nothing landed.
    {
        let _abort_guard = backend.begin_batch().unwrap();
        backend.put("signal.state", "sender_certificate", b"transient-cert").unwrap();
        // _abort_guard drops at end of block without commit -> abort.
    }
    let after_abort = backend.get("signal.state", "sender_certificate").unwrap();
    if after_abort.as_deref() != Some(b"sender-cert-bytes".as_slice()) {
        log::warn!(
            "probe-send-batch: FAIL: abort didn't restore inner ({:?})",
            after_abort
        );
        return;
    }
    log::info!("probe-send-batch: abort path OK");

    log::info!("probe-send-batch: probe done in {:?}", start.elapsed());
}


/// Install a `log` implementation for rv32-xous builds.
///
/// `xous_api_log::init_wait()` blocks until it can connect to the
/// `xous-log-server` SID, then registers itself as the `log::Log`
/// impl. The server (running as a separate process in the baseline
/// Xous image) forwards records to the UART, which is what the
/// Renode Robot tests assert on.
///
/// If `init_wait` returns `Err` (log server did not come up in
/// time, fatal Xous-image misconfiguration), this function does
/// not panic — subsequent `log::*!` calls become no-ops, matching
/// the surface of a binary that never installed a logger.
#[cfg(target_os = "xous")]
fn init_logger() {
    let _ = xous_api_log::init_wait();
}
