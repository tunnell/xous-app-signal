*** Comments ***
xas Stage 9b smoke test (Robot Framework + Renode).

Boots a Xous image containing the xas Signal client app and asserts
on UART output. The asserted strings come from our log::info!
calls in the splash flow — no real network access is exercised by
this test (the smoke is "binary boots, UI lays out, log goes to
UART"). Stage 10/11/12 hardware-integration tests extend this with
a mocked Signal server harness; they live in xas-link-mock.robot,
xas-receive-mock.robot, xas-send-mock.robot — currently TODO.

Run via:    renode-test tests/renode/xas-smoke.robot
            (or `cargo xtask renode-test` from the workspace root)

Prerequisites:
  - Renode 1.16+ on PATH (verified: 1.16.1.4499 in this dev env).
  - A Xous image with xas bundled. Two paths to produce one:
    Plan-A: integrate our xtask into xous-core's tree at
            apps/xas/, then `cargo xtask renode-image` from
            xous-core's root.
    Plan-B: build with `cargo xtask dist` here, then have a
            xous-core-side script copy `dist/xas-rv32/xas` into
            xous-core's image-builder before invoking
            `cargo xtask renode-image`.
  - The `xas-smoke.resc` script's `$xous_core_root` and
    `$xous_image` variables point at real local paths.

*** Settings ***
Suite Setup     Setup
Suite Teardown  Teardown
Test Setup      Reset Emulation
Test Teardown   Test Teardown
Resource        ${RENODEKEYWORDS}

*** Variables ***
${SCRIPT_DIR}=  ${CURDIR}

*** Keywords ***
Create Xas Machine
    Execute Command  $script_dir = '${SCRIPT_DIR}'
    Execute Command  include @${SCRIPT_DIR}/xas-smoke.resc

*** Test Cases ***
Should Boot And Print Splash
    Create Xas Machine
    Create Terminal Tester    sysbus.uart    timeout=30
    Start Emulation
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
    # Stage 9b's smoke is "binary booted, splash rendered, log
    # reached UART." We don't drive the keyboard from Robot; that's
    # what Stage 10/11/12 mock tests do. The xas binary's hosted
    # entry runs through the splash and exits when stdin EOFs.
    # On Renode there's no stdin EOF, so we instead assert the
    # splash banner reached UART.
    Wait For Line On Uart    xas
    Wait For Line On Uart    Signal client for Precursor

Should Reach Conversation List After Mock Link
    [Documentation]    Stage 11+ mock-server smoke. Currently a
    ...    placeholder — needs mocked-Signal-server side-channel
    ...    to inject a canned link/receive response. Skipped in
    ...    Stage 9b's first pass; tracked in REPORT-9b.md.
    [Tags]    skip
    Skip    Mocked Signal server harness not yet wired up
