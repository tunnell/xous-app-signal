//! Real PDDB-backed `KvBackend` for rv32-xous targets.
//!
//! Stage 13b-2 lands the actual implementation. Wraps
//! `xous_pddb_ipc::PddbClient` (the hand-rolled IPC client; bypasses
//! `services/pddb`'s gen1 dep cascade) and forwards `KvBackend`
//! operations to PDDB's wire protocol.
//!
//! Behavior:
//!
//! - `get` opens with `create_dict=false, create_key=false` and
//!   reads the whole key into a Vec<u8>. Returns `Ok(None)` on
//!   `NotFound`, the bytes on `Ok`, error otherwise.
//! - `put` opens with `create_dict=true, create_key=true`, writes
//!   the value, calls `flush_writes` to commit. Releases the handle
//!   on Drop.
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

#![cfg(all(feature = "pddb-backend", target_os = "xous"))]

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
    /// `probe-pddb-real` feature (Stage 13b-2 follow-up) is the
    /// expected consumer. `dead_code` allowed because the method is
    /// part of the public surface, not a private helper.
    #[allow(dead_code)]
    pub fn is_mounted(&self) -> bool {
        match self.client.lock() {
            Ok(g) => g.is_mounted(),
            Err(_) => false,
        }
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
        let mut handle = guard
            .open(dict, key, OpenOptions::create_all())
            .map_err(|e| map_ipc_err(e, "open for write"))?;
        handle.write_all(value).map_err(|e| Error::backend(format!("write: {}", e)))?;
        handle.flush().map_err(|e| Error::backend(format!("flush: {}", e)))?;
        Ok(())
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
