//! `PreKeyStore` impl, packed-key strategy.
//!
//! Per `docs/REPORT.md` Decision 1: a single PDDB key
//! (`signal.protocol.{aci,pni}.prekey_bundle["all"]`) holds
//! `Vec<(u32, Vec<u8>)>` — `(prekey_id, serialized_record)` pairs
//! covering every current one-time EC pre-key. Per-key storage would
//! burn one PDDB page per record (per-page AEAD); ~100 pre-keys × ~70
//! bytes packed easily fit in one page.
//!
//! Trade-off: every save/remove rewrites the whole vec. With ~100
//! entries × ~70 bytes that's ~7 KB per write — well under the
//! per-page rewrite cost of one-key-per-id. If the prekey count grows
//! materially we'll reconsider, but Signal's published prekey-replenish
//! count is 100 (`PRE_KEY_BATCH_SIZE`) and the lower-bound is 10
//! (`PRE_KEY_MINIMUM`); so the upper bound is well-behaved.

use async_trait::async_trait;
use presage::libsignal_service::protocol::{
    PreKeyId, PreKeyRecord, PreKeyStore, SignalProtocolError,
};

use super::{PREKEY_BUNDLE_KEY, PddbProtocolStore, dict_prekey_bundle};

/// Load the packed bundle, or an empty vec if the key doesn't exist.
fn load_bundle(store: &PddbProtocolStore) -> Result<Vec<(u32, Vec<u8>)>, SignalProtocolError> {
    let dict = dict_prekey_bundle(store.identity);
    match store
        .store
        .backend
        .get(&dict, PREKEY_BUNDLE_KEY)
        .map_err(backend_err)?
    {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| SignalProtocolError::InvalidState("decode prekey bundle", e.to_string())),
        None => Ok(Vec::new()),
    }
}

fn save_bundle(
    store: &PddbProtocolStore,
    bundle: &[(u32, Vec<u8>)],
) -> Result<(), SignalProtocolError> {
    let dict = dict_prekey_bundle(store.identity);
    let bytes = serde_json::to_vec(bundle)
        .map_err(|e| SignalProtocolError::InvalidState("encode prekey bundle", e.to_string()))?;
    store
        .store
        .backend
        .put(&dict, PREKEY_BUNDLE_KEY, &bytes)
        .map_err(backend_err)
}

#[async_trait(?Send)]
impl PreKeyStore for PddbProtocolStore {
    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord, SignalProtocolError> {
        let bundle = load_bundle(self)?;
        let id: u32 = prekey_id.into();
        bundle
            .into_iter()
            .find(|(rid, _)| *rid == id)
            .ok_or(SignalProtocolError::InvalidPreKeyId)
            .and_then(|(_, bytes)| PreKeyRecord::deserialize(&bytes))
    }

    async fn save_pre_key(
        &mut self,
        prekey_id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        let id: u32 = prekey_id.into();
        let bytes = record.serialize()?;
        let mut bundle = load_bundle(self)?;
        if let Some(existing) = bundle.iter_mut().find(|(rid, _)| *rid == id) {
            existing.1 = bytes;
        } else {
            bundle.push((id, bytes));
        }
        save_bundle(self, &bundle)
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<(), SignalProtocolError> {
        let id: u32 = prekey_id.into();
        let mut bundle = load_bundle(self)?;
        bundle.retain(|(rid, _)| *rid != id);
        save_bundle(self, &bundle)
    }
}

fn backend_err(e: crate::Error) -> SignalProtocolError {
    SignalProtocolError::InvalidState("kv backend", e.to_string())
}

pub(super) fn max_pre_key_id(
    store: &PddbProtocolStore,
) -> Result<Option<u32>, SignalProtocolError> {
    Ok(load_bundle(store)?.iter().map(|(id, _)| *id).max())
}
