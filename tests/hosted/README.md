# tests/hosted — running hosted-mode tests

Hosted mode runs the full Xous kernel + services + apps as a
single Linux process, with the GAM rendered to a minifb window
labelled "Precursor". It boots in seconds, uses your real Wi-Fi
via the host kernel, and talks to the real Signal server. It's
the workhorse for UI iteration and most logic-bug fixes — see
[`../README.md`](../README.md) for how it compares to the other
testing approaches.

## Prerequisites

- [`../../BUILDING.md`](../../BUILDING.md) sections 0 and 1 done
  (Rust toolchain, repos cloned with the `repos/xous-core`
  symlink in place)
- `xset q` returns without error (X11 display reachable; over
  SSH use `ssh -X`)
- A real Signal account with `signal-cli` installed as the test
  peer (BUILDING.md section 0 "Required for hosted path")

## Scripts in this folder

| File | Purpose |
|---|---|
| `test_link_qr.sh` | Headless smoke test: boots hosted, drives launcher to xas link screen, gates on the provisioning URL appearing in the kernel log. The cheapest end-to-end check. |
| `drive_link.py` | Helper used by `test_link_qr.sh` to script keystrokes into the minifb window. |
| `scan_receive.sh` | Boots hosted with a longer hold so you can scan the QR from your phone and verify a receive end-to-end. |
| `test_helpers.sh` | Shared bash helpers (sourced by the other scripts). |
| `test_env.example` | Template for `tests/hosted/test.env` — copy and fill in before running scripts that need a peer phone number. |

## Running the headless link smoke test

```sh
cd /path/to/xous-app-signal
INSPECT_HOLD=900 bash tests/hosted/test_link_qr.sh
```

`INSPECT_HOLD` keeps the kernel alive for that many seconds
after the QR code appears, so you can scan it from your phone
and observe the rest of the link flow interactively. Skip it
(or set to a small value) if you only want to validate that xas
reaches the QR-display stage.

The script writes its kernel log to a temp directory whose path
it prints on startup. Save that path if you need to diff
against a known-good run.

## Running ad-hoc hosted with signal-cli as peer

For send/receive testing you'll want a non-headless session.
Build xas first:

```sh
cd /path/to/xous-app-signal
cargo build --release -p xous-app-signal --features pddb-real,hosted
```

Then from the **xous-core** directory:

```sh
cd /path/to/xous-core
cargo xtask run xas:../xous-app-signal/target/release/xas
```

Once the minifb window appears: launcher → Apps → xas → Link
device. Scan the QR from your phone. Then send a message from
your other Signal account to your linked phone — it should
appear in xas's home screen within seconds. Send a reply to
verify the outbound path.

`signal-cli` makes a useful test peer because you can script
sends/receives from a separate terminal:

```sh
# In a side terminal — assumes signal-cli is registered to a
# different Signal account than the one xas links to
signal-cli -u +SECONDARY_NUMBER send -m "hello from side terminal" +PRIMARY_NUMBER
signal-cli -u +SECONDARY_NUMBER receive
```
