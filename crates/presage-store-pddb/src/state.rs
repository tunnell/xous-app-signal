//! `presage::store::StateStore` implementation for `PddbStore`.
//!
//! All state lives in a single PDDB dictionary, `signal.state`, with
//! one key per field. This matches presage-store-sqlite's `kv` table
//! layout. Per-field keys keep reads cheap and let `clear_registration`
//! walk the dict instead of editing a giant blob.
//!
//! Serialization choices:
//!
//! - `RegistrationData` — `serde_json` (debuggable from a `pddbcli`
//!   dump). Same choice presage-store-sqlite makes.
//! - `IdentityKeyPair` — `.serialize()` to a libsignal-defined binary
//!   form, `IdentityKeyPair::try_from(&[u8])` to read. Matches
//!   presage-store-sqlite's pattern.
//! - `SenderCertificate` — `.serialized()? -> Vec<u8>` /
//!   `SenderCertificate::deserialize(&[u8])`. Protocol-defined wire
//!   format; do not reframe.
//! - `MasterKey` — store the 32 raw bytes from `master_key.inner`;
//!   `MasterKey::from_slice` to read.

use presage::libsignal_service::prelude::MasterKey;
use presage::libsignal_service::protocol::{IdentityKeyPair, SenderCertificate};
use presage::manager::RegistrationData;
use presage::store::StateStore;

use crate::{Error, PddbStore};

/// PDDB dictionary that holds all `StateStore` keys.
const DICT: &str = "signal.state";

/// One key per `StateStore` field. The keys are short, opaque strings —
/// pddbcli readers will see them with their data when dumping the dict.
const KEY_REGISTRATION: &str = "registration";
const KEY_ACI_IDENTITY_KEY_PAIR: &str = "aci_identity_key_pair";
const KEY_PNI_IDENTITY_KEY_PAIR: &str = "pni_identity_key_pair";
const KEY_SENDER_CERTIFICATE: &str = "sender_certificate";
const KEY_MASTER_KEY: &str = "master_key";

impl StateStore for PddbStore {
    type StateStoreError = Error;

    async fn load_registration_data(&self) -> Result<Option<RegistrationData>, Error> {
        match self.backend.get(DICT, KEY_REGISTRATION)? {
            Some(bytes) => {
                let data: RegistrationData = serde_json::from_slice(&bytes)?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    async fn set_aci_identity_key_pair(&self, key_pair: IdentityKeyPair) -> Result<(), Error> {
        let bytes = key_pair.serialize();
        self.backend.put(DICT, KEY_ACI_IDENTITY_KEY_PAIR, &bytes)?;
        Ok(())
    }

    async fn set_pni_identity_key_pair(&self, key_pair: IdentityKeyPair) -> Result<(), Error> {
        let bytes = key_pair.serialize();
        self.backend.put(DICT, KEY_PNI_IDENTITY_KEY_PAIR, &bytes)?;
        Ok(())
    }

    async fn save_registration_data(&mut self, state: &RegistrationData) -> Result<(), Error> {
        let bytes = serde_json::to_vec(state).map_err(Error::encode)?;
        self.backend.put(DICT, KEY_REGISTRATION, &bytes)?;
        Ok(())
    }

    async fn sender_certificate(&self) -> Result<Option<SenderCertificate>, Error> {
        match self.backend.get(DICT, KEY_SENDER_CERTIFICATE)? {
            Some(bytes) => Ok(Some(SenderCertificate::deserialize(&bytes)?)),
            None => Ok(None),
        }
    }

    async fn save_sender_certificate(&self, certificate: &SenderCertificate) -> Result<(), Error> {
        let bytes = certificate.serialized()?;
        self.backend.put(DICT, KEY_SENDER_CERTIFICATE, bytes)?;
        Ok(())
    }

    async fn is_registered(&self) -> bool {
        // Same pattern as presage-store-sqlite: presence of the
        // registration blob is the source of truth. Errors collapse to
        // "not registered" rather than panicking — this matches the
        // trait, which doesn't return Result here.
        self.load_registration_data().await.ok().flatten().is_some()
    }

    async fn clear_registration(&mut self) -> Result<(), Error> {
        // Drops every key in the state dict — registration, both
        // identity key pairs, sender certificate, master key. The
        // protocol stores live in their own dicts and aren't touched
        // here; presage's `Store::clear` chains both.
        self.backend.delete_dict(DICT)?;
        Ok(())
    }

    async fn fetch_master_key(&self) -> Result<Option<MasterKey>, Error> {
        match self.backend.get(DICT, KEY_MASTER_KEY)? {
            Some(bytes) => MasterKey::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Error::Decode(format!("master key length: {e}"))),
            None => Ok(None),
        }
    }

    async fn store_master_key(&self, master_key: Option<&MasterKey>) -> Result<(), Error> {
        match master_key {
            Some(k) => self.backend.put(DICT, KEY_MASTER_KEY, &k.inner)?,
            None => self.backend.delete(DICT, KEY_MASTER_KEY)?,
        }
        Ok(())
    }
}
