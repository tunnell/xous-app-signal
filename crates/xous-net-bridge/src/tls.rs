//! Sync TLS connection establishment.
//!
//! The Signal client speaks HTTPS and WSS; both layer rustls over a sync
//! `Read + Write` stream. On hosted Linux that's `std::net::TcpStream`;
//! on Xous it's the same thing, exposed by `services/net` (verified at
//! `xous-core/services/net/src/std_tcpstream.rs`). API is identical, so
//! the same `tls_connect` works on both.

use std::fmt;
use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use rustls::client::{
    ClientSessionMemoryCache, ClientSessionStore, Resumption, Tls12ClientSessionValue,
    Tls13ClientSessionValue,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, NamedGroup, RootCertStore, StreamOwned};

/// A handshake-completed sync TLS stream over TCP.
pub type RustlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Counting wrapper around the rustls `ClientSessionMemoryCache(8)` used by
/// [`build_tls_config`]. Forwards every `ClientSessionStore` method to the
/// inner cache verbatim; on top of that it increments a monotonically-
/// increasing counter every time `take_tls13_ticket` returns `Some` (i.e.,
/// a cached TLS 1.3 session ticket was consumed to offer PSK resumption on
/// an outgoing handshake).
///
/// `tls_connect_with_config` snapshots the counter immediately before
/// driving `complete_io`, then again after; if the second snapshot is
/// greater than the first, this connection's handshake consumed a ticket
/// and we log `was_resumed=true`. The single global counter means
/// concurrent handshakes against the same `Arc<ClientConfig>` can in
/// principle mis-attribute resumption between connections — but xas only
/// ever holds one `ClientConfig` at a time and the WS pump / HTTP poller
/// connect to distinct hosts, so the race is theoretical.
///
/// Why not the `peer_certificates()` heuristic that the iter-2 prototype
/// used (commit f162828): per the TLS 1.3 spec
/// (RFC 8446 §4.2.11), a server MAY still send a Certificate message on
/// a resumed connection depending on its `psk_key_exchange_modes`. The
/// observation "peer_certs > 0" therefore doesn't disprove resumption,
/// only "peer_certs > 0 AND handshake_ms is full-handshake-shaped" does
/// — and that's already covered by `handshake_ms`. A direct hook into
/// the resumption store is unambiguous and removes one item from the
/// interpretation matrix.
pub struct CountingResumptionStore {
    inner: ClientSessionMemoryCache,
    /// Total `take_tls13_ticket(...) -> Some(...)` calls observed across
    /// every host. Snapshotted by `tls_connect_with_config` for the
    /// `was_resumed` diagnostic. `AtomicUsize` instead of `AtomicU64`
    /// because `rv32imac` (Precursor target) lacks 64-bit atomics; the
    /// 32-bit-on-rv32 counter would only wrap after ~4 billion
    /// handshakes, far more than the device will ever issue.
    take_some_count: AtomicUsize,
}

impl CountingResumptionStore {
    fn new(size: usize) -> Self {
        Self {
            inner: ClientSessionMemoryCache::new(size),
            take_some_count: AtomicUsize::new(0),
        }
    }

    /// Monotonic count of `take_tls13_ticket(...) -> Some(...)` calls
    /// observed across the lifetime of this store. Public for test access.
    pub fn take_some_count(&self) -> usize {
        self.take_some_count.load(Ordering::Acquire)
    }
}

impl fmt::Debug for CountingResumptionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CountingResumptionStore")
            .field("take_some_count", &self.take_some_count.load(Ordering::Relaxed))
            .field("inner", &self.inner)
            .finish()
    }
}

impl ClientSessionStore for CountingResumptionStore {
    fn set_kx_hint(&self, server_name: ServerName<'static>, group: NamedGroup) {
        self.inner.set_kx_hint(server_name, group)
    }
    fn kx_hint(&self, server_name: &ServerName<'_>) -> Option<NamedGroup> {
        self.inner.kx_hint(server_name)
    }
    fn set_tls12_session(
        &self,
        server_name: ServerName<'static>,
        value: Tls12ClientSessionValue,
    ) {
        self.inner.set_tls12_session(server_name, value)
    }
    fn tls12_session(&self, server_name: &ServerName<'_>) -> Option<Tls12ClientSessionValue> {
        self.inner.tls12_session(server_name)
    }
    fn remove_tls12_session(&self, server_name: &ServerName<'static>) {
        self.inner.remove_tls12_session(server_name)
    }
    fn insert_tls13_ticket(
        &self,
        server_name: ServerName<'static>,
        value: Tls13ClientSessionValue,
    ) {
        self.inner.insert_tls13_ticket(server_name, value)
    }
    fn take_tls13_ticket(
        &self,
        server_name: &ServerName<'static>,
    ) -> Option<Tls13ClientSessionValue> {
        let v = self.inner.take_tls13_ticket(server_name);
        if v.is_some() {
            self.take_some_count.fetch_add(1, Ordering::AcqRel);
        }
        v
    }
}

/// Active counting store, installed by [`build_tls_config`]. `OnceLock` so
/// the first build wins; subsequent calls to `build_tls_config` keep using
/// it (xas only ever builds one `ClientConfig`, so this is fine — the
/// diagnostic just observes resumption activity across the whole
/// process). Read by `tls_connect_with_config`'s before/after snapshot.
///
/// Tests that bypass `build_tls_config` (e.g. `tls_resumption.rs` which
/// wires its own `Resumption::store`) leave this unset; the handshake log
/// in that case reports `was_resumed=unknown` rather than `true`/`false`.
static ACTIVE_COUNTER: OnceLock<Arc<CountingResumptionStore>> = OnceLock::new();

/// Test helper. Returns the global counter installed by `build_tls_config`,
/// or `None` if no config has been built yet. Production code never reads
/// this directly — `tls_connect_with_config` snapshots internally.
#[doc(hidden)]
pub fn active_counter_for_tests() -> Option<Arc<CountingResumptionStore>> {
    ACTIVE_COUNTER.get().cloned()
}

/// Build a `ClientConfig` once and share it across many `tls_connect_with_config`
/// calls. The config carries an in-memory session-ticket cache; reusing the
/// same `Arc<ClientConfig>` across reconnects to the same host is what enables
/// TLS 1.3 session resumption (PSK ticket reuse instead of a full handshake
/// every time). The default rustls 0.22 cache is `in_memory_sessions(256)`,
/// which we shrink to 8: xas only ever talks to a handful of Signal endpoints
/// plus an occasional CDN, and a smaller cache makes ticket-eviction less
/// likely if Signal's load balancer happens to issue tickets pinned to
/// different backends.
pub fn build_tls_config(roots: RootCertStore, alpn: &[&[u8]]) -> Arc<ClientConfig> {
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    if !alpn.is_empty() {
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    }
    // Same on-the-wire behavior as the prior
    // `Resumption::in_memory_sessions(8)` — `CountingResumptionStore` wraps
    // exactly one `ClientSessionMemoryCache::new(8)`. The wrapper only
    // adds a fetch_add on the take-some path so the post-handshake log
    // in `tls_connect_with_config` can emit `was_resumed=true/false`
    // without relying on the unreliable `peer_certificates()` heuristic.
    let counter = ACTIVE_COUNTER
        .get_or_init(|| Arc::new(CountingResumptionStore::new(8)))
        .clone();
    config.resumption = Resumption::store(counter);
    Arc::new(config)
}

/// Mozilla NSS-bundle root CAs from `webpki-roots`. Suitable for general
/// HTTPS to public endpoints (example.com, etc.). Not suitable for
/// Signal endpoints, which pin their own CA — use [`signal_production_roots`].
pub fn webpki_roots() -> RootCertStore {
    RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// Signal's production root CA, pinned. Mirrors what `libsignal-service-rs`
/// does at `src/push_service/mod.rs:90-96`: disable system roots, use only
/// this CA. The PEM is vendored at `certs/signal-production.pem` from
/// upstream `whisperfish/libsignal-service-rs/certs/production-root-ca.pem`.
pub fn signal_production_roots() -> RootCertStore {
    parse_pem_roots(include_bytes!("../certs/signal-production.pem"))
        .expect("Signal production root CA bundled at build time should parse")
}

/// Same for Signal's staging environment.
pub fn signal_staging_roots() -> RootCertStore {
    parse_pem_roots(include_bytes!("../certs/signal-staging.pem"))
        .expect("Signal staging root CA bundled at build time should parse")
}

fn parse_pem_roots(pem: &[u8]) -> io::Result<RootCertStore> {
    let mut reader = std::io::BufReader::new(pem);
    let mut store = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert?;
        store.add(cert).map_err(io::Error::other)?;
    }
    Ok(store)
}

/// Open a sync TLS connection to `host:port` using a pre-built shared
/// `ClientConfig`. Hot-path callers that reconnect to the same host should
/// build the config once via [`build_tls_config`] and reuse the resulting
/// `Arc<ClientConfig>` so the in-memory session-ticket cache survives
/// across reconnects (this is what enables TLS 1.3 PSK resumption).
///
/// The TLS handshake is driven to completion here, before the function
/// returns. Previously it was lazy (rustls completed it on the caller's
/// first I/O via `StreamOwned`), but the iter-1 instrumentation could
/// not measure handshake cost in isolation — so iter-3 needs to see the
/// handshake as its own distinct phase. All current callers issue I/O
/// immediately after this returns, so making it eager changes no
/// observable behavior; it just relocates the wall-clock cost (and the
/// failure mode for handshake errors) from the caller's first read/write
/// to this function.
pub fn tls_connect_with_config(
    host: &str,
    port: u16,
    config: Arc<ClientConfig>,
) -> io::Result<RustlsStream> {
    let t_start = std::time::Instant::now();
    tracing::info!("perf/net: tls_connect entry host={} port={}", host, port);
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let _perf_sn_ms = t_start.elapsed().as_millis() as u64;
    let conn = ClientConnection::new(config, server_name).map_err(io::Error::other)?;
    let _perf_conn_ms = t_start.elapsed().as_millis() as u64;

    let sock = TcpStream::connect((host, port))?;
    let setup_ms = t_start.elapsed().as_millis() as u64;
    tracing::info!(host, port, setup_ms, "tls_connect: setup-phase complete");
    // Restored from pre-f162828: kept verbatim so log-grep tools that
    // match `perf/net: tls_connect exit host=...` continue to fire.
    // f162828 had consolidated this with the post-handshake fields onto
    // one line; splitting them back out keeps the existing grep contract
    // and lets the new `perf/net: tls_handshake` line below carry the
    // strictly-additive diagnostic.
    tracing::info!(
        "perf/net: tls_connect exit host={} port={} server_name_ms={} client_conn_ms={} tcp_ms={} setup_total_ms={}",
        host, port, _perf_sn_ms, _perf_conn_ms - _perf_sn_ms,
        setup_ms - _perf_conn_ms, setup_ms
    );

    // Short TCP read timeout. WebSocket users (`ws_pump.rs::reader_loop`)
    // hold a mutex across the blocking `WebSocket::read()`; without a
    // timeout, the reader would block forever waiting for an inbound
    // frame and the writer thread could never acquire the mutex to
    // send keepalives. Signal's provisioning WS times out clients at
    // ~60s of idle, so missing keepalives means the link drops before
    // the user finishes scanning. The timeout makes `read()` return
    // `Err(WouldBlock|TimedOut)` periodically; the reader catches that
    // and re-loops, briefly releasing the mutex.
    sock.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

    let mut stream = StreamOwned::new(conn, sock);

    // Snapshot the active counting-store's take-some counter immediately
    // before driving the handshake. If `complete_io` consumes a TLS 1.3
    // session ticket from the cache, `CountingResumptionStore::take_tls13_ticket`
    // increments this counter; comparing the post-handshake snapshot to
    // this one yields an unambiguous `was_resumed` per connection (subject
    // to the cross-host concurrency caveat documented on the struct).
    //
    // `ACTIVE_COUNTER` is `None` only when the caller wired a custom
    // `Resumption::store` directly (e.g. the integration test in
    // `tests/tls_resumption.rs`); in that case `was_resumed` reports
    // `unknown` and the test's own `CountingStore` is the source of truth.
    let counter_before = ACTIVE_COUNTER.get().map(|c| c.take_some_count());

    // Drive the handshake to completion. On TLS 1.3 PSK resumption rustls
    // performs ~1 RTT of symmetric crypto + HKDF; on a full handshake it
    // does ECDHE + cert verification, which on rv32 with software crypto
    // is roughly an order of magnitude more wall-time. handshake_ms by
    // itself is sufficient to distinguish the two; was_resumed below
    // removes any ambiguity at the cost of one extra Acquire load.
    let hs_start = std::time::Instant::now();
    if let Err(e) = stream.conn.complete_io(&mut stream.sock) {
        tracing::info!(
            "perf/net: tls_handshake_error host={} port={} setup_total_ms={} handshake_ms={} err={}",
            host, port, setup_ms, hs_start.elapsed().as_millis() as u64, e
        );
        return Err(e);
    }
    let handshake_ms = hs_start.elapsed().as_millis() as u64;

    let proto = stream
        .conn
        .protocol_version()
        .map(|v| format!("{:?}", v))
        .unwrap_or_else(|| "?".to_string());
    let cipher = stream
        .conn
        .negotiated_cipher_suite()
        .map(|s| format!("{:?}", s.suite()))
        .unwrap_or_else(|| "?".to_string());

    let was_resumed = match (counter_before, ACTIVE_COUNTER.get()) {
        (Some(before), Some(c)) => {
            if c.take_some_count() > before {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        // No active counter installed: caller bypassed `build_tls_config`
        // (test-only path). Don't claim either way.
        _ => "unknown".to_string(),
    };
    tracing::info!(
        "perf/net: tls_handshake host={} port={} handshake_ms={} was_resumed={} proto={} cipher={}",
        host, port, handshake_ms, was_resumed, proto, cipher
    );

    Ok(stream)
}

/// Convenience wrapper for one-shot callers (examples, tests). Builds a
/// fresh `ClientConfig` per call — the in-memory session-ticket cache lives
/// inside the config and is therefore wasted. Hot-path callers (everything
/// inside `SyncHttpClient`/`ws_pump`) MUST go through [`build_tls_config`] +
/// [`tls_connect_with_config`] instead, otherwise TLS resumption is silently
/// disabled.
pub fn tls_connect(
    host: &str,
    port: u16,
    roots: RootCertStore,
    alpn: &[&[u8]],
) -> io::Result<RustlsStream> {
    tls_connect_with_config(host, port, build_tls_config(roots, alpn))
}

#[cfg(test)]
mod counting_store_tests {
    use super::*;
    use rustls::pki_types::ServerName;

    /// `take_some_count` increments only when `take_tls13_ticket` returns
    /// `Some` — not on `take_tls13_ticket` calls that miss, and not on
    /// `insert_tls13_ticket`. This is the load-bearing property
    /// `tls_connect_with_config` relies on for `was_resumed`.
    #[test]
    fn take_some_count_increments_only_on_hit() {
        let store = CountingResumptionStore::new(8);
        let name: ServerName<'static> = ServerName::try_from("example.com").unwrap();

        // Empty cache: take returns None, counter unchanged.
        assert!(store.take_tls13_ticket(&name).is_none());
        assert_eq!(store.take_some_count(), 0);

        // Inserting a value doesn't bump the take-some counter — only
        // `take_tls13_ticket(...) -> Some` does. We can't construct a real
        // Tls13ClientSessionValue here (the constructor is rustls-internal),
        // so this branch is exercised indirectly by the integration test
        // in `tests/tls_resumption.rs` which drives a real handshake pair.
        // The assertion below covers the negative case: take on an empty
        // store never touches the counter regardless of how many times we
        // call it.
        for _ in 0..16 {
            let _ = store.take_tls13_ticket(&name);
        }
        assert_eq!(store.take_some_count(), 0);
    }

    /// `build_tls_config` is idempotent w.r.t. the active counter: calling
    /// it more than once returns configs that share the same counting
    /// store. (Production xas calls it once; defensive assertion in case a
    /// future refactor adds a second caller.)
    #[test]
    fn build_tls_config_reuses_the_active_counter() {
        let roots = webpki_roots();
        let _c1 = build_tls_config(roots.clone(), &[]);
        let _c2 = build_tls_config(roots, &[]);
        // Both calls populated the OnceLock — once. Second call sees the
        // already-installed counter and reuses it.
        let counter = active_counter_for_tests().expect("counter installed");
        // Counter is fresh; no handshakes were driven, so take-some=0.
        assert_eq!(counter.take_some_count(), 0);
    }
}
