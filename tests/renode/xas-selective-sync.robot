*** Comments ***
Selective-dict-sync boot-regression test.

What this verifies (locally, in Renode):

  Boot-survives the dirty-set tracking in
  `xous-core/services/pddb/src/backend/basis.rs`. Reaching
  `Requesting login password` proves:
   - `BasisCacheEntry::sync` compiled with the dirty-set field
     and selective code path,
   - none of the mutation sites (key_update, key_remove,
     key_list_remove, dict_add, dict_remove, dict_delete) tripped
     a compile- or runtime-error on the `mark_dirty`/`mark_clean`
     calls,
   - the basis-init path through `BasisCacheEntry::mount` populates
     `dirty_dicts: HashSet::new()` cleanly.

What this DOES NOT verify (hardware-only):

  The `n_dicts=1` assertion described in the design doc requires:
   - PDDB mounted (= user typed PIN on real device — gen1 TryMount
     cannot bypass that in Renode),
   - User running `pddb bulk_probe 1` in shellchat (= keyboard
     injection into GAM/IME, which Renode's display chain does not
     support without significant additional rig).

  On hardware, the maintainer flashes this build, unlocks PDDB,
  runs `pddb bulk_probe 1`, and greps the UART log:

    perf/pddb: BasisCacheEntry::sync entry basis=".System"
      n_dicts=1 dirty_set_size=1 total_dicts=22 cleanup=false

  The `n_dicts=1` part is the headline assertion. `dirty_set_size`
  lets the maintainer cross-check that the selective-sync path
  actually fired (not the cleanup fallback).

Build:

    cd /path/to/xas
    cargo build --release --target=riscv32imac-unknown-xous-elf \
        -p xous-app-signal --features pddb-real,precursor
    cp target/riscv32imac-unknown-xous-elf/release/xas \
        dist/xas-rv32/xas
    cd xous-core
    cargo xtask app-image \
        xas:.../xas --git-describe v0.9.21-0-g0000000 \
        --feature pddb/autobasis

Then `renode-test tests/renode/xas-selective-sync.robot`.

*** Settings ***
Suite Setup     Setup
Suite Teardown  Teardown
Test Setup      Reset Emulation
Test Teardown   Test Teardown
Resource        ${RENODEKEYWORDS}

*** Variables ***
${SCRIPT_DIR}=  ${CURDIR}
${UART_TIMEOUT}=  240

*** Keywords ***
Create Xas Machine
    Execute Command  $script_dir = '${SCRIPT_DIR}'
    Execute Command  include @${SCRIPT_DIR}/xas-smoke.resc

*** Test Cases ***
Should Boot Selective-Sync Kernel To Password Prompt
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    # The dirty-set tracking is on the boot critical path — every
    # PDDB-internal sync that fires before mount goes through the
    # selective `BasisCacheEntry::sync(cleanup=false)` code path.
    # If the dirty-set HashSet broke construction or any mark_dirty
    # call site fails to compile / panics, boot wedges short of
    # the password prompt.
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    Wait For Line On Uart    Requesting login password
