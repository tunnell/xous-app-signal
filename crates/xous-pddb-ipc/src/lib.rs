//! Hand-rolled PDDB IPC client for the xas Signal app.
//!
//! Replicates just enough of `xous-core/services/pddb`'s client surface
//! to back a `KvBackend` (`get` / `put` / `delete` / `delete_dict` /
//! `list_keys`). Goes around the `pddb` crate entirely because pulling
//! it in via path-dep cascades into 10+ xous-core services
//! (`spinor`, `root-keys`, `llio`, `tts-frontend`, `gam`, `modals`,
//! `keystore-api`, …) — that workspace-merge approach failed on its
//! first try.
//!
//! The wire structs in `api.rs` are byte-compatible verbatim copies
//! from `~/precursor-signal/repos/xous-core/services/pddb/src/api.rs`
//! (rkyv 0.8 → 0.8 across version bumps within the 0.8.x semver line).
//! An earlier IPC probe (commit `f7a9e7b`, asserted `is_mounted=false`
//! against the live PDDB server) is the reference smoke for this
//! protocol replication.
//!
//! Scope is deliberately limited to the operations our KvBackend
//! exercises. PDDB's basis-management, key-attribute, and bulk-read
//! surfaces are not exposed — when we eventually need any of those,
//! add them here mirroring `xous-core/services/pddb/src/lib.rs`.

pub mod api;
pub mod client;

pub use api::{Error, ErrorKind};
pub use client::{KeyHandle, OpenOptions, PddbClient};
