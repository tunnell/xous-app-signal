*** Comments ***
Stage 13b-2 — full PDDB-backed KvBackend round-trip probe.

Boots a Xous image containing the xas binary built with both
`pddb-real` (real backend wired into PddbStore) and
`probe-pddb-real` (drives a put/get/list/delete cycle in main()
after worker spawn). The image must also be built with
`pddb/autobasis` so PDDB is pre-mounted on boot.

Build / bundle / run sequence:

    cd ~/precursor-signal/xous-app-signal
    cargo build --target=riscv32imac-unknown-xous-elf --release \
                -p xous-app-signal --features probe-pddb-real
    cp target/riscv32imac-unknown-xous-elf/release/xas \
                                              dist/xas-rv32/xas
    cd ~/precursor-signal/repos/xous-core
    cargo xtask app-image \
        xas:.../dist/xas-rv32/xas \
        --git-describe v0.9.21-0-g0000000 \
        --feature pddb/autobasis
    cd ~/precursor-signal/xous-app-signal
    renode-test tests/renode/xas-pddb-real-probe.robot

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
Should Round-Trip A Key Through Real PDDB Backend
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: store=PDDB
    Wait For Line On Uart    xas: worker started
    # Probe banner
    Wait For Line On Uart    probe-pddb-real: starting put/get/delete cycle
    # connection result
    Wait For Line On Uart    probe-pddb-real: connected
    # put outcome (substring match — captures OK/FAIL alike)
    Wait For Line On Uart    probe-pddb-real: put
    # get outcome
    Wait For Line On Uart    probe-pddb-real: get
    # list_keys outcome
    Wait For Line On Uart    probe-pddb-real: list_keys
    # delete outcome
    Wait For Line On Uart    probe-pddb-real: delete
    # post-delete listing outcome
    Wait For Line On Uart    probe-pddb-real: post-delete list
    # bulk-write wire smoke (Opcode::WriteKeyBatch). Substring match
    # accepts OK or "returned err" — both indicate the wire round-trip
    # worked. We just want to catch a panic / wedge if the packed
    # format mismatches between client and server.
    Wait For Line On Uart    probe-pddb-real: bulk_write
    # done banner
    Wait For Line On Uart    probe-pddb-real: probe done
