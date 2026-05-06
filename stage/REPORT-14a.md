# Stage 14a — auto-link hardware probe

**Date.** 2026-05-06
**Status.** **Code landed**, ready to flash. The auto-link feature
fires `Cmd::LinkDevice` after worker spawn and renders the
provisioning URL as a QR code on the LCD via a hand-rolled
`xous-modals-ipc` client (~150 LoC, same pattern as the
`xous-pddb-ipc` crate from Stage 13b-2). User scans with the
Signal phone app, modal dismisses on keypress, link completes —
or fails with a specific error in the UART log that informs the
next flash iteration.

---

## 1. Architecture

```
xas main()
  ├── init_logger() / log "xas: starting"
  ├── build_store()                            ← real PDDB on rv32
  ├── run_signal_worker(store, cmd_rx, event_tx)
  │     └── spawns the presage::Manager state machine
  ├── log "xas: worker started"
  └── auto_link(cmd_tx.clone(), event_rx.clone())
        ├── XousNames + Trng + ModalsClient
        ├── cmd_tx.send(Cmd::LinkDevice { device_name })
        └── loop {
              event_rx.recv() {
                LinkUrl(url)              → modals.show_notification(text, Some(url))  ← QR rendered
                LinkComplete{aci,phone,…} → log success + final modal, break
                LinkError(msg)            → log failure + final modal, break
                other                     → log + continue
              }
            }
```

Three crates collaborate:

- `xous-signal-bridge` — already-existing Manager worker. Its
  `Cmd::LinkDevice` handler calls
  `presage::Manager::link_secondary_device`, which builds the
  `tsdevice://` URL and emits it as `Event::LinkUrl`. After the
  user scans, the worker decodes the encrypted provision envelope
  (received over the WS), persists registration data via
  `PddbStore::with_pddb_backend`, and emits `Event::LinkComplete`
  or `LinkError`.
- `xous-modals-ipc` — new this stage. Hand-rolled IPC client over
  `services/modals` that calls `show_notification(text, Some(qr))`
  to render a QR code. Bypasses the upstream `modals` crate's
  full `gam`/`blitstr2`/`ux-api` dep cascade — same trade-off as
  Stage 13b-2's `xous-pddb-ipc`.
- `xous-app-signal` — the `auto_link()` function plus an
  `auto-link` Cargo feature flag.

## 2. The QR rendering path

`services/modals/src/main.rs` handles `Opcode::Notification` by:

1. Decoding the lent rkyv buffer back into `ManagedNotification`.
2. Constructing a `Notification` widget.
3. If `qrtext.is_some()`, calling `notification.set_qrcode(text)`
   — this enables the QR overlay in the GAM modal layout.
4. Drawing the modal to Precursor's 336×536 LCD via blitstr2.
5. Parking until the user presses any key.

Capacity (per the comment at `services/modals/src/api.rs`): a
Type 40 (177×177) QR with Medium ECC encodes up to 3391
alphanumeric characters. Signal's `tsdevice://` provisioning URLs
fit comfortably within that — they're typically 100–200 chars.

## 3. Build + flash recipe

```sh
# 1. Build the rv32 binary with hardware features.
cd ~/precursor-signal/xous-app-signal
cargo build --target=riscv32imac-unknown-xous-elf --release \
            -p xous-app-signal --features pddb-real,auto-link
cp target/riscv32imac-unknown-xous-elf/release/xas dist/xas-rv32/xas

# 2. Bundle into a Xous image. xtask app-image picks up the prebuilt
#    ELF via the `name:path` cratespec syntax. --git-describe is
#    required because the fork has no reachable tags (Stage 9b-deploy B).
cd ~/precursor-signal/repos/xous-core
git checkout xas
cargo xtask app-image \
    xas:$HOME/precursor-signal/xous-app-signal/dist/xas-rv32/xas \
    --git-describe v0.9.21-0-g0000000

# Outputs (all under target/riscv32imac-unknown-xous-elf/release/):
#   xous.img       ← the bundled, signed kernel + image
#   loader.bin     ← the signed loader

# 3. Flash to Precursor over USB. Device must be in update mode
#    (hold left soft button while booting; LED goes red).
python3 tools/updater/precursorupdater/precursorusb.py \
    --soc target/riscv32imac-unknown-xous-elf/release/loader.bin \
    --kernel target/riscv32imac-unknown-xous-elf/release/xous.img
```

## 4. First-boot expected flow

1. **Boot.** Xous comes up, GAM renders the home screen.
2. **PDDB first-boot init** (one-time per Precursor). A modal
   prompts for a fresh password. User picks one. PDDB formats
   itself and persists. From here on the device boots straight to
   the home screen.
3. **Click Signal in the launcher menu.** xas starts as PID 27.
4. **Boot lines** appear on UART:
   ```
   xas: starting
   xas: store=PDDB (real)
   xas: worker started
   auto-link: starting
   auto-link: sending Cmd::LinkDevice { device_name = "xas-hardware-probe" }
   ```
5. **Provisioning WS connect.** Worker opens TLS+WS to
   `chat.signal.org`. UART:
   ```
   auto-link: link URL = sgnl://linkdevice?uuid=...&pub_key=...&capabilities=backup5
   ```
6. **QR modal appears on the LCD.** "Scan with the Signal phone
   app, then press any key." plus a QR overlay.
7. **User scans** with Signal mobile. Phone confirms; Signal's
   server forwards the encrypted provision envelope to the WS.
8. **User presses any key** to dismiss the modal.
9. **Worker decodes envelope, persists registration data**, emits
   `Event::LinkComplete`. UART:
   ```
   auto-link: LinkComplete device="xas-hardware-probe" aci=… phone=…
   ```
10. **Final modal** confirms the link details.

Total time on a working setup: ~5–10 seconds of human action plus
network round-trips.

## 5. Likely failure modes

In rough order of probability, with diagnostic UART line + fix path:

| symptom | UART signal | likely cause | fix |
|---------|-------------|--------------|-----|
| No `xas: starting` | (silence) | Image didn't bundle xas, or boot wedged in PDDB modal | Verify `PID 27: xas` in the app-image build output. If wedged at PDDB password modal, that's just the one-time setup — finish it. |
| `auto-link: sending` then nothing | parked on cmd_tx send | Worker died before consuming. Look for a panic earlier in log. | `RUST_BACKTRACE=1` won't help on rv32; lean on `log::trace!` adds. |
| `LinkError: timeout`, fast | <2 s | Net stack didn't reach DNS — WiFi not configured on Precursor. | `shellchat`'s `wlan` commands; configure SSID. |
| `LinkError: connection refused` | seconds | TLS handshake failed — certificate mismatch | Check `xous-net-bridge::signal_production_roots` against Signal's current cert chain. |
| `LinkError: WS handshake` | seconds | tungstenite version drift vs. server's WS subprotocol | Pin tungstenite version; check Signal protocol changes. |
| `LinkError: PDDB write` | after scan | Real-data path the unmounted-state probe didn't cover | First put/get/delete with real values — likely a small bug in xous-pddb-ipc's PddbBuf streaming layer. |
| QR modal renders but never dismisses | indefinite | User scanned but key press didn't register | Hardware kbd issue; investigate via `kbd-test` in shellchat. |

Each row is one flash-iter to surface and one to fix. **Expected
total: 1–4 flash cycles to first `LinkComplete`.**

## 6. Files touched

```
A  crates/xous-modals-ipc/Cargo.toml          (new crate)
A  crates/xous-modals-ipc/src/lib.rs          (~150 LoC client)
M  Cargo.toml                                 (workspace member)
M  crates/xous-app-signal/Cargo.toml          (auto-link feature + dep)
M  crates/xous-app-signal/src/main.rs         (auto_link() + call site)
M  vendor/libsignal-service-rs/src/provisioning/pipe.rs
                                              (rv32 clippy fix:
                                               #[expect] -> #[allow])
A  stage/REPORT-14a.md                        (this file)
```

Total new code: `xous-modals-ipc` (~150 LoC) + `auto_link()` (~90
LoC) ≈ 240 LoC. Within the 2× rule's headroom.

## 7. Verification

```
cargo build --target=riscv32imac-unknown-xous-elf --release \
            -p xous-app-signal --features pddb-real,auto-link    → ok (1m01s)
cargo xtask app-image xas:.../xas --git-describe …                → image rebuilt
                                                                   (PID 27: xas)

# After restoring the non-auto-link binary:
renode-test tests/renode/xas-smoke.robot                          → PASS (44 s)

cargo test -p xous-signal-bridge -p xous-app-signal-ui \
           -p presage-store-pddb                                  → 3 + 31 + 22 passed
cargo clippy --workspace --all-targets -- -D warnings             → clean (hosted)
cargo clippy -p xous-app-signal --features pddb-real,auto-link \
             --target=riscv32imac-unknown-xous-elf \
             -- -D warnings                                        → clean (rv32)
cargo fmt --all -- --check                                        → clean
```

## 8. What's next

**Stage 14b — first flash + iterate.** The user has a flashable
artifact now. One flash, watch UART, fix what breaks. Each
diagnostic iteration is informed by the table in §5. After
`LinkComplete` lights up green:

- **Stage 14c — auto-receive.** Same shape: feature flag drives
  `Cmd::StartReceive`, prints first received `Event::Message`.
- **Stage 14d — auto-send.** Same shape: hardcoded recipient
  (probably the user's own phone), `Cmd::SendMessage`, watch for
  `Event::SendComplete`.

MVP is done when 14c + 14d both light up green on the same hardware.
