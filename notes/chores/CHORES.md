# Chores — testing-infrastructure gaps surfaced by the MCP rewrite

This file collects small fixes / follow-ups discovered while
building `tools/mcp-server/` (the Python rewrite of the bash test
scripts). Each entry has a short rationale and a concrete next step
so it's actionable without re-reading the rewrite commits.

Most of the runtime gaps are now closed by the Python entry points
themselves; the entries below are about either:

- doc updates the rewrite didn't include in scope, or
- one-time host / Pi setup steps the bash scripts assumed but never
  enforced, or
- legacy `*.sh` knowledge that lives in muscle memory but doesn't
  match the new defaults.

---

## XOUS_TARGET inconsistency across the bash scripts

**Where it bit:** running `bash tests/precursor/build-and-bundle.sh`
followed by `bash tests/precursor/flash-via-pi.sh` would write the
image to one directory and then try to flash from another — because
`build-and-bundle.sh` defaulted `XOUS_TARGET=riscv32imac-unknown-xous-elf`
(the cargo target triple) while `flash-via-pi.sh` and
`flash-direct.sh` defaulted to the legacy `precursor-c809403e` alias.
`<xous-core>/target/$XOUS_TARGET/release/xous.img` resolves to a
different path under each.

**Status:** fixed. `dev` already had the build-and-bundle.sh fix in
commit `7655854 fix(build-and-bundle.sh): correct XOUS_TARGET
default to the cargo target triple`; the MCP rewrite goes the rest
of the way by routing both build and flash through one `Config`
object whose only `XOUS_TARGET` default is the cargo triple. The
`*.sh` scripts are now thin shims and have no defaults of their own.

**Still pending:** `tests/precursor/README.md` ("Environment
variables" table, last column) still documents the legacy alias as
the default. Update the table to match the new canonical value the
next time README docs get touched. (The README update was
intentionally not bundled with the rewrite to keep that PR
narrowly scoped — it's a doc-only correction.)

---

## "uart-logs file doesn't exist" friction

**Where it bites:** every Pi reboot, the persistent `screen -dmS
uart -L -Logfile ~/uart-logs/precursor-uart.log /dev/ttyAMA0 115200`
session dies. The next `watch-uart` invocation finds
`~/uart-logs/precursor-uart.log` is stale (or absent) and bails out.
The old bash script errored with a one-line "log file missing"; the
Python entry point prints the exact `ssh "$PI_HOST" 'mkdir -p ... &&
screen -dmS uart ...'` recipe to copy-paste.

**Real fix:** systemd unit (or `@reboot` cron entry) on the Pi that
auto-starts the UART screen session at boot. Not in scope here —
this is a Pi-side ops change. Sketch::

    # /etc/systemd/system/precursor-uart.service
    [Unit]
    Description=Persistent UART capture for Precursor on /dev/ttyAMA0
    After=network.target
    [Service]
    Type=forking
    User=pi
    ExecStart=/usr/bin/screen -dmS uart -L \
        -Logfile %h/uart-logs/precursor-uart.log /dev/ttyAMA0 115200
    Restart=on-failure
    [Install]
    WantedBy=multi-user.target

Then `systemctl enable --now precursor-uart.service` on the Pi.

---

## flash-via-pi robustness — closed

**Was:** `tests/precursor/flash-via-pi.sh` ran
`ssh "$PI_HOST" "python3 usb_update.py -k xous.img --bounce > $FLASH_LOG"`.
SSH disconnect or local Ctrl-C → python3 on the Pi gets SIGHUP and
the write halts mid-block. This hit on 2026-05-13 multiple times
and forced manual recovery.

**Closed by:** the rewrite routes the Pi-side flash through
`xas_mcp.ssh.screen_detached`, which wraps the command in `screen
-dmS … bash -c 'nohup … > log 2>&1'`. The local Python returns
immediately; the Pi keeps writing regardless of what happens on the
build host.

**Follow-up:** none on the build-host side. On the Pi side, watch
for stale `flash_*` screen sessions accumulating after aborted runs
— `screen -ls | awk '/\\.flash_/ {print $1}' | xargs -r -n1 screen
-S {} -X quit` cleans them up; consider adding to a Pi-side cron if
this becomes a regular nuisance.

---

## Orphan hosted-mode kernels

**Where it bit:** running `bash tests/hosted/test_link_qr.sh` twice
in a row, second invocation panics with `couldn't create socket for
DNS resolver`. Diagnosed in `feedback_pretest_kernel_cleanup` user
memory.

**Closed by:** `xas_mcp.tests_hosted.kill_orphan_kernels()` runs
before every hosted-mode test launch. Pattern is shared across
tests via the `cleanup_orphans=True` default on
`run_hosted_test()`.

**Follow-up:** the Python `tests/hosted/test_xas_round_trip.py` test
files (the long-running drive scripts, not the orchestrator) don't
go through `run_hosted_test` and still do their own pkill. That's
fine — those files were already doing the right thing per the
memory. No action needed.

---

## Stale env-var documentation in `tests/precursor/README.md`

**Specifically:**

- "Environment variables" table at the bottom still lists
  `XOUS_TARGET` default as `precursor-c809403e`. New canonical is
  `riscv32imac-unknown-xous-elf` (cargo target triple). The Python
  Config and `.env.example` both use the new value.
- "One-time Pi rig setup" mentions only one `PI_FLASH_DIR`
  convention (`~/xous-flash`) — that matches the Python default,
  so this is OK, but the README phrases it as "edit per device"
  which is no longer needed; the Python tools take it from env or
  `.env`.

**Action:** small README pass next time docs get touched. Out of
scope for this rewrite (which intentionally avoided changing
documented behavior).

---

## `usb_update.py` one-time staging

**Where it bites:** flashing from a fresh Pi (or a Pi that's been
reflashed since the last campaign) fails because
`~/xous-flash/usb_update.py` doesn't exist. The Python flash tool
detects this and prints the `scp ${XOUS_CORE_DIR}/tools/usb_update.py
${PI_HOST}:${PI_FLASH_DIR}/` recipe — same as the bash script did.

**Real fix:** the rewrite could ship a `xas_mcp.cli.pi_setup`
subcommand that does the scp + creates `~/uart-logs/` + starts the
UART screen session. Worth a future commit on the `mcp` branch —
the gap is a one-time-per-Pi friction, not a per-run one.

---

## Hosted-mode test catalog

Right now `xas_mcp.tests_hosted.KNOWN_HOSTED_TESTS` maps three
names: `link_qr`, `send_receive`, `signal_cli_echo`. The repo also
has `tests/hosted/test_xas_round_trip.py`,
`test_xas_round_trip_pcap.py`, and `scan_receive.sh` that aren't
registered. Either:

1. add them to the catalog (and write a CLI per — `xas_mcp.cli.test_<name>`), or
2. expose a free-form `run_hosted_test_path(script_path)` for one-off
   scripts.

The MCP server surface deliberately allow-lists rather than
free-forming the path — drives a discoverable set of tools. (1)
is the right move when the next test gets standardised.

---

## CLI entry points vs `pip install -e`

The shims under `tests/*/` set `PYTHONPATH` themselves, so callers
don't need to run `pip install -e tools/mcp-server` first. But the
`xas-mcp-server`, `xas-flash-via-pi`, etc. console-script entry
points declared in `pyproject.toml` only show up if you do install
the package. Document this once: either ship a `tools/mcp-server/bin/`
dir with shim binaries that mirror the console_scripts entries, or
just tell users "`pip install -e tools/mcp-server` gets you the
short commands; the shims work without it".

---

## Not on this list (intentionally)

These are not chores; documenting why so future sweeps don't pick
them up:

- **`tests/precursor/README.md` Brick prevention rules.** The
  rewrite preserves them exactly (`-k --bounce` only, no `-l`, no
  `--soc` / `--factory-reset`). Encoded as test asserts in
  `tests/test_flash.py`.
- **`tests/renode/xas-smoke.resc` working directory.** The Python
  runner cd's into `tests/renode/` before invoking renode-test for
  the same .resc include resolution reason the bash script did.
- **Vendored crypto crates** under `vendor/`. Out of scope for this
  task per the crypto-vetting constraint.
