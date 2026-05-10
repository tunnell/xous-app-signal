//! `KyberPreKeyStore` impl. Three methods.
//!
//! Per-id binary records in `signal.protocol.{aci,pni}.kyber_prekey`.
//! Each record on disk is a JSON envelope `{record: <bytes>,
//! is_last_resort: bool}` so that `mark_kyber_pre_key_used` and the
//! `KyberPreKeyStoreExt` trait know whether to delete or retain —
//! same flag the sqlite store carries as a column
//! (vendor/presage/presage-store-sqlite/src/protocol.rs:407-470).
//!
//! `mark_kyber_pre_key_used` consults a separate `kyber_meta` dict for
//! last-resort base-key dedup. Key = `"{kyber_id}.{ec_id}"`, value =
//! `base_key.serialize()` bytes. Inserting the same triple twice is the
//! "reused base key" error case.

use async_trait::async_trait;
use presage::libsignal_service::protocol::{
    CiphertextMessageType, GenericSignedPreKey, KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore,
    PublicKey, SignalProtocolError, SignedPreKeyId,
};
use serde::{Deserialize, Serialize};

use crate::list_keys_as_u32s;

use super::{PddbProtocolStore, dict_kyber_meta, dict_kyber_prekey, protocol_backend_err};

/// Wire envelope for a stored Kyber pre-key. JSON-encoded. The
/// `record` bytes are libsignal's binary `KyberPreKeyRecord::serialize`
/// output; we don't reframe.
#[derive(Serialize, Deserialize)]
pub(super) struct KyberStored {
    pub(super) record: Vec<u8>,
    pub(super) is_last_resort: bool,
}

pub(super) fn store_envelope(
    proto: &PddbProtocolStore,
    id: KyberPreKeyId,
    record: &KyberPreKeyRecord,
    is_last_resort: bool,
) -> Result<(), SignalProtocolError> {
    let dict = dict_kyber_prekey(proto.identity);
    let key = u32::from(id).to_string();
    let envelope = KyberStored {
        record: record.serialize()?,
        is_last_resort,
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|e| SignalProtocolError::InvalidState("encode kyber envelope", e.to_string()))?;
    proto
        .store
        .backend
        .put(&dict, &key, &bytes)
        .map_err(protocol_backend_err)
}

pub(super) fn load_envelope(
    proto: &PddbProtocolStore,
    id: KyberPreKeyId,
) -> Result<Option<KyberStored>, SignalProtocolError> {
    let dict = dict_kyber_prekey(proto.identity);
    let key = u32::from(id).to_string();
    match proto.store.backend.get(&dict, &key).map_err(protocol_backend_err)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SignalProtocolError::InvalidState("decode kyber envelope", e.to_string())),
        None => Ok(None),
    }
}

#[async_trait(?Send)]
impl KyberPreKeyStore for PddbProtocolStore {
    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, SignalProtocolError> {
        let envelope = load_envelope(self, kyber_prekey_id)?
            .ok_or(SignalProtocolError::InvalidKyberPreKeyId)?;
        KyberPreKeyRecord::deserialize(&envelope.record)
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        store_envelope(
            self,
            kyber_prekey_id,
            record,
            /* is_last_resort */ false,
        )
    }

    async fn mark_kyber_pre_key_used(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        ec_prekey_id: SignedPreKeyId,
        base_key: &PublicKey,
    ) -> Result<(), SignalProtocolError> {
        let envelope = load_envelope(self, kyber_prekey_id)?
            .ok_or(SignalProtocolError::InvalidKyberPreKeyId)?;

        if envelope.is_last_resort {
            // Last-resort: dedup against (kyber_id, ec_id, base_key).
            // Reusing the same triple is a protocol violation —
            // `InvalidMessage(PreKey, "reused base key")` matches sqlite.
            let dict = dict_kyber_meta(self.identity);
            let key = format!("{}.{}", u32::from(kyber_prekey_id), u32::from(ec_prekey_id));
            let new_base_bytes = base_key.serialize();

            if let Some(prior) = self.store.backend.get(&dict, &key).map_err(protocol_backend_err)? {
                if prior == new_base_bytes.as_ref() {
                    return Err(SignalProtocolError::InvalidMessage(
                        CiphertextMessageType::PreKey,
                        "reused base key",
                    ));
                }
            }
            self.store
                .backend
                .put(&dict, &key, &new_base_bytes)
                .map_err(protocol_backend_err)?;
        } else {
            // One-time: delete the prekey outright.
            let dict = dict_kyber_prekey(self.identity);
            let key = u32::from(kyber_prekey_id).to_string();
            self.store
                .backend
                .delete(&dict, &key)
                .map_err(protocol_backend_err)?;
        }

        Ok(())
    }
}

pub(super) fn count_kyber_pre_keys(
    store: &PddbProtocolStore,
    last_resort_only: bool,
) -> Result<usize, SignalProtocolError> {
    let dict = dict_kyber_prekey(store.identity);
    let ids = list_keys_as_u32s(&*store.store.backend, &dict).map_err(protocol_backend_err)?;
    if !last_resort_only {
        return Ok(ids.len());
    }
    let mut count = 0_usize;
    for id in ids {
        if let Some(env) = load_envelope(store, KyberPreKeyId::from(id))? {
            if env.is_last_resort {
                count += 1;
            }
        }
    }
    Ok(count)
}

pub(super) fn max_kyber_pre_key_id(
    store: &PddbProtocolStore,
) -> Result<Option<u32>, SignalProtocolError> {
    let dict = dict_kyber_prekey(store.identity);
    let ids = list_keys_as_u32s(&*store.store.backend, &dict).map_err(protocol_backend_err)?;
    Ok(ids.into_iter().max())
}

pub(super) fn max_last_resort_kyber_pre_key_id(
    store: &PddbProtocolStore,
) -> Result<Option<u32>, SignalProtocolError> {
    let dict = dict_kyber_prekey(store.identity);
    let ids = list_keys_as_u32s(&*store.store.backend, &dict).map_err(protocol_backend_err)?;
    let mut best: Option<u32> = None;
    for id in ids {
        if let Some(env) = load_envelope(store, KyberPreKeyId::from(id))? {
            if env.is_last_resort {
                best = Some(best.map_or(id, |b| b.max(id)));
            }
        }
    }
    Ok(best)
}
