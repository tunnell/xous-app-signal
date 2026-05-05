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

/// Open a sync TLS connection to `host:port`, optionally negotiating an
/// ALPN protocol. Returns a stream that implements `std::io::Read + Write`.
///
/// Trust anchors come from `webpki-roots` (Mozilla's NSS bundle). For
/// Signal's pinned-CA path (used by `libsignal-service-rs`), the caller
/// can substitute a custom `RootCertStore`; that's a follow-up at the
/// Stage 6 transport-fork integration.
pub fn tls_connect(host: &str, port: u16, alpn: &[&[u8]]) -> io::Result<RustlsStream> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

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
    Ok(StreamOwned::new(conn, sock))
}
