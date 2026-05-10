# Refactor proposals — post-MVP architecture review

Date: 2026-05-09. Author: a session that read the codebase
end-to-end after the Precursor send/receive milestone.

This is the deliverable of the "architecture review" chore in
`~/code/xas/CHORES.md`. It's a list of **proposals**, not a
refactor PR. Each item has motivation + effort estimate; pick
which (if any) to act on by opening a follow-up session per
proposal.

The intent is to capture friction surfaced by the MVP buildout
**while it's still fresh**, before the next feature push covers
it over.

---

## Headline

The architecture is in **broadly good shape** for a project at
this stage. Specifically:

- Inter-crate dependency graph is a clean DAG, no cycles, no
  surprising transitive paths.
- `Cmd` (8 variants) + `Event` (13 variants) is a thin,
  symmetric IPC surface for an entire Signal client. That's the
  right size — not over-engineered, not bloated.
- cfg-gating between hosted and hardware is surgical (no forked
  modules; gates at the function/statement level).
- The `ws_pump` three-thread design (setup / reader / writer)
  is the natural shape for bridging sync I/O into an async
  executor; the structure that emerged matches the structure
  the problem demands.

**Most of the proposals below are about clarity, not
correctness.** The codebase doesn't need a structural rewrite.
It does have some terminology drift, some ambiguous boundaries,
and a few crates whose role isn't clearly described in code.

---

## P1 — Rename one of the two `*-bridge` crates ✅ DONE

**Status:** implemented. The crate that was `xous-signal-bridge`
is now `xous-signal-worker`. Workspace member, path-deps, and
all imports updated; unit tests pass; the kept "bridge"
nomenclature now refers exclusively to `xous-net-bridge` (the
sync↔async transport bridge).

**Original problem (preserved for the record):** Two first-party
crates had "bridge" in the name but meant different things:

- `xous-signal-bridge` — the worker thread that runs
  `presage::Manager` inside a `LocalExecutor`. A
  *runtime/worker*, not a transport bridge.
- `xous-net-bridge` — the sync TLS + WS pump that bridges
  blocking I/O into the async executor. A *real* bridge in the
  data-flow sense.

A new contributor reading "bridge" expects sync↔async or
process↔kernel adapters; the signal-bridge wasn't either.

**Resolution.** Renamed `xous-signal-bridge` → `xous-signal-worker`
(the option that matches the existing thread name `signal-worker`
and the existing fn `run_signal_worker()`).

---

## P2 — Standardize "bridge" vs "worker" terminology in code + logs ✅ DONE

**Status:** implemented as the follow-up to P1. ~50 log
strings inside `xous-signal-worker/src/lib.rs` renamed
("bridge:" / "bridge/link:" / "bridge/send:" → "worker:" /
"worker/link:" / "worker/send:") plus the test-script greps
that match those prefixes (otherwise the headless link test
would silently fail to find the URL log). The noun "bridge"
in the codebase now refers exclusively to xous-net-bridge.

**Original problem (preserved for the record):** Even before
P1, the same thread was referred to as both "bridge" (in
tracing spans, `bridge: spawned manager_task`) and "worker"
(in comments, fn name `run_signal_worker`, thread name
`signal-worker`). Same thing, two names — small but recurring
reader tax.

---

## P3 — Decide the fate of `xous-app-signal-ui` crate

**Problem.** `xous-app-signal-ui` is 2k LoC and exists only as
a fallback for `cargo run -p xous-app-signal` standalone (no
Xous server reachable). The README labels it "stdin UI fallback
for standalone runs" after the polish pass. Open question: does
anyone actually run that flow?

The two real usage paths are:

- Hosted-mode tests (`tests/hosted/*.sh` + `cargo xtask run` in
  xous-core) — boots a real Xous + real GAM, hits `gam_app.rs`
  not the Stdin Ui.
- Unit tests (`cargo test --features hosted -p xous-app-signal --bins`)
  — exercise pure data modules, don't construct a Ui at all.
- Hardware (rv32) — also `gam_app.rs`.

So the Stdin Ui is touched by exactly one entry point: `cargo
run -p xous-app-signal` invoked outside any Xous environment.
Is that path load-bearing?

**Proposal — three options, in increasing aggressiveness:**

1. **Document and leave.** Add `crates/xous-app-signal-ui/README.md`
   explaining "this is the stdin fallback used when
   gam_app::run() can't reach a Xous server. The primary use is
   bare `cargo run` for sanity-checking xas's main loop without
   booting xous-core hosted." If we end up never using it, the
   doc explains why it's safe to delete later.
2. **Collapse to a single file** — move `xous-app-signal-ui/src/lib.rs`
   to `xous-app-signal/src/stdin_ui.rs`, drop the workspace
   member, drop the path-dep. 2k LoC isn't large enough to
   warrant its own crate boundary if the only consumer is
   `xous-app-signal/src/main.rs`.
3. **Delete entirely.** Drop the fallback. If `gam_app::run()`
   fails, surface the error instead of falling through. Anyone
   wanting to run xas standalone can boot it under hosted xous-core.

I lean **option 2** — the boundary doesn't earn its keep, but
the fallback path is cheap to keep around inline and useful
for `main.rs`-only iteration during heavy refactors.

**Effort.** Option 1: 30 min. Option 2: 2-3 hours (move files,
fix imports, drop one workspace member, retest). Option 3: 30
min (delete + remove fallback branch).

---

## P4 — Add a one-paragraph README to each crate

**Problem.** `crates/README.md` (just added) explains what
each crate does at a workspace level. But individual crates —
especially the IPC shims — have non-obvious rationale ("this
exists to bypass the dep cascade from `services/pddb`"). That
rationale lives only in the workspace README and the Cargo.toml
description field today.

When a new contributor opens `crates/xous-pddb-ipc/`, they
should see a README that says *why this is hand-rolled instead
of just `use xous_core::services::pddb`*. Right now they have
to grep.

**Proposal.** Add a 5-15 line `README.md` to each crate
covering: what it is, who depends on it, why it exists as a
separate crate (the rationale for each crate is captured in
`crates/README.md` "Why a multi-crate workspace" — propagate
the per-crate version into each subdir).

**Effort.** ~1 hour for all six crates plus xtask.

**Risk.** None.

---

## P5 — Move `stage/REPORT-*.md` to `docs/history/` (or delete)

**Problem.** `stage/REPORT-*.md` is 13 files of historical
per-stage execution reports from the original buildout
(REPORT-0 through REPORT-13). These reference doc files that no
longer exist (e.g., `docs/REPORT.md`, `docs/ROADMAP.md` — both
deleted in the polish pass) and contain session-internal
language ("Stage 9b Phase C-3 deliverable").

They're a record of *how* the project got built, not a
description of *what it is*. A new contributor seeing
`stage/REPORT-*.md` at the repo root will conclude they're
relevant docs and waste time reading them.

**Proposal.** Either:

- Move to `docs/history/` (archive but don't delete — they have
  archaeological value if someone needs to understand a design
  choice that emerged during a specific stage), or
- Delete entirely. Anyone who really needs them can git-log
  back to a pre-deletion commit.

I lean **move to `docs/history/`** with a one-paragraph
`docs/history/README.md` saying "these are historical
execution reports; they describe the iterative buildout, not
the current architecture — see ARCHITECTURE.md for that."

**Effort.** 15 min.

**Risk.** None. Pure file relocation.

---

## P6 — Vendor strategy decision

**Problem.** Today we vendor presage, libsignal-service-rs, and
curve25519-dalek as full source trees with `[patch.*]`
redirects. This was the right move for first-light: every line
that ends up on the device is in our git history; audit story
is "read this tree." No upstream surprises during bring-up.

Now we have:

- Three upstream PR drafts (in `~/code/xas/upstream_prs/`)
- A published `tunnell/xous-core` fork
- The intent to upstream the libsignal-service-rs keepalive
  change and the Rust std-side recv encoding fix

If those PRs land upstream over the next 6-12 months, the
vendoring buys us less and costs us more (manual rebases against
upstream changes; harder to test against the latest libsignal
features).

**Three options:**

1. **Stay fully vendored.** Audit-friendliness wins; the
   project's stated value is "user can read every line."
   Downside: rebase friction increases as upstream moves.
2. **Move to git-dep on our forks + cargo's `[patch]` to pin
   commits.** Cargo.toml says `presage = { git =
   "https://github.com/tunnell/presage", rev = "abc123" }`. The
   *fork* is what we audit; cargo just resolves to that git
   tree. Cleaner bookkeeping but the audit step is one redirect
   away.
3. **Move to git-dep on UPSTREAM + a small patch overlay** (e.g.,
   `cargo-patch`, or just `[patch] = { path = "patches/foo" }`).
   Tracks upstream automatically; only our deltas are checked
   in. Easier to keep current; harder to audit (need to apply
   patches to upstream to see what's actually compiling).

I lean **stay vendored for now (option 1) and revisit in 6
months once one or more of the upstream PRs has landed.** The
audit-friendliness aligns with the project's threat model
(journalists / activists who must trust the binary). The cost
of rebasing isn't bad as long as we periodically sync.

**Effort.** Decision-only for now. Execution of options 2 or 3
is 1-2 days each.

---

## P7 — Consider folding the local `xtask/` crate into shell scripts

**Problem.** The local `xtask/` crate has three subcommands:

- `build-rv32` — `cargo build --target riscv32imac-unknown-xous-elf --release -p xous-app-signal`
- `dist` — runs `build-rv32` then copies the ELF to a known location
- `renode-test` — invokes renode-test against the renode harness

The first two are now bypassed by `tests/precursor/build-and-bundle.sh`
(which calls `cargo build` + xous-core's `cargo xtask app-image-xip`
directly). The third is rarely used (Renode is not actively
exercised per `tests/README.md`).

**Proposal — two options:**

1. **Keep xtask, retire build-rv32 + dist as redundant.** Leave
   `xtask renode-test` in place since it's the documented Renode
   entry point. Update `BUILDING.md` line 308 (which says
   `cargo xtask dist`) to use the precursor script instead.
2. **Delete xtask entirely.** Move `renode-test` to `tests/renode/run-renode-tests.sh`
   (which already exists and just calls `cargo xtask dist` +
   renode-test); inline both. Drop the workspace member, the
   `.cargo/config.toml [alias] xtask = ...`, and the local
   xtask crate.

I lean **option 2** — the local xtask was useful when it
unified the build path; now that `tests/precursor/*.sh` exists for the
hardware path, xtask is a fourth way to invoke the same cargo
commands. Less is more.

**Effort.** Option 1: 30 min (just edit BUILDING.md). Option 2:
2-3 hours (inline the renode-test command, drop the crate,
update workspace + .cargo/config.toml + tests/renode/, retest
hosted).

---

## P8 — Investigate presage-store-pddb size (3 kLoC)

**Problem.** `presage-store-pddb` is the largest first-party
crate at 3039 LoC. presage's storage-trait surface is
genuinely wide (a dozen traits), but 3 kLoC is enough volume
to suspect dedupable structure — boilerplate per-trait that
could be macro'd, similar serialization patterns repeated, etc.

This isn't necessarily a problem (3 kLoC of "boring trait
impls" can be the right shape), but it's the only crate where
size raises a question.

**Proposal.** Spend 30 min reading `crates/presage-store-pddb/src/`
top-to-bottom. Count: how many distinct traits implemented; how
much per-trait code is boilerplate vs. actual logic; whether
common patterns (serde + PDDB write, or "fetch-then-deserialize")
are repeated 8 times in slightly-different forms.

If you find systematic duplication, write up a P8a follow-up
with a concrete macro/helper proposal. If the volume is
genuinely necessary trait surface, document that as the answer
and close the question.

**Effort.** 30 min investigation. Follow-up refactor (if any) is
sized after.

---

## P9 — Document the "bridge" boundaries in ARCHITECTURE.md

**Problem.** `docs/ARCHITECTURE.md` (393 lines) describes the
runtime well but doesn't have a *picture* of the bridge layers.
Specifically: how does a `Cmd::SendMessage` sent by the UI
actually reach a TLS write on the wire? It crosses ~5 layers:

1. UI emits `Cmd::SendMessage` on `cmd_tx` (async-channel)
2. `xous-signal-worker::run_signal_worker` matches it
3. Forwards via internal channel to `manager_task`
4. `manager_task` calls `presage::Manager::send_message`
5. presage calls libsignal-service-rs which calls
   `xous-net-bridge::HttpClient` or the `WebSocketChannels`
6. ws_pump's writer thread does `ws.send(frame)` over TLS

Each layer has a reason — but a contributor working on a send
bug needs to know which layer to instrument. A diagram or
called-out list in ARCHITECTURE.md would save them an hour.

**Proposal.** Add a "Layers a Cmd crosses" section to
ARCHITECTURE.md showing the 5-6 stops between UI and wire,
each with the file:line where it lives. Same for the inverse
flow (wire frame → Event surfaced to UI).

**Effort.** ~45 min: walk one Cmd + one Event end-to-end with
file:line refs, add the section.

**Risk.** None.

---

## How to act on this

Each proposal is independent. Pick by impact-vs-effort:

| Pick | If you have | Pays off |
|---|---|---|
| P5 (move stage/) | 15 min | Immediate doc cleanup; new visitors stop hitting archival noise |
| P2 (terminology) | 30 min | Tax-per-read goes down; pairs with P1 |
| P9 (layer diagram) | 45 min | Send-bug investigation gets faster |
| P4 (per-crate READMEs) | 1 hr | Onboarding clarity for IPC shims |
| P1 (rename signal-bridge) | 2 hr | Removes the worst naming collision |
| P3 (decide on stdin UI) | 30 min – 3 hr | Depends on option chosen |
| P7 (drop local xtask) | 30 min – 3 hr | Less surface area; one-fewer "way to build" |
| P6 (vendor strategy) | decision-only now | Long-term maintenance cost |
| P8 (presage-store-pddb sizing) | 30 min | Confirms or refutes a suspicion |

If acting on multiple in one session, **P1 + P2 + P9** is a
natural cluster (all about making the existing architecture
easier to read; all touch overlapping files). **P3 + P7** is
another cluster (both are "decide whether this layer earns its
keep").

Things this review **explicitly does not propose**:

- Restructuring the Cmd/Event interface — it's the right size.
- Splitting or merging crates beyond P1 + P3 + P7 — the rest
  of the dependency graph is fine.
- Changing the cfg-gating strategy between hosted and hardware
  — surgical gates are working.
- Replacing the ws_pump three-thread design — it matches the
  problem shape.

The review is a single session's pass. Subsequent sessions
that act on a proposal will surface things this missed; that's
expected, not a bug.
