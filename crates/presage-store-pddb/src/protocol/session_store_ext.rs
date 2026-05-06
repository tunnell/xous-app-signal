//! `SessionStoreExt` — bulk session deletion methods. Walks both the
//! in-memory cache and PDDB so deletions land in both stores.
//!
//! 4 user-facing methods (the 5th, `compute_safety_number`, has a
//! default impl in the trait). `delete_service_addr_device_session`
//! also has a default impl that we let through.

use async_trait::async_trait;
use presage::libsignal_service::prelude::SessionStoreExt;
use presage::libsignal_service::protocol::{
    DeviceId, ProtocolAddress, ServiceId, SignalProtocolError,
};
use presage::libsignal_service::push_service::DEFAULT_DEVICE_ID;

use super::{PddbProtocolStore, dict_session, session_store::session_key};

#[async_trait(?Send)]
impl SessionStoreExt for PddbProtocolStore {
    async fn get_sub_device_sessions(
        &self,
        name: &ServiceId,
    ) -> Result<Vec<DeviceId>, SignalProtocolError> {
        let uuid = name.raw_uuid().to_string();
        let main: u32 = u32::from(*DEFAULT_DEVICE_ID);

        // Combine cache (entries not yet flushed) with PDDB
        // (already-persisted entries). Same `(addr, device_id)` may
        // appear in both — we dedup the device-id vec at the end.
        let mut device_ids: Vec<u32> = Vec::new();

        {
            let cache = self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
            for ((id_kind, k), _) in cache.iter() {
                if *id_kind != self.identity {
                    continue;
                }
                if let Some((addr, dev_str)) = k.rsplit_once('.') {
                    if addr == uuid
                        && let Ok(dev) = dev_str.parse::<u32>()
                        && dev != main
                    {
                        device_ids.push(dev);
                    }
                }
            }
        }

        let dict = dict_session(self.identity);
        let keys = self.store.backend.list_keys(&dict).map_err(backend_err)?;
        for k in keys {
            if let Some((addr, dev_str)) = k.rsplit_once('.') {
                if addr == uuid
                    && let Ok(dev) = dev_str.parse::<u32>()
                    && dev != main
                {
                    device_ids.push(dev);
                }
            }
        }

        device_ids.sort_unstable();
        device_ids.dedup();
        Ok(device_ids
            .into_iter()
            .filter_map(|d| DeviceId::try_from(d).ok())
            .collect())
    }

    async fn delete_session(&self, address: &ProtocolAddress) -> Result<(), SignalProtocolError> {
        let key = session_key(self.identity, address);
        {
            let mut cache = self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
            cache.remove(&key);
        }
        {
            let mut dirty = self.store.session_dirty.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session dirty", "poisoned".into())
            })?;
            dirty.remove(&key);
        }
        let dict = dict_session(self.identity);
        self.store
            .backend
            .delete(&dict, &key.1)
            .map_err(backend_err)
    }

    async fn delete_all_sessions(&self, address: &ServiceId) -> Result<usize, SignalProtocolError> {
        // Count UNIQUE `(uuid, device_id)` entries removed across both
        // cache and PDDB. An entry that lives in both places is one
        // session, not two — without dedup, sessions present in both
        // double-count.
        use std::collections::HashSet;

        let uuid = address.raw_uuid().to_string();
        let dict = dict_session(self.identity);
        let mut affected: HashSet<String> = HashSet::new();

        // Drop any cache entries for this address.
        {
            let mut cache = self.store.session_cache.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session cache", "poisoned".into())
            })?;
            let mut dirty = self.store.session_dirty.lock().map_err(|_| {
                SignalProtocolError::InvalidState("session dirty", "poisoned".into())
            })?;
            cache.retain(|(id_kind, k), _| {
                if *id_kind != self.identity {
                    return true;
                }
                let matches_addr = k.rsplit_once('.').is_some_and(|(addr, _)| addr == uuid);
                if matches_addr {
                    affected.insert(k.clone());
                    dirty.remove(&(*id_kind, k.clone()));
                }
                !matches_addr
            });
        }

        // Drop any persisted entries.
        let keys = self.store.backend.list_keys(&dict).map_err(backend_err)?;
        for k in keys {
            if let Some((addr, _)) = k.rsplit_once('.') {
                if addr == uuid {
                    self.store.backend.delete(&dict, &k).map_err(backend_err)?;
                    affected.insert(k);
                }
            }
        }

        Ok(affected.len())
    }
}

fn backend_err(e: crate::Error) -> SignalProtocolError {
    SignalProtocolError::InvalidState("kv backend", e.to_string())
}
