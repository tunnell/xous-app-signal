# Stage 4 — Stop and surface: the design question is now active

Status: **partial / blocked on user decision**.

## What was attempted

Started Stage 4: add `presage` as a dep, build, then implement `StateStore` against a mock backend.

Adding presage immediately exposed two issues that REPORT.md anticipated:

1. **`curve25519-dalek` fork conflict** (REPORT.md Risk #3, predicted). Two copies of `curve25519-dalek 4.1.3` in the dep graph: one from crates.io (transitively pulled), one from `signalapp/curve25519-dalek` (libsignal's `[patch.crates-io]`). zkgroup compiled against the fork; other consumers compiled against crates.io. Result: 220 errors all variants of "found `RistrettoPoint`, expected `RistrettoPoint`" (same name, two different concrete types).

   **Cause.** libsignal's own `[patch.crates-io]` (at `libsignal/Cargo.toml:237`) only applies when libsignal is the build root. When libsignal is consumed as a dep (as we do, via presage → libsignal-service-rs), patches don't propagate.

   **Fix.** Mirrored libsignal's `curve25519-dalek` patch into our workspace `Cargo.toml`:
   ```toml
   [patch.crates-io.curve25519-dalek]
   git = "https://github.com/signalapp/curve25519-dalek"
   tag = "signal-curve25519-4.1.3"
   ```
   After this, hosted-mode build of the full Whisperfish stack succeeds:
   ```
   Compiling signal-crypto, zkcredential, zkgroup, libsignal-protocol,
             libsignal-service, presage, presage-store-pddb
   Finished `dev` profile in 19s
   ```

2. **`mio` does not support `riscv32imac-unknown-xous-elf`** (REPORT.md Decision 2 + 3 predicted). Trying `cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb` produced 47 errors in mio:
   ```
   error[E0308]: mismatched types
      --> mio-1.2.0/src/net/udp.rs:768:9
   768 | /         unsafe {
   769 | |             #[cfg(any(unix, target_os = "hermit", target_os = "wasi"))]
   ...
   = expected `UdpSocket`, found `()`
   ```
   mio's `cfg` ladder covers Unix, Windows, WASI, hermit. Xous is none of those, so the function bodies are cfg-removed, leaving placeholders that don't satisfy the function signatures.

   **Cause.** Dep chain (verified by `cargo tree -e all --invert`):
   ```
   mio v1.2.0
   └── tokio v1.52.2
       └── hyper-util v0.1.20
           └── reqwest v0.12.28
               └── reqwest-websocket v0.4.4
                   └── libsignal-service v0.1.0
                       └── presage v0.8.0-dev
                           └── presage-store-pddb (us)
   ```
   This is the exact tokio + reqwest + reqwest-websocket coupling that REPORT.md Decision 3 (replace transport) and Decision 2 (remove tokio from presage) are designed to break.

## The design question to surface

**rv32 cross-compile of `presage-store-pddb` is gated on Stage 6 (libsignal-service-rs transport fork) + Stage 7 (presage tokio removal).** The ROADMAP currently ordered work as 4 → 5 → 6 → 7. We can:

### Option A — Continue Stage 4 hosted-mode-only

Implement `StateStore` + `ContentsStore::profile` against the mock backend. Verify hosted-mode tests pass. Skip the rv32 cross-compile verification step for Stage 4 (and Stage 5) with a documented "gated on Stage 6+7" caveat. Add the rv32 verification back in once Stages 6+7 land.

- **Pros**: linear progress; Stage 4–5 are mostly mechanical trait impls and don't need network at all.
- **Cons**: we lose the per-stage rv32 sanity check for two stages.

### Option B — Reorder: do Stage 6+7 next, return to 4–5 after

Skip ahead and tackle the libsignal-service-rs transport fork + presage tokio removal first. After those, Stage 4–5's storage-trait impls automatically get rv32 verification.

- **Pros**: rv32 cross-compile is restored before any storage code is written; we don't accumulate rv32-untested code.
- **Cons**: Stage 6 is the largest single piece of forking work in the project (~2 kLoC patch on libsignal-service-rs); doing it before any storage work means we don't have a full integration target until much later. Higher risk of the fork-rebase getting stale.

### Option C — Parallel: do 4-5 (hosted) and 6-7 in parallel

Write the storage trait impls (Stages 4–5) and the transport fork (Stage 6) simultaneously, on different branches. They're independent at the trait-boundary level. Merge when both ready.

- **Pros**: parallelism; rv32 verification restored on a similar timeline; Stage 4 impls are straightforward and unblock other work.
- **Cons**: requires two separate concurrent agent contexts; more bookkeeping; if the fork's API changes diverge from what the storage impl expects, mid-stream coordination is needed.

## Recommendation

**Option A** is the lowest-risk path forward for an agent-driven sequence. Stage 4–5 are highly mechanical (~50 trait method impls, mostly `serde::serialize` / `bincode::deserialize` plumbing), and shipping them on hosted-mode-only doesn't accumulate technical debt — the same code will pass rv32 verification automatically once Stages 6+7 land. Add a TODO marker at every Stage 4–5 verification step listing the gated rv32 check, so we don't lose track. Then attack Stage 6+7 with the storage layer already done and exercisable via mocks.

This is also the order that mirrors how the dev community builds these things: storage layer first, then transport. Reversing it (Option B) would mean writing the transport fork against a still-evolving storage trait surface.

Option C is appealing on paper but requires more coordination than feels worth it for a small project.

## What was committed for Stage 4 so far

- Workspace `Cargo.toml`: added `[patch.crates-io].curve25519-dalek = signalapp-fork @ signal-curve25519-4.1.3` with a comment pointing at the long-term resolution at Stage 9 (xous-core fork conflict per REPORT.md Risk #3).
- `crates/presage-store-pddb/Cargo.toml`: added `presage = { git = "https://github.com/whisperfish/presage", rev = "600c4ed" }`.
- `Cargo.lock`: regenerated. The Whisperfish stack (libsignal v0.91.0 by tag, libsignal-service-rs HEAD pinned by rev, presage HEAD pinned by rev) is fully resolvable on hosted x86_64.

What's NOT committed yet:
- `StateStore` impl on `PddbStore`.
- `ContentsStore::profile` / `save_profile` impl.
- Mock backend.
- Tests.

These are deferred until the user picks Option A / B / C.

## Verification status (partial)

```sh
$ cargo build -p presage-store-pddb
    ...
    Compiling presage-store-pddb v0.0.1
    Finished `dev` profile in 19s
✓ Hosted-mode build of full Whisperfish stack works.

$ cargo check --target=riscv32imac-unknown-xous-elf -p presage-store-pddb
✗ FAILS — 47 errors in mio (transitively via tokio via reqwest via libsignal-service-rs)
   Gated on Stage 6 (libsignal-service-rs transport fork) + Stage 7
   (presage tokio removal). Per REPORT.md Decisions 2 & 3.

$ cargo run -p xous-app-signal --bin xas      ✓ still works
$ cargo run --example https_get -p xous-net-bridge   ✓ still works
$ cargo run --example signal_ws_keepalive -p xous-net-bridge   ✓ still works
$ cargo tree --workspace -d                   ✓ no duplicates
$ cargo fmt --all -- --check                  ✓ clean
$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
```

## Suggested ROADMAP refinements

These should be applied regardless of which option (A/B/C) the user picks:

1. **Stage 4 should add `[patch.crates-io].curve25519-dalek` to its deliverable list.** Currently the curve25519-dalek conflict is mentioned only in the Risk register; Stage 4 is the stage that makes it active. Suggested addition to Stage 4 step 1:

   > Step 1.5: Add `[patch.crates-io].curve25519-dalek = git=signalapp/curve25519-dalek, tag=signal-curve25519-4.1.3` to the workspace `Cargo.toml`. Without this, zkgroup fails to compile with 220 type-mismatch errors because libsignal's own `[patch.crates-io]` doesn't propagate when libsignal is consumed as a dep. Per REPORT.md Risk #3, this conflicts with xous-core's `tunnell/curve25519-dalek` fork; resolution deferred to Stage 9.

2. **Stages 4 and 5 should explicitly note that rv32 cross-compile is gated on Stages 6+7.** Add to the verification block:

   > ```
   > # cargo check --target=riscv32imac-unknown-xous-elf
   > # gated on Stage 6 (libsignal-service-rs transport fork) — mio doesn't
   > # support Xous; replacing reqwest_websocket+reqwest with our sync pump
   > # at Stage 6 removes mio from the dep tree. Defer this verification
   > # step until Stage 7 lands; for now, rely on hosted-mode tests.
   > ```

3. **Stage 6 prerequisites should explicitly link back to Stages 4–5's gated rv32 verification.** When Stage 6 lands, the agent should re-run rv32 cross-compile of all earlier stages and confirm they now pass.

## Open questions / things to revisit

1. **Same `curve25519-dalek` conflict at Stage 9 (rv32 hardware bring-up).** xous-core's own `[patch.crates-io].curve25519-dalek` (`tunnell/curve25519-dalek`) is incompatible with libsignal's. We've adopted Signal's fork now to make zkgroup compile; at Stage 9 we'll need to reconcile. Two paths: (a) at the merge-into-xous-core point, override xous-core's patch with Signal's — losing whatever Xous-specific patches the `tunnell/curve25519-dalek` fork carries; (b) rebase one fork on top of the other into a meta-fork that carries both sets of patches. **Decision needed before Stage 9.**

2. **Same `getrandom 0.3` issue may resurface in libsignal-service-rs's transitive deps.** Stage 3 hit this with tungstenite 0.29 and downgraded to 0.21. libsignal-service-rs pulls reqwest which may transitively pull `getrandom 0.3` somewhere. We can't see this at Stage 4 because tokio fails first; once tokio is gone (Stage 7), getrandom 0.3 may become the next blocker. Surfacing for awareness.

## Files changed (since Stage 3)

```
modified:
  Cargo.toml                                   (+[patch.crates-io].curve25519-dalek)
  Cargo.lock                                   (regenerated; full Whisperfish stack)
  crates/presage-store-pddb/Cargo.toml        (+presage as git dep)

new:
  stage/REPORT-4.md                            (this file)
```
