# Stage 2 — TLS smoke test (rustls 0.22.2 + std TcpStream)

Status: **complete**.

## What was done

1. `crates/xous-net-bridge/Cargo.toml`: added `rustls = "=0.22.2"` (precise pin to xous-core's version) and `webpki-roots = "1.0"`.
2. `crates/xous-net-bridge/src/tls.rs` (~50 lines): public `tls_connect(host, port, alpn) -> io::Result<RustlsStream>` wrapping `rustls::ClientConfig::builder().with_root_certificates(...).with_no_client_auth()` + `webpki-roots`'s NSS bundle as trust anchors. Uses `std::net::TcpStream` underneath — same API works on Xous (`xous-core/services/net/src/std_tcpstream.rs` exposes the std-compatible TCP).
3. `crates/xous-net-bridge/examples/https_get.rs`: connects to `example.com:443`, sends `GET / HTTP/1.1`, prints status line.
4. `[patch.crates-io].getrandom = path = "/home/tunnell/precursor-signal/repos/xous-core/imports/getrandom"` added to workspace `Cargo.toml`. This was deferred from Stage 0 with a TODO; **Stage 2 forced its earlier resolution** because rustls' transitive `ring → getrandom` chain hits `getrandom`'s "unsupported target" compile-error on rv32 without the xous-core fork.

## Verification

```sh
$ cargo run --example https_get -p xous-net-bridge
HTTP/1.1 200 OK

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-net-bridge
    Checking xous-core's getrandom v0.2.12 (forked path) ✓
    Checking xous-ipc, xous-api-log, xous-api-names, rkyv (transitively
        pulled by getrandom-xous since it talks to Xous's TRNG service) ✓
    Checking ring v0.17.7 (xous-fork) ✓
    Checking rustls v0.22.2 + rustls-webpki + webpki-roots ✓
    Checking xous-net-bridge v0.0.1 ✓
    Finished in 9.22s

$ cargo build --workspace --release        # clean
$ cargo build --workspace --profile=release-small   # clean
$ cargo tree --workspace -d                # nothing to print
$ cargo fmt --all -- --check               # clean (after one rustfmt-driven edit)
$ cargo clippy --workspace --all-targets -- -D warnings   # clean (after `io::Error::other` migration)
```

The rv32 cross-compile is the headline result. **rustls + ring + getrandom now build for `riscv32imac-unknown-xous-elf`** — the entire crypto-and-network bottom half is operational on Xous's target. This is the strongest pre-Stage-9 sanity check we'll get.

## Issues encountered and resolved

### `webpki-roots` duplicate (caught by `cargo tree -d`)

`webpki-roots = "0.26"` (initial choice) transitively pulls `webpki-roots v1.0.7` for its data (the v0.26 line apparently re-exports v1.0's bundle). Result: two copies of `webpki-roots` in the dep graph.

**Fix:** bumped explicit dep to `webpki-roots = "1.0"`. The 1.0.x line drops the re-export indirection. After this, `cargo tree -d` shows nothing.

Lesson for future stages: every new dep we add, run `cargo tree -d` immediately. The dev specifically called this out.

### `getrandom` rv32 compile-error (Stage 0 deferral comes due)

```
error: target is not supported, for more information see:
       https://docs.rs/getrandom/#unsupported-targets
   --> getrandom-0.2.17/src/lib.rs:351:9
```

upstream `getrandom` 0.2 has a `#[cfg]` ladder that compile-errors on unrecognized targets. `riscv32imac-unknown-xous-elf` is unrecognized.

xous-core has a forked `getrandom` at `imports/getrandom` (v0.2.12) that adds Xous as a target and wires it to the Xous TRNG service. Stage 0 deferred this patch because it's path-based and only valid in xous-core's tree.

**Fix:** added `[patch.crates-io].getrandom = path = "/home/tunnell/precursor-signal/repos/xous-core/imports/getrandom"`. This is brittle (it's an absolute path on this developer's machine) but unblocks rv32 cross-compile right now. Documented in `Cargo.toml` with a comment pointing at the long-term fix (Stage 9: merge into xous-core or fork the patch into a publishable repo).

### Two clippy/fmt nits

- `rustfmt` wanted the `.map(|s| ...)` lambda in `examples/https_get.rs` reformatted across multiple lines. Fixed.
- `clippy::io_other_error`: prefer `io::Error::other(e)` over `io::Error::new(ErrorKind::Other, e)`. Fixed in `src/tls.rs`.

Both caught by the verification checks per Stage 0 refinement (`cargo fmt --check` and `cargo clippy -- -D warnings` are now in every stage's verification).

## Binary sizes

| Binary | release-small | release | Δ release-small from Stage 1 |
|---|---|---|---|
| `xas` (no-deps app, just smol) | 373 KB | 3.12 MB | (unchanged from Stage 1; no new deps in `xous-app-signal`) |
| `https_get` (rustls + ring) | **1.15 MB** | 7.99 MB | n/a (new binary) |

**rustls + ring + webpki-roots costs ~780 KB stripped** on hosted x86_64 (1.15 MB - 373 KB). Most of that is `ring`'s precomputed crypto tables and rustls' protocol state machinery. For an HTTPS-capable client this is the unavoidable floor; rv32 is typically ~30% larger.

## rv32 dep tree (relevant slice)

```
xous-net-bridge
├── rustls v0.22.2
│   ├── ring v0.17.7 (xous-fork)
│   │   └── getrandom v0.2.12 (xous-fork) ← new for Stage 2
│   │       └── xous, xous-ipc, xous-api-log, xous-api-names, rkyv
│   └── rustls-webpki v0.102.8
│       ├── ring v0.17.7 (xous-fork) (*)
│       └── untrusted v0.9.0
└── webpki-roots v1.0.7
```

The pull-in of xous-ipc/xous-api-log via getrandom is interesting: xous-core's getrandom-xous calls Xous's TRNG service via Xous IPC, so on rv32 we now have the entire Xous syscall surface available transitively. That's expected and harmless on rv32; on hosted Linux (where getrandom falls back to /dev/urandom), these crates aren't pulled.

## Deviations from the ROADMAP

1. **`webpki-roots` version.** ROADMAP said "webpki-roots" without a version; I picked 0.26 first (older, more conservative), hit the duplicate, and bumped to 1.0. Suggest the ROADMAP recommend `1.0` directly.

2. **Stage 2 forced the `getrandom` patch resolution earlier than Stage 9.** ROADMAP Stage 0 said the path-based patches (`aes`, `getrandom`) could wait until Stage 9. In practice, Stage 2's rv32 cross-compile of rustls hits `getrandom` and forces resolution now. Documented inline in `Cargo.toml`. The ROADMAP should reflect this.

## Suggested ROADMAP refinements

1. **Stage 2 step 1 — specify `webpki-roots` version.** The current text says just `webpki-roots`. Suggest:

   > 1. Add `rustls = "=0.22.2"` and `webpki-roots = "1.0"` to `xous-net-bridge/Cargo.toml`. If you accidentally pick `webpki-roots = "0.26"`, you'll see a duplicate-version warning from `cargo tree -d` because v0.26 re-exports v1.0's data.

2. **Acknowledge Stage 2 brings `getrandom` patch forward.** Add a note to Stage 0 step 3 and a corresponding step in Stage 2:

   > Stage 2 step 1.5: Add `[patch.crates-io].getrandom = path = "..."` pointing at xous-core's `imports/getrandom`. The upstream `getrandom` 0.2 compile-errors on unrecognized targets, and rv32-xous is unrecognized; the xous-core fork adds Xous as a target. Long-term resolution (merge into xous-core or fork the patch into a publishable repo) is still Stage 9, but the patch redirect must exist now for Stage 2's rv32 cross-compile to succeed.

3. **Note ALPN parameter type.** `tls_connect`'s `alpn: &[&[u8]]` is correct; rustls expects `Vec<Vec<u8>>` ALPN. Worth a one-line clarification in the deliverables.

## Open questions / things to revisit

1. **Custom CA pinning vs webpki-roots.** `libsignal-service-rs` pins its own root CA at `libsignal-service-rs/certs/`. Stage 6 (transport fork) needs to layer that pinning on top of our `tls_connect`. The current `tls_connect` API takes the system CA bundle implicitly; we'll likely add an overload or a config parameter at Stage 6.

2. **Brittle absolute path for `getrandom` patch.** The current Cargo.toml has a hardcoded path that only works on the dev's machine. This is a real blocker for any other developer reproducing the build. Highest-leverage fix: at Stage 9, decide whether to merge our workspace into xous-core (which makes the path patches "just work") or to maintain a vendor/getrandom-xous publishable fork.

3. **No `aes` patch yet.** xous-core's `aes` patch (path-based at `services/aes`, hardware-accelerated AES via Precursor's AES engine) hasn't shown up transitively yet. It'll come in around Stage 4–5 once libsignal's signal-crypto pulls aes-gcm. Need to apply the same path-patch trick then. Note for future stage report.

## Files changed (since Stage 1 commit)

```
modified:
  Cargo.toml                                           (+webpki-roots/getrandom patches; +context comments)
  Cargo.lock                                           (regenerated)
  crates/xous-net-bridge/Cargo.toml                    (+rustls, +webpki-roots)
  crates/xous-net-bridge/src/lib.rs                    (re-export tls module)

new:
  crates/xous-net-bridge/src/tls.rs                    (~50 lines)
  crates/xous-net-bridge/examples/https_get.rs        (~30 lines)
  stage/REPORT-2.md                                    (this file)
```
