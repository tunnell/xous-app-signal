//! Sync HTTP/1.1 client implementing `libsignal_service::transport::HttpClient`.
//!
//! Uses our existing `tls_connect` for TLS, hand-rolled HTTP/1.1
//! request/response framing for the body. Avoids pulling `ureq`
//! (which bundles its own rustls and would conflict with our `=0.22.2`
//! pin) and `reqwest` (tokio-coupled).
//!
//! Sync→async bridge: the `HttpClient::execute` trait method is async, but
//! the underlying request runs on a worker thread to keep the executor
//! unblocked. Each request spawns a one-shot thread; the async future
//! awaits a `oneshot`-style `async-channel` that the worker posts to.
//! Acceptable cost for the request rate libsignal-service-rs imposes
//! (single-digit requests per user-action).

use std::io::{Read, Write};
use std::sync::Arc;

use async_trait::async_trait;
use libsignal_service::transport::{
    BasicAuth, HeaderMap, HttpClient, HttpError, HttpRequest, HttpResponse, WebSocketChannels,
};
use rustls::{ClientConfig, RootCertStore};

use crate::tls::{build_tls_config, tls_connect_with_config};

/// Sync HTTP/1.1 client. One-shot connection per request (`Connection: close`);
/// no connection pool — but the underlying `Arc<ClientConfig>` is shared
/// across every HTTP and WebSocket connect, which keeps the rustls
/// in-memory session-ticket cache alive across reconnects (TLS resumption).
pub struct SyncHttpClient {
    config: Arc<ClientConfig>,
    user_agent: String,
    timeout: std::time::Duration,
}

impl SyncHttpClient {
    pub fn new(roots: RootCertStore, user_agent: String) -> Self {
        // Both `execute` (HTTP/1.1) and `connect_websocket` (which upgrades
        // an HTTP/1.1 request) want ALPN "http/1.1", so a single shared
        // config covers all transport in this client.
        let config = build_tls_config(roots, &[b"http/1.1"]);
        Self {
            config,
            user_agent,
            timeout: std::time::Duration::from_secs(65),
        }
    }
}

#[async_trait(?Send)]
impl HttpClient for SyncHttpClient {
    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (tx, rx) = async_channel::bounded(1);
        let config = Arc::clone(&self.config);
        let user_agent = self.user_agent.clone();
        let timeout = req.timeout.unwrap_or(self.timeout);

        // Spawn a one-shot worker thread to run the sync HTTP exchange.
        // The async future blocks on `rx.recv().await` until the worker
        // completes.
        std::thread::Builder::new()
            .name("xous-net-bridge-http".into())
            .spawn(move || {
                let result = sync_execute(req, config, &user_agent, timeout);
                let _ = tx.send_blocking(result);
            })
            .map_err(|e| HttpError::Network(format!("thread spawn: {e}")))?;

        rx.recv()
            .await
            .map_err(|_| HttpError::Network("HTTP worker thread died".to_string()))?
    }

    async fn connect_websocket(
        &self,
        url: url::Url,
        headers: HeaderMap,
        auth: Option<BasicAuth>,
    ) -> Result<WebSocketChannels, HttpError> {
        crate::ws_pump::connect_websocket(Arc::clone(&self.config), url, headers, auth).await
    }
}

/// Run a single HTTP/1.1 request/response cycle synchronously.
fn sync_execute(
    req: HttpRequest,
    config: Arc<ClientConfig>,
    user_agent: &str,
    _timeout: std::time::Duration,
) -> Result<HttpResponse, HttpError> {
    let _perf_start = std::time::Instant::now();
    let host = req
        .url
        .host_str()
        .ok_or_else(|| HttpError::InvalidUrl("missing host".to_string()))?
        .to_string();
    let port = req.url.port_or_known_default().unwrap_or(443);
    let path = match req.url.query() {
        Some(q) => format!("{}?{}", req.url.path(), q),
        None => req.url.path().to_string(),
    };
    let _perf_method = req.method.as_str().to_string();
    let _perf_url = format!("{}", req.url);
    let _perf_body_len = req.body.as_deref().map(|b| b.len()).unwrap_or(0);
    tracing::info!(
        "perf/net: http_req entry method={} url={} body_len={}",
        _perf_method, _perf_url, _perf_body_len
    );

    let _perf_pre_tls = std::time::Instant::now();
    let mut stream = tls_connect_with_config(&host, port, config)
        .map_err(|e| HttpError::Network(format!("tls connect: {e}")))?;
    let _perf_tls_ms = _perf_pre_tls.elapsed().as_millis();
    // Set a read timeout on the underlying TcpStream so a hung server
    // doesn't block the worker thread forever. rustls::StreamOwned exposes
    // `.sock` as the inner Read+Write stream.
    if let Err(e) = stream.sock.set_read_timeout(Some(_timeout)) {
        tracing::debug!("could not set read timeout: {e}");
    }
    // Bound writer's TCP retransmit budget; without this, a server-initiated
    // Close mid-write blocks ~89 s on hardware. Refs #16.
    if let Err(e) = stream.sock.set_write_timeout(Some(_timeout)) {
        tracing::debug!("could not set write timeout: {e}");
    }

    // Construct the request bytes.
    let mut request = Vec::with_capacity(256);
    write!(request, "{} {} HTTP/1.1\r\n", req.method.as_str(), path)
        .map_err(|e| HttpError::Encode(e.to_string()))?;
    write!(request, "Host: {}\r\n", host).map_err(|e| HttpError::Encode(e.to_string()))?;
    write!(request, "User-Agent: {}\r\n", user_agent)
        .map_err(|e| HttpError::Encode(e.to_string()))?;
    write!(request, "Connection: close\r\n").map_err(|e| HttpError::Encode(e.to_string()))?;
    write!(request, "Accept: */*\r\n").map_err(|e| HttpError::Encode(e.to_string()))?;

    if let Some(BasicAuth { username, password }) = req.auth {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let creds = format!("{username}:{password}");
        let encoded = STANDARD.encode(creds.as_bytes());
        write!(request, "Authorization: Basic {encoded}\r\n")
            .map_err(|e| HttpError::Encode(e.to_string()))?;
    }

    for (name, value) in &req.headers {
        let v = value
            .to_str()
            .map_err(|_| HttpError::Encode("header value not ASCII".to_string()))?;
        write!(request, "{}: {}\r\n", name.as_str(), v)
            .map_err(|e| HttpError::Encode(e.to_string()))?;
    }

    let body = req.body.as_deref().unwrap_or(&[]);
    write!(request, "Content-Length: {}\r\n", body.len())
        .map_err(|e| HttpError::Encode(e.to_string()))?;
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(body);

    let _perf_pre_write = std::time::Instant::now();
    stream
        .write_all(&request)
        .map_err(|e| HttpError::Network(format!("write: {e}")))?;
    stream
        .flush()
        .map_err(|e| HttpError::Network(format!("flush: {e}")))?;
    let _perf_write_ms = _perf_pre_write.elapsed().as_millis();

    // Read until EOF (since we sent Connection: close).
    let _perf_pre_read = std::time::Instant::now();
    let mut raw = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut raw)
        .map_err(|e| HttpError::Network(format!("read: {e}")))?;
    let _perf_read_ms = _perf_pre_read.elapsed().as_millis();

    let resp = parse_http_response(&raw);
    let (_perf_status, _perf_resp_body_len) = match &resp {
        Ok(r) => (r.status.as_u16(), r.body.len()),
        Err(_) => (0u16, 0usize),
    };
    tracing::info!(
        "perf/net: http_req exit method={} url={} req_body_len={} status={} resp_body_len={} tls_ms={} write_ms={} read_ms={} total_ms={}",
        _perf_method, _perf_url, _perf_body_len,
        _perf_status, _perf_resp_body_len,
        _perf_tls_ms, _perf_write_ms, _perf_read_ms,
        _perf_start.elapsed().as_millis()
    );
    resp
}

/// Parse a raw HTTP/1.1 response. Handles `Content-Length`-bounded bodies
/// and `Connection: close`-terminated bodies (read-to-EOF). Does not
/// handle chunked transfer-encoding — Signal's chat endpoints are
/// fixed-length per request, so this is sufficient for MVP.
fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, HttpError> {
    let header_end = find_header_end(raw)
        .ok_or_else(|| HttpError::Decode("no header terminator".to_string()))?;
    let header_block = &raw[..header_end];
    let body = raw[header_end + 4..].to_vec();

    let header_text = std::str::from_utf8(header_block)
        .map_err(|_| HttpError::Decode("non-UTF-8 header".to_string()))?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| HttpError::Decode("missing status line".to_string()))?;
    let mut status_parts = status_line.splitn(3, ' ');
    let _http_version = status_parts.next();
    let status_code = status_parts
        .next()
        .ok_or_else(|| HttpError::Decode("missing status code".to_string()))?
        .parse::<u16>()
        .map_err(|e| HttpError::Decode(e.to_string()))?;
    let status =
        http::StatusCode::from_u16(status_code).map_err(|e| HttpError::Decode(e.to_string()))?;

    let mut headers = HeaderMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(":") {
            if let (Ok(n), Ok(v)) = (
                http::HeaderName::try_from(name.trim()),
                http::HeaderValue::try_from(value.trim()),
            ) {
                headers.insert(n, v);
            }
        }
    }

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}
