*** Comments ***
Exploratory PDDB IPC probe.

Boots the same Xous image as xas-smoke.robot, but expects the
binary to have been built with `--features probe-pddb`. The probe
calls xous-core's PDDB Mount Poller via raw `xous::send_message`
(the same path a hand-rolled PDDB client would take) and logs
the result.

Run via:    renode-test tests/renode/xas-pddb-probe.robot

Prerequisites:
  - Same as xas-smoke.robot.
  - The bundled xas binary must be built with `cargo build
    --features probe-pddb` and redist'd:
        cargo build --target=riscv32imac-unknown-xous-elf --release \
                    -p xous-app-signal --features probe-pddb
        cp target/riscv32imac-unknown-xous-elf/release/xas \
                                                 dist/xas-rv32/xas
        cd <your xous-core checkout>
        cargo xtask app-image xas:.../xas --git-describe v0.9.21-0-g0000000

*** Settings ***
Suite Setup     Setup
Suite Teardown  Teardown
Test Setup      Reset Emulation
Test Teardown   Test Teardown
Resource        ${RENODEKEYWORDS}

*** Variables ***
${SCRIPT_DIR}=  ${CURDIR}
${UART_TIMEOUT}=  120

*** Keywords ***
Create Xas Machine
    Execute Command  $script_dir = '${SCRIPT_DIR}'
    Execute Command  include @${SCRIPT_DIR}/xas-smoke.resc

*** Test Cases ***
Should Probe PDDB Mount Poller
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    # Probe banner — confirms feature flag took effect.
    Wait For Line On Uart    probe-pddb: starting PDDB mount-poller probe
    # Connection establishment line. Substring match on
    # "connected to" so it works regardless of how long the
    # XousNames lookup takes.
    Wait For Line On Uart    probe-pddb: connected to PDDB Mount Poller
    # The result line — substring match on "Poll" so it captures
    # OK / FAIL / unexpected regardless of outcome.
    Wait For Line On Uart    probe-pddb: Poll
    # Final probe done banner.
    Wait For Line On Uart    probe-pddb: probe done
