//! `presage::store::ContentsStore` implementation for `PddbStore`.
//!
//! Stage 4 only fills in `profile` and `save_profile` — the round-trip
//! path that the Stage 4 tests exercise. The other ~25 methods are
//! `unimplemented!`-stubbed and become Stage 5c work. Keeping them as
//! stubs (rather than deleting the impl) means the trait surface compiles
//! today, so any code wired to `Store: presage::Store` at Stage 8 builds;
//! it just panics if it touches a Stage 5c method.
//!
//! Profile keys are derived `sha256(uuid_bytes || profile_key_bytes)` and
//! used as the per-profile PDDB key inside the `signal.profiles` dict.
//! Same scheme presage-store-sled uses
//! (`profile_key_for_uuid` at vendor/presage/presage-store-sled/src/lib.rs:275).

use std::ops::RangeBounds;

use presage::{
    AvatarBytes,
    libsignal_service::{
        Profile,
        content::Content,
        prelude::{ProfileKey, Uuid},
        protocol::ServiceId,
        zkgroup::GroupMasterKeyBytes,
    },
    model::{contacts::Contact, groups::Group},
    store::{ContentsStore, StickerPack, Thread},
};
use sha2::{Digest, Sha256};

use crate::{Error, PddbStore};

const DICT_PROFILES: &str = "signal.profiles";

/// `sha256(uuid_bytes || profile_key_bytes)` rendered as lowercase hex.
/// Matches presage-store-sled's `profile_key_for_uuid`. The hash hides
/// the underlying profile key from anyone reading the on-disk dict
/// listing, and it gives a fixed-length printable key the PDDB API can
/// store.
fn profile_dict_key(uuid: Uuid, key: ProfileKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(uuid.as_bytes());
    hasher.update(key.get_bytes());
    format!("{:x}", hasher.finalize())
}

impl ContentsStore for PddbStore {
    type ContentsStoreError = Error;

    type ContactsIter = std::iter::Empty<Result<Contact, Error>>;
    type GroupsIter = std::iter::Empty<Result<(GroupMasterKeyBytes, Group), Error>>;
    type MessagesIter = std::iter::Empty<Result<Content, Error>>;
    type StickerPacksIter = std::iter::Empty<Result<StickerPack, Error>>;

    // --- Profiles (the Stage 4 round-trip path) ---

    async fn save_profile(
        &mut self,
        uuid: Uuid,
        key: ProfileKey,
        profile: Profile,
    ) -> Result<(), Error> {
        let dict_key = profile_dict_key(uuid, key);
        let bytes = serde_json::to_vec(&profile).map_err(Error::encode)?;
        self.backend.put(DICT_PROFILES, &dict_key, &bytes)?;
        Ok(())
    }

    async fn profile(&self, uuid: Uuid, key: ProfileKey) -> Result<Option<Profile>, Error> {
        let dict_key = profile_dict_key(uuid, key);
        match self.backend.get(DICT_PROFILES, &dict_key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    // --- Stage 5c stubs below. The bodies stay synchronous-feeling
    // (just `unimplemented!`) — they don't await anything; the `async`
    // lets them satisfy the `impl Future` return type the trait demands.

    async fn clear_profiles(&mut self) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_profiles");
    }

    async fn clear_contents(&mut self) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_contents");
    }

    async fn clear_messages(&mut self) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_messages");
    }

    async fn clear_thread(&mut self, _thread: &Thread) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_thread");
    }

    async fn save_message(&self, _thread: &Thread, _message: Content) -> Result<(), Error> {
        unimplemented!("Stage 5c: save_message");
    }

    async fn delete_message(&mut self, _thread: &Thread, _timestamp: u64) -> Result<bool, Error> {
        unimplemented!("Stage 5c: delete_message");
    }

    async fn message(&self, _thread: &Thread, _timestamp: u64) -> Result<Option<Content>, Error> {
        unimplemented!("Stage 5c: message");
    }

    async fn messages(
        &self,
        _thread: &Thread,
        _range: impl RangeBounds<u64>,
    ) -> Result<Self::MessagesIter, Error> {
        unimplemented!("Stage 5c: messages");
    }

    async fn clear_contacts(&mut self) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_contacts");
    }

    async fn save_contact(&mut self, _contact: &Contact) -> Result<(), Error> {
        unimplemented!("Stage 5c: save_contact");
    }

    async fn contacts(&self) -> Result<Self::ContactsIter, Error> {
        unimplemented!("Stage 5c: contacts");
    }

    async fn contact_by_id(&self, _id: &ServiceId) -> Result<Option<Contact>, Error> {
        unimplemented!("Stage 5c: contact_by_id");
    }

    async fn clear_groups(&mut self) -> Result<(), Error> {
        unimplemented!("Stage 5c: clear_groups");
    }

    async fn save_group(
        &self,
        _master_key: GroupMasterKeyBytes,
        _group: impl Into<Group>,
    ) -> Result<(), Error> {
        unimplemented!("Stage 5c: save_group");
    }

    async fn groups(&self) -> Result<Self::GroupsIter, Error> {
        unimplemented!("Stage 5c: groups");
    }

    async fn group(&self, _master_key: GroupMasterKeyBytes) -> Result<Option<Group>, Error> {
        unimplemented!("Stage 5c: group");
    }

    async fn save_group_avatar(
        &self,
        _master_key: GroupMasterKeyBytes,
        _avatar: &AvatarBytes,
    ) -> Result<(), Error> {
        unimplemented!("Stage 5c: save_group_avatar");
    }

    async fn group_avatar(
        &self,
        _master_key: GroupMasterKeyBytes,
    ) -> Result<Option<AvatarBytes>, Error> {
        unimplemented!("Stage 5c: group_avatar");
    }

    async fn upsert_profile_key(&mut self, _uuid: &Uuid, _key: ProfileKey) -> Result<bool, Error> {
        unimplemented!("Stage 5c: upsert_profile_key");
    }

    async fn profile_key(&self, _service_id: &ServiceId) -> Result<Option<ProfileKey>, Error> {
        unimplemented!("Stage 5c: profile_key");
    }

    async fn save_profile_avatar(
        &mut self,
        _uuid: Uuid,
        _key: ProfileKey,
        _profile: &AvatarBytes,
    ) -> Result<(), Error> {
        unimplemented!("Stage 5c: save_profile_avatar");
    }

    async fn profile_avatar(
        &self,
        _uuid: Uuid,
        _key: ProfileKey,
    ) -> Result<Option<AvatarBytes>, Error> {
        unimplemented!("Stage 5c: profile_avatar");
    }

    async fn add_sticker_pack(&mut self, _pack: &StickerPack) -> Result<(), Error> {
        unimplemented!("Stage 5c: add_sticker_pack");
    }

    async fn sticker_pack(&self, _id: &[u8]) -> Result<Option<StickerPack>, Error> {
        unimplemented!("Stage 5c: sticker_pack");
    }

    async fn remove_sticker_pack(&mut self, _id: &[u8]) -> Result<bool, Error> {
        unimplemented!("Stage 5c: remove_sticker_pack");
    }

    async fn sticker_packs(&self) -> Result<Self::StickerPacksIter, Error> {
        unimplemented!("Stage 5c: sticker_packs");
    }
}
