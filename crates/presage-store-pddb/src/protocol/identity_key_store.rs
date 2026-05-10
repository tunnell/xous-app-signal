//! `IdentityKeyStore` impl. Five methods.
//!
//! - `get_identity_key_pair`: pulled from the StateStore-side
//!   `signal.state["{aci|pni}_identity_key_pair"]` key written by
//!   `set_aci_identity_key_pair` / `set_pni_identity_key_pair`. The
//!   bytes are libsignal's binary `IdentityKeyPair::serialize()`
//!   format.
//! - `get_local_registration_id`: comes from the registration blob
//!   loaded by `StateStore::load_registration_data`. Same source the
//!   sqlite store uses.
//! - `save_identity` / `is_trusted_identity` / `get_identity`: per-
//!   `ProtocolAddress` keys in `signal.protocol.{aci,pni}.identity`.

use async_trait::async_trait;
use presage::libsignal_service::protocol::{
    Direction, IdentityChange, IdentityKey, IdentityKeyPair, IdentityKeyStore, ProtocolAddress,
    SignalProtocolError,
};
use presage::store::StateStore;
use tracing::warn;

use super::{PddbProtocolStore, dict_identity, protocol_backend_err};

/// Key inside `signal.state` holding the ACI/PNI identity keypair —
/// matches the constants in `state.rs` (kept in sync deliberately;
/// this trait sits across the StateStore/ProtocolStore boundary).
const STATE_DICT: &str = "signal.state";
fn state_key_identity_key_pair(identity: super::IdentityType) -> &'static str {
    match identity {
        super::IdentityType::Aci => "aci_identity_key_pair",
        super::IdentityType::Pni => "pni_identity_key_pair",
    }
}

fn identity_key(address: &ProtocolAddress) -> String {
    format!("{}.{}", address.name(), u32::from(address.device_id()))
}

#[async_trait(?Send)]
impl IdentityKeyStore for PddbProtocolStore {
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, SignalProtocolError> {
        let key = state_key_identity_key_pair(self.identity);
        let bytes = self
            .store
            .backend
            .get(STATE_DICT, key)
            .map_err(protocol_backend_err)?
            .ok_or_else(|| {
                SignalProtocolError::InvalidState(
                    "get_identity_key_pair",
                    format!("no {key} stored"),
                )
            })?;
        IdentityKeyPair::try_from(&bytes[..])
    }

    async fn get_local_registration_id(&self) -> Result<u32, SignalProtocolError> {
        let data = self
            .store
            .load_registration_data()
            .await
            .map_err(|e| {
                SignalProtocolError::InvalidState("get_local_registration_id", e.to_string())
            })?
            .ok_or_else(|| {
                SignalProtocolError::InvalidState(
                    "get_local_registration_id",
                    "no registration data".into(),
                )
            })?;
        Ok(data.registration_id)
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<IdentityChange, SignalProtocolError> {
        let dict = dict_identity(self.identity);
        let key = identity_key(address);
        let new_bytes = identity.serialize();

        let prior = self
            .store
            .backend
            .get(&dict, &key)
            .map_err(protocol_backend_err)?;
        let changed = matches!(prior, Some(ref old) if old.as_slice() != new_bytes.as_ref());

        self.store
            .backend
            .put(&dict, &key, &new_bytes)
            .map_err(protocol_backend_err)?;

        Ok(IdentityChange::from_changed(changed))
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> Result<bool, SignalProtocolError> {
        match self.get_identity(address).await? {
            // Same TOFU policy presage-store-sqlite uses: known-and-equal
            // is trusted; known-and-different defers to the store-level
            // `trust_new_identities` flag (`Trust` accepts the change,
            // `Reject` rejects).
            Some(stored) if &stored == identity => Ok(true),
            Some(_) => Ok(matches!(
                self.store.trust_new_identities,
                presage::model::identity::OnNewIdentity::Trust
            )),
            None => {
                warn!(?address, "trusting new identity (TOFU)");
                Ok(true)
            }
        }
    }

    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, SignalProtocolError> {
        let dict = dict_identity(self.identity);
        let key = identity_key(address);
        match self
            .store
            .backend
            .get(&dict, &key)
            .map_err(protocol_backend_err)?
        {
            Some(bytes) => Ok(Some(IdentityKey::decode(&bytes)?)),
            None => Ok(None),
        }
    }
}
