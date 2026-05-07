//! Sync TLS + WebSocket transport pump for the Signal client.
//!
//! Owns a dedicated Xous thread that holds a sync
//! `tungstenite::WebSocket<rustls::StreamOwned<ClientConnection, TcpStream>>`
//! and forwards frames to/from the async executor thread via
//! `async-channel`.

pub mod http;
pub mod tls;
pub mod ws;
pub mod ws_pump;

pub use http::SyncHttpClient;
pub use tls::{
    RustlsStream, signal_production_roots, signal_staging_roots, tls_connect, webpki_roots,
};
pub use ws::ws_connect;
