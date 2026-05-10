# Stage 3 — WebSocket smoke test against Signal's provisioning endpoint

Status: **complete**.

## What was done

1. `crates/xous-net-bridge/Cargo.toml`: added `tungstenite = "0.21"` (sync, upstream snapview/tungstenite-rs) with `default-features = false, features = ["handshake"]`. We do not use tungstenite's bundled TLS — our own `tls_connect` (Stage 2) wraps the socket so we keep the rustls version pinned at `=0.22.2`. Also added `rustls-pemfile = "2"`.

2. **`tls_connect` API generalized** to take a `RootCertStore` parameter so callers can pin specific CAs. Added two helpers:
   - `webpki_roots()` — Mozilla NSS bundle (used by `https_get` for example.com).
   - `signal_production_roots()` — parses the Signal-pinned CA from a vendored PEM (`crates/xous-net-bridge/certs/signal-production.pem`, copied from `whisperfish/libsignal-service-rs/certs/production-root-ca.pem`).
   - `signal_staging_roots()` — same for staging.

3. `crates/xous-net-bridge/src/ws.rs` (~25 lines): public `ws_connect(host, port, path, roots) -> (WebSocket<RustlsStream>, Response)` opens TCP + TLS, then runs the WebSocket handshake via `tungstenite::client(request, stream)`. Returns the server's HTTP response so callers can inspect handshake headers if needed.

4. `crates/xous-net-bridge/examples/signal_ws_keepalive.rs`: connects to `wss://chat.signal.org:443/v1/websocket/provisioning/` with the pinned production CA, sets a 5s read timeout, attempts to read one frame, then closes cleanly.

## Verification

```sh
$ cargo run --example signal_ws_keepalive -p xous-net-bridge
handshake: 101 Switching Protocols
got: binary frame (94 bytes)
closed

$ cargo run --example https_get -p xous-net-bridge
HTTP/1.1 200 OK

$ cargo run -p xous-app-signal --bin xas
got: hello

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-net-bridge
    Checking tungstenite v0.21.0
    Checking rustls v0.22.2 + rustls-webpki v0.102.8
    Checking xous-net-bridge v0.0.1 ✓

$ cargo build --workspace --release / --profile=release-small   # clean
$ cargo tree --workspace -d                                     # nothing to print
$ cargo fmt --all -- --check                                    # clean (after auto-fmt of new files)
$ cargo clippy --workspace --all-targets -- -D warnings         # clean
```

The headline result is the **94-byte binary frame Signal's server pushed immediately after the handshake**. That's a real Signal protocol frame landing on our smoke test — proof that:

- Our pinned root CA correctly chains to Signal's server cert.
- ALPN `http/1.1` is what Signal expects.
- The unauth `/v1/websocket/provisioning/` endpoint accepts our handshake.

The frame is almost certainly a `ProvisioningStep` protobuf (the URL we'd display as a QR if we were running the full link flow). We don't decode it here; that's Stage 6+ when we wire libsignal-service-rs's `ProvisioningPipe::stream()`.

## Issues encountered and resolved

### Initial TLS handshake failure: `UnknownIssuer`

First attempt used `webpki-roots` (Mozilla NSS bundle) and got:
```
IO error: invalid peer certificate: UnknownIssuer
```
Signal pins its own self-signed CA (`O = "Signal Messenger, LLC"`), confirmed via `openssl s_client -connect chat.signal.org:443`. Mozilla's bundle doesn't include it.

**Fix:** vendored `production-root-ca.pem` and `staging-root-ca.pem` from `whisperfish/libsignal-service-rs/certs/` into `crates/xous-net-bridge/certs/`. Refactored `tls_connect` to take a `RootCertStore` parameter so callers can choose: webpki-roots for general HTTPS, Signal's pinned CA for Signal endpoints. After this, handshake succeeded with `101 Switching Protocols`.

This mirrors what `libsignal-service-rs` itself does at [`src/push_service/mod.rs:90-103`](https://github.com/whisperfish/libsignal-service-rs/blob/main/src/push_service/mod.rs#L90-L103) — `tls_built_in_root_certs(false)` + custom root.

### tungstenite 0.29 → `getrandom 0.3` rv32 break

First tried `tungstenite = "0.29"`. tungstenite 0.29 has `rand = "0.9.0"` as a non-optional dep; `rand` 0.9 transitively pulls `rand_core` 0.9 → `getrandom` 0.3. xous-core's getrandom fork is 0.2.12 (pinned by `[patch.crates-io].getrandom`). `getrandom` 0.3 hit the same "target is not supported" compile-error on rv32, only with a different (custom-backend-driven) error message.

**Fix:** downgraded to `tungstenite = "0.21"`, which uses `rand 0.8 → getrandom 0.2` and is correctly patched. tungstenite 0.21's API is essentially identical for our usage (handshake, read frame, close); minor differences in `Message`/`CloseFrame` types accommodated.

This is a **second instance** of the same pattern as Stage 2's `getrandom` issue: any new dep that pulls `rand`-or-`getrandom` will need to land on the 0.2 line, not 0.3, until xous-core publishes a `getrandom` 0.3 fork. **Surfaced as a follow-up** in the open-questions section.

### URL path: `/v1/websocket/provisioning/` not `/v1/keepalive/provisioning`

The ROADMAP Stage 3 step 3 said to use `/v1/keepalive/provisioning`. That's actually the **in-protocol keepalive path** (sent inside the WebSocket protocol after the upgrade), not the URL path for the WS upgrade. The actual upgrade target is `/v1/websocket/provisioning/`, per `libsignal-service-rs/src/provisioning/mod.rs:163-170`'s `link_device` call to `push_service.ws("/v1/websocket/provisioning/", "/v1/keepalive/provisioning", ...)`. Used the correct one. Surfacing as a ROADMAP refinement.

## Binary size

| Binary | release-small | Δ from previous stage |
|---|---|---|
| `xas` | 373 KB | unchanged (no new deps in `xous-app-signal`) |
| `https_get` | 1.15 MB | unchanged (Stage 2) |
| `signal_ws_keepalive` | **1.24 MB** | +90 KB over `https_get` (tungstenite + sha1 + small bits) |

tungstenite 0.21 + a vendored ~3 KB CA cert costs ~90 KB stripped. Reasonable.

## rv32 dep tree (relevant additions for Stage 3)

```
xous-net-bridge
├── tungstenite v0.21.0
│   ├── byteorder
│   ├── bytes
│   ├── data-encoding
│   ├── http v1
│   ├── httparse
│   ├── log
│   ├── rand v0.8 → getrandom v0.2 (xous-fork)
│   ├── sha1
│   ├── thiserror
│   └── utf-8
├── rustls v0.22.2 (Stage 2 carryover)
└── webpki-roots v1.0 (Stage 2 carryover)
```

All transitive deps are familiar Rust ecosystem crates; nothing surprising.

## Deviations from the ROADMAP

1. **WS upgrade path correction.** ROADMAP said `/v1/keepalive/provisioning`; actual path is `/v1/websocket/provisioning/`. Used the correct one.

2. **`tls_connect` signature change.** ROADMAP Stage 2 had `tls_connect(host, port, alpn) -> RustlsStream` with implicit webpki-roots. To support Signal's pinned CA without a special-case branch, I made it `tls_connect(host, port, roots, alpn)` — caller provides the cert store. Both Stage 2's https_get and Stage 3's signal_ws_keepalive updated to match.

3. **tungstenite version: 0.21 not 0.29.** ROADMAP said 0.29. 0.29's transitive `getrandom 0.3` doesn't have a Xous-compatible fork yet; downgraded to 0.21 (uses `getrandom 0.2`). Downgrade is the right call until/unless xous-core gets a `getrandom 0.3` fork.

## Suggested ROADMAP refinements

1. **WS upgrade path.** Correct Stage 3 step 3 to:

   > 3. Use `/v1/websocket/provisioning/` as the WS upgrade URL — this is the unauth provisioning channel where `link_device` connects (`libsignal-service-rs/src/provisioning/mod.rs:163-170`). The string `/v1/keepalive/provisioning` is the in-protocol keepalive path, sent *inside* the WS connection after the handshake; not the upgrade URL.

2. **tungstenite version.** Pin to `0.21` (or whichever line uses `getrandom 0.2` until xous-core has a 0.3 fork). Suggested rewrite of Stage 3 step 1:

   > 1. Add `tungstenite = "0.21"` (sync, snapview/tungstenite-rs) with `default-features = false, features = ["handshake"]`. Use 0.21 specifically; later versions (≥ 0.22) pull `rand 0.9` → `getrandom 0.3`, which xous-core does not yet have a fork for. We feed our own `tls_connect` stream into `tungstenite::client(request, stream)` so we don't rely on tungstenite's bundled TLS.

3. **Pinned-CA support in `tls_connect`.** Stage 2's deliverable text should reflect that `tls_connect`'s signature is `(host, port, roots, alpn)` and that the workspace exports both `webpki_roots()` and `signal_production_roots()` helpers. Otherwise Stage 3 (and every subsequent network stage) has to refactor.

## Open questions / things to revisit

1. **`getrandom 0.3` xous-fork status.** tungstenite 0.22+ and any future crate that uses `rand 0.9` will pull `getrandom 0.3`, which xous-core hasn't forked. Three resolutions worth tracking:
   - Wait for upstream `getrandom` 0.3 to ship a Xous backend (low probability).
   - File a fork of xous-core's getrandom-xous against the 0.3 line (one-time cost, then unblocks all future deps).
   - Stay on 0.2-line deps as long as possible (this means staying on tungstenite 0.21, eventually old).
   The decision should be made before Stage 5 (when libsignal pulls many `rand`/`getrandom`-using transitive deps) and ideally before Stage 6 (libsignal-service-rs fork — its current main may use rand 0.9).

2. **Stage 3 doesn't decode the binary frame.** We confirmed Signal pushed a 94-byte frame but didn't parse it. The frame is presumably a `ProvisioningStep::Url` protobuf. Decoding it would be Stage 4+ when we have prost in the dep tree. Left as out-of-scope for Stage 3 per the ROADMAP.

## Files changed (since Stage 2 commit)

```
modified:
  Cargo.toml                                                   (workspace patches unchanged from Stage 2)
  Cargo.lock                                                   (regenerated)
  crates/xous-net-bridge/Cargo.toml                            (+rustls-pemfile, +tungstenite 0.21)
  crates/xous-net-bridge/src/lib.rs                            (export ws + new helpers)
  crates/xous-net-bridge/src/tls.rs                            (signature change; pinned-CA helpers)
  crates/xous-net-bridge/examples/https_get.rs                 (use webpki_roots())

new:
  crates/xous-net-bridge/src/ws.rs                             (~25 lines)
  crates/xous-net-bridge/examples/signal_ws_keepalive.rs       (~50 lines)
  crates/xous-net-bridge/certs/signal-production.pem           (vendored)
  crates/xous-net-bridge/certs/signal-staging.pem              (vendored)
  stage/REPORT-3.md                                            (this file)
```
