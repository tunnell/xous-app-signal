//! `SessionStore` impl with in-memory dirty-set cache.
//!
//! Every received message advances the ratchet, which means
//! `store_session` is on the receive hot path. PDDB writes cost at
//! least one 4 KiB page each (per-page AEAD); a write-through impl
//! burns a page per ratchet step. So we buffer.
//!
//! - `store_session` writes to an in-memory `HashMap<SessionKey,
//!   SessionRecord>` and marks the entry dirty. No PDDB write yet.
//! - `load_session` consults the cache first, then PDDB.
//! - `PddbStore::flush_sessions` walks the dirty set and persists.
//!   The receive loop calls this on `Received::QueueEmpty`; tests
//!   call it explicitly to verify durability.
//!
//! Bound on data loss: a power-cut between ratchet step and flush
//! leaves session state slightly behind the peer's view. libsignal
//! handles divergence by re-keying when sends fail (visible to the
//! user as a fresh safety-number prompt). Acceptable trade-off; the
//! alternative (write-through) is too slow for offline-message-burst
//! catch-up.

use async_trait::async_trait;
use std::collections::HashMap;

use presage::libsignal_service::protocol::{
    ProtocolAddress, SessionRecord, SessionStore, SignalProtocolError,
};

use super::{IdentityType, PddbProtocolStore, backend_get_json_protocol, dict_session};

/// Cache key — `(identity, address.name(), device_id)`. The address
/// part is split from the device id so `flush_sessions` can group
/// by address and bundle every device's session into a single PDDB
/// key.
pub(crate) type SessionKey = (IdentityType, String, u32);

pub(crate) fn session_key(identity: IdentityType, address: &ProtocolAddress) -> SessionKey {
    (
        identity,
        address.name().to_string(),
        u32::from(address.device_id()),
    )
}

/// On-disk shape for one PDDB session key: `device_id ->
/// SessionRecord::serialize() bytes`. JSON for debuggability —
/// matches the choice in `pre_key_store::save_bundle`.
pub(crate) type SessionBundle = HashMap<u32, Vec<u8>>;

#[async_trait(?Send)]
impl SessionStore for PddbProtocolStore {
    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, SignalProtocolError> {
        let key = session_key(self.identity, address);

        // 1. Cache hit (most-recent ratchet state lives here until flush).
        {
            let cache = self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
            if let Some(rec) = cache.get(&key) {
                return Ok(Some(rec.clone()));
            }
        }

        // 2. Fall through to PDDB. One key per address; the value is a
        //    `SessionBundle` (device_id → serialized SessionRecord).
        let dict = dict_session(self.identity);
        let Some(bundle) = backend_get_json_protocol::<SessionBundle>(
            &*self.store.backend,
            &dict,
            &key.1,
            "decode session bundle",
        )?
        else {
            return Ok(None);
        };

        // Populate the cache with every device's record so a follow-up
        // `load_session` for a sibling device skips PDDB.
        {
            let mut cache = self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
            for (dev_id, ser) in &bundle {
                let cache_key = (self.identity, key.1.clone(), *dev_id);
                if !cache.contains_key(&cache_key) {
                    cache.insert(cache_key, SessionRecord::deserialize(ser)?);
                }
            }
        }

        match bundle.get(&key.2) {
            Some(ser) => SessionRecord::deserialize(ser).map(Some),
            None => Ok(None),
        }
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), SignalProtocolError> {
        let key = session_key(self.identity, address);
        let mut cache =
            self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
        cache.insert(key.clone(), record.clone());
        let mut dirty =
            self.store.session_dirty.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session dirty", "poisoned".into())
            })?;
        dirty.insert(key);
        Ok(())
    }
}
