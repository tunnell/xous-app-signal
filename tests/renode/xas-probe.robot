*** Comments ***
Exploratory network-reachability probe.

Boots the same Xous image as xas-smoke.robot, but expects the
binary to have been built with `--features probe-flow`. The probe
fires after `xas: worker started` and runs three TCP-connect
attempts: Google DNS (8.8.8.8:53), Cloudflare HTTPS (1.1.1.1:443),
and Signal prod (chat.signal.org:443). The Robot test only waits
on the probe's `done` line — the per-probe outcomes are captured
in the renode-test log either way (xas-probe.fail0.log on fail).

Run via:    renode-test tests/renode/xas-probe.robot

Prerequisites:
  - Same as xas-smoke.robot (Renode, image with xas bundled).
  - The bundled xas binary must be built with `--features
    probe-flow` and then redist'd via the manual flow:
        cargo build --target=riscv32imac-unknown-xous-elf --release \
                    -p xous-app-signal --features probe-flow
        cp target/riscv32imac-unknown-xous-elf/release/xas \
                                                 dist/xas-rv32/xas
        cd ~/precursor-signal/repos/xous-core
        cargo xtask app-image xas:.../xas --git-describe v0.9.21-0-g0000000
  - The smoke build (no probe-flow) should NOT be in the image
    when running this test, or the probe lines won't appear.

*** Settings ***
Suite Setup     Setup
Suite Teardown  Teardown
Test Setup      Reset Emulation
Test Teardown   Test Teardown
Resource        ${RENODEKEYWORDS}

*** Variables ***
${SCRIPT_DIR}=  ${CURDIR}
# Probe sleep budget: 3 connects × 10 s timeout each = 30 s probe
# wall-clock; plus boot. 240 s gives slack for slow CI.
${UART_TIMEOUT}=  240

*** Keywords ***
Create Xas Machine
    Execute Command  $script_dir = '${SCRIPT_DIR}'
    Execute Command  include @${SCRIPT_DIR}/xas-smoke.resc

*** Test Cases ***
Should Run Network Probe
    Create Xas Machine
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    # Boot lines, same as smoke. If these fail to appear we never
    # reached probe code anyway.
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    # Probe banner — confirms feature flag took effect.
    Wait For Line On Uart    probe: starting network reachability probe
    # One Wait per probe target. Each matches either "CONNECT OK"
    # or "CONNECT FAIL" via substring match on the label, so the
    # test passes regardless of outcome — the *outcome* is what we
    # want logged.
    Wait For Line On Uart    probe: google-dns
    Wait For Line On Uart    probe: cloudflare-https
    Wait For Line On Uart    probe: signal-prod
    Wait For Line On Uart    probe: network probe done
