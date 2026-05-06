# Stage 5 — Implement remaining storage traits over PDDB

Status: **complete.** All 9 storage traits implemented + `Store`
blanket. 22 unit tests pass on hosted; rv32 cross-compile passes; fmt
+ clippy clean. Stage 4 ended at 713 LoC; Stage 5 lands the crate at
**2801 LoC** (+2088).

After this stage, `PddbStore` satisfies the full `presage::store::Store`
trait — it can be passed to `Manager::register` / `Manager::link_secondary`
/ `Manager::load_registered`. The remaining gap before the Manager
state machine is real (Stages 8–12) is the worker-thread + IPC
scaffolding that wires the store to a long-running `presage::Manager`
on a Xous worker thread.

## Crate layout

```
crates/presage-store-pddb/src/
├── lib.rs           (909 LoC)  KvBackend, PddbStore, OnNewIdentity,
│                                session cache, flush_sessions, tests
├── error.rs         ( 55 LoC)  Error + StoreError + From<serde_json>
├── state.rs         (117 LoC)  StateStore (10 methods)            [Stage 4]
├── content.rs       (544 LoC)  ContentsStore (~30 methods, full)  [Stage 5c]
├── store.rs         ( 60 LoC)  Store blanket + clear()            [Stage 5]
├── backend_mock.rs  ( 74 LoC)  In-memory KvBackend
└── protocol/
    ├── mod.rs                       (130 LoC)  IdentityType, PddbProtocolStore,
    │                                            dict-name helpers, ProtocolStore
    ├── identity_key_store.rs        (141 LoC)  IdentityKeyStore (5 methods)
    ├── pre_key_store.rs             ( 97 LoC)  PreKeyStore (3, packed bundle)
    ├── signed_pre_key_store.rs      ( 70 LoC)  SignedPreKeyStore (2)
    ├── kyber_pre_key_store.rs       (184 LoC)  KyberPreKeyStore (3, dedup)
    ├── session_store.rs             ( 90 LoC)  SessionStore (2, cached)
    ├── sender_key_store.rs          ( 54 LoC)  SenderKeyStore (2)
    ├── pre_keys_store.rs            ( 50 LoC)  PreKeysStore (8)         [Stage 5b]
    ├── kyber_pre_key_store_ext.rs   ( 85 LoC)  KyberPreKeyStoreExt (5)  [Stage 5b]
    └── session_store_ext.rs         (141 LoC)  SessionStoreExt (4)      [Stage 5b]
```

## Stage 5a — six libsignal protocol storage traits

ACI vs PNI is a **runtime** split via `IdentityType`, the same shape
`presage-store-sqlite` uses (`vendor/presage/presage-store-sqlite/
src/protocol.rs:30-44`). A `PddbProtocolStore` wraps a `PddbStore`
clone + an `IdentityType` discriminator and routes its dict-name
lookups through `dict_session(self.identity)`,
`dict_identity(self.identity)`, etc. Two clones for the same identity
share their underlying state via `Arc`.

Dictionary layout matches `docs/REPORT.md` §Decision 1:

| Dict | Holds | Layout |
|---|---|---|
| `signal.protocol.{aci,pni}.session` | per `(uuid, device_id)` | one key `"{uuid}.{device_id}"` per session, value = `SessionRecord::serialize` bytes |
| `signal.protocol.{aci,pni}.identity` | per `ProtocolAddress` | one key per address, value = `IdentityKey::serialize` bytes |
| `signal.protocol.{aci,pni}.prekey_bundle` | **packed** | single key `"all"`, value = JSON `Vec<(u32, Vec<u8>)>` of all current pre-keys |
| `signal.protocol.{aci,pni}.signed_prekey` | per id | one key per id (decimal string), value = `SignedPreKeyRecord::serialize` bytes |
| `signal.protocol.{aci,pni}.kyber_prekey` | per id | one key per id, value = JSON `{record, is_last_resort}` envelope |
| `signal.protocol.{aci,pni}.kyber_meta` | last-resort dedup table | key = `"{kyber_id}.{ec_id}"`, value = `base_key.serialize()` |
| `signal.protocol.{aci,pni}.sender_key` | per `(addr, dist_uuid)` | key = `"{addr_name}.{device_id}.{dist_uuid}"` |

### `IdentityKeyStore` (`identity_key_store.rs`, 5 methods)

`get_identity_key_pair` / `get_local_registration_id` cross the
StateStore/ProtocolStore boundary — they read the identity keypair
written by `StateStore::set_aci_identity_key_pair` /
`StateStore::set_pni_identity_key_pair` from the
`signal.state["{aci|pni}_identity_key_pair"]` key, and pull
`registration_id` from the same `RegistrationData` blob `StateStore`
manages. Same source the sqlite store reads from
(`vendor/presage/presage-store-sqlite/src/protocol.rs:550-575`).

`save_identity` / `is_trusted_identity` / `get_identity` operate on
the `signal.protocol.{aci,pni}.identity` dict, with the
`OnNewIdentity` policy on `PddbStore` deciding what
`is_trusted_identity` returns when an existing address has a
different key.

### `PreKeyStore` (`pre_key_store.rs`, 3 methods, packed)

Per `docs/REPORT.md` §Decision 1 the PreKey store uses the
**packed-key strategy**: every save/remove rewrites a single PDDB key
holding `Vec<(u32, Vec<u8>)>` of all current pre-keys. With ~100
prekeys at ~70 bytes each, the rewrite is well under one PDDB page;
the alternative (one key per id) would burn 100 pages per replenish
since each PDDB write costs at least one page (per-page AEAD; see
`backend/hw.rs:58-60` reference in the decision doc).

`save_pre_key`/`remove_pre_key` take `&mut self` from the trait but
need only `&self` semantics on the backend — the bundle is read,
edited, and rewritten in one call.

### `SignedPreKeyStore` / `KyberPreKeyStore` (per-id)

These are smaller (a handful of records per identity) so per-id
storage is the right primitive. `KyberPreKeyStore` records on disk
carry a JSON `{record: <bytes>, is_last_resort: bool}` envelope —
the bit needs to survive across `mark_kyber_pre_key_used` so the
last-resort dedup path knows which dict to consult. Same
`is_last_resort` flag the sqlite store carries as a column
(`vendor/presage/presage-store-sqlite/src/protocol.rs:407-470`).

`mark_kyber_pre_key_used`'s last-resort branch consults a separate
`signal.protocol.{aci,pni}.kyber_meta` dict for `(kyber_id, ec_id) →
base_key` dedup. Replaying the same triple yields
`SignalProtocolError::InvalidMessage(PreKey, "reused base key")`
matching the upstream contract.

### `SessionStore` — in-memory dirty-set + flush

Per `docs/REPORT.md` §Decision 5 the receive hot path's
`store_session` does **not** write through to PDDB. Instead it writes
to an in-memory cache (`PddbStore.session_cache`,
`HashMap<(IdentityType, String), SessionRecord>`) and marks the entry
dirty (`PddbStore.session_dirty`, `HashSet<...>`).
`PddbStore::flush_sessions` walks the dirty set and persists. The
Stage 11 receive loop will call this on `Received::QueueEmpty`; for
Stage 5 the unit tests call it explicitly to verify durability.

`load_session` consults the cache first, then falls through to PDDB
— that gives ratchet-step writes O(1) amortised cost without
sacrificing correctness on cold reads.

The `Mutex<HashMap>`/`Mutex<HashSet>` are kept *outside* the
`PddbProtocolStore` (which is created fresh on every
`Store::aci_protocol_store` call) so all clones of `PddbStore` share
the cache. This is the property that makes
`SessionStoreExt::delete_all_sessions` correct — it walks both the
cache and PDDB and dedups by address.

### `SenderKeyStore` (per-(addr, device, dist_uuid))

Two methods, straightforward per-key storage. Distribution UUIDs
serialise to their hex `simple()` form for printable PDDB keys.

### `ProtocolStore` blanket (`mod.rs`)

Empty `impl presage::libsignal_service::protocol::ProtocolStore for
PddbProtocolStore {}` — composes the five required protocol traits
per [`presage/src/store.rs:342-355`](https://github.com/whisperfish/presage/blob/main/presage/src/store.rs#L342-L355).

## Stage 5b — libsignal-service-rs extension traits

### `PreKeysStore` (8 methods)

Eight ID-counter / count methods. "next id" semantics match
presage-store-sqlite (max + 1, or 1 for an empty store). Counts come
from `list_keys`-then-len for per-id dicts and from the packed
bundle's vec-len for the prekey bundle.

### `KyberPreKeyStoreExt` (5 methods)

`store_last_resort_kyber_pre_key` / `load_last_resort_kyber_pre_keys`
/ `remove_kyber_pre_key` operate on the same dict as
`KyberPreKeyStore` but flip `is_last_resort=true` in the envelope.
`load_last_resort_kyber_pre_keys` walks the dict and filters.

`mark_all_one_time_kyber_pre_keys_stale_if_necessary` and
`delete_all_stale_one_time_kyber_pre_keys` are
`unimplemented!("should not be used yet")` matching
presage-store-sqlite's identical stubs (`vendor/presage/
presage-store-sqlite/src/protocol.rs:530-544`). presage's manager
doesn't currently call them.

### `SessionStoreExt` (4 user-facing methods)

`get_sub_device_sessions` / `delete_session` / `delete_all_sessions`
all walk both the cache and PDDB. `delete_all_sessions` dedups by
`(identity, address)` so a session present in both places counts
once. `compute_safety_number` and
`delete_service_addr_device_session` use the trait's default impls.

## Stage 5c — full `ContentsStore` (~30 methods)

Stage 4 left ~25 `unimplemented!("Stage 5c: …")` stubs. Stage 5c
fills them all in. Dictionary layout per Decision 1:

| Dict | Holds | Key | Value |
|---|---|---|---|
| `signal.contacts` | per `ServiceId` | uuid string | JSON `Contact` |
| `signal.groups` | per master_key | hex(master_key) | JSON `Group` |
| `signal.group_avatars` | per master_key | hex(master_key) | raw bytes |
| `signal.profile_keys` | per uuid | uuid string | JSON `ProfileKey` |
| `signal.profiles` | per profile | sha256(uuid \|\| profile_key) | JSON `Profile` |
| `signal.profile_avatars` | per profile | sha256(uuid \|\| profile_key) | raw bytes |
| `signal.sticker_packs` | per id | hex(id) | JSON `StickerPack` |
| `signal.threads.<thread_hex>` | per thread | 16-hex-char timestamp | JSON `StoredMessage` |

### Messages-by-thread

One **dictionary per thread**, where `thread_hex` is
`sha256("contact:" + uuid)` or `sha256("group:" + base64(master_key))`
— same derivation presage-store-sled uses. Per-message keys are
16-hex-character zero-padded `u64` timestamps so PDDB's lexicographic
key ordering matches arrival order.

Values are JSON `StoredMessage` envelopes wrapping libsignal's
prost-encoded `Content` body alongside spelled-out `Metadata` fields.
This matches presage-store-sqlite's pattern (`content.rs:154` —
prost bytes in one column, metadata in others). We don't adopt the
sled `InternalSerialization.proto` wrapper: it requires a build
script + a textsecure proto.

`messages(thread, range)` lists the dict's keys, parses each as a
`u64`, filters by range, sorts, and preloads — `list_keys` is
non-streaming (Decision 1 documents the cost), so the eager `Vec` is
what the underlying capability exposes. Stage 11 may add a per-thread
in-memory message-key index if profiling demands.

### `clear_messages` and `clear_thread`

`clear_thread(t)` is `backend.delete_dict(thread_dict_name(t))` —
one O(1) PDDB op per thread. `clear_messages` (without a thread arg)
is a no-op for the same reason sled's similar method is: PDDB
doesn't expose a "drop all dicts matching prefix" primitive, and
keeping a top-level index of all threads adds write cost on every
`save_message`. Stage 11 may revisit; not a blocker.

## `Store` blanket (`store.rs`)

`Store::aci_protocol_store` / `pni_protocol_store` return fresh
`PddbProtocolStore` clones. `Store::clear` chains
`clear_registration` + `clear_contents` and additionally drops every
protocol-store dict (sessions, identities, pre-key bundles, signed
pre-keys, kyber pre-keys, kyber metadata, sender keys) on both ACI
and PNI sides. Also clears the in-memory session cache so a future
flush doesn't re-persist dropped data.

## Verification

```
$ cargo test -p presage-store-pddb
running 22 tests
test tests::contact_round_trip ... ok
test tests::clear_registration_resets_state ... ok
test tests::clones_share_backend ... ok
test tests::empty_store_reports_unregistered ... ok
test tests::group_round_trip ... ok
test tests::identity_key_pair_round_trip ... ok
test tests::identity_key_store_round_trip ... ok
test tests::kyber_pre_key_last_resort_dedup ... ok
test tests::kyber_pre_key_one_time_round_trip_and_use ... ok
test tests::master_key_round_trip ... ok
test tests::message_round_trip_and_range ... ok
test tests::next_pre_key_id_increments ... ok
test tests::pre_key_store_round_trip_packed ... ok
test tests::profile_key_round_trip ... ok
test tests::profile_round_trip ... ok
test tests::registration_data_round_trip ... ok
test tests::sender_key_round_trip ... ok
test tests::session_cache_then_flush_then_load ... ok
test tests::session_store_ext_delete_paths ... ok
test tests::signed_pre_key_round_trip ... ok
test tests::sticker_pack_round_trip ... ok
test tests::store_clear_resets_everything ... ok
test result: ok. 22 passed; 0 failed; 0 ignored

$ cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb
✓ rv32 cross-compile passes (Stage 6 + 7 unblocked this at Stage 4;
   Stage 5 keeps it green).

$ cargo fmt --all -- --check                              ✓ clean
$ cargo clippy -p presage-store-pddb --all-targets -- -D warnings   ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo tree --workspace -d                               ⚠ same as Stage 6
                                                          (no new dups from
                                                          Stage 5)
```

### Test coverage summary (22 tests)

| Trait | Test |
|---|---|
| `StateStore` | `empty_store_reports_unregistered`, `registration_data_round_trip`, `identity_key_pair_round_trip`, `master_key_round_trip`, `clear_registration_resets_state` |
| `IdentityKeyStore` | `identity_key_store_round_trip` (save / get / `IdentityChange::{NewOrUnchanged, ReplacedExisting}` / `get_identity_key_pair` / `get_local_registration_id`) |
| `PreKeyStore` | `pre_key_store_round_trip_packed` (save / get / remove against packed bundle) |
| `SignedPreKeyStore` | `signed_pre_key_round_trip` |
| `KyberPreKeyStore` | `kyber_pre_key_one_time_round_trip_and_use` (one-time deletes), `kyber_pre_key_last_resort_dedup` (last-resort dedup error) |
| `SessionStore` | `session_cache_then_flush_then_load` (cache-then-flush + idempotent flush) |
| `SenderKeyStore` | `sender_key_round_trip` |
| `PreKeysStore` | `next_pre_key_id_increments` |
| `KyberPreKeyStoreExt` | `kyber_pre_key_last_resort_dedup` (also covers `store_last_resort_kyber_pre_key` + `load_last_resort_kyber_pre_keys`) |
| `SessionStoreExt` | `session_store_ext_delete_paths` (`get_sub_device_sessions` / `delete_session` / `delete_all_sessions`) |
| `ContentsStore` (Stage 5c) | `contact_round_trip`, `group_round_trip`, `profile_round_trip` (Stage 4), `profile_key_round_trip`, `sticker_pack_round_trip`, `message_round_trip_and_range` |
| `Store` (blanket) | `store_clear_resets_everything`, `clones_share_backend` |

## Files changed (this commit)

```
modified:
  Cargo.lock                                                (resolver: + async-trait,
                                                              chrono, prost 0.13,
                                                              tracing, base64)
  crates/presage-store-pddb/Cargo.toml                      (+5 prod deps)
  crates/presage-store-pddb/src/lib.rs                      (260 → 909 LoC: cache,
                                                              dirty set, flush, tests)
  crates/presage-store-pddb/src/content.rs                  (208 → 544 LoC: ~25 stubs
                                                              replaced with real impls
                                                              + StoredMessage envelope)

new:
  crates/presage-store-pddb/src/store.rs                    (60 LoC; Store blanket)
  crates/presage-store-pddb/src/protocol/mod.rs             (130 LoC)
  crates/presage-store-pddb/src/protocol/identity_key_store.rs       (141 LoC)
  crates/presage-store-pddb/src/protocol/pre_key_store.rs            ( 97 LoC)
  crates/presage-store-pddb/src/protocol/signed_pre_key_store.rs     ( 70 LoC)
  crates/presage-store-pddb/src/protocol/kyber_pre_key_store.rs      (184 LoC)
  crates/presage-store-pddb/src/protocol/session_store.rs            ( 90 LoC)
  crates/presage-store-pddb/src/protocol/sender_key_store.rs         ( 54 LoC)
  crates/presage-store-pddb/src/protocol/pre_keys_store.rs           ( 50 LoC)
  crates/presage-store-pddb/src/protocol/kyber_pre_key_store_ext.rs  ( 85 LoC)
  crates/presage-store-pddb/src/protocol/session_store_ext.rs        (141 LoC)

new (docs):
  stage/REPORT-5.md                                                   (this file)
```

## Outstanding work (not Stage 5)

- **Stage 8**: worker-thread + IPC scaffolding — where `presage::Manager`
  loops on a Xous worker thread and calls into our `PddbStore` clones.
- **Stage 9**: hardware/Renode bring-up. Three follow-ups outstanding
  from Stage 6.1 (`getrandom 0.3` custom backend, `upload_to_cdn0`
  multipart, `u32e_backend` SOC feature wiring) — these are blockers
  for hardware deploy but not for storage validation.
- **Stage 11 / 12**: receive-loop and send-flow MVPs — these will
  exercise the session cache + flush pattern under realistic load and
  may reveal where `clear_messages` (currently a no-op) needs an
  index dict, where `messages(range)` needs a per-thread index, or
  whether the JSON-wrapped message format needs to switch to
  postcard for binary-size wins.
