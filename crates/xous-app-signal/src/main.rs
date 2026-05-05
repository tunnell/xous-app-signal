//! Xous Signal app entry point.
//!
//! Stage 1: smol-rs `LocalExecutor` + `async-channel` + `futures-timer`
//! smoke test. Spawns one task that sleeps then sends a string over a
//! channel; the main task receives and prints it. Validates the runtime
//! we'll use throughout the project per docs/REPORT.md Decision 2.
//!
//! See docs/ROADMAP.md Stage 1 for context.

use std::time::Duration;

use async_channel::bounded;
use async_executor::LocalExecutor;
use futures_lite::future::block_on;
use futures_timer::Delay;

fn main() {
    let executor = LocalExecutor::new();
    let (tx, rx) = bounded::<&'static str>(1);

    // Spawn the producer task. It's `!Send` because LocalExecutor doesn't
    // require Send; this matches the constraint we'll have once presage's
    // Store handle (which holds a !Send PddbStore handle on Xous) starts
    // appearing in spawned futures.
    let producer = executor.spawn(async move {
        Delay::new(Duration::from_millis(100)).await;
        tx.send("hello").await.expect("channel closed");
    });

    // Drive the executor and the consumer future on the current thread.
    block_on(executor.run(async {
        let msg = rx.recv().await.expect("producer dropped without sending");
        println!("got: {msg}");
        // Make sure the producer task actually completed too — catches
        // forgotten `.detach()` bugs where the spawn handle is dropped
        // and the task is cancelled before sending.
        producer.await;
    }));
}
