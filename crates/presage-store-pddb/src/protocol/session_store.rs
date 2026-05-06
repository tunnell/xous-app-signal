//! `SessionStore` impl with in-memory dirty-set cache.
//!
//! Per `docs/REPORT.md` Decision 5: every received message advances
//! the ratchet, which means `store_session` is on the receive hot
//! path. PDDB writes cost at least one 4 KiB page each (per-page
//! AEAD); a write-through impl burns a page per ratchet step. So we
//! buffer.
//!
//! - `store_session` writes to an in-memory `HashMap<SessionKey,
//!   SessionRecord>` and marks the entry dirty. No PDDB write yet.
//! - `load_session` consults the cache first, then PDDB.
//! - `PddbStore::flush_sessions` walks the dirty set and persists. The
//!   Stage 11 receive loop calls this on `Received::QueueEmpty`; tests
//!   call it explicitly to verify durability.
//!
//! Bound on data loss: a power-cut between ratchet step and flush
//! leaves session state slightly behind the peer's view. libsignal
//! handles divergence by re-keying when sends fail (visible to the
//! user as a fresh safety-number prompt). Acceptable trade-off per
//! Decision 5; the alternative (write-through) is too slow for
//! offline-message-burst catch-up.

use async_trait::async_trait;
use presage::libsignal_service::protocol::{
    ProtocolAddress, SessionRecord, SessionStore, SignalProtocolError,
};

use super::{IdentityType, PddbProtocolStore, dict_session};

/// Cache key — identity disambiguator + flat address-string. Same
/// shape used as the on-disk PDDB key for clean cache → flush
/// translation.
pub(crate) type SessionKey = (IdentityType, String);

pub(crate) fn session_key(identity: IdentityType, address: &ProtocolAddress) -> SessionKey {
    (
        identity,
        format!("{}.{}", address.name(), u32::from(address.device_id())),
    )
}

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

        // 2. Fall through to PDDB.
        let dict = dict_session(self.identity);
        match self.store.backend.get(&dict, &key.1).map_err(backend_err)? {
            Some(bytes) => SessionRecord::deserialize(&bytes).map(Some),
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

fn backend_err(e: crate::Error) -> SignalProtocolError {
    SignalProtocolError::InvalidState("kv backend", e.to_string())
}
