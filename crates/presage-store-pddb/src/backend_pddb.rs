//! Real PDDB-backed `KvBackend`.
//!
//! Wraps `xous_pddb_ipc::PddbClient` (the hand-rolled IPC client;
//! bypasses `services/pddb`'s gen1 dep cascade) and forwards
//! `KvBackend` operations to PDDB's wire protocol.
//!
//! Behavior:
//!
//! - `get` opens with `create_dict=false, create_key=false` and
//!   reads the whole key into a Vec<u8>. Returns `Ok(None)` on
//!   `NotFound`, the bytes on `Ok`, error otherwise.
//! - `put` opens with `create_dict=true, create_key=true` and writes
//!   the value. No client-side `flush_writes`: the PDDB server's
//!   `Opcode::WriteKey` handler already calls `basis_cache.sync(...)`
//!   on every write (see body for the line-cite), so `WriteKeyFlush`
//!   is redundant. Releases the handle on Drop.
//! - `delete` and `delete_dict` are direct opcodes; `NotFound` on
//!   delete is mapped to `Ok(())` (idempotent).
//! - `list_keys` calls `KeyCountInDict` + `ListKeyV2` chain, returns
//!   `Ok(Vec::new())` if the dict is empty, `NotFound` -> empty Vec
//!   for upstream-compatible behavior.
//!
//! Design notes:
//!
//! - A single `Mutex<PddbClient>` shared via `Arc` across `PddbStore`
//!   clones (matching the `PddbStore::clone()` shallow-share
//!   contract). The `Mutex` serializes IPC requests; PDDB's server
//!   is itself single-threaded so concurrent requests would queue
//!   anyway.
//! - On `is_mounted() == false`, every operation returns
//!   `Error::backend("PDDB not mounted")`. The store layer is
//!   expected to surface this as a presage `StoreError` and the
//!   caller (worker thread) decides whether to retry / wait.

#![cfg(feature = "pddb-backend")]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use xous_pddb_ipc::{ErrorKind as IpcErrorKind, KeyHandle, OpenOptions, PddbClient};

use crate::{Error, KvBackend};

/// Real PDDB-backed `KvBackend`. Constructed via
/// `PddbStore::with_pddb_backend()` (added in `lib.rs` for this
/// stage).
#[derive(Debug)]
pub struct PddbBackend {
    client: Arc<Mutex<PddbClient>>,
}

impl PddbBackend {
    /// Connect to the running PDDB server. Returns `Err` if the
    /// server's SID isn't registered (i.e. PDDB isn't running) or
    /// the connection request itself fails.
    ///
    /// Does *not* block on PDDB being mounted — that's intentional.
    /// A caller that needs a mounted store should poll `is_mounted`
    /// before issuing reads/writes; otherwise the per-op `NotMounted`
    /// error tells them when the cache is cold.
    pub fn connect() -> Result<Self, Error> {
        PddbClient::new()
            .map(|c| PddbBackend { client: Arc::new(Mutex::new(c)) })
            .map_err(|e| Error::backend(format!("PDDB connect: {}", e)))
    }

    /// Forward to `PddbClient::is_mounted` so callers can pre-check.
    /// Currently unused inside this crate; `xous-app-signal`'s
    /// `probe-pddb-real` feature is the expected consumer.
    /// `dead_code` allowed because the method is part of the public
    /// surface, not a private helper.
    #[allow(dead_code)]
    pub fn is_mounted(&self) -> bool {
        match self.client.lock() {
            Ok(g) => g.is_mounted(),
            Err(_) => false,
        }
    }

    /// Trigger an interactive mount. Blocks until the user enters
    /// the password and the mount completes, or the server returns
    /// non-OK. Use this from test paths that need a guaranteed-mounted
    /// PDDB before issuing put/get.
    #[allow(dead_code)]
    pub fn try_mount(&self) -> Result<bool, Error> {
        let guard = self.lock()?;
        guard.try_mount().map_err(|e| map_ipc_err(e, "try_mount"))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, PddbClient>, Error> {
        self.client
            .lock()
            .map_err(|_| Error::backend("PDDB client mutex poisoned"))
    }
}

impl KvBackend for PddbBackend {
    fn get(&self, dict: &str, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let guard = self.lock()?;
        let mut handle = match guard.open(dict, key, OpenOptions::default()) {
            Ok(h) => h,
            Err(e) if e.kind == IpcErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(map_ipc_err(e, "open for read")),
        };
        let mut bytes = Vec::new();
        read_all(&mut handle, &mut bytes).map_err(|e| Error::backend(format!("read: {}", e)))?;
        Ok(Some(bytes))
    }

    fn put(&self, dict: &str, key: &str, value: &[u8]) -> Result<(), Error> {
        let guard = self.lock()?;
        // Refs #14: xous-core PDDB's `WriteKey` opcode passes
        // `truncate=false` to `key_update`, so overwriting an
        // existing larger key leaves the trailing bytes intact and
        // `get` returns them concatenated to the new value. Delete
        // first to force a fresh allocation. NotFound on a never-
        // -written key is normal — ignore.
        match guard.delete_key(dict, key) {
            Ok(()) => {}
            Err(e) if e.kind == IpcErrorKind::NotFound => {}
            Err(e) => return Err(map_ipc_err(e, "put: pre-delete")),
        }
        let mut handle = guard
            .open(dict, key, OpenOptions::create_all())
            .map_err(|e| map_ipc_err(e, "open for write"))?;
        // Note: no explicit `handle.flush()` here. The PDDB server's
        // `Opcode::WriteKey` handler at xous-core/services/pddb/src/
        // main.rs:2293-2294 already calls `basis_cache.sync(...)` after
        // every key_update with the comment "for now, do an expensive
        // sync operation after every write to ensure data integrity",
        // so data is durable on WriteKey return. A client-side
        // `handle.flush()` issues `Opcode::WriteKeyFlush` whose handler
        // (main.rs:2313-2329) also calls `basis_cache.sync(...)` — a
        // redundant multi-basis sync per put. Dropping it saves 1 IPC
        // and 1 basis sync per logical write.
        handle.write_all(value).map_err(|e| Error::backend(format!("write: {}", e)))?;
        Ok(())
    }

    fn put_batch(&self, entries: &[(&str, &str, &[u8])]) -> Result<(), Error> {
        if entries.is_empty() {
            return Ok(());
        }
        let guard = self.lock()?;
        // Single IPC, single trailing basis sync server-side. The
        // upstream `Opcode::WriteKeyBatch` handler applies each entry
        // with `truncate=true` so the `delete_key` prelude we'd
        // otherwise need for #14 is unnecessary on the batch path.
        guard
            .write_batch(entries)
            .map_err(|e| map_ipc_err(e, "write_batch"))
    }

    fn delete(&self, dict: &str, key: &str) -> Result<(), Error> {
        let guard = self.lock()?;
        match guard.delete_key(dict, key) {
            Ok(()) => Ok(()),
            Err(e) if e.kind == IpcErrorKind::NotFound => Ok(()),
            Err(e) => Err(map_ipc_err(e, "delete_key")),
        }
    }

    fn delete_dict(&self, dict: &str) -> Result<(), Error> {
        let guard = self.lock()?;
        guard.delete_dict(dict).map_err(|e| map_ipc_err(e, "delete_dict"))
    }

    fn list_keys(&self, dict: &str) -> Result<Vec<String>, Error> {
        let guard = self.lock()?;
        match guard.list_keys(dict) {
            Ok(v) => Ok(v),
            Err(e) if e.kind == IpcErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(map_ipc_err(e, "list_keys")),
        }
    }
}

/// Drain `handle` into `out`. Mirrors `std::io::Read::read_to_end`
/// but bounds the read on the chunk size (4 KiB matches PDDB's
/// per-buffer ceiling).
fn read_all(handle: &mut KeyHandle<'_>, out: &mut Vec<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 4096];
    loop {
        let n = handle.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

fn map_ipc_err(e: xous_pddb_ipc::Error, ctx: &str) -> Error {
    match e.kind {
        IpcErrorKind::NotMounted => Error::backend(format!("{}: PDDB not mounted", ctx)),
        IpcErrorKind::AccessDenied => Error::backend(format!("{}: access denied", ctx)),
        IpcErrorKind::NoFreeSpace => Error::backend(format!("{}: out of space", ctx)),
        IpcErrorKind::InvalidInput => Error::backend(format!("{}: invalid input ({})", ctx, e.msg)),
        IpcErrorKind::NotFound => Error::backend(format!("{}: not found", ctx)),
        IpcErrorKind::Internal | IpcErrorKind::Ipc => {
            Error::backend(format!("{}: {}", ctx, e.msg))
        }
    }
}
