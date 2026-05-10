# Stage 9b-deploy — Phase A: rv32 logger via xous-api-log

**Date.** 2026-05-06
**Scope landed.** Phase A only (logger plumbing). Phases B (image
bundling + launcher wiring) and C (real PDDB / trng / u32e for runtime
flows) are scoped but not executed — see §3 for why splitting was the
right call.

---

## 1. What this phase delivered

`crates/xous-app-signal/Cargo.toml` gained a target-cfg-gated
dependency on `xous-api-log = "0.1.69"` — pinned to the same version
the other apps in xous-core (transientdisk, repl, ball) depend on, so
our binary resolves to the same crates.io release the published Xous
image links against.

`crates/xous-app-signal/src/main.rs` `init_logger()` for
`target_os = "xous"` is now:

```rust
#[cfg(target_os = "xous")]
fn init_logger() {
    let _ = xous_api_log::init_wait();
}
```

`init_wait()` (per `~/precursor-signal/repos/xous-core/api/
xous-api-log/src/lib.rs:91`) blocks until the running `xous-log-server`
is reachable on its known SID, then registers itself as the
`log::Log` impl. Subsequent `log::info!("xas: starting")` and
`log::info!("xas: worker started")` calls go through the server
which forwards to the hardware UART — which is what the Renode Robot
test asserts on.

### Verification

```
cargo build --target=riscv32imac-unknown-xous-elf --release -p xous-app-signal
    → release ELF built in 1m 42s; size 52,472,060 bytes
```

ELF symbol audit (`nm` on the rv32 binary):

```
__getrandom_v03_custom            present (still the Stage 9a panic stub)
main                              present
xous_api_log::XousLogger          present (XousLogger static + log impl)
xous_api_log::XOUS_LOGGER_CONNECTION  present (the SID-cid slot)
```

Hosted-side sanity unchanged:

```
cargo build / test / clippy / fmt --check    → all green
```

The `xous-api-log` crate doesn't enter the hosted dep graph (gated by
`[target.'cfg(target_os = "xous")'.dependencies]`), so hosted CI has
zero new surface.

---

## 2. What this phase did *not* do

End-to-end Renode smoke pass (`xas: starting` / `xas: worker started`
seen on UART) is not yet exercised. It depends on Phase B work that
lives outside the standalone workspace.

**Phase B — image bundling + launcher wiring.** Concrete steps:

1. **Bundle the prebuilt ELF into a Xous image.** xous-core's xtask
   already supports this via `CrateSpec::BinaryFile` (per
   `~/precursor-signal/repos/xous-core/xtask/src/builder.rs:43`). The
   CLI form is:

   ```sh
   cd ~/precursor-signal/repos/xous-core
   cargo xtask app-image xas:/home/tunnell/precursor-signal/xous-app-signal/dist/xas-rv32/xas
   ```

   The `name:path` form is parsed at `builder.rs:128–135`. The `xas:`
   prefix gives the bundled binary an app name; the path is the
   release ELF that `cargo xtask dist` produces in our standalone
   workspace.

2. **Register `xas` in the launcher manifest.** Without an entry in
   `apps/manifest.json` and `apps/i18n.json`, the bundled binary
   sits in flash but never gets started — the kernel doesn't auto-run
   apps. A minimal manifest entry (modelled on the existing `hello`
   entry) lands on the `tunnell/xous-core/xas` branch.

3. **Auto-launch in Renode.** Two options:

   - **B-3a: keypress automation.** The Robot test uses Renode's
     keyboard injection to navigate launcher → `xas`. Brittle (depends
     on launcher menu order) but doesn't touch the kernel or GAM. Use
     Renode's `WriteString` or `WriteChar` against the simulated
     keyboard COM. The Robot test grows three or four `WriteChar`
     lines before the `Wait For Line On Uart` assertions.
   - **B-3b: kernel-side auto-start patch.** Patch xous-core's
     loader to start `xas` automatically when present. Cleaner for
     the smoke test but a meaningful kernel change that needs to land
     upstream (or be retained on the `xas` branch as a smoke-test-
     only patch flagged behind a feature gate).

   B-3a is recommended for Stage 9b-deploy Phase B; B-3b is a Stage
   13 hardware-deploy concern.

4. **Update `tests/renode/xas-smoke.resc`.** Switch the Plan-A
   image-bundled branch from placeholder to active. The current
   `.resc` already supports it via `$xous_image`; what's missing is
   the runtime path of "build the image with our binary in it".

5. **Update `tests/renode/run-renode-tests.sh`.** Add a step before
   `renode-test`: invoke `cargo xtask app-image` in the xous-core
   tree to rebuild the image with our latest dist'd ELF.

**Phase C — real backends.** The smoke test doesn't exercise these,
but the actual MVP flows (link / receive / send) do:

- Real `backend_pddb.rs` behind the `pddb-backend` feature flag.
  Path-dep on `~/precursor-signal/repos/xous-core/services/pddb/`,
  `Arc<Mutex<Pddb>>` per `PddbStore`, `signal` basis. This is the
  step that *will* hit the `[patch.crates-io].aes` blocker — pddb's
  internal encryption uses xous-core's services/aes IPC shim, and
  our standalone workspace doesn't apply that patch. Working
  hypothesis: the path-dep'd pddb compiles against upstream `aes`
  (from crates.io) and silently loses hardware acceleration. If
  that's not true, fall back to a hand-rolled PDDB IPC client.
- Real `__getrandom_v03_custom` body. Calls `trng::Trng::fill_buf`
  via path-dep on `~/precursor-signal/repos/xous-core/services/trng/`.
  Same patch-blocker risk profile as PDDB.
- u32e backend re-enable. Uncomment
  `--cfg curve25519_dalek_backend="u32e_backend"` in
  `.cargo/config.toml`. Verify rv32 build still passes.

---

## 3. Why split A / B / C

Two distinct dep risks live in this stage's scope:

1. **Phase A is local.** Adding `xous-api-log` to our Cargo.toml
   pulls in published crates.io releases (`xous`, `xous-ipc`,
   `xous-api-log`). They don't need any of xous-core's `[patch.
   crates-io]` rewrites. Our rv32 build graph is undisturbed.

2. **Phase C touches the patch table.** Path-deps on
   `services/{pddb,trng}` pull in xous-core's internal aes-shim
   assumptions. The standalone workspace's `[patch.crates-io]` table
   has zkgroup's `Aes256Enc` need, which is incompatible with
   xous-core's `aes = path "services/aes"` rewrite. This is the same
   blocker that killed the workspace-merge attempt. Resolving it
   needs design work (vendor pddb? hand-rolled IPC client? services/aes
   patched to add Aes256Enc?) that's not bounded by a single stage.

3. **Phase B is xous-core-side.** Manifest patch and Robot keypress
   automation live on the `tunnell/xous-core/xas` branch, not in our
   standalone workspace. They need their own commit + PR.

Doing A in this commit, surfacing B and C as independent follow-ups,
keeps each piece reviewable without dragging the others.

---

## 4. Files touched

```
M  crates/xous-app-signal/Cargo.toml         (xous-api-log dep)
M  crates/xous-app-signal/src/main.rs        (init_wait() in rv32 stub)
A  stage/REPORT-9b-deploy.md                 (this file)
```

Three lines of Cargo.toml, six lines of main.rs body, one new report.
No changes to any other crate.

---

## 5. Status of stages going forward

- **Stage 9b** — test scaffolding: **landed** (commit `4d0341e`).
- **Stage 9b-deploy A** — rv32 logger: **landed** (this commit).
- **Stage 9b-deploy B** — image bundle + launcher: **scoped, not
  executed**. Lives on `tunnell/xous-core/xas` branch.
- **Stage 9b-deploy C** — real backends + u32e: **scoped, not
  executed**. Blocked on the aes-patch resolution.
- **Stages 10/11/12** — link/receive/send (logic + UI): **landed at
  Stage 9c level**, exercised on hosted mock backend.
- **Stage 13 (not yet in ROADMAP)** — hardware deploy: combines
  Phase B+C with on-device flow verification.
