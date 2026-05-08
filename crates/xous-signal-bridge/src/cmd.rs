//! IPC vocabulary between the app's main thread and the
//! `presage::Manager` worker.
//!
//! `Cmd` flows main → worker, `Event` flows worker → main. The
//! async-channel duality (`send_blocking`/`recv_blocking` from sync,
//! `send`/`recv` from async) lets the same channel pair work from
//! both sides without `block_on` gymnastics.
//!
//! New variants for linking, incoming messages, and send-message are
//! contained additive changes here.

/// Commands sent from the app (main thread) to the Manager worker.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Channel-roundtrip ping. The worker replies with `Event::Pong`
    /// without touching the store. Useful as a startup liveness probe.
    Hello,

    /// Ask the worker for `Manager::registration_data().whoami()`.
    /// On a fresh install this fails fast inside `load_registered`
    /// because the mock store has no registration data, which is the
    /// expected path the test exercises. Once a linked store lands,
    /// this becomes the real "who am I to the Signal server" probe.
    GetWhoami,

    /// Link this device as a secondary to a phone-resident Signal
    /// account. The worker calls
    /// `presage::Manager::link_secondary_device`, which streams a
    /// provisioning URL out (we forward as `Event::LinkUrl`) and
    /// blocks until the phone confirms. On success the worker emits
    /// `Event::LinkComplete` and *retains the resulting `Manager` for
    /// subsequent receive/send calls*. On failure it emits
    /// `Event::LinkError` and the worker stays unregistered.
    LinkDevice { device_name: String },

    /// Cancel an in-flight link. Best-effort — if the
    /// worker is already past the URL step, the cancel may be ignored
    /// and the link completes anyway.
    LinkCancel,

    /// Start the receive loop. Requires a linked Manager
    /// (i.e. Cmd::LinkDevice must have completed and persisted, or
    /// the worker must have rehydrated from PDDB at startup). The
    /// worker moves the Manager into a long-running "manager task"
    /// that streams `Received` items from `Manager::receive_messages`,
    /// translates each `Received::Content` into `Event::Message`,
    /// and calls `store.flush_sessions()` on `Received::QueueEmpty`.
    /// The manager task also multiplexes inbound `Cmd::SendMessage`
    /// requests by selecting between the receive stream and an
    /// internal send channel; on each send it drops the stream, calls
    /// `Manager::send_message`, then re-opens the stream.
    StartReceive,

    /// Send a 1:1 text message to the named recipient.
    /// Routed through the same manager task that owns the receive
    /// stream. `recipient` is a `service_id_string()` (e.g.
    /// `"00000000-0000-4000-8000-000000000001"` for an Aci); the
    /// worker parses it back into `ServiceId` before calling
    /// `Manager::send_message`. `body` is the plaintext UTF-8
    /// message body. `timestamp` is the client-generated unix-ms
    /// timestamp the UI used when it optimistically appended the
    /// outgoing message to its in-RAM history; the worker echoes
    /// this same value back in `Event::SendComplete { timestamp }`
    /// and `Event::SendError { timestamp: Some(_) }` so the UI
    /// can correlate event-to-pending-message in its optimistic-
    /// render path. Worker also uses this as the Signal-protocol
    /// timestamp on the wire.
    ///
    /// Requires `Cmd::StartReceive` to have run first — the manager
    /// task is the only place the Manager is reachable. Sending
    /// before receive starts gets a `SendError { reason: "not
    /// receiving; ...", timestamp: None }`.
    SendMessage { recipient: String, body: String, timestamp: u64 },

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
    /// the IPC boundary forces stringification anyway (Xous IPC
    /// won't carry trait-object errors), and a stringly-typed
    /// outcome is what the UI ultimately needs.
    Whoami(Result<String, String>),

    /// The provisioning URL the user must scan/copy with
    /// their Signal phone. Emitted once per `Cmd::LinkDevice`,
    /// promptly after the worker calls `link_secondary_device`. The
    /// URL is a `tsdevice://...` deep-link — render as text or as a
    /// QR depending on the surface.
    LinkUrl(String),

    /// Linking succeeded. The worker has just transitioned
    /// to `Manager<S, Registered>` internally and persisted the
    /// registration data via `StateStore`. The fields below are
    /// pulled from `RegistrationData` for display.
    LinkComplete {
        device_name: String,
        aci: String,
        phone: String,
    },

    /// Linking failed before the user-visible UI could
    /// finish. Reasons include network error, timeout, and the phone
    /// rejecting the link request. String-typed because the IPC
    /// boundary forces stringification (same shape as `Whoami`).
    LinkError(String),

    /// Confirms the receive loop is established. Emitted
    /// after `Manager::receive_messages` has returned a stream and
    /// the worker is parked on its `next()`. UI uses this to
    /// transition the status indicator from "starting" to "listening".
    ReceiveStarted,

    /// A single decrypted incoming message. Fields are
    /// flattened from `presage::libsignal_service::content::Content`
    /// for the same IPC reason `LinkComplete` is — string-typed
    /// payloads cross the boundary cleanly. Only `DataMessage`-style
    /// text bodies are surfaced; control / sync / receipt messages
    /// from the receive stream are silently dropped at the worker
    /// (their effect on the store is already applied).
    Message {
        /// `service_id_string()` of the sender — the thread key the
        /// UI groups conversations by.
        sender: String,
        /// Display-friendly sender label resolved from the contacts
        /// store at the time the message was processed: `Some(phone)`,
        /// `Some(name)`, or `None` when the contact isn't in the store
        /// (first-sight from a peer who isn't yet synced from the
        /// linked phone). UIs should fall back to `sender` (the UUID)
        /// when this is `None`.
        sender_phone: Option<String>,
        sender_name: Option<String>,
        /// Plaintext body. Empty string means an attachment-only or
        /// reaction-only DataMessage that we can't render in MVP.
        body: String,
        /// Server timestamp (UNIX millis), matches what `Content`
        /// carries.
        timestamp: u64,
    },

    /// Receive loop hit a fatal error and unwound. The
    /// worker's Manager is consumed at this point; the user must
    /// re-link to resume.
    ReceiveError(String),

    /// A `Cmd::SendMessage` succeeded. `timestamp` is the
    /// server-side timestamp the message was tagged with — useful
    /// to the UI for echoing the sent message into the conversation
    /// list as an outgoing entry.
    SendComplete { timestamp: u64 },

    /// A `Cmd::SendMessage` failed. Common reasons:
    /// invalid recipient UUID, network error, recipient session
    /// expired (in which case Signal expects a re-key on next
    /// attempt). `timestamp` is the same client-generated timestamp
    /// the UI used when it optimistically appended the outgoing
    /// message; it lets the UI find the right pending row to mark
    /// failed. `None` means the failure happened before a timestamp
    /// was assigned (e.g. recipient parse failed, manager task died
    /// before the call).
    SendError { reason: String, timestamp: Option<u64> },

    /// Confirms the worker is winding down. The main thread joins
    /// after receiving this — same way Manager state machines on
    /// other Whisperfish ports signal teardown.
    ShuttingDown,
}
