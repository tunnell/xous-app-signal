//! Error type for `presage-store-pddb`.
//!
//! Implements `presage::store::StoreError`. Variants cover the three
//! places a store call can fail: the underlying KV backend (PDDB or the
//! mock HashMap), serde encode/decode, and libsignal protocol round-trips
//! for the binary IdentityKeyPair / SenderCertificate fields.

use presage::store::StoreError as PresageStoreError;

/// Errors raised by `PddbStore`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A `KvBackend` operation failed (PDDB I/O, mock-backend lock
    /// poisoning, etc.). The string is the backend's own error rendering.
    #[error("kv backend: {0}")]
    Backend(String),

    /// `serde_json` failed to serialize a value before storing it.
    #[error("encode: {0}")]
    Encode(String),

    /// `serde_json` failed to deserialize a stored value, or a libsignal
    /// `deserialize`/`from_slice` call rejected raw bytes from disk.
    #[error("decode: {0}")]
    Decode(String),

    /// A libsignal protocol error — surfaced from
    /// `IdentityKeyPair::deserialize`, `SenderCertificate::deserialize`,
    /// `SenderCertificate::serialized()`.
    #[error("protocol: {0}")]
    Protocol(#[from] presage::libsignal_service::protocol::SignalProtocolError),
}

impl PresageStoreError for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        // serde_json conflates encode and decode errors in one type.
        // Keep both variants on our side — the From impl picks `Decode`
        // because that's the more common direction when round-tripping
        // through the store; encode-side callsites use
        // `.map_err(Error::encode)` explicitly.
        Error::Decode(e.to_string())
    }
}

impl Error {
    pub(crate) fn encode<E: std::fmt::Display>(e: E) -> Self {
        Error::Encode(e.to_string())
    }

    pub(crate) fn backend<E: std::fmt::Display>(e: E) -> Self {
        Error::Backend(e.to_string())
    }
}
