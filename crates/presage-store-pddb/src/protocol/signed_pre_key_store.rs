//! `SignedPreKeyStore` impl. Per-id, JSON-wrapper-free.
//!
//! Two methods, both per-id storage in
//! `signal.protocol.{aci,pni}.signed_prekey`. Records are stored as
//! their libsignal binary form (`record.serialize()?`) — same shape
//! presage-store-sqlite uses (vendor/presage/presage-store-sqlite/
//! src/protocol.rs:341-360).

use async_trait::async_trait;
use presage::libsignal_service::protocol::{
    GenericSignedPreKey, SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore,
};

use super::{PddbProtocolStore, dict_signed_prekey, protocol_backend_err};

#[async_trait(?Send)]
impl SignedPreKeyStore for PddbProtocolStore {
    async fn get_signed_pre_key(
        &self,
        signed_prekey_id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, SignalProtocolError> {
        let dict = dict_signed_prekey(self.identity);
        let key = u32::from(signed_prekey_id).to_string();
        let bytes = self
            .store
            .backend
            .get(&dict, &key)
            .map_err(protocol_backend_err)?
            .ok_or(SignalProtocolError::InvalidSignedPreKeyId)?;
        SignedPreKeyRecord::deserialize(&bytes)
    }

    async fn save_signed_pre_key(
        &mut self,
        signed_prekey_id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        let dict = dict_signed_prekey(self.identity);
        let key = u32::from(signed_prekey_id).to_string();
        let bytes = record.serialize()?;
        self.store
            .backend
            .put(&dict, &key, &bytes)
            .map_err(protocol_backend_err)
    }
}

pub(super) fn count_signed_pre_keys(
    store: &PddbProtocolStore,
) -> Result<usize, SignalProtocolError> {
    let dict = dict_signed_prekey(store.identity);
    store
        .store
        .backend
        .list_keys(&dict)
        .map(|keys| keys.len())
        .map_err(protocol_backend_err)
}

pub(super) fn max_signed_pre_key_id(
    store: &PddbProtocolStore,
) -> Result<Option<u32>, SignalProtocolError> {
    let dict = dict_signed_prekey(store.identity);
    let keys = store.store.backend.list_keys(&dict).map_err(protocol_backend_err)?;
    Ok(keys.iter().filter_map(|k| k.parse::<u32>().ok()).max())
}
