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

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// A handshake-completed sync TLS stream over TCP.
pub type RustlsStream = StreamOwned<ClientConnection, TcpStream>;

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

/// Open a sync TLS connection to `host:port` using the supplied trust
/// anchors, optionally negotiating an ALPN protocol. Returns a stream
/// that implements `std::io::Read + Write`.
pub fn tls_connect(
    host: &str,
    port: u16,
    roots: RootCertStore,
    alpn: &[&[u8]],
) -> io::Result<RustlsStream> {
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    if !alpn.is_empty() {
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    }
    let config = Arc::new(config);

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let conn = ClientConnection::new(config, server_name).map_err(io::Error::other)?;

    let sock = TcpStream::connect((host, port))?;
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
    Ok(StreamOwned::new(conn, sock))
}
