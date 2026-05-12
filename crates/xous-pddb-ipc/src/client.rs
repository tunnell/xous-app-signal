//! `PddbClient` — high-level wrapper over PDDB's IPC server.
//!
//! Mirrors `services/pddb/src/lib.rs` for the operations our
//! `KvBackend` needs. Keep the function shapes and error mapping
//! aligned so that swapping in `pddb::Pddb` later (if we ever vendor
//! it) is a one-line change.

use std::io::{self, Read, Write};

use num_traits::ToPrimitive;
use xous::{CID, Message, send_message};
use xous_ipc::Buffer;

use crate::api::{
    ApiToken, DICT_NAME_LEN, Error, ErrorKind, KEY_NAME_LEN, MAX_PDDBKLISTLEN, Opcode,
    PDDB_BUF_DATA_LEN, PddbBuf, PddbDictRequest, PddbKeyList, PddbKeyRequest, PddbRequestCode,
    PddbRetcode, SERVER_NAME_PDDB, SERVER_NAME_PDDB_POLLER,
};

/// Top-level client. Holds long-lived connections to both the main
/// PDDB server (for KV ops) and the mount poller (for non-blocking
/// mount-state checks).
#[derive(Debug)]
pub struct PddbClient {
    main_conn: CID,
    poller_conn: CID,
}

impl PddbClient {
    /// Look up the two PDDB SIDs and connect. Both connections are
    /// long-lived; `Drop` releases them.
    pub fn new() -> Result<Self, Error> {
        let xns = xous_names::XousNames::new().map_err(|e| {
            Error::new(ErrorKind::Ipc, format!("XousNames::new failed: {:?}", e))
        })?;
        let main_conn = xns.request_connection_blocking(SERVER_NAME_PDDB).map_err(|e| {
            Error::new(ErrorKind::Ipc, format!("connect to PDDB main server: {:?}", e))
        })?;
        let poller_conn = xns
            .request_connection_blocking(SERVER_NAME_PDDB_POLLER)
            .map_err(|e| {
                Error::new(ErrorKind::Ipc, format!("connect to PDDB mount poller: {:?}", e))
            })?;
        Ok(Self { main_conn, poller_conn })
    }

    /// Non-blocking mount check via the poller server (matches
    /// `services/pddb/src/lib.rs:30` `is_mounted_nonblocking`).
    /// Returns `false` if the poller's SID is unreachable, mirroring
    /// the upstream behavior.
    pub fn is_mounted(&self) -> bool {
        match send_message(self.poller_conn, Message::new_blocking_scalar(0, 0, 0, 0, 0)) {
            Ok(xous::Result::Scalar1(v)) => v != 0,
            _ => false,
        }
    }

    /// Trigger an interactive mount: pops the gam password modal,
    /// waits for the user to enter the password, then mounts. Blocks
    /// until the mount succeeds (`Ok(true)`) or the server declines
    /// (`Ok(false)` — e.g. wrong password, forced abort).
    ///
    /// Server returns `Scalar2(retcode, failcount)` where `retcode == 0`
    /// means success (mounted, or already mounted) — see
    /// `services/pddb/src/main.rs::Opcode::TryMount`.
    pub fn try_mount(&self) -> Result<bool, Error> {
        let resp = send_message(
            self.main_conn,
            Message::new_blocking_scalar(Opcode::TryMount.to_usize().unwrap(), 0, 0, 0, 0),
        )
        .map_err(|e| Error::new(ErrorKind::Ipc, format!("TryMount: {:?}", e)))?;
        match resp {
            xous::Result::Scalar2(retcode, _) => Ok(retcode == 0),
            xous::Result::Scalar1(retcode) => Ok(retcode == 0),
            other => Err(Error::new(ErrorKind::Ipc, format!("TryMount: {:?}", other))),
        }
    }

    /// Open (and optionally create) a `(dict, key)` pair, returning
    /// a streaming `KeyHandle` whose `Read`/`Write` impls round-trip
    /// `PddbBuf` pages.
    pub fn open(
        &self,
        dict: &str,
        key: &str,
        opts: OpenOptions,
    ) -> Result<KeyHandle<'_>, Error> {
        if dict.len() > DICT_NAME_LEN - 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "dictionary name too long"));
        }
        if key.len() > KEY_NAME_LEN - 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "key name too long"));
        }

        let request = PddbKeyRequest {
            basis_specified: false,
            basis: String::new(),
            dict: dict.to_string(),
            key: key.to_string(),
            token: None,
            create_dict: opts.create_dict,
            create_key: opts.create_key,
            alloc_hint: opts.alloc_hint.map(|h| h as u64),
            cb_sid: None,
            result: PddbRequestCode::Uninit,
        };
        let mut buf = Buffer::into_buf(request)
            .map_err(|_| Error::new(ErrorKind::Ipc, "Buffer::into_buf"))?;
        buf.lend_mut(self.main_conn, Opcode::KeyRequest.to_u32().unwrap())
            .map_err(|_| Error::new(ErrorKind::Ipc, "lend_mut KeyRequest"))?;
        let response: PddbKeyRequest = buf
            .to_original::<PddbKeyRequest, _>()
            .map_err(|_| Error::new(ErrorKind::Ipc, "to_original KeyRequest"))?;

        match response.result {
            PddbRequestCode::NoErr => {
                let token = response
                    .token
                    .ok_or_else(|| Error::new(ErrorKind::Internal, "server returned no token"))?;
                Ok(KeyHandle {
                    conn: self.main_conn,
                    token,
                    pos: 0,
                    buf: Buffer::new(core::mem::size_of::<PddbBuf>()),
                })
            }
            PddbRequestCode::AccessDenied => {
                Err(Error::new(ErrorKind::AccessDenied, "dict/key access denied"))
            }
            PddbRequestCode::NotMounted => Err(Error::new(ErrorKind::NotMounted, "PDDB not mounted")),
            PddbRequestCode::NoFreeSpace => Err(Error::new(ErrorKind::NoFreeSpace, "out of space")),
            PddbRequestCode::NotFound => Err(Error::new(ErrorKind::NotFound, "dict/key not found")),
            other => Err(Error::new(ErrorKind::Internal, format!("KeyRequest: {:?}", other))),
        }
    }

    /// Delete a single key. Wraps `Opcode::DeleteKey`.
    ///
    /// Server-side handler reads `PddbKeyRequest` (not `PddbDictRequest`) —
    /// see `services/pddb/src/main.rs::Opcode::DeleteKey` and the upstream
    /// client at `services/pddb/src/lib.rs::delete_key`.
    pub fn delete_key(&self, dict: &str, key: &str) -> Result<(), Error> {
        let request = PddbKeyRequest {
            basis_specified: false,
            basis: String::new(),
            dict: dict.to_string(),
            key: key.to_string(),
            token: None,
            create_dict: false,
            create_key: false,
            alloc_hint: None,
            cb_sid: None,
            result: PddbRequestCode::Uninit,
        };
        let mut buf = Buffer::into_buf(request)
            .map_err(|_| Error::new(ErrorKind::Ipc, "Buffer::into_buf"))?;
        buf.lend_mut(self.main_conn, Opcode::DeleteKey.to_u32().unwrap())
            .map_err(|_| Error::new(ErrorKind::Ipc, "lend_mut DeleteKey"))?;
        let response: PddbKeyRequest = buf
            .to_original::<PddbKeyRequest, _>()
            .map_err(|_| Error::new(ErrorKind::Ipc, "to_original DeleteKey"))?;
        match response.result {
            PddbRequestCode::NoErr => Ok(()),
            PddbRequestCode::NotFound => Err(Error::new(ErrorKind::NotFound, "key not found")),
            PddbRequestCode::NotMounted => {
                Err(Error::new(ErrorKind::NotMounted, "PDDB not mounted"))
            }
            other => Err(Error::new(ErrorKind::Internal, format!("DeleteKey: {:?}", other))),
        }
    }

    /// Delete a dictionary and all its keys. Wraps `Opcode::DeleteDict`.
    ///
    /// Server-side handler reads `PddbKeyRequest` — same wire format as
    /// `delete_key`. The original code path here used `PddbDictRequest`
    /// (a similar but distinct rkyv type), which the server happily
    /// deserialized with garbage field values, returning nonsensical
    /// codes like `Create`.
    pub fn delete_dict(&self, dict: &str) -> Result<(), Error> {
        let request = PddbKeyRequest {
            basis_specified: false,
            basis: String::new(),
            dict: dict.to_string(),
            key: String::new(),
            token: None,
            create_dict: false,
            create_key: false,
            alloc_hint: None,
            cb_sid: None,
            result: PddbRequestCode::Uninit,
        };
        let mut buf = Buffer::into_buf(request)
            .map_err(|_| Error::new(ErrorKind::Ipc, "Buffer::into_buf"))?;
        buf.lend_mut(self.main_conn, Opcode::DeleteDict.to_u32().unwrap())
            .map_err(|_| Error::new(ErrorKind::Ipc, "lend_mut DeleteDict"))?;
        let response: PddbKeyRequest = buf
            .to_original::<PddbKeyRequest, _>()
            .map_err(|_| Error::new(ErrorKind::Ipc, "to_original DeleteDict"))?;
        match response.result {
            PddbRequestCode::NoErr => Ok(()),
            PddbRequestCode::NotFound => Ok(()), // delete-non-existent is fine
            PddbRequestCode::NotMounted => {
                Err(Error::new(ErrorKind::NotMounted, "PDDB not mounted"))
            }
            other => Err(Error::new(ErrorKind::Internal, format!("DeleteDict: {:?}", other))),
        }
    }

    /// List all keys in a dictionary. Wraps `Opcode::KeyCountInDict`
    /// (gets count + token) followed by repeated `Opcode::ListKeyV2`
    /// calls (drains the packed key list one buffer at a time, each
    /// up to ~4 KiB of `(len-prefix, name)` records).
    ///
    /// Returns an empty `Vec` if the dict has no keys; returns a
    /// `NotFound` error if the dict itself doesn't exist (which is
    /// upstream's convention — see `services/pddb/src/lib.rs:list_keys`).
    pub fn list_keys(&self, dict: &str) -> Result<Vec<String>, Error> {
        if dict.len() > DICT_NAME_LEN - 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "dict name too long"));
        }

        // Phase 1: KeyCountInDict — gets the listing token. The token
        // disambiguates concurrent listings (PDDB's listing protocol
        // is stateful on the server side).
        let token = random_token_4();
        let request = PddbDictRequest {
            basis_specified: false,
            basis: String::new(),
            dict: dict.to_string(),
            key: String::new(),
            index: 0,
            token,
            code: PddbRequestCode::Uninit,
            bulk_limit: None,
            key_count: 0,
            found_key_count: 0,
        };
        let resp = self.dict_request_send(request, Opcode::KeyCountInDict)?;
        match resp.code {
            PddbRequestCode::NoErr => {}
            PddbRequestCode::NotFound => {
                return Err(Error::new(ErrorKind::NotFound, "dictionary not found"));
            }
            PddbRequestCode::NotMounted => {
                return Err(Error::new(ErrorKind::NotMounted, "PDDB not mounted"));
            }
            other => {
                return Err(Error::new(ErrorKind::Internal, format!("KeyCountInDict: {:?}", other)));
            }
        }

        // Phase 2: drain via ListKeyV2 until end == true.
        let mut keys = Vec::new();
        loop {
            let req = PddbKeyList {
                token,
                end: false,
                retcode: PddbRetcode::Uninit,
                data: [0u8; MAX_PDDBKLISTLEN],
            };
            let mut buf = Buffer::into_buf(req)
                .map_err(|_| Error::new(ErrorKind::Ipc, "Buffer::into_buf KeyList"))?;
            buf.lend_mut(self.main_conn, Opcode::ListKeyV2.to_u32().unwrap())
                .map_err(|_| Error::new(ErrorKind::Ipc, "lend_mut ListKeyV2"))?;
            let response: PddbKeyList = buf
                .to_original::<PddbKeyList, _>()
                .map_err(|_| Error::new(ErrorKind::Ipc, "to_original KeyList"))?;

            match response.retcode {
                PddbRetcode::Ok => {}
                PddbRetcode::AccessDenied => {
                    return Err(Error::new(
                        ErrorKind::AccessDenied,
                        "key list locked by another process",
                    ));
                }
                other => {
                    return Err(Error::new(ErrorKind::Internal, format!("ListKeyV2: {:?}", other)));
                }
            }

            // Packed list format: (u8 len, [u8] name) repeating, with
            // a 0-length record indicating end of buffer.
            let mut idx = 0;
            while idx < MAX_PDDBKLISTLEN && response.data[idx] != 0 {
                let strlen = response.data[idx] as usize;
                idx += 1;
                if idx + strlen > MAX_PDDBKLISTLEN {
                    return Err(Error::new(ErrorKind::Internal, "key list overran buffer"));
                }
                let name = std::str::from_utf8(&response.data[idx..idx + strlen]).map_err(|_| {
                    Error::new(ErrorKind::Internal, "key name not utf-8")
                })?;
                keys.push(name.to_string());
                idx += strlen;
            }

            if response.end {
                break;
            }
        }

        Ok(keys)
    }

    fn dict_request(&self, dict: &str, key: &str) -> Result<PddbDictRequest, Error> {
        if dict.len() > DICT_NAME_LEN - 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "dict name too long"));
        }
        if key.len() > KEY_NAME_LEN - 1 {
            return Err(Error::new(ErrorKind::InvalidInput, "key name too long"));
        }
        Ok(PddbDictRequest {
            basis_specified: false,
            basis: String::new(),
            dict: dict.to_string(),
            key: key.to_string(),
            index: 0,
            token: [0; 4],
            code: PddbRequestCode::Uninit,
            bulk_limit: None,
            key_count: 0,
            found_key_count: 0,
        })
    }

    fn dict_request_send(
        &self,
        request: PddbDictRequest,
        op: Opcode,
    ) -> Result<PddbDictRequest, Error> {
        let mut buf = Buffer::into_buf(request)
            .map_err(|_| Error::new(ErrorKind::Ipc, "Buffer::into_buf"))?;
        buf.lend_mut(self.main_conn, op.to_u32().unwrap())
            .map_err(|_| Error::new(ErrorKind::Ipc, format!("lend_mut {:?}", op)))?;
        buf.to_original::<PddbDictRequest, _>()
            .map_err(|_| Error::new(ErrorKind::Ipc, "to_original DictRequest"))
    }
}

impl Drop for PddbClient {
    fn drop(&mut self) {
        // Best-effort. Each connection was registered against this
        // process; if disconnect fails (e.g. during shutdown) the
        // kernel cleans them up via the process teardown path.
        unsafe {
            let _ = xous::disconnect(self.main_conn);
            let _ = xous::disconnect(self.poller_conn);
        }
    }
}

/// Options for `PddbClient::open`. Mirrors what `services/pddb/src/lib.rs::get`
/// accepts (omitting `basis_name` and the change-callback — both
/// outside our KvBackend scope).
#[derive(Default, Clone, Copy)]
pub struct OpenOptions {
    pub create_dict: bool,
    pub create_key: bool,
    pub alloc_hint: Option<usize>,
}

impl OpenOptions {
    pub fn create_all() -> Self {
        Self { create_dict: true, create_key: true, alloc_hint: None }
    }
}

/// A streaming handle to an open `(dict, key)` pair. Implements
/// `Read` + `Write` so callers can pipe arbitrary-sized values
/// through the 4072-byte `PddbBuf` data area.
pub struct KeyHandle<'a> {
    conn: CID,
    token: ApiToken,
    pos: u64,
    buf: Buffer<'a>,
}

impl<'a> KeyHandle<'a> {
    /// Send `Opcode::WriteKeyFlush` to commit pending writes. Must be
    /// called before drop on a write path; otherwise PDDB may not
    /// persist the data.
    ///
    /// The server returns a `PddbRetcode` as a scalar — `Ok = 1`
    /// (the enum starts at `Uninit = 0`). Mirrors
    /// `services/pddb/src/lib.rs::Pddb::sync` upstream.
    pub fn flush_writes(&mut self) -> Result<(), Error> {
        let token = self.token;
        let resp = send_message(
            self.conn,
            Message::new_blocking_scalar(
                Opcode::WriteKeyFlush.to_usize().unwrap(),
                token[0] as usize,
                token[1] as usize,
                token[2] as usize,
                0,
            ),
        )
        .map_err(|e| Error::new(ErrorKind::Ipc, format!("WriteKeyFlush: {:?}", e)))?;
        let xous::Result::Scalar1(rcode) = resp else {
            return Err(Error::new(
                ErrorKind::Ipc,
                format!("WriteKeyFlush: unexpected {:?}", resp),
            ));
        };
        match rcode {
            r if r == PddbRetcode::Ok as usize => Ok(()),
            r if r == PddbRetcode::BasisLost as usize => Err(Error::new(
                ErrorKind::Internal,
                "WriteKeyFlush: BasisLost",
            )),
            r if r == PddbRetcode::DiskFull as usize => Err(Error::new(
                ErrorKind::NoFreeSpace,
                "WriteKeyFlush: DiskFull",
            )),
            r if r == PddbRetcode::AccessDenied as usize => Err(Error::new(
                ErrorKind::AccessDenied,
                "WriteKeyFlush: AccessDenied",
            )),
            r if r == PddbRetcode::InternalError as usize => Err(Error::new(
                ErrorKind::Internal,
                "WriteKeyFlush: InternalError",
            )),
            other => Err(Error::new(
                ErrorKind::Internal,
                format!("WriteKeyFlush: unknown retcode={}", other),
            )),
        }
    }
}

impl<'a> Read for KeyHandle<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let readlen = {
            let pbuf = PddbBuf::from_slice_mut(self.buf.as_mut());
            pbuf.token = self.token;
            let n = if buf.len() <= PDDB_BUF_DATA_LEN { buf.len() } else { PDDB_BUF_DATA_LEN };
            pbuf.len = n as u16;
            pbuf.retcode = PddbRetcode::Uninit;
            pbuf.position = self.pos;
            n
        };
        self.buf
            .lend_mut(self.conn, Opcode::ReadKey.to_u32().unwrap())
            .map_err(|_| io::Error::other("lend_mut ReadKey"))?;
        let pbuf = PddbBuf::from_slice_mut(self.buf.as_mut());
        match pbuf.retcode {
            PddbRetcode::Ok => {
                let got = pbuf.len as usize;
                if got > readlen {
                    return Err(io::Error::other("server returned more data than requested"));
                }
                buf[..got].copy_from_slice(&pbuf.data[..got]);
                self.pos += got as u64;
                Ok(got)
            }
            PddbRetcode::UnexpectedEof => Ok(0),
            PddbRetcode::BasisLost => Err(io::Error::new(io::ErrorKind::BrokenPipe, "basis lost")),
            PddbRetcode::AccessDenied => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "access denied",
            )),
            other => Err(io::Error::other(format!("ReadKey retcode {:?}", other))),
        }
    }
}

impl<'a> Write for KeyHandle<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let writelen = {
            let pbuf = PddbBuf::from_slice_mut(self.buf.as_mut());
            pbuf.token = self.token;
            let n = if buf.len() <= PDDB_BUF_DATA_LEN { buf.len() } else { PDDB_BUF_DATA_LEN };
            pbuf.data[..n].copy_from_slice(&buf[..n]);
            pbuf.len = n as u16;
            pbuf.retcode = PddbRetcode::Uninit;
            pbuf.position = self.pos;
            n
        };
        self.buf
            .lend_mut(self.conn, Opcode::WriteKey.to_u32().unwrap())
            .map_err(|_| io::Error::other("lend_mut WriteKey"))?;
        let pbuf = PddbBuf::from_slice_mut(self.buf.as_mut());
        match pbuf.retcode {
            PddbRetcode::Ok => {
                let wrote = pbuf.len as usize;
                if wrote > writelen {
                    return Err(io::Error::other("server reported writing more than supplied"));
                }
                self.pos += wrote as u64;
                Ok(wrote)
            }
            PddbRetcode::DiskFull => Err(io::Error::new(io::ErrorKind::OutOfMemory, "disk full")),
            PddbRetcode::BasisLost => Err(io::Error::new(io::ErrorKind::BrokenPipe, "basis lost")),
            PddbRetcode::AccessDenied => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "access denied",
            )),
            other => Err(io::Error::other(format!("WriteKey retcode {:?}", other))),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // PDDB `flush` semantically means "commit my writes", which
        // is `Opcode::WriteKeyFlush` (a separate opcode from `flush`
        // in the std::io::Write sense). Map the std flush call onto
        // it here so `presage-store-pddb` can rely on standard
        // patterns.
        self.flush_writes()
            .map_err(|e| io::Error::other(format!("WriteKeyFlush: {}", e)))
    }
}

impl<'a> Drop for KeyHandle<'a> {
    fn drop(&mut self) {
        // Best-effort `Opcode::KeyDrop`. If we miss it the server
        // garbage-collects via its own token aging; not catastrophic.
        let _ = send_message(
            self.conn,
            Message::new_blocking_scalar(
                Opcode::KeyDrop.to_usize().unwrap(),
                self.token[0] as usize,
                self.token[1] as usize,
                self.token[2] as usize,
                0,
            ),
        );
    }
}

/// Random 128-bit token for stateful list operations. PDDB's server
/// uses the token to disambiguate concurrent `ListKeyV2` calls.
/// We don't need cryptographic randomness — just uniqueness within
/// the lifetime of a single listing — so we lean on the kernel's
/// `xous::create_server_id` helper, the same path xous-core's gen2
/// PDDB code takes.
fn random_token_4() -> [u32; 4] {
    xous::create_server_id().expect("create_server_id").to_array()
}
