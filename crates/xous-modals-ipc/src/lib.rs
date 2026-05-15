//! Hand-rolled Modals IPC client for the xas Signal app.
//!
//! Replicates just `show_notification` from xous-core's
//! `services/modals` so we can put a QR code on the LCD without
//! pulling in the full `gam`/`blitstr2`/`ux-api` dep cascade. Same
//! pattern as [`xous_pddb_ipc`].
//!
//! The wire structs (`ManagedNotification`, `Opcode::GetMutex`,
//! `Opcode::Notification`) are byte-compatible verbatim copies from
//! `~/precursor-signal/repos/xous-core/services/modals/src/api.rs`.
//! rkyv 0.8 ↔ 0.8 wire compatibility (same as the PDDB IPC client).
//!
//! Server-side: when `qrtext: Some(s)` is set, `services/modals/src/
//! main.rs` calls `notification.set_qrcode(qrtext)`, which renders a
//! QR-code image alongside the notification text on Precursor's
//! 336×536 monochrome LCD. Capacity: a Type 40 (177×177) QR with
//! Medium ECC encodes up to 3391 alphanumeric characters — comfortably
//! fits Signal's `tsdevice://` provisioning URLs.
//!
//! # Trust boundary
//!
//! Different shape from [`xous_pddb_ipc`]: no secret value bytes
//! cross this boundary, but the rendered text — typically a
//! provisioning URL or a status message — is GAM-rendered on the
//! local screen and contributes to the UX trust path. The
//! `message` and `qrtext` fields **are** caller-controlled
//! plaintext, so the caller is responsible for not routing
//! attacker-influenced bytes here. For xas the only production
//! caller is `xous_app_signal::main::auto_link`, which displays the
//! Signal-server-issued provisioning URL — see W-W.1 in
//! `~/REFACTOR_NOTES.md` for the open log-discipline item around
//! that URL.

#![cfg(target_os = "xous")]

use num_traits::ToPrimitive;
use xous::{CID, Message, send_message};
use xous_ipc::Buffer;

const SERVER_NAME_MODALS: &str = "_Modal Dialog Server_";

#[derive(num_derive::ToPrimitive)]
#[repr(u32)]
enum Opcode {
    /// `Notification = 2` — blocking modal with text + optional QR.
    Notification = 2,
    /// `GetMutex = 15` — claim the modals server's serializing mutex
    /// before issuing any user-facing op. Required by the upstream
    /// protocol; releasing happens implicitly on the next op return.
    GetMutex = 15,
}

/// Verbatim copy of `services/modals/src/api.rs::ManagedNotification`.
/// The wire layout must match exactly — adjust only if upstream
/// changes (and bump xous-ipc-pinned version simultaneously).
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
struct ManagedNotification {
    pub token: [u32; 4],
    pub message: String,
    pub qrtext: Option<String>,
}

/// Operation outcome. Same shape as `xous-pddb-ipc::Error` for
/// uniform handling at the call site.
#[derive(Debug)]
pub enum Error {
    Connect(xous::Error),
    Trng(xous::Error),
    Mutex(String),
    Ipc(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connect(e) => write!(f, "Modals connect failed: {:?}", e),
            Error::Trng(e) => write!(f, "Modals token TRNG fill failed: {:?}", e),
            Error::Mutex(m) => write!(f, "Modals mutex acquire failed: {}", m),
            Error::Ipc(m) => write!(f, "Modals IPC: {}", m),
        }
    }
}

impl std::error::Error for Error {}

/// Lightweight client for `services/modals`. One per process.
pub struct ModalsClient {
    conn: CID,
    /// Per-client identifier carried across the GetMutex / Notification
    /// pair so the server can tell which caller currently holds the
    /// modal. Initialized once at construction.
    token: [u32; 4],
}

impl ModalsClient {
    /// Connect to the modals server and seed our identifying token
    /// from xous-core's TRNG service. Both deps (`XousNames`, `Trng`)
    /// are passed in rather than constructed internally so the caller
    /// can reuse existing instances — important on rv32 where
    /// connection counts are bounded.
    pub fn new(xns: &xous_names::XousNames, trng: &trng::Trng) -> Result<Self, Error> {
        let conn = xns
            .request_connection_blocking(SERVER_NAME_MODALS)
            .map_err(Error::Connect)?;
        let mut token = [0u32; 4];
        trng.fill_buf(&mut token).map_err(Error::Trng)?;
        Ok(Self { conn, token })
    }

    /// Display a blocking notification modal. If `qrtext` is `Some`,
    /// the modal also renders the string as a QR code. Returns when
    /// the user presses any key to dismiss.
    pub fn show_notification(
        &self,
        message: &str,
        qrtext: Option<&str>,
    ) -> Result<(), Error> {
        // Step 1: claim the modals mutex. Returns `Scalar1(1)` on
        // success per `services/modals/src/lib.rs::lock`.
        match send_message(
            self.conn,
            Message::new_blocking_scalar(
                Opcode::GetMutex.to_usize().unwrap(),
                self.token[0] as usize,
                self.token[1] as usize,
                self.token[2] as usize,
                self.token[3] as usize,
            ),
        ) {
            Ok(xous::Result::Scalar1(1)) => {}
            Ok(xous::Result::Scalar1(other)) => {
                return Err(Error::Mutex(format!("unexpected GetMutex code {}", other)));
            }
            Ok(other) => {
                return Err(Error::Mutex(format!("unexpected GetMutex resp {:?}", other)));
            }
            Err(e) => return Err(Error::Mutex(format!("send: {:?}", e))),
        }

        // Step 2: lend the rkyv-serialized payload. The server
        // dispatches to the renderer thread, draws the modal +
        // optional QR, and parks until the user presses any key.
        // `lend` (not `lend_mut`) because the server doesn't write
        // back into our buffer — the call returns when the user
        // dismisses, with no payload.
        let spec = ManagedNotification {
            token: self.token,
            message: message.to_string(),
            qrtext: qrtext.map(|s| s.to_string()),
        };
        let buf = Buffer::into_buf(spec)
            .map_err(|_| Error::Ipc("Buffer::into_buf".to_string()))?;
        buf.lend(self.conn, Opcode::Notification.to_u32().unwrap())
            .map_err(|e| Error::Ipc(format!("lend Notification: {:?}", e)))?;

        Ok(())
    }
}

impl Drop for ModalsClient {
    fn drop(&mut self) {
        // Best-effort disconnect. If the kernel rejects (process
        // teardown), it'll clean up the CID via process exit.
        unsafe {
            let _ = xous::disconnect(self.conn);
        }
    }
}
