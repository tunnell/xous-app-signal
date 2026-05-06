//! Real PDDB-backed `KvBackend` for rv32-xous targets.
//!
//! Stage 9a scaffolding: the file exists so the feature-flag
//! plumbing (`pddb-backend`) and the cfg-gated `target_os = "xous"`
//! conditional both land in this commit. The actual `pddb::Pddb`
//! call-throughs land in Stage 9b once the workspace is merged into
//! the `tunnell/xous-core-for-xas` fork — that's when the `pddb`
//! crate becomes path-resolvable.
//!
//! When implemented, this module will mirror the `KvBackend` trait
//! from `lib.rs` and forward calls to `pddb::Pddb` per
//! `xous-core/services/pddb/src/lib.rs:532` (get) /
//! `:737` (delete_dict) / `:831` (list_keys). Holding a single
//! `Arc<Mutex<Pddb>>` per `PddbStore` keeps the handle shareable
//! across `PddbStore::clone()` calls (the `Mutex` serializes IPC
//! requests; PDDB's server is itself single-threaded).
//!
//! The basis name is hard-coded to `"signal"` for now. Stage 11+
//! may revisit if multi-account support becomes a deliverable.

#![cfg(all(feature = "pddb-backend", target_os = "xous"))]

// Stage 9b: replace this stub with the real impl. Keep the module
// behind `cfg(all(feature = "pddb-backend", target_os = "xous"))`
// so hosted builds — where `pddb` isn't path-resolvable — never see
// it. The `_test_compile` fn is a sentinel: if Stage 9b lands
// without removing it, that's a sign the wiring isn't done.
#[allow(dead_code)]
fn _test_compile_stub() {}
