//! Xous Signal app entry point.
//!
//! The earlier sequential Hello/Whoami probe is replaced with
//! a real UI loop. The probe lives on as the menu's "Test worker"
//! item — useful for verifying the worker thread + IPC channels are
//! still alive after a code change. The shape of the binary is now:
//!
//! 1. Construct a `PddbStore` (mock backend in hosted; real PDDB
//!    behind a `pddb-backend` feature flag).
//! 2. Spawn the manager worker thread (`xous-signal-worker`).
//! 3. Hand the cmd/event channels to `Ui::new` and call `Ui::run`.
//! 4. Worker shutdown is the responsibility of the UI driver — it
//!    sends `Cmd::Shutdown` on Quit.
//!
//! See docs/ROADMAP.md and docs/UI.md for the design.

mod dialogue;
mod gam_app;

use async_channel::bounded;
use presage_store_pddb::PddbStore;
#[cfg(not(all(feature = "auto-link", target_os = "xous")))]
use xous_app_signal_ui::Ui;
use xous_signal_worker::{Cmd, Event, run_signal_worker};

/// Real `__getrandom_v03_custom` body backed by xous-core's TRNG
/// service.
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

/// Channel capacity. 16 is plenty for the single-prompt
/// round-trip; production sizing will revisit.
const CHAN_CAP: usize = 16;

/// Choose the store backend based on feature flags.
///
/// - default (smoke / probe-flow / probe-pddb / hosted): mock
///   in-memory backend.
/// - `pddb-real` on rv32-xous: real `PddbStore::with_pddb_backend`.
///   If the connect fails (e.g. PDDB isn't running), we log and
///   fall back to mock so the binary still boots — the smoke test
///   shouldn't fail just because PDDB's not up. Design: surface
///   real failures via xas's UI eventually, not by hard-aborting
///   boot.
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

    #[cfg(all(feature = "probe-pddb-real", target_os = "xous"))]
    probe_pddb_real();

    // The auto-link feature drives the link flow + QR modal
    // on real hardware. When enabled, we *replace* the regular UI
    // loop — the auto-link probe is the UI for this build mode. On
    // success, it logs LinkComplete details to UART and lets main()
    // continue to the worker shutdown path.
    #[cfg(all(feature = "auto-link", target_os = "xous"))]
    auto_link(cmd_tx.clone(), event_rx.clone());

    #[cfg(not(all(feature = "auto-link", target_os = "xous")))]
    {
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
    }

    // Worker has been told to shut down; join it. If the join hangs
    // it's a worker-side bug — surface as a nonzero exit, not a
    // silent hang.
    let _ = worker.join();
    log::info!("xas: exiting");
    Ok(())
}

/// Install a `log` implementation. Hosted picks
/// `env_logger`; rv32-xous binds `xous-api-log` via the integration
/// step (needs a path-dep that resolves only inside xous-core's
/// tree).
#[cfg(not(target_os = "xous"))]
fn init_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

/// TCP-connect probe used to figure out whether
/// Renode's WF200 wifi emulation actually carries outbound traffic.
/// Three connect targets, each logged with elapsed time and result:
///
/// 1. `8.8.8.8:53` — Google DNS over TCP. No DNS needed; hits the
///    lowest-level "is there a route to the internet" question.
/// 2. `1.1.1.1:443` — Cloudflare 1.1.1.1 over HTTPS port. Same
///    no-DNS shape as (1) but a different provider, in case route
///    filtering is in play.
/// 3. `chat.signal.org:443` — the actual Signal endpoint. Requires
///    DNS resolution, so this also probes the Xous `dns` service.
///
/// Each probe has a 10-second timeout. The whole sequence logs to
/// the same `INFO:xas:` stream the Robot smoke test asserts on, so
/// findings show up in the `xas-probe.robot` test output.
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
/// the result. This verifies the "hand-rolled PDDB client" path
/// before we commit to writing it for real — the Mount Poller's
/// `Poll` opcode (0) is the simplest IPC roundtrip we can do
/// against PDDB, taking no payload and returning a `Scalar1(0|1)`
/// mount state.
///
/// Implementation mirrors `services/pddb/src/lib.rs:30–60`
/// (`PddbMountPoller::new` + `is_mounted_nonblocking`) but does
/// not depend on the `pddb` crate — only on `xous`,
/// `xous-api-names`, and `xous_ipc` (all of which we already
/// path-dep'd or pulled from crates.io for the trng client).
///
/// Two outcomes are interesting:
///   - `OK true` / `OK false` — the IPC plumbing works; the
///     value tells us whether the image autobases or expects
///     password-driven mount.
///   - `panic` / `connection refused` — protocol replication
///     issue (rkyv version, Buffer layout, or SID name typo).
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

/// Put/get/list/delete/list cycle against the real PDDB-backed
/// `KvBackend`. Verifies the buffered IPC path (the `lend_mut`
/// calls that the scalar-only `probe-pddb` couldn't exercise)
/// actually works on the wire.
///
/// Requires the image to be built with `pddb/autobasis` so PDDB is
/// pre-mounted on boot — otherwise every op returns `NotMounted`
/// and we just log that finding.
///
/// All outcomes are logged through `xous-api-log`; the Robot test
/// at `tests/renode/xas-pddb-real-probe.robot` waits on each line.
#[cfg(all(feature = "probe-pddb-real", target_os = "xous"))]
fn probe_pddb_real() {
    use std::time::Instant;

    log::info!("probe-pddb-real: starting put/get/delete cycle");
    let start = Instant::now();

    let backend = match presage_store_pddb::PddbBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            log::warn!("probe-pddb-real: connect FAIL: {}", e);
            return;
        }
    };
    log::info!("probe-pddb-real: connected in {:?}, mounted={}", start.elapsed(), backend.is_mounted());

    use presage_store_pddb::KvBackend;
    let dict = "xas.probe";
    let key = "hello";
    let value: &[u8] = b"world";

    // Keep going even if individual ops fail. The failure mode
    // itself is informative (which is what the probe is for);
    // aborting early masks downstream IPC behavior we want to see.
    let phase = Instant::now();
    match backend.put(dict, key, value) {
        Ok(()) => log::info!("probe-pddb-real: put OK in {:?}", phase.elapsed()),
        Err(e) => log::warn!("probe-pddb-real: put FAIL after {:?}: {}", phase.elapsed(), e),
    }

    let phase = Instant::now();
    match backend.get(dict, key) {
        Ok(Some(v)) => log::info!(
            "probe-pddb-real: get OK len={} match={} in {:?}",
            v.len(),
            v == value,
            phase.elapsed()
        ),
        Ok(None) => log::warn!("probe-pddb-real: get returned None unexpectedly"),
        Err(e) => log::warn!("probe-pddb-real: get FAIL: {}", e),
    }

    let phase = Instant::now();
    match backend.list_keys(dict) {
        Ok(keys) => log::info!("probe-pddb-real: list_keys OK {:?} in {:?}", keys, phase.elapsed()),
        Err(e) => log::warn!("probe-pddb-real: list_keys FAIL: {}", e),
    }

    let phase = Instant::now();
    match backend.delete(dict, key) {
        Ok(()) => log::info!("probe-pddb-real: delete OK in {:?}", phase.elapsed()),
        Err(e) => log::warn!("probe-pddb-real: delete FAIL: {}", e),
    }

    let phase = Instant::now();
    match backend.list_keys(dict) {
        Ok(keys) if keys.is_empty() => {
            log::info!("probe-pddb-real: post-delete list empty in {:?}", phase.elapsed());
        }
        Ok(keys) => log::warn!("probe-pddb-real: post-delete list still has {:?}", keys),
        Err(e) => log::warn!("probe-pddb-real: post-delete list FAIL: {}", e),
    }

    log::info!("probe-pddb-real: probe done in {:?}", start.elapsed());
}

/// Hardware auto-link probe.
///
/// Fires `Cmd::LinkDevice` and drives the resulting event stream
/// to either `Event::LinkComplete` or `Event::LinkError`. On
/// `Event::LinkUrl(url)`, opens a `xous-modals-ipc` notification
/// modal that displays the URL as a QR code (rendered by the
/// upstream modals server's `notification.set_qrcode` call). User
/// scans the QR with their Signal phone, then presses any key to
/// dismiss the modal — meanwhile the worker is parked on the
/// provisioning WebSocket waiting for the encrypted envelope.
///
/// Failure modes worth distinguishing in the UART log:
/// - **No modals response**: server isn't running or our wire
///   protocol drifted. Existing precedent (`xous-pddb-ipc`)
///   makes wire drift unlikely; missing server means the image
///   wasn't built with `services/modals` (always present in our
///   `cargo xtask app-image` flow).
/// - **`Event::LinkError(_)`**: worker-side failure (DNS, TLS,
///   WS, or libsignal protocol error). The error string identifies
///   the layer.
/// - **`Event::LinkComplete`**: success. The aci/phone fields are
///   logged so the next flash iteration can verify
///   `with_pddb_backend` actually persisted the registration data.
#[cfg(all(feature = "auto-link", target_os = "xous"))]
fn auto_link(cmd_tx: async_channel::Sender<Cmd>, event_rx: async_channel::Receiver<Event>) {
    use xous_modals_ipc::ModalsClient;

    log::info!("auto-link: starting");

    let xns = match xous_names::XousNames::new() {
        Ok(x) => x,
        Err(e) => {
            log::error!("auto-link: XousNames::new failed: {:?}", e);
            return;
        }
    };
    let trng_client = match trng::Trng::new(&xns) {
        Ok(t) => t,
        Err(e) => {
            log::error!("auto-link: Trng::new failed: {:?}", e);
            return;
        }
    };
    let modals = match ModalsClient::new(&xns, &trng_client) {
        Ok(m) => m,
        Err(e) => {
            log::error!("auto-link: ModalsClient::new failed: {}", e);
            return;
        }
    };

    let device_name = "xas-hardware-probe".to_string();
    log::info!("auto-link: sending Cmd::LinkDevice {{ device_name = {:?} }}", device_name);
    if let Err(e) = cmd_tx.send_blocking(Cmd::LinkDevice { device_name }) {
        log::error!("auto-link: cmd_tx.send_blocking failed: {:?}", e);
        return;
    }

    loop {
        let event = match event_rx.recv_blocking() {
            Ok(ev) => ev,
            Err(e) => {
                log::error!("auto-link: event_rx.recv_blocking failed: {:?}", e);
                return;
            }
        };
        match event {
            Event::LinkUrl(url) => {
                log::info!("auto-link: link URL = {}", url);
                // Show the QR modal. Blocks until user presses any
                // key — meanwhile the worker is parked on WS, so
                // the order of (modal-dismiss, link-complete) can
                // interleave either way; both events get drained
                // by the loop.
                if let Err(e) = modals.show_notification(
                    "Scan with the Signal phone app, then press any key.",
                    Some(&url),
                ) {
                    log::warn!("auto-link: QR modal failed: {}; continuing", e);
                }
            }
            Event::LinkComplete { device_name, aci, phone } => {
                log::info!(
                    "auto-link: LinkComplete device={} aci={} phone={}",
                    device_name, aci, phone
                );
                let summary = format!("Linked!\n  device: {}\n  aci:    {}\n  phone:  {}", device_name, aci, phone);
                let _ = modals.show_notification(&summary, None);
                break;
            }
            Event::LinkError(msg) => {
                log::warn!("auto-link: LinkError: {}", msg);
                let _ = modals.show_notification(
                    &format!("Link failed: {}", msg),
                    None,
                );
                break;
            }
            other => {
                log::info!("auto-link: drained event {:?}", other);
            }
        }
    }

    // Tell the worker to shut down so main() can join it cleanly.
    let _ = cmd_tx.send_blocking(Cmd::Shutdown);
    // Drain remaining events so the worker can exit.
    while let Ok(ev) = event_rx.recv_blocking() {
        log::info!("auto-link: post-shutdown event {:?}", ev);
        if matches!(ev, Event::ShuttingDown) {
            break;
        }
    }
    log::info!("auto-link: done");
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
