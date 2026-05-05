//! Glue between `presage::Manager` and the Xous IPC server.
//!
//! Hosts a dedicated Xous thread that runs an `async_executor::LocalExecutor`,
//! owns the `presage::Manager`, and forwards Cmd opcodes (from the IPC server)
//! and Event values (back to the IPC server) over `async-channel` queues.
//! See `docs/REPORT.md` Decision 4.
//!
//! Stage 0: skeleton only.
