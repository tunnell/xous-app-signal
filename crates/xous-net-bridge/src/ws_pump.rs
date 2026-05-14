//! Sync WebSocket pump worker thread.
//!
//! The async-side caller (libsignal-service-rs's
//! `SignalWebSocketProcess::run`) gets a pair of `async-channel`
//! frame-channels. Internally a worker thread holds the sync
//! `tungstenite::WebSocket` and bridges frames in both directions.
//!
//! The bridge uses **two** worker threads, not one:
//!
//! - **reader thread**: blocks on `ws.read_message()`, forwards each frame
//!   to `incoming_tx.send_blocking(...)`.
//! - **writer thread**: blocks on `outgoing_rx.recv_blocking()`, forwards
//!   each frame via `ws.write_message(...)`.
//!
//! Single-threaded designs deadlock on `ws.read_message()` — that call
//! blocks until a frame arrives, so a single thread can't service the
//! outgoing channel while waiting. Two threads sharing the underlying
//! `WebSocket` (via `Mutex`) is the standard fix; tungstenite's
//! `WebSocket<S>` is `Send` and the `Mutex<WebSocket>` pattern is the
//! documented approach for multi-threaded usage.

use std::sync::{Arc, Mutex};

use libsignal_service::transport::{BasicAuth, HeaderMap, HttpError, WebSocketChannels, WsFrame};
use rustls::ClientConfig;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::{CloseFrame, Message, WebSocket};

use crate::tls::{RustlsStream, tls_connect_with_config};

const FRAME_CHANNEL_CAPACITY: usize = 16;

pub(crate) async fn connect_websocket(
    config: Arc<ClientConfig>,
    url: url::Url,
    headers: HeaderMap,
    auth: Option<BasicAuth>,
) -> Result<WebSocketChannels, HttpError> {
    let (handshake_done_tx, handshake_done_rx) = async_channel::bounded(1);
    let (out_tx, out_rx) = async_channel::bounded::<WsFrame>(FRAME_CHANNEL_CAPACITY);
    let (in_tx, in_rx) =
        async_channel::bounded::<Result<WsFrame, HttpError>>(FRAME_CHANNEL_CAPACITY);

    // Reader/writer worker threads (spawned only after a successful
    // handshake; the handshake itself runs on a dedicated setup thread).
    std::thread::Builder::new()
        .name("xous-net-bridge-ws-setup".into())
        .spawn(move || {
            let ws = match handshake(config, url, headers, auth) {
                Ok(ws) => ws,
                Err(e) => {
                    let _ = handshake_done_tx.send_blocking(Err(e));
                    return;
                }
            };
            // Wrap the WS in an Arc<Mutex<>> so reader and writer threads
            // can share it. tungstenite's WebSocket<S> isn't Sync, but we
            // serialize all access through the Mutex.
            let ws = Arc::new(Mutex::new(ws));
            let _ = handshake_done_tx.send_blocking(Ok(()));

            // Reader thread: pull frames off the WS, push to in_tx.
            let reader_ws = Arc::clone(&ws);
            let reader_tx = in_tx.clone();
            let reader = std::thread::Builder::new()
                .name("xous-net-bridge-ws-reader".into())
                .spawn(move || reader_loop(reader_ws, reader_tx))
                .expect("ws reader thread spawn");

            // Writer thread: pull frames off out_rx, push to the WS.
            let writer_ws = Arc::clone(&ws);
            let writer = std::thread::Builder::new()
                .name("xous-net-bridge-ws-writer".into())
                .spawn(move || writer_loop(writer_ws, out_rx))
                .expect("ws writer thread spawn");

            // Wait for both to finish; once either ends (channel closed,
            // remote close, etc.), drop everything.
            let _ = reader.join();
            let _ = writer.join();
            // Drop in_tx so the executor sees the channel close.
            drop(in_tx);
        })
        .map_err(|e| HttpError::Network(format!("ws setup thread spawn: {e}")))?;

    // Wait for the handshake to complete before returning.
    handshake_done_rx
        .recv()
        .await
        .map_err(|_| HttpError::Network("ws setup thread died".to_string()))??;

    Ok(WebSocketChannels {
        outgoing: out_tx,
        incoming: in_rx,
    })
}

fn handshake(
    config: Arc<ClientConfig>,
    url: url::Url,
    headers: HeaderMap,
    auth: Option<BasicAuth>,
) -> Result<WebSocket<RustlsStream>, HttpError> {
    let host = url
        .host_str()
        .ok_or_else(|| HttpError::InvalidUrl("missing host".to_string()))?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);

    let stream = tls_connect_with_config(&host, port, config)
        .map_err(|e| HttpError::Network(format!("tls connect: {e}")))?;

    // Build a tungstenite ClientRequest; merge in headers/auth.
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| HttpError::Network(format!("ws client_request: {e}")))?;
    let req_headers = request.headers_mut();
    for (name, value) in &headers {
        req_headers.insert(name, value.clone());
    }
    if let Some(BasicAuth { username, password }) = auth {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode(format!("{username}:{password}").as_bytes());
        if let Ok(v) = http::HeaderValue::try_from(format!("Basic {encoded}")) {
            req_headers.insert(http::header::AUTHORIZATION, v);
        }
    }

    let (ws, _resp) = tungstenite::client(request, stream)
        .map_err(|e| HttpError::Network(format!("ws handshake: {e}")))?;
    Ok(ws)
}

fn reader_loop(
    ws: Arc<Mutex<WebSocket<RustlsStream>>>,
    tx: async_channel::Sender<Result<WsFrame, HttpError>>,
) {
    // Diagnostic: count + log every frame the reader gets off the wire.
    // Mirrors the writer-side instrumentation. The combined trace tells
    // us whether KA responses are arriving from the server (matched by
    // the timing of writer "send ok ka" lines and reader "recv frame
    // kind=Binary" lines, plus the libsignal "ka response received"
    // log added in this same image).
    let mut frame_count: u64 = 0;
    loop {
        // Pull one message from the WS. read() blocks on the underlying
        // TCP, which has a short SO_RCVTIMEO set in `tls_connect`.
        // When no inbound frame arrives within the timeout, read()
        // returns `WouldBlock`/`TimedOut`; we drop the mutex, briefly
        // letting the writer thread acquire it (so periodic
        // libsignal-service-rs keepalives can actually go out), then
        // re-loop.
        let _perf_read_start = std::time::Instant::now();
        let msg = {
            let mut guard = match ws.lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::warn!(frame_count, "ws reader: mutex poisoned, exiting");
                    break;
                }
            };
            guard.read()
        };
        let _perf_read_ms = _perf_read_start.elapsed().as_millis();

        let (kind, payload_len, frame) = match msg {
            Ok(Message::Binary(b)) => {
                let len = b.len();
                ("Binary", len, Ok(WsFrame::Binary(b)))
            }
            Ok(Message::Text(s)) => {
                let len = s.len();
                ("Text", len, Ok(WsFrame::Text(s)))
            }
            Ok(Message::Ping(b)) => {
                let len = b.len();
                ("Ping", len, Ok(WsFrame::Ping(b)))
            }
            Ok(Message::Pong(b)) => {
                let len = b.len();
                ("Pong", len, Ok(WsFrame::Pong(b)))
            }
            Ok(Message::Close(frame)) => {
                let (code, reason) = match frame {
                    Some(CloseFrame { code, reason }) => (code.into(), reason.into_owned()),
                    None => (1005, String::new()),
                };
                frame_count += 1;
                tracing::info!(
                    frame_count,
                    kind = "Close",
                    code,
                    "ws reader: recv frame, exiting",
                );
                let _ = tx.send_blocking(Ok(WsFrame::Close { code, reason }));
                break;
            }
            Ok(Message::Frame(_)) => continue, // raw control frame; ignore
            Err(tungstenite::Error::ConnectionClosed) => {
                tracing::info!(frame_count, "ws reader: ConnectionClosed, exiting");
                break;
            }
            Err(tungstenite::Error::AlreadyClosed) => {
                tracing::info!(frame_count, "ws reader: AlreadyClosed, exiting");
                break;
            }
            // Read timed out with nothing to read. Yield and re-loop —
            // see the keepalive comment above.
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Brief sleep to avoid pegging the CPU between timeouts;
                // also gives the writer thread a clear window to acquire
                // the mutex.
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Err(e) => {
                let s = format!("{e}");
                tracing::warn!(frame_count, error = %s, "ws reader: read err, forwarding");
                ("Err", 0, Err(HttpError::Network(format!("ws read: {s}"))))
            }
        };

        // Successful (or err-as-frame) reception: bump and log.
        frame_count += 1;
        tracing::info!(frame_count, kind, payload_len, "ws reader: recv frame");
        tracing::info!(
            "perf/net: ws recv kind={} payload_len={} read_ms={}",
            kind, payload_len, _perf_read_ms
        );

        if tx.send_blocking(frame).is_err() {
            // Receiver dropped; close the connection.
            tracing::warn!(frame_count, "ws reader: tx send_blocking failed (receiver gone), exiting");
            break;
        }
    }
    tracing::info!(frame_count, "ws reader: loop exited");
}

fn writer_loop(ws: Arc<Mutex<WebSocket<RustlsStream>>>, rx: async_channel::Receiver<WsFrame>) {
    // Diagnostic: count frames seen + sent so we can correlate with
    // the keepalive-side instrumentation in libsignal-service-rs and
    // tell whether queued KA frames actually leave the host.
    let mut frame_count: u64 = 0;
    while let Ok(frame) = rx.recv_blocking() {
        frame_count += 1;
        let (kind, payload_len) = match &frame {
            WsFrame::Binary(b) => ("Binary", b.len()),
            WsFrame::Text(s) => ("Text", s.len()),
            WsFrame::Ping(b) => ("Ping", b.len()),
            WsFrame::Pong(b) => ("Pong", b.len()),
            WsFrame::Close { .. } => ("Close", 0),
        };
        tracing::info!(frame_count, kind, payload_len, "ws writer: dequeued frame");

        let msg = match frame {
            WsFrame::Binary(b) => Message::Binary(b),
            WsFrame::Text(s) => Message::Text(s),
            WsFrame::Ping(b) => Message::Ping(b),
            WsFrame::Pong(b) => Message::Pong(b),
            WsFrame::Close { code, reason } => Message::Close(Some(CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::from(code),
                reason: reason.into(),
            })),
        };

        let _perf_send_start = std::time::Instant::now();
        let send_result = {
            let mut guard = match ws.lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::warn!(frame_count, "ws writer: mutex poisoned, exiting");
                    break;
                }
            };
            guard.send(msg)
        };
        let _perf_send_ms = _perf_send_start.elapsed().as_millis();

        match send_result {
            Ok(()) => {
                tracing::info!(frame_count, kind, payload_len, "ws writer: send ok");
                tracing::info!(
                    "perf/net: ws send kind={} payload_len={} send_ms={}",
                    kind, payload_len, _perf_send_ms
                );
            }
            Err(e) => {
                tracing::warn!(frame_count, kind, ?e, "ws writer: send failed, exiting");
                tracing::info!(
                    "perf/net: ws send_err kind={} payload_len={} send_ms={} err={:?}",
                    kind, payload_len, _perf_send_ms, e
                );
                break;
            }
        }
    }
    tracing::info!(frame_count, "ws writer: loop exited");
}
