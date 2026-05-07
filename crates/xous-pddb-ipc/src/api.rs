//! PDDB wire protocol — verbatim copies of the structs and opcodes
//! the server expects. Source of truth:
//! `~/precursor-signal/repos/xous-core/services/pddb/src/api.rs`.
//!
//! Anything not exercised by the `KvBackend`-driving call paths
//! (basis management, attribute queries, bulk reads, …) is stripped
//! out. If you find yourself needing a struct that's not here, copy
//! it from the upstream `api.rs` rather than re-deriving — the wire
//! schema must match exactly.

use bitfield::bitfield;
use num_derive::{FromPrimitive, ToPrimitive};

pub const SERVER_NAME_PDDB: &str = "_Plausibly Deniable Database_";
pub const SERVER_NAME_PDDB_POLLER: &str = "_PDDB Mount Poller_";

pub const BASIS_NAME_LEN: usize = 64;
pub const DICT_NAME_LEN: usize = 127 - 4 - 4 - 4 - 4; // 111
pub const KEY_NAME_LEN: usize = 127 - 8 - 8 - 8 - 4 - 4; // 95

pub type ApiToken = [u32; 3];

/// PDDB main server opcodes. Subset of the upstream `Opcode` enum —
/// only what we actually send. Discriminants are taken verbatim from
/// `services/pddb/src/api.rs` and **must not be reordered**.
#[derive(Debug, Clone, Copy, FromPrimitive, ToPrimitive)]
#[repr(u32)]
pub enum Opcode {
    IsMounted = 0,
    DeleteKey = 8,
    DeleteDict = 9,
    KeyAttributes = 10,
    KeyCountInDict = 11,
    KeyRequest = 15,
    ReadKey = 16,
    WriteKey = 17,
    WriteKeyFlush = 18,
    KeyDrop = 20,
    ListKeyV2 = 45,
}

/// Mount poller opcodes (different SID from the main server).
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum PollOp {
    Poll = 0,
}

/// Per-request return code embedded in IPC structs.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
pub enum PddbRequestCode {
    Create = 0,
    Open = 1,
    Close = 2,
    Delete = 3,
    NoErr = 4,
    NotMounted = 5,
    NoFreeSpace = 6,
    NotFound = 7,
    InternalError = 8,
    AccessDenied = 9,
    Uninit = 10,
    DuplicateEntry = 11,
    BulkRead = 12,
}

/// Per-buffer return code on streaming reads/writes.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PddbRetcode {
    Uninit = 0,
    Ok = 1,
    BasisLost = 2,
    AccessDenied = 3,
    UnexpectedEof = 4,
    InternalError = 5,
    DiskFull = 6,
}

bitfield! {
    /// Per-key flags reported by `KeyAttributes`. We don't manipulate
    /// these directly — they're plumbed through verbatim so the
    /// `PddbKeyAttrIpc` struct's wire layout matches.
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub struct KeyFlags(u32);
    impl Debug;
    pub valid, set_valid: 0;
    pub unresolved, set_unresolved: 1;
}

/// IPC payload for `Opcode::KeyRequest` (open / create-and-open a
/// dict + key). Server fills `token` and `result` on return.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PddbKeyRequest {
    pub basis_specified: bool,
    pub basis: String,
    pub dict: String,
    pub key: String,
    pub token: Option<ApiToken>,
    pub create_dict: bool,
    pub create_key: bool,
    pub alloc_hint: Option<u64>,
    pub cb_sid: Option<[u32; 4]>,
    pub result: PddbRequestCode,
}

/// IPC payload for `Opcode::DeleteKey`, `Opcode::DeleteDict`,
/// `Opcode::KeyCountInDict`. Server reads `dict` (and sometimes
/// `key`) and fills `code` / `key_count` / `found_key_count`.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
pub struct PddbDictRequest {
    pub basis_specified: bool,
    pub basis: String,
    pub dict: String,
    pub key: String,
    pub index: u32,
    pub token: [u32; 4],
    pub code: PddbRequestCode,
    pub bulk_limit: Option<usize>,
    pub key_count: u32,
    pub found_key_count: u32,
}

/// IPC payload for `Opcode::ListKeyV2`. Server packs the dictionary's
/// keys into `data` as a sequence of `(u8 length, [u8] name)` records,
/// terminated by a 0-length record. Multi-call: server sets `end =
/// true` on the last response.
pub const MAX_PDDBKLISTLEN: usize = 4064;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PddbKeyList {
    pub token: [u32; 4],
    pub data: [u8; MAX_PDDBKLISTLEN],
    pub retcode: PddbRetcode,
    pub end: bool,
}

/// Page-aligned streaming buffer for `Opcode::ReadKey` and
/// `Opcode::WriteKey`. C-repr because the server reads it as a raw
/// page (not rkyv-serialized) — `xous_ipc::Buffer::lend_mut`
/// remap-lends the page directly.
pub const PDDB_BUF_DATA_LEN: usize = 4072;

#[repr(C, align(4096))]
pub struct PddbBuf {
    pub token: ApiToken,
    pub retcode: PddbRetcode,
    pub reserved: u8,
    pub len: u16,
    pub position: u64,
    pub data: [u8; PDDB_BUF_DATA_LEN],
}

impl PddbBuf {
    /// Cast a `&mut [u8]` slice (from `Buffer::as_mut`) to a `&mut
    /// PddbBuf`. The lifetime of the returned reference is bounded
    /// by the slice; PDDB's server will mutate it in-place during
    /// `lend_mut`.
    ///
    /// Only the slice's *pointer* is used — its length is
    /// deliberately ignored. `Buffer::as_mut` returns `[..self.used]`
    /// which is 0 on a fresh `Buffer::new(4096)`, but the underlying
    /// allocation is always page-rounded (≥ 4096 bytes). Upstream
    /// `services/pddb/src/api.rs::PddbBuf::from_slice_mut` does the
    /// same pointer-only cast.
    pub fn from_slice_mut(s: &mut [u8]) -> &mut Self {
        // Safety: caller passes a buffer backed by a page-aligned
        // 4096-byte allocation; the slice may report a shorter
        // length but the underlying memory is the full page.
        unsafe { &mut *(s.as_mut_ptr() as *mut Self) }
    }
}

const _ASSERT_PDDB_BUF_SIZE: () = {
    if core::mem::size_of::<PddbBuf>() != 4096 {
        panic!("PddbBuf must be exactly one 4096-byte page");
    }
};

// ---- Error type ------------------------------------------------------

/// All operation outcomes funnel through this. Mirrors the relevant
/// `std::io::ErrorKind` cases plus a few PDDB-specific ones, kept
/// stringly-typed at the boundary for IPC-friendliness.
#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    pub msg: String,
}

impl Error {
    pub fn new(kind: ErrorKind, msg: impl Into<String>) -> Self {
        Self { kind, msg: msg.into() }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.msg)
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotMounted,
    NotFound,
    AccessDenied,
    InvalidInput,
    NoFreeSpace,
    Internal,
    Ipc,
}
