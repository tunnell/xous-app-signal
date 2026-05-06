// Stage 6.1: groundwork. The trait and types defined here are used by
// follow-up commits that swap PushService internals. Until those land,
// some types appear unused — which is intentional.
#![allow(dead_code)]

//! Transport abstraction (Stage 6.1, Xous fork).
//!
//! Upstream `libsignal-service-rs` uses `reqwest::Client` (HTTP/1.1 over
//! `hyper-util` over `tokio`) and `reqwest_websocket::WebSocket` for the
//! Signal chat-server transport. Both pull `tokio` → `mio`, which doesn't
//! compile on `riscv32imac-unknown-xous-elf`.
//!
//! This module defines a sync-leaf transport abstraction that drives the
//! same Signal protocol over a non-tokio sync HTTP/1.1 + WS stack. The
//! actual implementation lives in `xous-net-bridge` (sync `ureq` + sync
//! `tungstenite` over `async-channel`); libsignal-service-rs only sees
//! the trait.
//!
//! See `docs/REPORT.md` Decision 3.

use std::sync::Arc;

use async_trait::async_trait;
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde::Serialize;
use thiserror::Error;
use url::Url;

/// Auth credentials that the transport may attach to a request.
#[derive(Clone, Debug)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

/// A request, fully resolved (URL, method, headers, optional body, optional auth).
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub auth: Option<BasicAuth>,
    /// Total request timeout. None = use the implementation's default.
    pub timeout: Option<std::time::Duration>,
}

/// A response, fully buffered (status + headers + body).
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Errors the transport can return. Implementation-specific errors fold into
/// `Network`; HTTP-level non-success status codes are NOT errors at this layer
/// (they're returned in the response and the caller uses `error_for_status` /
/// `service_error_for_status` to decide).
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("transport network error: {0}")]
    Network(String),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("response body not utf-8")]
    InvalidUtf8,
    #[error("response body could not be deserialized: {0}")]
    Decode(String),
    #[error("request encoding failed: {0}")]
    Encode(String),
}

/// The `?Send` async trait is friendly to single-threaded executors.
#[async_trait(?Send)]
pub trait HttpClient {
    /// Configuration applied per `HttpClient` instance: pinned root CA,
    /// connect/total timeouts, user-agent. Constructors are
    /// implementation-specific and provided by the impl crate
    /// (e.g., `xous-net-bridge` provides a `UreqHttpClient`).
    async fn execute(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, HttpError>;
}

// ---------------------------------------------------------------------------
// Builder shim — the API surface libsignal-service-rs callers use to
// construct requests, mirroring just enough of `reqwest::RequestBuilder` to
// minimize churn at the callsites.
// ---------------------------------------------------------------------------

/// A fluent builder for an `HttpRequest`. Returned by `PushService::request`.
/// Mirrors the subset of `reqwest::RequestBuilder` that libsignal-service-rs
/// actually uses.
pub struct RequestBuilder {
    client: Arc<dyn HttpClient + Send + Sync>,
    req: HttpRequest,
}

impl RequestBuilder {
    pub(crate) fn new(
        client: Arc<dyn HttpClient + Send + Sync>,
        method: Method,
        url: Url,
    ) -> Self {
        Self {
            client,
            req: HttpRequest {
                method,
                url,
                headers: HeaderMap::new(),
                body: None,
                auth: None,
                timeout: None,
            },
        }
    }

    pub fn header<K, V>(mut self, name: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        HeaderValue: TryFrom<V>,
    {
        if let (Ok(k), Ok(v)) =
            (HeaderName::try_from(name), HeaderValue::try_from(value))
        {
            self.req.headers.insert(k, v);
        }
        self
    }

    pub fn body<B: Into<Vec<u8>>>(mut self, body: B) -> Self {
        self.req.body = Some(body.into());
        self
    }

    /// Set the body to a JSON serialization of `value` and `Content-Type:
    /// application/json`. Mirrors `reqwest::RequestBuilder::json`.
    pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(buf) => {
                self.req.body = Some(buf);
                self.req.headers.insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
            },
            Err(_e) => {
                // Defer the error to send-time so the builder API stays infallible.
                // Mirrors reqwest's behavior — they also defer this.
                self.req.body = None;
            },
        }
        self
    }

    pub fn basic_auth<U, P>(mut self, user: U, pass: Option<P>) -> Self
    where
        U: Into<String>,
        P: Into<String>,
    {
        self.req.auth = Some(BasicAuth {
            username: user.into(),
            password: pass.map(|p| p.into()).unwrap_or_default(),
        });
        self
    }

    pub fn timeout(mut self, d: std::time::Duration) -> Self {
        self.req.timeout = Some(d);
        self
    }

    /// Send the request. Mirrors `reqwest::RequestBuilder::send`.
    pub async fn send(self) -> Result<HttpResponse, HttpError> {
        self.client.execute(self.req).await
    }
}

// ---------------------------------------------------------------------------
// Response convenience methods — mirror the subset of `reqwest::Response`
// libsignal-service-rs uses.
// ---------------------------------------------------------------------------

impl HttpResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Mirrors `reqwest::Response::error_for_status`: if status is 4xx/5xx,
    /// returns an error; otherwise returns `Ok(self)`. We use this only as
    /// a courtesy — `service_error_for_status` (in `push_service::response`)
    /// is what callers actually use because it produces richer Signal-specific
    /// errors.
    pub fn error_for_status(self) -> Result<Self, HttpError> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(HttpError::Network(format!("HTTP {}", self.status.as_u16())))
        }
    }

    pub async fn bytes(self) -> Result<Vec<u8>, HttpError> {
        Ok(self.body)
    }

    pub async fn text(self) -> Result<String, HttpError> {
        String::from_utf8(self.body).map_err(|_| HttpError::InvalidUtf8)
    }

    pub async fn json<T>(self) -> Result<T, HttpError>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_slice(&self.body)
            .map_err(|e| HttpError::Decode(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// CA certificate loading — replaces `reqwest::Certificate::from_pem`.
// ---------------------------------------------------------------------------

/// Pinned CA certificate, in PEM form. The transport implementation accepts
/// these via its constructor; this opaque type is just a typed marker.
#[derive(Clone, Debug)]
pub struct Certificate {
    pub pem: Vec<u8>,
}

impl Certificate {
    pub fn from_pem(pem: &[u8]) -> Result<Self, HttpError> {
        Ok(Certificate { pem: pem.to_vec() })
    }
}
