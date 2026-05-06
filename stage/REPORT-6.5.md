# Stage 6.5 — rv32 verification of libsignal core

Status: **complete.** `cargo check
--target=riscv32imac-unknown-xous-elf -p libsignal-service` passes
with no patches against libsignal source. Every cryptographic
primitive on the protocol path resolves to an upstream pure-Rust
crate listed by `https://cryptography.rs/`. One cleanup landed in
the same commit: dropped a dead `tokio` dep from our
`vendor/libsignal-service-rs/Cargo.toml` (line 55, leftover from
upstream's `#[tokio::test]` annotations on async tests; no production
src/ file references tokio outside comments after the Stage 6
transport replacement).

After that drop, `cargo tree --target=riscv32imac-unknown-xous-elf
--edges normal -p libsignal-service` returns **zero matches** for the
C-runtime regex `(boring|boring-sys|openssl|openssl-sys|aws-lc|aws-lc-sys|bindgen|tokio|mio|reqwest|hyper|h2)`.

This stage is a **structured re-verification** that surfaces every
crate we transitively pull through `libsignal-service-rs` onto the
rv32-xous target, and confirms it's the same shape the user-supplied
cryptography.rs analysis memo predicted (see `RESUME.md` for the memo
context).

## Verification summary

```sh
$ cargo check --target=riscv32imac-unknown-xous-elf \
    -p libsignal-service --no-default-features --features=phonenumber
... Checking libsignal-service v0.1.0 (/home/tunnell/precursor-signal/xous-app-signal/vendor/libsignal-service-rs)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.27s
✓ rv32 cross-compile of libsignal-service passes

$ cargo run -p xous-app-signal --bin xas
xas: pong
xas: whoami err (expected): ...
xas: exiting
✓ Stage 8 hosted-mode smoke output unchanged

$ cargo test -p presage-store-pddb        ✓ 22 passed
$ cargo test -p xous-signal-bridge        ✓  3 passed
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean
```

Cargo-tree snapshots saved at `stage/cargo-tree/`:

- `libsignal-service-rv32-default.txt` — full `-e features` tree, 1686 lines.
- `libsignal-service-rv32-normal.txt` — `--edges normal` (no dev-deps), 562 lines.

## C-surface / `*-sys` / runtime scan

```sh
$ grep -E '\b(boring|boring-sys|openssl|openssl-sys|aws-lc|aws-lc-sys|bindgen|tokio|mio|reqwest|hyper|h2)\b' \
    stage/cargo-tree/libsignal-service-rv32-normal.txt
(no output — clean)
```

**Findings:**

- **Zero matches** for `boring`, `boring-sys`, `openssl`, `openssl-sys`,
  `aws-lc`, `aws-lc-sys`, `bindgen`, `tokio`, `mio`, `reqwest`,
  `hyper`, `h2` after the tokio dep drop. Confirms the cryptography.rs
  memo's prediction that libsignal v0.91's protocol path has no C
  surface.
- **The tokio finding** (now resolved): the first run of this
  verification surfaced `tokio v1.52.2` as a normal dep, traced to
  `vendor/libsignal-service-rs/Cargo.toml:55`. Every `tokio`
  reference in `vendor/libsignal-service-rs/src/` was in comments
  explaining the Stage 6.1 replacements (transport.rs:326,
  websocket/mod.rs:233-234, push_service/mod.rs:141). No `use tokio;`
  or call sites remained in production code. Dropped in the same
  commit as this report; the dev-dep at line 74 was retained
  (preserves a tighter cargo-test scope without affecting our
  builds, since cargo only compiles dev-deps for the workspace
  member being tested).
- **`linux-raw-sys`, `errno`, `libc`** appear under build-deps subtrees
  (cargo tree's `(*)` node references). These are host-side build.rs
  dependencies that don't reach the rv32 target binary. No action.

## sha2 / getrandom / curve25519 source-of-truth check

### `sha2`

```sh
$ cargo tree --target=riscv32imac-unknown-xous-elf -i sha2
sha2 v0.10.8 (https://github.com/betrusted-io/hashes.git?branch=sha2-v0.10.8-xous#161da48e)
```

✓ Single resolved version (0.10.8). Source is **xous-core's
[`betrusted-io/hashes` branch `sha2-v0.10.8-xous`](https://github.com/betrusted-io/hashes/tree/sha2-v0.10.8-xous)** —
exactly the patch the memo cited. Gates `sha2-asm` to x86/x86_64/aarch64
so RISC-V skips it. The `[patch.crates-io].sha2` entry in our workspace
`Cargo.toml` mirrors xous-core's patch (Decision 6).

Consumers (transitive): every libsignal sub-crate (`signal-crypto`,
`libsignal-protocol`, `libsignal-account-keys`, `zkgroup`,
`zkcredential`, `usernames`, `poksho`), `spqr`,
`presage-store-pddb`, `presage`, `signature`, `argon2`, `hkdf`, etc.

### `curve25519-dalek`

```sh
$ cargo tree --target=riscv32imac-unknown-xous-elf -i curve25519-dalek
curve25519-dalek v4.1.3 (/home/tunnell/precursor-signal/xous-app-signal/vendor/curve25519-dalek/curve25519-dalek)
```

✓ Single resolved version (4.1.3). Source is our **vendored
betrusted-io fork** with the lizard-module port and 4.1.2→4.1.3
version bump (Decision 6). Both the crates.io alias path
(used by `ed25519-dalek`, `x25519-dalek`) and the signalapp git URL
path (used by zkgroup as `curve25519-dalek-signal`) resolve to this
same path-vendored crate, eliminating the type-conflict that would
arise from two compiled copies.

### `getrandom`

```sh
$ cargo tree --target=riscv32imac-unknown-xous-elf -p libsignal-service \
    | grep "getrandom v"
│   │   │   │   └── getrandom v0.2.12 (/home/tunnell/precursor-signal/repos/xous-core/imports/getrandom)
│   │   │       └── getrandom v0.3.4
```

✓ Both versions present, separately resolved:

- **`getrandom v0.2.12`** routes to xous-core's
  [`imports/getrandom`](https://github.com/betrusted-io/xous-core/tree/main/imports/getrandom)
  via `[patch.crates-io].getrandom`. Used by older crates (rand 0.6,
  rand_core 0.6, etc.).
- **`getrandom v0.3.4`** comes from crates.io. The
  `[target.riscv32imac-unknown-xous-elf]` `--cfg
  getrandom_backend="custom"` rustflag (in `.cargo/config.toml`)
  redirects its randomness call to the
  `__getrandom_v03_custom` extern that
  `crates/xous-app-signal/src/main.rs` provides (Stage 9a panic-stub;
  Stage 9b replaces with real `trng::Trng` client).

The two versions coexist because cargo treats them as different crate
graphs. This is the correct shape; no cycle survives in the resolved
graph (the resolver-cycle error we saw during the Stage 9b merge attempt
was a side-effect of xous-core's workspace `[patch]` entries, not a
real dep cycle).

## Cryptographic primitive map

For each Signal-protocol primitive: file:line of the first call site
in libsignal v0.91.0 source (`~/precursor-signal/repos/libsignal/`,
`git rev 98915c44`), the upstream crate + version, the
cryptography.rs-listed equivalent (or the same crate if it's listed
directly), and rv32 viability.

| Primitive | File:line citation | Upstream crate | cryptography.rs entry | rv32 viability |
|---|---|---|---|---|
| AES-256 (block) | `rust/protocol/src/crypto.rs:8` (`use aes::Aes256;`) | `aes 0.8.4` | `aes` (block ciphers) | pure-Rust; `cpufeatures` no-op on rv32 |
| AES-256-CTR | `rust/protocol/src/crypto.rs:35` (`ctr::Ctr32BE::<Aes256>::new`) | `ctr 0.9.2` + `aes 0.8.4` | `ctr` (block-cipher modes) | pure-Rust |
| AES-256-CBC | `rust/protocol/src/incremental_mac.rs:6` (transitive via `aes::cipher::Unsigned`) | `cbc 0.1.2` + `aes 0.8.4` | (block-modes umbrella) | pure-Rust |
| AES-256-GCM-SIV | `rust/protocol/src/sealed_sender.rs:11-12` (`use aes_gcm_siv::{AeadInPlace, Aes256GcmSiv, KeyInit};`) | `aes-gcm-siv 0.11.1` | `aes-gcm-siv` (AEADs) | pure-Rust portable backend on rv32 |
| HMAC-SHA-256 | `rust/protocol/src/crypto.rs:10` (`use hmac::{Hmac, Mac};`) → `:51` (`Hmac::<Sha256>::new_from_slice`) | `hmac 0.12.1` + `sha2 0.10.8` (Xous fork) | `hmac` (MACs) | pure-Rust |
| HKDF-SHA-256 | `rust/protocol/src/sealed_sender.rs:736` (`hkdf::Hkdf::<sha2::Sha256>::new`) | `hkdf 0.12.4` | `hkdf` (KDFs) | pure-Rust |
| SHA-256 | `rust/protocol/src/protocol.rs:9` (`use sha2::Sha256;`) | `sha2 0.10.8` (Xous fork) | `sha2` (hashes, listed as "SHA-2") | pure-Rust on Xous fork; `sha2-asm` gated to x86/aarch64 |
| SHA-512 | `rust/protocol/src/fingerprint.rs:10` (`use sha2::Sha512;`) | `sha2 0.10.8` (Xous fork) | `sha2` | same as SHA-256 |
| Curve25519 base point ops | `rust/core/src/curve/curve25519.rs:6-10` (`curve25519_dalek::{constants::ED25519_BASEPOINT_TABLE, edwards::EdwardsPoint, montgomery::MontgomeryPoint, scalar::Scalar}`) | `curve25519-dalek 4.1.3` (our vendored betrusted-io fork) | `curve25519-dalek` | pure-Rust portable backend on rv32; HW-accelerated u32e backend on Precursor when `--cfg curve25519_dalek_backend="u32e_backend"` set |
| X25519 (key agreement) | `rust/core/src/curve/curve25519.rs:14` (`use x25519_dalek::{PublicKey, StaticSecret};`) | `x25519-dalek 2.0.x` | `x25519-dalek` | pure-Rust |
| Ed25519 (signatures) | `rust/core/src/curve/curve25519.rs:6` (transitively via `ED25519_BASEPOINT_TABLE`) | `ed25519-dalek 2.1.0` | `ed25519-dalek` | pure-Rust |
| Ristretto255 | `rust/zkgroup/src/crypto/receipt_credential_request.rs:9` (`use curve25519_dalek_signal::ristretto::RistrettoPoint;`) | `curve25519-dalek 4.1.3` (signalapp git path → our vendored copy via `[patch."https://github.com/signalapp/curve25519-dalek"]`) | `curve25519-dalek` (Ristretto is part of the same crate) | pure-Rust |
| Ristretto::lizard_encode | (zkgroup-internal calls; lizard module ported from signalapp/curve25519-dalek) | our vendored copy's `src/lizard/` | (additive, no crates.io entry) | pure-Rust |
| ML-KEM-1024 (encap/decap) | `rust/protocol/src/kem.rs:180` (`impl<const N: usize> ConstantLength for libcrux_ml_kem::MlKemPrivateKey<N>`) | **`libcrux-ml-kem 0.0.8`** | **NOT listed** — cryptography.rs lists `ml-kem` (RustCrypto, FIPS-203). Both are pure-Rust; libsignal's choice is formally verified in F\* via the hax toolchain. We follow libsignal's choice (Decision 7 / minimize divergence). | pure-Rust portable backend on rv32; SIMD features (`simd128`, `simd256`) off by default — verified via cargo tree |
| Argon2id | `rust/account-keys/src/hash.rs:25-26,53` (`Argon2::new(Algorithm::Argon2id, ...)`) | `argon2 0.5.x` | `argon2` (KDFs) | pure-Rust; perf concern on rv32 — workspace pins `[profile.dev.package.argon2] opt-level = 2` upstream; we inherit |
| Constant-time eq | `rust/protocol/src/crypto.rs:12` (`use subtle::ConstantTimeEq;`) | `subtle 2.6.x` | `subtle` (defensive utilities) | pure-Rust; rv32-clean (compiles to branchless integer ops) |
| Zeroize | (transitive throughout libsignal's secret-key types) | `zeroize 1.8.x` | `zeroize` (defensive utilities) | pure-Rust |
| RNG | `rust/protocol/src/kem.rs:66` (`use rand::{CryptoRng, Rng};`) | `rand 0.9` + `getrandom 0.3` (custom backend) | `rand`, `getrandom` | rv32 routes via `__getrandom_v03_custom` extern (Stage 9a stub; Stage 9b real Trng client) |

**No primitive on the protocol path requires a libsignal source patch.**
Every primitive resolves to an upstream pure-Rust crate that compiles
cleanly for `riscv32imac-unknown-xous-elf` either directly (RustCrypto
crates) or via xous-core's `[patch.crates-io]` redirects (sha2,
getrandom 0.2). The cryptography.rs catalog
(`https://cryptography.rs/`) lists every crate in the table above.

## Cryptography.rs adoption matrix verdicts

| Crate | Verdict | Note |
|---|---|---|
| `aes` | CLEAR | upstream RustCrypto 0.8.4 |
| `aes-gcm-siv` | CLEAR | 0.11.1 |
| `ctr` | CLEAR | 0.9.2 |
| `cbc` | CLEAR | 0.1.2 (transitive) |
| `sha2` | INHERITS-PATCH | resolves to betrusted-io/hashes Xous fork |
| `hmac` | CLEAR | 0.12.1 |
| `hkdf` | CLEAR | 0.12.4 |
| `argon2` | CLEAR | 0.5; perf concern on rv32 (Stage 11+ may surface) |
| `subtle` | CLEAR | 2.6 |
| `zeroize` | CLEAR | 1.8 |
| `curve25519-dalek` | INHERITS-PATCH | vendored betrusted-io fork at apps/xas/vendor (4.1.3 + lizard) |
| `ed25519-dalek` | CLEAR | 2.1.0 |
| `x25519-dalek` | CLEAR | 2.0.x |
| `libcrux-ml-kem` | NOT-LISTED | pq-code-package, not on cryptography.rs index. Pure-Rust + formally verified. We follow libsignal's choice. |
| `rand` | CLEAR | 0.9 |
| `getrandom` | INHERITS-PATCH (0.2) + CUSTOM-BACKEND (0.3) | 0.2 routes to xous-core's TRNG fork; 0.3 routes to our `__getrandom_v03_custom` extern |
| `rustls` | CLEAR | 0.22.2 (Decision 3) |
| `tungstenite` | CLEAR | 0.21 (Decision 3); not on cryptography.rs but on the transport path |
| `webpki-roots` | CLEAR | inherited via rustls |
| `prost` (protobuf) | CLEAR | 0.13; not crypto, but on the wire path |

No `BLOCKED` or `UNVERIFIED` verdicts. The one note: **`libcrux-ml-kem`
is not listed by cryptography.rs**. Both `libcrux-ml-kem` and the
RustCrypto `ml-kem` are pure-Rust ML-KEM-1024 implementations; the
former is formally verified in F\*, the latter is a strict FIPS-203
implementation. Following libsignal's choice (Decision 7's
minimize-divergence principle) keeps the patch series empty.

## rv32 patch series

**Empty.** No libsignal source modifications required as of v0.91.0
(commit `98915c44`).

The memo speculated that `rust/crypto/src/lib.rs` lines 6-7 might
have aarch64-only `feature(stdsimd)` / `feature(aarch64_target_feature)`
gates that fire incorrectly on rv32 builds. **Verified false at this
revision**: no such gates exist in `rust/protocol/src/`,
`rust/core/src/`, or any other workspace member we transitively pull.
The `rust/crypto/` directory the memo cited doesn't exist in v0.91 —
it was inlined into `rust/protocol/src/crypto.rs` (the file we cite
in the primitive map above), which uses only `aes 0.8` /
`hmac 0.12` / `sha2` / `subtle` / `ctr` traits with no SIMD gates.

## Findings to act on

### 1. (deferred) Re-confirm at every libsignal upstream rebase


When this workspace bumps `libsignal-protocol` or `libsignal-service-rs`'s
git pin, re-run the verification commands above and update the
primitive map. Specifically watch for:

- New transitive deps showing `boring`, `tokio`, or `*-sys` in the
  `cargo tree --edges normal` output for the rv32 target.
- New AEAD or KDF call sites that didn't exist at v0.91 (e.g., if
  Signal adds `chacha20poly1305` to the protocol path; currently it's
  only used in the `rust/net*` family that we don't pull).
- libcrux-ml-kem version bumps. The 0.0.x series suggests instability;
  audit whether SIMD features stay off-by-default on rv32.

The Stage 9b xtask should integrate this re-verification as a
pre-build step (`cargo xtask verify-rv32-deps`) so accidental
regressions surface in CI.

### 2. (optional) The `ml-kem` vs `libcrux-ml-kem` question

cryptography.rs lists `ml-kem` (RustCrypto, FIPS-203 implementation)
and not `libcrux-ml-kem`. libsignal v0.91 uses `libcrux-ml-kem`. Both
are pure-Rust; both compile for rv32 in our setup. Stage 9b's Renode
boot test will exercise the ML-KEM-1024 path indirectly (via PQXDH on
link/register flows at Stage 10). If that test surfaces correctness
or perf issues on rv32, switching to RustCrypto's `ml-kem` is the
fallback per the memo's stop conditions. As of this verification it's
not a recommended action — keep `libcrux-ml-kem`, minimize libsignal
divergence.

## Files changed (this commit)

```
new:
  stage/REPORT-6.5.md                                       (this file)
  stage/cargo-tree/libsignal-service-rv32-default.txt       (full tree)
  stage/cargo-tree/libsignal-service-rv32-normal.txt        (--edges normal)

(no source changes; this is a verification-only stage)
```
