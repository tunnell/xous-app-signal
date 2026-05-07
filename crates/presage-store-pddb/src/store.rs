//! `presage::store::Store` blanket — combines `StateStore`,
//! `ContentsStore`, and the ACI/PNI protocol-store accessors.

use presage::store::{ContentsStore, StateStore, Store};

use crate::{Error, IdentityType, PddbProtocolStore, PddbStore};

impl Store for PddbStore {
    type Error = Error;
    type AciStore = PddbProtocolStore;
    type PniStore = PddbProtocolStore;

    /// Clear *everything* — state, profiles, every protocol-store dict
    /// (both ACI and PNI), and the in-memory session cache. Per-impl
    /// `clear_*` methods are wired up here.
    async fn clear(&mut self) -> Result<(), <Self as StateStore>::StateStoreError> {
        // Wipe registration data + identity keypairs + sender cert +
        // master key.
        self.clear_registration().await?;
        // Wipe all messages, contacts, groups, profiles, sticker packs.
        self.clear_contents().await?;
        // Wipe both protocol-store dictionaries (sessions, identities,
        // pre-keys, signed pre-keys, kyber pre-keys, sender keys).
        for identity in [IdentityType::Aci, IdentityType::Pni] {
            self.backend
                .delete_dict(&crate::protocol::dict_session(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_identity(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_prekey_bundle(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_signed_prekey(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_kyber_prekey(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_kyber_meta(identity))?;
            self.backend
                .delete_dict(&crate::protocol::dict_sender_key(identity))?;
        }

        // Drop in-memory session state — otherwise a flush after
        // `clear` would re-persist it.
        if let Ok(mut cache) = self.session_cache.lock() {
            cache.clear();
        }
        if let Ok(mut dirty) = self.session_dirty.lock() {
            dirty.clear();
        }

        Ok(())
    }

    fn aci_protocol_store(&self) -> Self::AciStore {
        PddbProtocolStore::new(self.clone(), IdentityType::Aci)
    }

    fn pni_protocol_store(&self) -> Self::PniStore {
        PddbProtocolStore::new(self.clone(), IdentityType::Pni)
    }
}
