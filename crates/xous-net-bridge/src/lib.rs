//! Sync TLS + WebSocket transport pump for the Signal client.
//!
//! Owns a dedicated Xous thread that holds a sync
//! `tungstenite::WebSocket<rustls::StreamOwned<ClientConnection, TcpStream>>`
//! and forwards frames to/from the async executor thread via
//! `async-channel`. See `docs/REPORT.md` Decision 3.
//!
//! Stage 0: skeleton only.
