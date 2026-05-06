#![warn(clippy::large_futures)]

mod errors;
pub mod manager;
pub mod model;
mod serde;
pub mod store;

pub use libsignal_service;
/// Protobufs used in Signal protocol and service communication
pub use libsignal_service::proto;

pub use errors::Error;
pub use manager::Manager;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "-rs-", env!("CARGO_PKG_VERSION"));

pub type AvatarBytes = Vec<u8>;

// Stage 7 (Xous fork): tokio-removal — see docs/REPORT.md Decision 2.
//
// Manager methods occasionally spawn fire-and-forget background tasks
// (sync-message replies, sticker downloads, contact upserts). Upstream
// uses `tokio::task::spawn_local` / `tokio::spawn`, which require an
// ambient Tokio runtime context. We don't have one — Xous worker threads
// run an `async_executor::LocalExecutor` instead.
//
// To avoid threading an executor parameter through every Manager
// constructor, we use a thread-local: the worker thread calls
// `presage::set_executor(...)` once at startup with a `'static`
// LocalExecutor reference, and the Manager spawn sites route through
// `presage::spawn_detached(...)`. The executor must outlive the program
// (typical pattern: `Box::leak(Box::new(LocalExecutor::new()))`).
pub mod runtime {
    use std::cell::RefCell;
    use std::future::Future;

    use async_executor::LocalExecutor;

    thread_local! {
        static PRESAGE_EXECUTOR: RefCell<Option<&'static LocalExecutor<'static>>> =
            const { RefCell::new(None) };
    }

    /// Install the per-thread `LocalExecutor` reference that presage's
    /// spawn sites will use. Call once on the worker thread before any
    /// Manager method.
    pub fn set_executor(exec: &'static LocalExecutor<'static>) {
        PRESAGE_EXECUTOR.with(|cell| *cell.borrow_mut() = Some(exec));
    }

    /// Spawn a `'static + !Send`-OK future as a detached background task
    /// on the thread-local executor. Panics if `set_executor` was not
    /// called on this thread first — that's a programmer error and
    /// failing fast catches it.
    pub fn spawn_detached<F>(future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        PRESAGE_EXECUTOR.with(|cell| {
            let exec = cell
                .borrow()
                .expect("presage::runtime::set_executor must be called on this thread first");
            exec.spawn(future).detach();
        });
    }
}

pub use runtime::{set_executor, spawn_detached};
