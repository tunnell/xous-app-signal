*** Comments ***
Phase 2 — `BufferingBackend` + `BatchGuard` smoke under Renode.

Boots the standard xas-smoke image with xas built under the
`probe-send-batch` feature. The probe drives a synthetic
"send-shaped" sequence of writes through `BufferingBackend`
wrapping a `MockBackend`, then a commit, then an abort path. It
logs UART lines that this Robot test asserts on.

The probe uses `MockBackend` rather than the real PDDB because
the gen1 PDDB can't auto-mount in Renode (no rootkeys + no
password modal injection). The wrapper semantics are independent
of inner backend; this probe validates that the abstraction
compiles for rv32 and runs under the real xous async runtime.
The host-side `cargo test -p presage-store-pddb` covers the same
abstraction in 15 unit tests.

Build / run sequence (mirrors xas-pddb-real-probe.robot):

    cd <xas repo root>
    cargo build --target=riscv32imac-unknown-xous-elf --release \
                -p xous-app-signal --features precursor,probe-send-batch
    cp target/riscv32imac-unknown-xous-elf/release/xas \
                                             dist/xas-rv32/xas
    cd xous-core
    cargo xtask app-image \
        xas:<xas repo root>/target/riscv32imac-unknown-xous-elf/release/xas \
        --git-describe v0.9.21-0-g0000000
    cd <xas repo root>
    renode-test tests/renode/xas-send-batch.robot

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
Should Buffer Writes Commit And Abort
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    # Baseline xas boot.
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    # Probe banner — confirms the feature flag took effect.
    Wait For Line On Uart    probe-send-batch: starting
    Wait For Line On Uart    probe-send-batch: backend constructed
    # batch begin
    Wait For Line On Uart    probe-send-batch: batch begin OK
    # three writes buffered, with count=3
    Wait For Line On Uart    probe-send-batch: 3 writes buffered
    # intra-batch read-through visible
    Wait For Line On Uart    probe-send-batch: intra-batch read-through OK
    # commit OK with replay count
    Wait For Line On Uart    probe-send-batch: commit OK
    # post-commit reads land
    Wait For Line On Uart    probe-send-batch: post-commit reads match
    # abort path works
    Wait For Line On Uart    probe-send-batch: abort path OK
    # final done line
    Wait For Line On Uart    probe-send-batch: probe done
