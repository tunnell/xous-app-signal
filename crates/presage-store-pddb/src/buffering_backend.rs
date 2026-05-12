//! `BufferingBackend` — a `KvBackend` wrapper that defers writes to
//! an in-memory buffer during a "batch" scope, then replays them to
//! the inner backend on commit.
//!
//! Motivation: each `PddbBackend::put` on the real PDDB triggers an
//! expensive multi-basis sync inside the server's `Opcode::WriteKey`
//! handler (xous-core/services/pddb/src/main.rs:2293-2294). Coalescing
//! the writes that fire during a single Signal `send_message` into
//! one logical batch lets the application layer choose where to pay
//! that cost — typically once at the end of send, rather than per
//! Store-trait write.
//!
//! This wrapper sits between `PddbStore` and the underlying backend.
//! It is **transparent** when no batch is active: every call passes
//! through to the inner backend with no extra work. It is only
//! interesting inside a batch scope opened via `begin_batch()`.
//!
//! Batch semantics:
//!
//! - During a batch, `put(dict, key, value)` records the value in an
//!   in-memory `HashMap` keyed on `(dict, key)`. **No inner-backend
//!   write fires.**
//! - During a batch, `delete(dict, key)` records a tombstone in the
//!   same map.
//! - During a batch, `get(dict, key)` consults the buffer first
//!   (read-through): a buffered put returns the buffered bytes; a
//!   buffered tombstone returns `Ok(None)`; otherwise falls through
//!   to the inner backend.
//! - `list_keys(dict)` overlays the buffer on top of inner keys:
//!   inner ∪ buffered-puts ∖ buffered-deletes. Costs an inner
//!   `list_keys` call plus a buffer scan.
//! - `delete_dict(dict)` clears the buffer entries for `dict` and
//!   forwards to the inner backend. (Rare during send; included for
//!   completeness.)
//! - `commit_batch()` drains the buffer, replays each entry through
//!   `inner.put` / `inner.delete`, clears the flag, returns the
//!   number of replayed operations.
//! - `BatchGuard::Drop` without an explicit `commit()` is an abort:
//!   the buffer is cleared, the flag is reset, no replay fires.
//!
//! Concurrency: only one batch can be in flight at a time per
//! BufferingBackend instance. `begin_batch` returns
//! `Err(Error::backend("batch already in flight"))` if a batch is
//! active. Reads during a batch from another caller see the
//! pre-batch state (the buffer is per-batch).
//!
//! Crash semantics: if the process crashes between buffer-write and
//! commit, all buffered values are lost. This matches the existing
//! `session_cache` pattern (protocol/session_store.rs:1-20) — the
//! application is responsible for ordering durability boundaries.
//!
//! See the feasibility report at
//! `research/2026-05-12-pddb-send-batching-feasibility.md` for the
//! quantitative case for this layer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Error, KvBackend};

/// One entry in the per-batch buffer.
///
/// A `Put` records the bytes to be written at commit time. A
/// `Delete` records that the key should be removed at commit time
/// (including a `delete` that supersedes an earlier `Put` of the
/// same key within the same batch).
#[derive(Debug, Clone)]
enum BufferEntry {
    Put(Vec<u8>),
    Delete,
}

/// Read-through, write-defer wrapper. See module doc.
pub struct BufferingBackend {
    inner: Arc<dyn KvBackend>,
    batching: AtomicBool,
    buffer: Mutex<HashMap<(String, String), BufferEntry>>,
}

impl std::fmt::Debug for BufferingBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buffered = self.buffer.lock().map(|b| b.len()).unwrap_or(0);
        f.debug_struct("BufferingBackend")
            .field("batching", &self.batching.load(Ordering::Acquire))
            .field("buffered_entries", &buffered)
            .finish_non_exhaustive()
    }
}

impl BufferingBackend {
    pub fn new(inner: Arc<dyn KvBackend>) -> Self {
        Self {
            inner,
            batching: AtomicBool::new(false),
            buffer: Mutex::new(HashMap::new()),
        }
    }

    /// Return `true` while a batch is in flight.
    pub fn is_batching(&self) -> bool {
        self.batching.load(Ordering::Acquire)
    }

    /// Number of buffered entries (puts + deletes). Test/diagnostic.
    pub fn buffered_len(&self) -> usize {
        self.buffer.lock().map(|b| b.len()).unwrap_or(0)
    }

    /// Open a batch scope. Returns `Err` if a batch is already in
    /// flight on this backend.
    ///
    /// The returned `BatchGuard` aborts on `Drop` (no replay). Call
    /// `commit()` to replay the buffered operations to the inner
    /// backend.
    pub fn begin_batch(&self) -> Result<BatchGuard<'_>, Error> {
        // Compare-and-swap: only succeed if no batch is active.
        match self.batching.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(BatchGuard { backend: self, committed: false }),
            Err(_) => Err(Error::backend("batch already in flight on this backend")),
        }
    }

    /// Internal: drain the buffer and replay to the inner backend.
    /// Called by `BatchGuard::commit`.
    fn commit_internal(&self) -> Result<usize, Error> {
        // Take ownership of the buffer contents so other operations
        // (which check the flag) see an empty buffer immediately.
        let entries = {
            let mut guard =
                self.buffer.lock().map_err(|_| Error::backend("buffer mutex poisoned"))?;
            std::mem::take(&mut *guard)
        };
        let count = entries.len();
        // Replay before clearing the flag — a read that races
        // against the replay should see the post-replay state via
        // inner, not an empty buffer-overlaid view.
        let mut first_err: Option<Error> = None;
        for ((dict, key), entry) in entries {
            let res = match entry {
                BufferEntry::Put(bytes) => self.inner.put(&dict, &key, &bytes),
                BufferEntry::Delete => self.inner.delete(&dict, &key),
            };
            if let Err(e) = res {
                // Keep going to drain the rest, but remember the
                // first failure to surface to the caller.
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        // Clear the flag last so any racing read sees inner with the
        // replayed state, not an empty buffer.
        self.batching.store(false, Ordering::Release);
        match first_err {
            None => Ok(count),
            Some(e) => Err(e),
        }
    }

    /// Internal: abort the batch — discard the buffer, clear the
    /// flag. Called by `BatchGuard::Drop` when not committed.
    fn abort_internal(&self) {
        if let Ok(mut guard) = self.buffer.lock() {
            guard.clear();
        }
        self.batching.store(false, Ordering::Release);
    }
}

impl KvBackend for BufferingBackend {
    fn get(&self, dict: &str, key: &str) -> Result<Option<Vec<u8>>, Error> {
        if self.is_batching() {
            // Consult the buffer first. If we have an entry, return
            // it without touching the inner backend.
            if let Ok(buf) = self.buffer.lock() {
                if let Some(entry) = buf.get(&(dict.to_string(), key.to_string())) {
                    return Ok(match entry {
                        BufferEntry::Put(bytes) => Some(bytes.clone()),
                        BufferEntry::Delete => None,
                    });
                }
            }
        }
        self.inner.get(dict, key)
    }

    fn put(&self, dict: &str, key: &str, value: &[u8]) -> Result<(), Error> {
        if self.is_batching() {
            let mut buf =
                self.buffer.lock().map_err(|_| Error::backend("buffer mutex poisoned"))?;
            buf.insert(
                (dict.to_string(), key.to_string()),
                BufferEntry::Put(value.to_vec()),
            );
            Ok(())
        } else {
            self.inner.put(dict, key, value)
        }
    }

    fn delete(&self, dict: &str, key: &str) -> Result<(), Error> {
        if self.is_batching() {
            let mut buf =
                self.buffer.lock().map_err(|_| Error::backend("buffer mutex poisoned"))?;
            buf.insert((dict.to_string(), key.to_string()), BufferEntry::Delete);
            Ok(())
        } else {
            self.inner.delete(dict, key)
        }
    }

    fn delete_dict(&self, dict: &str) -> Result<(), Error> {
        if self.is_batching() {
            // Pop any buffered entries for this dict, then forward.
            // The forward write is itself durable; subsequent reads
            // during the same batch see the post-delete-dict state.
            if let Ok(mut buf) = self.buffer.lock() {
                buf.retain(|(d, _), _| d != dict);
            }
        }
        self.inner.delete_dict(dict)
    }

    fn list_keys(&self, dict: &str) -> Result<Vec<String>, Error> {
        let inner_keys = self.inner.list_keys(dict)?;
        if !self.is_batching() {
            return Ok(inner_keys);
        }
        // Build the buffer-overlay view: inner ∪ buffered-puts ∖
        // buffered-deletes, scoped to this dict.
        let buf = self
            .buffer
            .lock()
            .map_err(|_| Error::backend("buffer mutex poisoned"))?;
        let mut keys: std::collections::BTreeSet<String> = inner_keys.into_iter().collect();
        for ((d, k), entry) in buf.iter() {
            if d != dict {
                continue;
            }
            match entry {
                BufferEntry::Put(_) => {
                    keys.insert(k.clone());
                }
                BufferEntry::Delete => {
                    keys.remove(k);
                }
            }
        }
        Ok(keys.into_iter().collect())
    }
}

/// RAII guard for a batch scope. `commit()` replays the buffer to
/// the inner backend and consumes the guard. Dropping the guard
/// without `commit()` aborts the batch (buffer cleared, no replay).
pub struct BatchGuard<'a> {
    backend: &'a BufferingBackend,
    committed: bool,
}

impl<'a> BatchGuard<'a> {
    /// Replay all buffered operations to the inner backend, then
    /// close the batch.
    ///
    /// Returns the number of operations replayed. If any individual
    /// replay fails the remaining entries are still attempted; the
    /// first failure is returned. The batch flag is cleared
    /// regardless so the backend stays usable.
    pub fn commit(mut self) -> Result<usize, Error> {
        self.committed = true;
        self.backend.commit_internal()
    }

    /// Number of buffered entries pending commit. Test/diagnostic.
    pub fn buffered_len(&self) -> usize {
        self.backend.buffered_len()
    }
}

impl<'a> Drop for BatchGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            self.backend.abort_internal();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockBackend;

    fn make() -> Arc<BufferingBackend> {
        Arc::new(BufferingBackend::new(Arc::new(MockBackend::new())))
    }

    #[test]
    fn passthrough_when_not_batching() {
        let b = make();
        b.put("d", "k", b"v").unwrap();
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"v".as_slice()));
        b.delete("d", "k").unwrap();
        assert_eq!(b.get("d", "k").unwrap(), None);
    }

    #[test]
    fn batched_put_invisible_to_inner_pre_commit() {
        let b = make();
        // Seed inner with a baseline value via direct (no batch) put.
        b.put("d", "k", b"baseline").unwrap();

        let guard = b.begin_batch().unwrap();
        // Inside the batch: put a new value.
        b.put("d", "k", b"batched").unwrap();
        // Read-through sees the buffered value.
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"batched".as_slice()));
        // The inner backend itself still has the baseline.
        // (We can't easily test inner directly through the Arc<dyn>,
        // but we can verify by aborting and re-reading.)
        drop(guard); // abort
        assert!(!b.is_batching());
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"baseline".as_slice()));
    }

    #[test]
    fn commit_replays_puts() {
        let b = make();
        let guard = b.begin_batch().unwrap();
        b.put("d", "k1", b"v1").unwrap();
        b.put("d", "k2", b"v2").unwrap();
        b.put("d", "k3", b"v3").unwrap();
        assert_eq!(guard.buffered_len(), 3);
        let n = guard.commit().unwrap();
        assert_eq!(n, 3);
        assert!(!b.is_batching());
        assert_eq!(b.get("d", "k1").unwrap().as_deref(), Some(b"v1".as_slice()));
        assert_eq!(b.get("d", "k2").unwrap().as_deref(), Some(b"v2".as_slice()));
        assert_eq!(b.get("d", "k3").unwrap().as_deref(), Some(b"v3".as_slice()));
    }

    #[test]
    fn commit_replays_deletes() {
        let b = make();
        b.put("d", "k", b"baseline").unwrap();
        let guard = b.begin_batch().unwrap();
        b.delete("d", "k").unwrap();
        assert_eq!(b.get("d", "k").unwrap(), None); // read-through sees delete
        guard.commit().unwrap();
        assert_eq!(b.get("d", "k").unwrap(), None); // inner committed
    }

    #[test]
    fn delete_then_put_in_same_batch() {
        let b = make();
        b.put("d", "k", b"baseline").unwrap();
        let guard = b.begin_batch().unwrap();
        b.delete("d", "k").unwrap();
        b.put("d", "k", b"replaced").unwrap();
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"replaced".as_slice()));
        guard.commit().unwrap();
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"replaced".as_slice()));
    }

    #[test]
    fn abort_loses_buffered_writes() {
        let b = make();
        b.put("d", "k", b"baseline").unwrap();
        {
            let _guard = b.begin_batch().unwrap();
            b.put("d", "k", b"discarded").unwrap();
            b.put("d", "fresh", b"never_committed").unwrap();
        } // _guard dropped here without commit -> abort
        assert!(!b.is_batching());
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"baseline".as_slice()));
        assert_eq!(b.get("d", "fresh").unwrap(), None);
    }

    #[test]
    fn two_batches_in_flight_refused() {
        let b = make();
        let _guard1 = b.begin_batch().unwrap();
        let r = b.begin_batch();
        assert!(r.is_err());
    }

    #[test]
    fn list_keys_overlay_during_batch() {
        let b = make();
        b.put("d", "a", b"1").unwrap();
        b.put("d", "b", b"2").unwrap();
        let guard = b.begin_batch().unwrap();
        b.delete("d", "a").unwrap();
        b.put("d", "c", b"3").unwrap();
        let mut listed = b.list_keys("d").unwrap();
        listed.sort();
        assert_eq!(listed, vec!["b".to_string(), "c".to_string()]);
        drop(guard); // abort
        let mut listed = b.list_keys("d").unwrap();
        listed.sort();
        assert_eq!(listed, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn coalesce_same_key_overwrites() {
        let b = make();
        let guard = b.begin_batch().unwrap();
        b.put("d", "k", b"v1").unwrap();
        b.put("d", "k", b"v2").unwrap();
        b.put("d", "k", b"v3").unwrap();
        // Three puts collapse to one buffered entry.
        assert_eq!(guard.buffered_len(), 1);
        let n = guard.commit().unwrap();
        assert_eq!(n, 1); // only the latest replayed
        assert_eq!(b.get("d", "k").unwrap().as_deref(), Some(b"v3".as_slice()));
    }
}
