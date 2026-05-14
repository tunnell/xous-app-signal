//! Sync TLS connection establishment.
//!
//! The Signal client speaks HTTPS and WSS; both layer rustls over a sync
//! `Read + Write` stream. On hosted Linux that's `std::net::TcpStream`;
//! on Xous it's the same thing, exposed by `services/net` (verified at
//! `xous-core/services/net/src/std_tcpstream.rs`). API is identical, so
//! the same `tls_connect` works on both.

use std::io;
use std::net::TcpStream;
use std::sync::Arc;

use rustls::client::Resumption;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// A handshake-completed sync TLS stream over TCP.
pub type RustlsStream = StreamOwned<ClientConnection, TcpStream>;

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
    config.resumption = Resumption::in_memory_sessions(8);
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

    // Drive the handshake to completion. On TLS 1.3 PSK resumption rustls
    // performs ~1 RTT of symmetric crypto + HKDF; on a full handshake it
    // does ECDHE + cert verification, which on rv32 with software crypto
    // is roughly an order of magnitude more wall-time. Logging
    // handshake_ms is therefore enough to distinguish the two even
    // without a direct "was resumed" flag (rustls 0.22 does not expose
    // one publicly).
    let hs_start = std::time::Instant::now();
    if let Err(e) = stream.conn.complete_io(&mut stream.sock) {
        tracing::info!(
            "perf/net: tls_connect handshake_error host={} port={} setup_total_ms={} handshake_ms={} err={}",
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
    // peer_certificates() is the closest public signal in rustls 0.22 for
    // resumption: on a TLS 1.3 PSK resumption the server skips the
    // Certificate message, so this is `None` (or empty). On a full
    // handshake it carries the chain rustls just validated. iter-3
    // empirical validation against chat.signal.org will confirm the
    // mapping holds for our cipher/curve negotiation.
    let peer_certs = stream
        .conn
        .peer_certificates()
        .map(|c| c.len())
        .unwrap_or(0);
    tracing::info!(
        "perf/net: tls_connect exit host={} port={} server_name_ms={} client_conn_ms={} tcp_ms={} setup_total_ms={} handshake_ms={} proto={} cipher={} peer_certs={}",
        host, port,
        _perf_sn_ms,
        _perf_conn_ms - _perf_sn_ms,
        setup_ms - _perf_conn_ms,
        setup_ms,
        handshake_ms,
        proto,
        cipher,
        peer_certs
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
