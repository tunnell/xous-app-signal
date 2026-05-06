//! PDDB-backed implementation of `presage`'s storage traits.
//!
//! Per `docs/REPORT.md` Decision 1 (storage layout), the trait impls
//! sit on top of an internal `KvBackend` abstraction:
//!
//! - `KvBackend` exposes `get / put / delete / delete_dict / list_keys`
//!   keyed on `(dict_name, key_name)` — the same shape PDDB itself
//!   exposes. The Stage 8 `PddbBackend` implementation forwards these
//!   into the `pddb::Pddb` API; the Stage 4 `MockBackend` is an
//!   in-memory `HashMap` for hosted-mode testing.
//!
//! - `PddbStore` owns an `Arc<dyn KvBackend>` and implements the
//!   presage traits over it. `Clone + Send + Sync + 'static`, the
//!   bound demanded by `presage::store::Store`.
//!
//! Stage 4 only fills in `StateStore` (10 methods) and the
//! `ContentsStore::profile` / `save_profile` round-trip path. The
//! remaining `ContentsStore` methods compile as `unimplemented!`-stubs
//! (Stage 5c work). The libsignal protocol stores (Stage 5a) and
//! libsignal-service-rs extension traits (Stage 5b) are not implemented
//! here yet.

#![deny(missing_debug_implementations)]

use std::fmt;
use std::sync::Arc;

mod backend_mock;
mod content;
mod error;
mod state;

pub use backend_mock::MockBackend;
pub use error::Error;

/// Internal KV abstraction used by all `PddbStore` trait impls.
///
/// The `(dict, key)` shape mirrors PDDB's API: dicts hold related keys
/// and can be dropped wholesale (`delete_dict`), which is what
/// `clear_registration` / `clear_profiles` / etc. exploit.
///
/// Implementations must be cheap to share across threads — `PddbStore`
/// holds the backend behind an `Arc` and clones the `Arc` on
/// `PddbStore::clone()`, so all callers see the same underlying state.
pub trait KvBackend: Send + Sync + fmt::Debug {
    fn get(&self, dict: &str, key: &str) -> Result<Option<Vec<u8>>, Error>;
    fn put(&self, dict: &str, key: &str, value: &[u8]) -> Result<(), Error>;
    fn delete(&self, dict: &str, key: &str) -> Result<(), Error>;
    fn delete_dict(&self, dict: &str) -> Result<(), Error>;
    fn list_keys(&self, dict: &str) -> Result<Vec<String>, Error>;
}

/// PDDB-backed store implementing presage's `StateStore` and
/// `ContentsStore` traits.
///
/// `Clone` is shallow — clones share the same backend via `Arc`.
/// `presage::store::Store` requires `Clone + Send + Sync + 'static`;
/// every clone of a `PddbStore` therefore observes the same on-disk
/// state, which is the behaviour Manager assumes when it stashes a
/// store handle behind shared state.
#[derive(Clone, Debug)]
pub struct PddbStore {
    backend: Arc<dyn KvBackend>,
}

impl PddbStore {
    /// Build a store from any `KvBackend`. Tests use `MockBackend`;
    /// Stage 8 will add a `PddbBackend` constructor that takes a
    /// `pddb::Pddb` handle.
    pub fn new(backend: Arc<dyn KvBackend>) -> Self {
        Self { backend }
    }

    /// Convenience for hosted-mode tests — wraps a fresh `MockBackend`.
    pub fn with_mock_backend() -> Self {
        Self::new(Arc::new(MockBackend::new()))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use futures_lite::future::block_on;
    use presage::libsignal_service::{
        Profile,
        prelude::{MasterKey, ProfileKey, Uuid},
        protocol::IdentityKeyPair,
    };
    use presage::manager::RegistrationData;
    use presage::store::{ContentsStore, StateStore};

    use super::*;

    /// Minimal fixture matching the real shape of the data the
    /// link/register flows produce. `RegistrationData` has private
    /// fields so we construct it via JSON round-trip — the on-disk
    /// format the StateStore actually serializes is the same JSON.
    /// `phone_number` and `profile_key` go in as their fully-typed
    /// serde representations (struct and byte array, respectively).
    fn fixture_registration() -> RegistrationData {
        let phone_number =
            serde_json::to_value(phonenumber::parse(None, "+15555550100").unwrap()).unwrap();
        // RegistrationData::profile_key serializes as base64 (not a
        // byte array) — see vendor/presage/presage/src/serde.rs
        // serde_profile_key. We deserialize from a fixed base64 string
        // here so the test doesn't depend on a particular encoding
        // detail of `ProfileKey::generate`.
        let json_str = format!(
            "{{\"signal_servers\":\"Production\",\
              \"device_name\":\"xous-test\",\
              \"phone_number\":{phone_number},\
              \"uuid\":\"00000000-0000-4000-8000-000000000001\",\
              \"pni\":\"00000000-0000-4000-8000-000000000002\",\
              \"password\":\"test-password\",\
              \"device_id\":2,\
              \"registration_id\":1234,\
              \"pni_registration_id\":5678,\
              \"profile_key\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"}}"
        );
        serde_json::from_str(&json_str).expect("fixture deserializes")
    }

    fn fixture_profile() -> Profile {
        Profile {
            name: None,
            about: Some("hello from xous".to_string()),
            about_emoji: Some("\u{1F44B}".to_string()),
            avatar: None,
            unrestricted_unidentified_access: false,
        }
    }

    #[test]
    fn empty_store_reports_unregistered() {
        let store = PddbStore::with_mock_backend();
        block_on(async {
            assert!(!store.is_registered().await);
            assert!(store.load_registration_data().await.unwrap().is_none());
            assert!(store.sender_certificate().await.unwrap().is_none());
            assert!(store.fetch_master_key().await.unwrap().is_none());
        });
    }

    #[test]
    fn registration_data_round_trip() {
        let mut store = PddbStore::with_mock_backend();
        let reg = fixture_registration();
        block_on(async {
            store.save_registration_data(&reg).await.unwrap();
            assert!(store.is_registered().await);
            let loaded = store.load_registration_data().await.unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(&loaded).unwrap(),
                serde_json::to_value(&reg).unwrap(),
            );
        });
    }

    #[test]
    fn identity_key_pair_round_trip() {
        let store = PddbStore::with_mock_backend();
        let mut rng = rand::rng();
        let aci_kp = IdentityKeyPair::generate(&mut rng);
        let pni_kp = IdentityKeyPair::generate(&mut rng);

        block_on(async {
            // Stage 4 only persists identity key pairs; the matching
            // load path is in Stage 5a (it lives on the protocol-store
            // trait, not StateStore). We assert via the raw backend
            // instead — the bytes round-trip through the dict.
            store.set_aci_identity_key_pair(aci_kp).await.unwrap();
            store.set_pni_identity_key_pair(pni_kp).await.unwrap();
            assert!(
                store
                    .backend
                    .get("signal.state", "aci_identity_key_pair")
                    .unwrap()
                    .is_some()
            );
            assert!(
                store
                    .backend
                    .get("signal.state", "pni_identity_key_pair")
                    .unwrap()
                    .is_some()
            );
        });
    }

    #[test]
    fn master_key_round_trip() {
        let store = PddbStore::with_mock_backend();
        let raw = [0xab_u8; 32];
        let mk = MasterKey::from_slice(&raw).unwrap();
        block_on(async {
            store.store_master_key(Some(&mk)).await.unwrap();
            let loaded = store.fetch_master_key().await.unwrap().unwrap();
            assert_eq!(loaded.inner, raw);

            // Storing None clears it.
            store.store_master_key(None).await.unwrap();
            assert!(store.fetch_master_key().await.unwrap().is_none());
        });
    }

    #[test]
    fn clear_registration_resets_state() {
        let mut store = PddbStore::with_mock_backend();
        let reg = fixture_registration();
        let mk = MasterKey::from_slice(&[0x77_u8; 32]).unwrap();
        block_on(async {
            store.save_registration_data(&reg).await.unwrap();
            store.store_master_key(Some(&mk)).await.unwrap();
            assert!(store.is_registered().await);

            store.clear_registration().await.unwrap();
            assert!(!store.is_registered().await);
            assert!(store.load_registration_data().await.unwrap().is_none());
            assert!(store.fetch_master_key().await.unwrap().is_none());
        });
    }

    #[test]
    fn profile_round_trip() {
        let mut store = PddbStore::with_mock_backend();
        let uuid = Uuid::from_str("00000000-0000-4000-8000-000000000003").unwrap();
        let key = ProfileKey::generate([1u8; 32]);
        let profile = fixture_profile();
        block_on(async {
            assert!(store.profile(uuid, key).await.unwrap().is_none());
            store
                .save_profile(uuid, key, profile.clone())
                .await
                .unwrap();
            let loaded = store.profile(uuid, key).await.unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(&loaded).unwrap(),
                serde_json::to_value(&profile).unwrap(),
            );
        });
    }

    /// A `PddbStore` clone shares its backend — both clones see the
    /// same writes. This is the property `Manager` relies on when it
    /// hands store clones to its sub-components.
    #[test]
    fn clones_share_backend() {
        let store_a = PddbStore::with_mock_backend();
        let mut store_b = store_a.clone();
        block_on(async {
            store_b
                .save_registration_data(&fixture_registration())
                .await
                .unwrap();
            assert!(store_a.is_registered().await);
        });
    }
}
