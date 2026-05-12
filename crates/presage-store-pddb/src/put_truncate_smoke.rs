//! Smoke test: `PddbBackend::put` must shrink a key when overwriting
//! with a smaller value (refs #14). Run from a hosted-xas startup
//! path or rv32 test runner; needs live PDDB IPC.

use crate::backend_pddb::PddbBackend;
use crate::KvBackend;

const DICT: &str = "xas.put_truncate_smoke";
const KEY: &str = "buffer";
const LARGE_LEN: usize = 50_000;
const SMALL_LEN: usize = 10_000;
const LARGE_MARKER: u8 = 0xAA;
const SMALL_MARKER: u8 = 0x55;

#[derive(Debug)]
pub enum SmokeResult {
    Pass,
    Fail {
        expected_len: usize,
        actual_len: usize,
        sample_tail: Vec<u8>,
    },
    Error(String),
}

/// Write a 50 KB blob, overwrite with 10 KB, read back. Pass iff the
/// read-back is exactly 10 KB of `SMALL_MARKER`. A tail of `LARGE_MARKER`
/// bytes is the signature of the truncation bug.
pub fn smoke_put_truncates(backend: &PddbBackend) -> SmokeResult {
    let large = vec![LARGE_MARKER; LARGE_LEN];
    if let Err(e) = backend.put(DICT, KEY, &large) {
        return SmokeResult::Error(format!("put(LARGE): {}", e));
    }
    match backend.get(DICT, KEY) {
        Ok(Some(v)) if v.len() == LARGE_LEN => {}
        Ok(Some(v)) => {
            let _ = backend.delete(DICT, KEY);
            return SmokeResult::Error(format!("LARGE round-trip len={}, want {}", v.len(), LARGE_LEN));
        }
        Ok(None) => return SmokeResult::Error("get(LARGE) returned None".to_string()),
        Err(e) => {
            let _ = backend.delete(DICT, KEY);
            return SmokeResult::Error(format!("get(LARGE): {}", e));
        }
    }

    let small = vec![SMALL_MARKER; SMALL_LEN];
    if let Err(e) = backend.put(DICT, KEY, &small) {
        let _ = backend.delete(DICT, KEY);
        return SmokeResult::Error(format!("put(SMALL): {}", e));
    }
    let got = match backend.get(DICT, KEY) {
        Ok(Some(v)) => v,
        Ok(None) => return SmokeResult::Error("get(SMALL) returned None".to_string()),
        Err(e) => {
            let _ = backend.delete(DICT, KEY);
            return SmokeResult::Error(format!("get(SMALL): {}", e));
        }
    };
    let _ = backend.delete(DICT, KEY);

    let tail = got[got.len().saturating_sub(16)..].to_vec();
    if got.len() == SMALL_LEN && got.iter().all(|&b| b == SMALL_MARKER) {
        SmokeResult::Pass
    } else {
        SmokeResult::Fail {
            expected_len: SMALL_LEN,
            actual_len: got.len(),
            sample_tail: tail,
        }
    }
}
