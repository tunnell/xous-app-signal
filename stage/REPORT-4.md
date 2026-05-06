# Stage 4 — `presage-store-pddb`: skeleton + `StateStore` + `ContentsStore::profile`

Status: **complete.** All Stage 4 deliverables landed. 7 unit tests pass on
hosted; rv32 cross-compile passes; clippy + fmt clean.

This report supersedes the partial Stage 4 report that documented only the
Cargo dep wiring and curve25519-dalek vendoring (kept below as "What
landed in the partial pass" for context). The main Stage 4 work — actual
`StateStore` impl and the profile round-trip — is what this update covers.

## Decision: ordering recap

Stage 4 main work was deferred at the partial pass (see "What landed in
the partial pass" below) because rv32 cross-compile was blocked by
`mio` (transitively pulled by tokio via reqwest via libsignal-service-rs).
Stages 6 + 7 broke that coupling: Stage 6 forked libsignal-service-rs's
transport (replaced reqwest+reqwest-websocket with a sync `HttpClient`
trait + per-request worker thread + tungstenite-based WS pump), and
Stage 7 forked presage to remove tokio. After both landed,
`cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb`
passes — which means Stage 4's storage code can ship with rv32
verification on its first commit, the property the Option B reordering
was designed to deliver.

## What landed in this stage

### Crate layout

```
crates/presage-store-pddb/
├── Cargo.toml
└── src/
    ├── lib.rs           (260 LoC) — KvBackend trait, PddbStore struct, tests
    ├── error.rs         ( 55 LoC) — Error enum + StoreError + From<serde_json>
    ├── state.rs         (117 LoC) — StateStore impl (10 methods)
    ├── content.rs       (208 LoC) — ContentsStore impl (profile real, rest stubbed)
    └── backend_mock.rs  ( 74 LoC) — In-memory KvBackend
```

Total: 713 LoC of new code (excluding `Cargo.toml`).

### `KvBackend` trait

```rust
pub trait KvBackend: Send + Sync + fmt::Debug {
    fn get(&self, dict: &str, key: &str) -> Result<Option<Vec<u8>>, Error>;
    fn put(&self, dict: &str, key: &str, value: &[u8]) -> Result<(), Error>;
    fn delete(&self, dict: &str, key: &str) -> Result<(), Error>;
    fn delete_dict(&self, dict: &str) -> Result<(), Error>;
    fn list_keys(&self, dict: &str) -> Result<Vec<String>, Error>;
}
```

`(dict, key) -> bytes` — the same shape PDDB exposes. The trait is what
the storage trait impls run against; the backend is swappable. Stage 4
ships the in-memory `MockBackend`; Stage 8 will add the
`pddb::Pddb`-backed implementation.

### `PddbStore`

```rust
#[derive(Clone, Debug)]
pub struct PddbStore {
    backend: Arc<dyn KvBackend>,
}
```

`Clone` is shallow (`Arc::clone`), so `Manager`-internal store clones
all observe the same on-disk state. This is the property
`presage::store::Store` requires (`Clone + Send + Sync + 'static`).

### `StateStore` impl (state.rs)

All 10 methods implemented (matches the trait at
`vendor/presage/presage/src/store.rs:36-83`):

| method | storage |
|---|---|
| `load_registration_data` | `signal.state["registration"]`, JSON |
| `save_registration_data` | same key, JSON-encode |
| `set_aci_identity_key_pair` | `signal.state["aci_identity_key_pair"]`, libsignal `.serialize()` |
| `set_pni_identity_key_pair` | `signal.state["pni_identity_key_pair"]`, libsignal `.serialize()` |
| `sender_certificate` | `signal.state["sender_certificate"]`, `SenderCertificate::deserialize` |
| `save_sender_certificate` | same key, `.serialized()?` |
| `is_registered` | `load_registration_data().is_some()` |
| `clear_registration` | `delete_dict("signal.state")` |
| `fetch_master_key` | `signal.state["master_key"]`, `MasterKey::from_slice` |
| `store_master_key` | same key (`Some` writes, `None` deletes) |

All state lives in a single PDDB dictionary, `signal.state`, with one
key per field — per `docs/REPORT.md` Decision 1 (storage layout). This
matches `presage-store-sqlite`'s `kv` table layout:
`set_aci_identity_key_pair` and friends each call `INSERT OR REPLACE`
on a single string key (vendor/presage/presage-store-sqlite/src/lib.rs:258-272).
Per-field keys keep reads cheap and let `clear_registration` do a single
`delete_dict` call instead of editing a giant blob.

### `ContentsStore` partial impl (content.rs)

`save_profile` and `profile` are real — they store under
`signal.profiles[sha256(uuid || profile_key)]` with `serde_json` for the
body. The hash key matches `presage-store-sled`'s `profile_key_for_uuid`
(vendor/presage/presage-store-sled/src/lib.rs:275); it gives a fixed-
length printable key and hides the underlying profile key from anyone
listing the dict.

The other ~25 `ContentsStore` methods are `unimplemented!`-stubbed with
`"Stage 5c: <method_name>"` panic messages. Compiles today; Stage 5c
fills them in. Iterator types use `std::iter::Empty<Result<T, Error>>`
since no method that returns one is implemented yet.

### `MockBackend` (backend_mock.rs)

`Mutex<HashMap<(String, String), Vec<u8>>>`. Plaintext — PDDB's per-page
AES-256-GCM-SIV is the backend's responsibility, not the trait impl's,
and the trait impl is what these tests cover. Stays around as the test
harness for every storage trait we add at Stage 5.

### Error type (error.rs)

```rust
pub enum Error {
    Backend(String),       // KvBackend operation failed
    Encode(String),        // serde_json encode
    Decode(String),        // serde_json decode or libsignal deserialize
    Protocol(#[from] SignalProtocolError),
}
impl PresageStoreError for Error {}
impl From<serde_json::Error> for Error { /* default → Decode */ }
```

`Encode` and `Decode` are split because `serde_json::Error` doesn't
distinguish the two by value. Encode-side callsites use
`.map_err(Error::encode)` explicitly; the default `From<serde_json::Error>`
classifies unknown errors as `Decode`, which is the more common
direction at the dict boundary.

## Verification

```
$ cargo test -p presage-store-pddb
running 7 tests
test tests::empty_store_reports_unregistered ... ok
test tests::master_key_round_trip ... ok
test tests::profile_round_trip ... ok
test tests::identity_key_pair_round_trip ... ok
test tests::clear_registration_resets_state ... ok
test tests::clones_share_backend ... ok
test tests::registration_data_round_trip ... ok
test result: ok. 7 passed; 0 failed; 0 ignored

$ cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb
✓ rv32 cross-compile passes (Stage 6 + 7 unblocked this — first time
  presage-store-pddb has cross-compiled with the storage code present).

$ cargo fmt --all -- --check                              ✓ clean
$ cargo clippy -p presage-store-pddb --all-targets -- -D warnings   ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo tree --workspace -d                               ⚠ same as Stage 6
                                                          (no new dups from
                                                          Stage 4 main work)
```

### Test coverage

| test | what it verifies |
|---|---|
| `empty_store_reports_unregistered` | `is_registered()`, `load_registration_data()`, `sender_certificate()`, `fetch_master_key()` all empty/None on a fresh store |
| `registration_data_round_trip` | save → load → compare via JSON equality (RegistrationData has private fields; built via JSON deserialize) |
| `identity_key_pair_round_trip` | ACI + PNI keypairs persist (loaded via raw backend — the matching load path lives on `ProtocolStore`, Stage 5a) |
| `master_key_round_trip` | `Some(&mk)` writes, `None` deletes, `fetch_master_key` reads back |
| `clear_registration_resets_state` | After clear: registration gone, master key gone, `is_registered() == false` |
| `profile_round_trip` | save_profile → profile → JSON-equal to input |
| `clones_share_backend` | `store_a.clone()`'s save is observable from `store_a` (the property Manager relies on) |

### Test scaffolding decisions

- **`futures_lite::future::block_on`** to drive async tests — no tokio
  in the dev-dep tree; matches the smol primitives the rest of the
  workspace uses.
- **`RegistrationData` fixture via JSON** — its `password` and
  `profile_key` fields are `pub(crate)`. The test builds the struct by
  deserializing a fixed JSON literal, with the `phone_number` field
  dropped in via `serde_json::to_value(phonenumber::parse(...))` (the
  type's `serde::Deserialize` rejects bare strings) and the
  `profile_key` field as a fixed base64 string (the
  `#[serde(with = "serde_profile_key")]` attribute serializes
  `ProfileKey` as base64, not as a byte array).

## Outstanding work this stage does NOT cover

Stage 4 establishes the storage pattern; it does not build out the full
storage surface. Three categories remain, all per the ROADMAP:

- **Stage 5a — six libsignal protocol storage traits.** `IdentityKeyStore`,
  `PreKeyStore`, `SignedPreKeyStore`, `KyberPreKeyStore`,
  `SenderKeyStore`, `SessionStore`. Required for any encrypted
  send/receive. The aci-vs-pni split means each trait is implemented
  twice (one impl per `IdentityType`).
- **Stage 5b — libsignal-service-rs extension traits.** `PreKeysStore`,
  `SessionStoreExt`, `SenderKeyStore` (the libsignal-service-rs flavor).
- **Stage 5c — full `ContentsStore`.** Messages, contacts, groups,
  sticker packs, profile keys, profile avatars, group avatars. ~25
  methods, ~600 LoC by sled's example. The Stage 5c work pattern is
  established here — same `signal.<thing>` dict naming, same JSON or
  binary serialization choices.

## Files changed (this commit)

```
modified:
  Cargo.lock                                  (resolver picked up new dev-deps)
  crates/presage-store-pddb/Cargo.toml        (+serde, serde_json, sha2,
                                                 thiserror, futures-lite,
                                                 phonenumber [dev], rand [dev])

new:
  crates/presage-store-pddb/src/error.rs      (55 LoC)
  crates/presage-store-pddb/src/state.rs      (117 LoC)
  crates/presage-store-pddb/src/content.rs    (208 LoC)
  crates/presage-store-pddb/src/backend_mock.rs (74 LoC)

modified:
  crates/presage-store-pddb/src/lib.rs        (skeleton 9 LoC → 260 LoC)
  stage/REPORT-4.md                           (this file; supersedes the
                                                 partial-pass report)
```

---

## What landed in the partial pass (kept for history)

The Stage 4 partial pass landed the Cargo dep wiring + curve25519-dalek
vendoring before Stages 6 + 7. That work is preserved verbatim below for
context.

### 1. `presage` is now a workspace dep

`crates/presage-store-pddb/Cargo.toml` declares `presage = { git = "https://github.com/whisperfish/presage", rev = "600c4ed" }`. The full Whisperfish stack (libsignal v0.91.0 by tag, libsignal-service-rs HEAD by rev, presage HEAD by rev) is now resolvable. Hosted-mode `cargo build -p presage-store-pddb` succeeds.

### 2. curve25519-dalek strategy: vendored `betrusted-io/curve25519-dalek` (Precursor HW-accelerated) + version bump + lizard port

The Precursor curve25519 IP core is **Precursor-only** (per bunnie, 2026-05) — not on the Bao1x tape-out, which has a different PKE engine. We're targeting **Precursor first**; Bao1x is a future swap (different backend would need to be written).

The vendored copy at `vendor/curve25519-dalek/` is `betrusted-io/curve25519-dalek` (carries the u32e IP-core driver at `curve25519-dalek/src/backend/serial/u32e/`), with three small modifications:

1. Manifest version bumped `4.1.2` → `4.1.3` so the `[patch.crates-io]` redirect matches what libsignal's zkgroup declares (`curve25519-dalek = "4.1.3"`).
2. The `src/lizard/` module ported verbatim from `signalapp/curve25519-dalek` (`signal-curve25519-4.1.3` tag): 4 `RistrettoPoint` methods used by zkgroup (`lizard_encode<H>`, `lizard_decode<H>`, `from_uniform_bytes_single_elligator`, `decode_253_bits`). Additive vs the betrusted-io fork — no API conflicts.
3. One `pub mod lizard;` line in `src/lib.rs`.

That's the entire delta over upstream betrusted-io.

**HW acceleration activation.** The u32e backend is selected at compile time by `--cfg curve25519_dalek_backend="u32e_backend"`. We auto-set this for rv32-xous via `.cargo/config.toml` (currently disabled pending the Precursor SOC feature wiring on `utralib`; see Stage 6.1 phase 3f notes). On Precursor hardware, ECC operations route through the IP core.

Workspace `[patch.crates-io]`:

```toml
[patch.crates-io.curve25519-dalek]
path = "vendor/curve25519-dalek/curve25519-dalek"

[patch.crates-io.curve25519-dalek-derive]
path = "vendor/curve25519-dalek/curve25519-dalek-derive"

# libsignal also imports curve25519-dalek directly via the git URL alias
# `curve25519-dalek-signal = { git = "...signalapp/...", package = "curve25519-dalek" }`
# at libsignal/Cargo.toml:90. [patch.crates-io] doesn't redirect git sources,
# so we additionally patch the git URL.
[patch."https://github.com/signalapp/curve25519-dalek"]
curve25519-dalek = { path = "vendor/curve25519-dalek/curve25519-dalek" }
curve25519-dalek-derive = { path = "vendor/curve25519-dalek/curve25519-dalek-derive" }
```

**Future-target story.** The choice is target-scoped via `.cargo/config.toml`, not workspace-scoped. Adding Bao1x support later means writing a new backend module (e.g. `src/backend/serial/bao1x_pke/`) and adding another `[target.…]` block to `.cargo/config.toml`. The Precursor decision doesn't lock us out.

`docs/REPORT.md` §Decision 6 and Risk #3 have been rewritten to document this strategy.
