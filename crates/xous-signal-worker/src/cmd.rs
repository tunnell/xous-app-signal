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

    /// Read account-identity info from the loaded Manager and
    /// reply via `Event::AccountInfo`. UI uses this to populate
    /// the Profile screen on a cold start where the device was
    /// already linked from a prior session — `Event::LinkComplete`
    /// only fires on a fresh link or when load_registered succeeds
    /// during the worker's startup retry budget (which can expire
    /// before PDDB unlocks). Re-tries `load_registered` if the
    /// Manager isn't loaded yet.
    GetAccountInfo,

    /// User-triggered: contact-sync round-trip with the linked phone.
    /// Calls `manager.request_contacts()`, which sends a self-targeted
    /// `SyncMessage.Request{type=CONTACTS}`; the primary phone replies
    /// with the contacts blob; presage's existing `SynchronizeMessage`
    /// handler saves the entries to `ContentsStore`. After completion,
    /// the worker walks each newly-saved contact and emits one
    /// `Event::ContactResolved` per (uuid, name) so the UI can swap any
    /// rendered UUID labels for the synced names without restarting.
    ///
    /// Skipped on link by default (it adds 30-90s of WS roundtrips on
    /// rv32 and races the WS-rotation problem); F2 in the post-link UI
    /// is the user-triggered way to populate contacts on demand.
    SyncContacts,

    /// User-triggered: log out from the linked Signal account.
    /// Tears down the manager (drops the WS pump, ends the receive
    /// stream), wipes the PDDB-backed Store (registration data,
    /// identity keypairs, sender cert, master key, all protocol-store
    /// dictionaries, all messages-by-thread, all profiles + contacts),
    /// and emits `Event::LoggedOut`. The worker stays alive — the user
    /// can immediately send a fresh `Cmd::LinkDevice` to relink.
    ///
    /// Note: this does NOT remove the device from the primary phone's
    /// Linked Devices list. The user must do that manually if they
    /// want; otherwise the entry sits in the list as a stale link.
    /// (Removing it from the primary requires `unlink_secondary`,
    /// which is a primary-only API per presage.)
    Logout,

    /// User-triggered: lookup a Signal username (e.g., `alice.42`)
    /// to its ACI, so the UI can open a `Screen::Thread { uuid }`
    /// for it. Result arrives as `Event::ContactResolveResult`.
    ResolveUsername(String),

    /// Tell the worker to drain its event channel and exit. The main
    /// thread sends this before joining the worker handle so we don't
    /// rely on dropping the cmd channel sender (which works but is
    /// implicit).
    Shutdown,
}

/// Identity fields read from `Manager::registration_data()`.
/// Mirrors the shape of `Event::LinkComplete` but lives in its
/// own struct since we now also surface it via `Event::AccountInfo`.
#[derive(Debug, Clone)]
pub struct AccountInfoData {
    pub device_name: String,
    pub aci: String,
    pub phone: String,
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

    /// Reply to `Cmd::GetAccountInfo`. Carries the same fields as
    /// `LinkComplete` but fires on demand rather than tied to the
    /// link/load lifecycle. `Err(reason)` when the manager isn't
    /// loaded (e.g. PDDB still locked or the device was never
    /// linked).
    AccountInfo(Result<AccountInfoData, String>),

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

    /// First-touch profile lookup completed for a sender we
    /// previously surfaced as `Event::Message { sender_name: None }`.
    /// The UI walks its in-RAM messages and replaces any rows whose
    /// `author_label` is the bare UUID with this `name`.
    ///
    /// Best-effort: emitted only after the receive stream is paused
    /// (between iterations of `manager_task`'s outer loop), so the
    /// UI may continue showing the UUID for some seconds after the
    /// first message arrives. On 404 (sender opted out of profile
    /// fetches) or other error, no event is emitted — the UI keeps
    /// the UUID indefinitely until the next user-triggered Sync.
    ContactResolved {
        /// ACI of the resolved sender, formatted as a UUID.
        aci_uuid: presage::libsignal_service::prelude::Uuid,
        /// Profile name (`given_name [+ ' ' + family_name]`) decoded
        /// from the cipher response. Empty profile names are filtered
        /// out at the worker — this `name` is always non-empty.
        name: String,
    },

    /// `Cmd::SyncContacts` finished. The worker has already emitted
    /// one `ContactResolved` per (uuid, name) it saw in the freshly-
    /// synced contacts table. UI uses this terminator to flip the
    /// "Syncing…" indicator off.
    SyncComplete,

    /// `Cmd::SyncContacts` failed. UI surfaces the reason via a
    /// modal/notification.
    SyncError(String),

    /// `Cmd::Logout` finished. Manager dropped, store wiped. UI
    /// resets account state, clears messages/dialogues, transitions
    /// to `Screen::Menu` with `MenuItem::Link`.
    LoggedOut,

    /// Server-forced auth expiry: Signal sent WS close code 4401
    /// ("Reauthentication required") and our credential-refresh
    /// path failed to recover (N consecutive 403s on the refreshed
    /// WS). Emitted by `manager_task` after the reauth retry budget
    /// is exhausted. UI mirrors the `LoggedOut` reset but surfaces
    /// the reason as a banner so the user understands they need to
    /// re-link rather than thinking the device is generally broken.
    /// See #13 for the underlying bug. The String is a short
    /// human-readable reason (typically the last 403 response body
    /// or a synthesized "reauthentication failed after N retries").
    SignalAuthExpired(String),

    /// Server-forced displacement: Signal sent WS close code 4409
    /// ("Connected elsewhere"). This fires when a different
    /// authenticated WS for the same (accountIdentifier, deviceId)
    /// pair connects to the server — Signal-Server's
    /// `ConflictingMessageConsumerException` displaces the older
    /// listener. Common scenario: another xas instance, or this
    /// same xas's previous WS that got displaced by a fresh
    /// reconnect while the server's slot hadn't cleared yet.
    /// Emitted by `manager_task` when the receive stream sees
    /// 4409 — treated as terminal rather than retried, because
    /// auto-reconnect would just self-displace again. UI mirrors
    /// the `LoggedOut` reset with a banner explaining that another
    /// device or app instance took over. User re-links to use this
    /// device. See #1 for the underlying bug. The String is a
    /// short human-readable reason for diagnostics (typically
    /// "Server reported 4409 'Connected elsewhere' — another
    /// device with this Signal account is active.").
    SignalConflictingDevice(String),

    /// `Cmd::ResolveUsername` finished. Result is `Some(aci)` if the
    /// username resolved (UI opens a Thread for that uuid),
    /// `None` if no such username exists, or
    /// `Err(reason)` for any other failure (network, etc.).
    UsernameResolveResult(Result<Option<presage::libsignal_service::prelude::Uuid>, String>),

    /// Confirms the worker is winding down. The main thread joins
    /// after receiving this — same way Manager state machines on
    /// other Whisperfish ports signal teardown.
    ShuttingDown,
}
