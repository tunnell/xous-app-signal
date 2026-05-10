//! libsignal protocol storage traits implemented over `PddbStore`.
//!
//! Six required traits (`IdentityKeyStore`, `PreKeyStore`,
//! `SignedPreKeyStore`, `KyberPreKeyStore`, `SessionStore`,
//! `SenderKeyStore`) plus their `ProtocolStore` blanket, and the three
//! libsignal-service-rs extension traits (`PreKeysStore`,
//! `KyberPreKeyStoreExt`, `SessionStoreExt`) on the same struct.
//!
//! ACI vs PNI is a runtime split, not a type-level one: a single
//! `PddbProtocolStore` carries an `IdentityType` discriminator and
//! routes its dictionary names through it. Same shape
//! `presage-store-sqlite` uses (vendor/presage/presage-store-sqlite/
//! src/protocol.rs:30-44).
//!
//! Dictionary layout:
//!
//! - `signal.protocol.{aci,pni}.session` — per `(uuid, device_id)`,
//!   key = `"{uuid}.{device_id}"`. **Hot.** Backed by an in-memory
//!   dirty-set cache; writes hit `session_cache` only,
//!   `flush_sessions` persists.
//! - `signal.protocol.{aci,pni}.identity` — per `ProtocolAddress`,
//!   key = `"{name}.{device_id}"`.
//! - `signal.protocol.{aci,pni}.prekey_bundle` — single packed key
//!   (`"all"`) holding `Vec<(u32, Vec<u8>)>` of all current one-time
//!   EC pre-keys. Packed because ~100 pre-keys × ~70 bytes is a fraction
//!   of one PDDB page; per-key would burn 100 pages (per-page AEAD).
//! - `signal.protocol.{aci,pni}.signed_prekey` — per id.
//! - `signal.protocol.{aci,pni}.kyber_prekey` — per id; the
//!   `is_last_resort` bit lives inside the JSON-wrapper alongside the
//!   record bytes.
//! - `signal.protocol.{aci,pni}.kyber_meta` — last-resort dedup table.
//!   Key = `"{kyber_id}.{ec_id}"`, value = `base_key.serialize()`.
//! - `signal.protocol.{aci,pni}.sender_key` — per
//!   `(addr_name, device_id, distribution_uuid)`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::PddbStore;

/// ACI vs PNI discriminator. The two protocol stores share their
/// schema — only the dict-name suffix changes.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum IdentityType {
    Aci,
    Pni,
}

impl IdentityType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            IdentityType::Aci => "aci",
            IdentityType::Pni => "pni",
        }
    }
}

impl fmt::Display for IdentityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// PDDB-backed implementation of every libsignal protocol storage
/// trait, parameterised at runtime by `IdentityType`.
///
/// `Clone` is shallow — two clones share the same `PddbStore`
/// (including its session cache) and the same identity discriminator,
/// which is exactly what `presage::Store::{aci,pni}_protocol_store`
/// expects.
#[derive(Clone, Debug)]
pub struct PddbProtocolStore {
    pub(crate) store: PddbStore,
    pub(crate) identity: IdentityType,
}

impl PddbProtocolStore {
    pub(crate) fn new(store: PddbStore, identity: IdentityType) -> Self {
        Self { store, identity }
    }
}

// --- Dictionary name helpers ---

pub(crate) fn dict_session(identity: IdentityType) -> String {
    format!("signal.protocol.{}.session", identity.as_str())
}

pub(crate) fn dict_identity(identity: IdentityType) -> String {
    format!("signal.protocol.{}.identity", identity.as_str())
}

pub(crate) fn dict_prekey_bundle(identity: IdentityType) -> String {
    format!("signal.protocol.{}.prekey_bundle", identity.as_str())
}

pub(crate) fn dict_signed_prekey(identity: IdentityType) -> String {
    format!("signal.protocol.{}.signed_prekey", identity.as_str())
}

pub(crate) fn dict_kyber_prekey(identity: IdentityType) -> String {
    format!("signal.protocol.{}.kyber_prekey", identity.as_str())
}

pub(crate) fn dict_kyber_meta(identity: IdentityType) -> String {
    format!("signal.protocol.{}.kyber_meta", identity.as_str())
}

pub(crate) fn dict_sender_key(identity: IdentityType) -> String {
    format!("signal.protocol.{}.sender_key", identity.as_str())
}

/// Single packed key inside the `prekey_bundle` dict. We never split
/// out per-id keys — the whole vec rewrites on every save/remove.
pub(crate) const PREKEY_BUNDLE_KEY: &str = "all";

mod identity_key_store;
mod kyber_pre_key_store;
mod kyber_pre_key_store_ext;
mod pre_key_store;
mod pre_keys_store;
mod sender_key_store;
pub(crate) mod session_store;
mod session_store_ext;
mod signed_pre_key_store;

/// `ProtocolStore` blanket impl — composes the five required protocol
/// traits (per [`presage/src/store.rs:342-355`](https://github.com/whisperfish/presage/blob/main/presage/src/store.rs#L342-L355)).
impl presage::libsignal_service::protocol::ProtocolStore for PddbProtocolStore {}

/// Convert a `crate::Error` into the `SignalProtocolError` shape that
/// every libsignal protocol-store impl in this crate needs to return.
/// Centralized here so the eight impls don't each redefine it.
pub(crate) fn protocol_backend_err(
    e: crate::Error,
) -> presage::libsignal_service::protocol::SignalProtocolError {
    presage::libsignal_service::protocol::SignalProtocolError::InvalidState(
        "kv backend",
        e.to_string(),
    )
}

/// Protocol-flavored `backend_get_json`: takes a `&'static str` decode
/// context and returns `Result<Option<T>, SignalProtocolError>`. Lets
/// protocol-store impls share the `backend.get + serde_json::from_slice`
/// pattern while keeping their per-callsite error context (e.g.
/// "decode prekey bundle" vs "decode kyber envelope") that the
/// generic `backend_get_json` would replace with the less informative
/// "kv backend: ..." form.
pub(crate) fn backend_get_json_protocol<T: for<'de> serde::Deserialize<'de>>(
    backend: &dyn crate::KvBackend,
    dict: &str,
    key: &str,
    decode_context: &'static str,
) -> Result<Option<T>, presage::libsignal_service::protocol::SignalProtocolError> {
    use presage::libsignal_service::protocol::SignalProtocolError;
    match backend.get(dict, key).map_err(protocol_backend_err)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SignalProtocolError::InvalidState(decode_context, e.to_string())),
        None => Ok(None),
    }
}

/// Protocol-flavored `backend_put_json`: takes a `&'static str` encode
/// context. Inverse of `backend_get_json_protocol`.
pub(crate) fn backend_put_json_protocol<T: serde::Serialize + ?Sized>(
    backend: &dyn crate::KvBackend,
    dict: &str,
    key: &str,
    value: &T,
    encode_context: &'static str,
) -> Result<(), presage::libsignal_service::protocol::SignalProtocolError> {
    use presage::libsignal_service::protocol::SignalProtocolError;
    let bytes = serde_json::to_vec(value)
        .map_err(|e| SignalProtocolError::InvalidState(encode_context, e.to_string()))?;
    backend.put(dict, key, &bytes).map_err(protocol_backend_err)
}
