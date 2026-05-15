//! Single-shot sync WSS connection helper.
//!
//! Wraps `tungstenite::client` over the TLS stream produced by
//! [`crate::tls::tls_connect`]. tungstenite handles only the HTTP/1.1
//! Upgrade handshake; the underlying TLS is wired in via our own
//! [`crate::tls::tls_connect`] so the rustls version stays pinned at
//! `=0.22.2` (see the workspace `Cargo.toml`).
//!
//! Production traffic goes through [`crate::ws_pump`] instead — this
//! module is for examples (`examples/signal_ws_keepalive.rs`) and
//! smoke tests where the caller does not need the reader/writer
//! thread pair.

use std::io;

use rustls::RootCertStore;
use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::client::Response;
use tungstenite::protocol::WebSocket;

use crate::tls::{RustlsStream, tls_connect};

/// Open a WSS connection to `host:port`+`path`, validating the server's
/// certificate against the supplied root CA store. Returns the WebSocket
/// and the server's HTTP handshake response.
///
/// For Signal endpoints, pass [`crate::signal_production_roots()`]; for
/// general endpoints, [`crate::webpki_roots()`].
pub fn ws_connect(
    host: &str,
    port: u16,
    path: &str,
    roots: RootCertStore,
) -> io::Result<(WebSocket<RustlsStream>, Response)> {
    let url = format!("wss://{host}:{port}{path}");
    let request = url.into_client_request().map_err(io::Error::other)?;

    // ALPN "http/1.1" matches what reqwest/tungstenite would normally send
    // and what Signal's chat WS endpoint expects.
    let stream = tls_connect(host, port, roots, &[b"http/1.1"])?;

    let (ws, resp) =
        tungstenite::client(request, stream).map_err(|e| io::Error::other(e.to_string()))?;
    Ok((ws, resp))
}
