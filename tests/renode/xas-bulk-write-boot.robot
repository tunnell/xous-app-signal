*** Comments ***
Boot-regression test for hardware-flag builds with PDDB enabled.

Verifies that a canonical hardware-flag build (no auto-firing
probe features) boots far enough that PDDB finishes service
registration and reaches the password prompt — i.e. no
`ServerNotFound` cascade in unrelated services (llio, trng, modals,
susres).

This test is the regression guard against re-introducing an
auto-fire that races with xous-names server registration during
boot. xas deliberately avoids calling
`presage_store_pddb::PddbBackend::connect()` immediately after
spawning the worker; bulk-write benchmarking is exposed via the
shellchat `pddb bulk_probe` command (user-invoked after PIN entry
and PDDB mount), not via an auto-fire feature.

Reaching the `Requesting login password` line means the PDDB
service started cleanly, which can only happen after llio + trng +
modals + susres have all registered with xous-names.

Build / bundle / run sequence:

    cd /path/to/xas
    cargo build --target=riscv32imac-unknown-xous-elf --release \
                -p xous-app-signal --features pddb-real,precursor
    cp target/riscv32imac-unknown-xous-elf/release/xas \
                                              dist/xas-rv32/xas
    cd xous-core
    cargo xtask app-image \
        xas:.../dist/xas-rv32/xas \
        --git-describe v0.9.21-0-g0000000 \
        --feature pddb/autobasis
    cd /path/to/xas
    renode-test tests/renode/xas-bulk-write-boot.robot

NOTE: this test does NOT pass `pddb/autobasis` to the build flags
that would cause it to fail on the password prompt. The point is
to confirm the boot path REACHES the password prompt without
crashing first — what the user is unable to satisfy in Renode is
the password modal itself, not the prompt.

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
Should Boot Bulk-Write Image Through PDDB Service Registration
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    # xas process starts (means xas's own boot is OK)
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    # PDDB has reached the point of waiting for the user — this only
    # fires after llio/trng/modals/susres/keystore have all registered
    # with xous-names and PDDB's own init has progressed to the
    # password-unlock step. Failure mode (the regression we guard
    # against): a `ServerNotFound` cascade aborts one of these services
    # before PDDB gets there, and this line never appears within the
    # UART timeout.
    Wait For Line On Uart    Requesting login password
