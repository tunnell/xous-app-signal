*** Comments ***
xas boot-smoke test (Robot Framework + Renode).

Boots a Xous image containing the xas Signal client app (bundled
into xous-core via the `xas` entry in apps/manifest.json) and
asserts that the two log lines emitted at the top of xas's main()
appear on the UART console. Like every other Xous PID, xas starts
at boot — no user interaction required to reach the asserted lines.

Run via:    renode-test tests/renode/xas-smoke.robot
            (or `cargo xtask renode-test` from the workspace root,
             or ./tests/renode/run-renode-tests.sh which also
             rebuilds the dist artifact first)

Prerequisites:
  - Renode 1.16+ on PATH.
  - A Xous image with xas bundled. The easiest path is
    `tests/renode/run-renode-tests.sh` (from the repo root),
    which builds the rv32 ELF, bundles a fresh xous.img into the
    xous-core the .resc boots, then runs this test. To do it by
    hand, mirror that script: build with
    `--features pddb-real,precursor`, then
    `cargo xtask app-image-xip xas:<elf> vault transientdisk
    --kernel-feature big-heap --git-describe v0.9.21-0-g0000000`
    from your xous-core checkout. The `--git-describe` is needed
    because the fork has no reachable tags; xous-sign-image
    otherwise fails on `git describe`. Any v-prefixed semver works.
  - `xas-smoke.resc` resolves `$xous_core_root` via the
    repos/xous-core symlink (BUILDING.md §1); override with
    `renode -e '$xous_core_root=@<path>'` for a non-standard layout.

*** Settings ***
Suite Setup     Setup
Suite Teardown  Teardown
Test Setup      Reset Emulation
Test Teardown   Test Teardown
Resource        ${RENODEKEYWORDS}

*** Variables ***
${SCRIPT_DIR}=  ${CURDIR}
# Xous boot in Renode goes through kernel init, all services
# (graphics, gam, pddb, etc.), and then app PIDs. Several seconds
# per stage. Generous timeout to accommodate slow CI hardware.
${UART_TIMEOUT}=  120

*** Keywords ***
Create Xas Machine
    Execute Command  $script_dir = '${SCRIPT_DIR}'
    Execute Command  include @${SCRIPT_DIR}/xas-smoke.resc

*** Test Cases ***
Should Boot And Run Xas
    Create Xas Machine
    # betrusted.resc creates two UART-shaped peripherals on the SoC
    # machine: `sysbus.uart` (kernel-only output: panics, process
    # termination notices) and `sysbus.console` (xous-log-server's
    # destination: every log::info! call from every app). Our
    # asserted lines come from log::info! in xas's main(), so the
    # tester targets `sysbus.console`.
    Create Terminal Tester    sysbus.console    timeout=${UART_TIMEOUT}    machine=SoC
    Start Emulation
    Wait For Line On Uart    xas: starting
    Wait For Line On Uart    xas: worker started
