# Stage 9b-deploy Phase B — image bundling + Renode smoke

**Date.** 2026-05-06
**Status.** Smoke test **passes end-to-end**. xas binary boots inside
a Xous image in Renode, both `log::info!("xas: starting")` and
`log::info!("xas: worker started")` reach the simulated UART. Total
test runtime: 44 seconds wall-clock.

---

## 1. The end-to-end pipeline

```
xous-app-signal/                 (this workspace, master branch)
  cargo xtask dist           →  dist/xas-rv32/xas (52 MB rv32 ELF)

xous-core/                       (tunnell/xous-core, xas branch)
  apps/manifest.json         +  "xas" entry  (committed)
  cargo xtask app-image          xas:<dist-elf-path>
                              →  target/.../xous.img (11.6 MB,
                                 xas at PID 27)

xous-app-signal/
  renode-test                    tests/renode/xas-smoke.robot
                              →  Wait For Line On Uart  xas: starting
                                 Wait For Line On Uart  xas: worker started
                                 PASS (44 s)
```

Each row above is independent and reviewable on its own.

---

## 2. Two pieces of friction worth keeping

The pipeline took two corrections to land. Both are non-obvious and
likely to bite again — they're documented here and in the `.robot`
file's comments so the next person hits them as one-line fixes.

### 2.1 `--git-describe` for fork-with-no-tags

`cargo xtask app-image …` invokes `xous-sign-image` at the end of the
build, which calls `xous-semver`'s `from_git()` → `git describe`. The
`tunnell/xous-core` fork has no reachable tags from the `xas` branch's
HEAD, so `git describe` exits non-zero and the signing step fails
with `Error: "no major version"`.

Workaround: pass any v-prefixed semver explicitly:

```sh
cargo xtask app-image xas:/path/to/xas --git-describe v0.9.21-0-g0000000
```

The flag's purpose per `xtask/src/main.rs:65` is exactly this case
("for build systems that lack git state"). The smoke-test wrapper
(`run-renode-tests.sh`) does not yet pass it because it runs
`cargo xtask dist` (our standalone xtask) not xous-core's xtask.
A follow-up to wire `--git-describe` into a wrapper that calls into
xous-core's xtask is straightforward; for now the user runs the
xous-core invocation by hand once per source change.

### 2.2 `sysbus.console`, not `sysbus.uart`

The Precursor SoC has two UART-shaped peripherals:

- `sysbus.uart` — kernel-only output. Receives panics, `[!] Terminating
  process …` notices, and the boot banner. *Does not* receive
  `log::info!` from apps or services.
- `sysbus.console` — `xous-log-server`'s destination. Receives every
  `log::info!`/`warn!`/`error!` from every PID, prefixed by
  `INFO:<target>: <message>`.

Both go through xous-log internally, but the kernel's "early"
diagnostic UART is `sysbus.uart` (used before xous-log-server is up);
once the log server is up, app log output is routed to
`sysbus.console`. The `Create Terminal Tester` keyword in the Robot
test must target `sysbus.console` to match on app log lines. This
isn't well documented in xous-core; the canonical reference is
`emulation/betrusted.resc:60–61` (`showAnalyzer uart` / `showAnalyzer
console`).

The `.robot` file now has a comment block explaining this — see
`tests/renode/xas-smoke.robot:38–43`.

---

## 3. What `[!] Terminating process with PID 27` means

The Renode log shows xas (PID 27) terminating at virt: 1.92s, ~0.3s
after `xas: exiting`. This is **expected**: our binary's `main()`
returns after `Ui::new(cmd_tx, event_rx).run()?` returns, which
happens immediately on rv32 because the hosted UI driver's stdin loop
hits EOF on Xous's stdin (Xous doesn't have a meaningful stdin for
GUI apps). The kernel terminates a process whose `main()` returned —
that's the normal exit path.

For Stage 9c (UI loop) and beyond, `Ui::new(...).run()` will be
adapted to drive GAM events on Xous instead of stdin lines. That's
out of scope for the smoke test, which only asserts the boot lines.

---

## 4. What this phase delivered

### In `tunnell/xous-core` on the `xas` branch:

```
M  apps/manifest.json            +12 lines (xas entry)
```

Just the manifest entry. `apps/i18n.json` is gitignored — generated
from `manifest.json` by xtask at build time per
`xtask/src/app_manifest.rs:48`.

### In `xous-app-signal` on `master`:

```
M  tests/renode/xas-smoke.resc   simplified to delegate to betrusted.resc
M  tests/renode/xas-smoke.robot  sysbus.console, machine=SoC, 120s timeout
M  .gitignore                    dist/, logs/, report.html, etc.
A  stage/REPORT-9b-deploy-B.md   (this file)
```

No source-code changes in `crates/`. Phase A's logger plumbing was
the only code change needed.

---

## 5. Verification

```
$ cd ~/precursor-signal/xous-app-signal
$ cargo xtask dist
xtask: copied target/.../xas -> dist/xas-rv32/xas (52472060 bytes)

$ cd ~/precursor-signal/repos/xous-core
$ cargo xtask app-image xas:/home/tunnell/precursor-signal/xous-app-signal/dist/xas-rv32/xas \
                       --git-describe v0.9.21-0-g0000000
… PID 27: xas …
Signed loader at .../loader.bin
Signed kernel at .../xous.img

$ cd ~/precursor-signal/xous-app-signal
$ renode-test tests/renode/xas-smoke.robot
Suite tests/renode/xas-smoke.robot finished successfully in 43.95 seconds.
Tests finished successfully :)
```

Selected lines from the simulated UART (extracted from
`logs/xas-smoke.Should_Boot_And_Run_Xas.fail0.log` of an earlier run
that had the wrong UART target — same boot trace):

```
[host: 29.03s|virt:  1.56s] INFO:xas: xas: starting (crates/xous-app-signal/src/main.rs:52)
[host: 29.59s|virt:  1.59s] INFO:xas: xas: worker started (crates/xous-app-signal/src/main.rs:60)
[host: 30.24s|virt:  1.66s] â xas   [OFF]                                      â
[host: 30.69s|virt:  1.68s] â                       xas                        â
[host: 31.86s|virt:  1.72s] INFO:xas: xas: exiting (crates/xous-app-signal/src/main.rs:71)
[host: 33.86s|virt:  1.92s] [!] Terminating process with PID 27
```

The "â" characters are box-drawing UTF-8 bytes that the analyzer's
ASCII view mangles — the actual UART byte stream is intact. (The
hosted UI's `render_frame` writes to stdout; on Xous, stdout is
backed by the same console UART.)

---

## 6. Stage 9b-deploy phase status going forward

- **Phase A** — rv32 logger: **landed** (commit `32094a5`).
- **Phase B** — image bundling + Renode smoke: **landed** (this
  commit + xous-core xas branch).
- **Phase C** — real PDDB / trng / u32e backends for runtime flows:
  **scoped, not executed**. Still blocked on the `[patch.crates-io].
  aes` blocker described in REPORT-9b-deploy.md §3. The smoke test
  passing doesn't unblock Phase C — Phase C is about replacing the
  mock backends so the link/receive/send flows actually work on
  hardware, which the smoke test doesn't exercise.

---

## 7. Reproducing locally

```sh
# From a clean checkout of xous-app-signal at master:
cd ~/precursor-signal/xous-app-signal
cargo xtask dist                         # produces dist/xas-rv32/xas

# From the xous-core checkout on the xas branch:
cd ~/precursor-signal/repos/xous-core
git checkout xas
cargo xtask app-image \
    xas:$HOME/precursor-signal/xous-app-signal/dist/xas-rv32/xas \
    --git-describe v0.9.21-0-g0000000
# Produces target/riscv32imac-unknown-xous-elf/release/{loader.bin,xous.img}

# Run the smoke test:
cd ~/precursor-signal/xous-app-signal
renode-test tests/renode/xas-smoke.robot
# → Suite … finished successfully in ~45 s.
```

Iteration loop: change source in `crates/`, rerun `cargo xtask dist`,
rerun `cargo xtask app-image …` in xous-core (this re-bundles the
ELF without rebuilding the kernel — the xtask is incremental), rerun
`renode-test`. Total feedback time per iteration: 1–2 minutes after
the first build.
