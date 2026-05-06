//! IPC vocabulary between the app's main thread and the
//! `presage::Manager` worker.
//!
//! Stage 8 establishes the channel-shaped IPC: `Cmd` flows main → worker,
//! `Event` flows worker → main. The async-channel duality
//! (`send_blocking`/`recv_blocking` from sync, `send`/`recv` from async)
//! lets the same channel pair work from both sides without `block_on`
//! gymnastics.
//!
//! Stage 10 adds the linking-flow vocabulary. Stage 11 will add
//! incoming-message events; Stage 12 adds the send-message command.
//! Each new variant is a contained additive change here.

/// Commands sent from the app (main thread) to the Manager worker.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Channel-roundtrip ping. The worker replies with `Event::Pong`
    /// without touching the store. Useful as a startup liveness probe.
    Hello,

    /// Ask the worker for `Manager::registration_data().whoami()` —
    /// at Stage 8 this fails fast inside `load_registered` because the
    /// mock store has no registration data, which is the expected path
    /// the test exercises. Once Stage 10 lands a linked store, this
    /// becomes the real "who am I to the Signal server" probe.
    GetWhoami,

    /// Stage 10: link this device as a secondary to a phone-resident
    /// Signal account. The worker calls
    /// `presage::Manager::link_secondary_device`, which streams a
    /// provisioning URL out (we forward as `Event::LinkUrl`) and
    /// blocks until the phone confirms. On success the worker emits
    /// `Event::LinkComplete` and *retains the resulting `Manager` for
    /// subsequent receive/send calls*. On failure it emits
    /// `Event::LinkError` and the worker stays unregistered.
    LinkDevice { device_name: String },

    /// Stage 10: cancel an in-flight link. Best-effort — if the
    /// worker is already past the URL step, the cancel may be ignored
    /// and the link completes anyway.
    LinkCancel,

    /// Tell the worker to drain its event channel and exit. The main
    /// thread sends this before joining the worker handle so we don't
    /// rely on dropping the cmd channel sender (which works but is
    /// implicit).
    Shutdown,
}

/// Events sent from the worker back to the app (main thread).
#[derive(Debug, Clone)]
pub enum Event {
    /// Reply to `Cmd::Hello`.
    Pong,

    /// Reply to `Cmd::GetWhoami`. We keep it as `Result<String, String>`
    /// rather than presage's `WhoAmIResponse` / `presage::Error<E>`:
    /// the IPC boundary forces stringification anyway (Xous IPC at
    /// Stage 9+ won't carry trait-object errors), and a stringly-typed
    /// outcome is what the UI ultimately needs.
    Whoami(Result<String, String>),

    /// Stage 10: the provisioning URL the user must scan/copy with
    /// their Signal phone. Emitted once per `Cmd::LinkDevice`,
    /// promptly after the worker calls `link_secondary_device`. The
    /// URL is a `tsdevice://...` deep-link — render as text or as a
    /// QR depending on the surface.
    LinkUrl(String),

    /// Stage 10: linking succeeded. The worker has just transitioned
    /// to `Manager<S, Registered>` internally and persisted the
    /// registration data via `StateStore`. The fields below are
    /// pulled from `RegistrationData` for display.
    LinkComplete {
        device_name: String,
        aci: String,
        phone: String,
    },

    /// Stage 10: linking failed before the user-visible UI could
    /// finish. Reasons include network error, timeout, and the phone
    /// rejecting the link request. String-typed because the IPC
    /// boundary forces stringification (same shape as `Whoami`).
    LinkError(String),

    /// Confirms the worker is winding down. The main thread joins
    /// after receiving this — same way Manager state machines on
    /// other Whisperfish ports signal teardown.
    ShuttingDown,
}
