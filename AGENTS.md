# AGENTS.md

This file tells a coding agent — human or automated — how the
`xous-app-signal` (xas) repository wants to be touched. It is not
a tutorial: it links out to [BUILDING.md], [RELEASING.md],
[docs/ARCHITECTURE.md], [tests/README.md], and
[tests/precursor/README.md] for anything substantive. Read those
when their topic comes up; read this one first. `AGENTS.md` is the
only instruction file in this repo; symlink to it if your tool
expects another name.

## Where things live

```
xous-app-signal/
├── crates/                       owned in-tree; edit freely
│   ├── xous-app-signal/          binary entry (binary name: xas)
│   ├── xous-signal-worker/       presage::Manager + Cmd/Event channels
│   ├── xous-net-bridge/          sync TLS + WS + HTTP transport
│   ├── xous-pddb-ipc/            PDDB IPC client
│   ├── xous-modals-ipc/          modals IPC client
│   └── presage-store-pddb/       presage::Store impl over PDDB
├── docs/                         ARCHITECTURE.md and friends
├── tests/{hosted,renode,precursor}/   READMEs in hosted/ and precursor/; robot/resc files in renode/
└── vendor/                       READ-ONLY; see "Vendored + frozen branches"
```

The companion kernel-side tree (`xous-core`) is typically a
sibling directory; hardware builds depend on a specific branch of
it (see "Vendored + frozen branches"). The maintainer also keeps a
`notes/` workspace outside the repo for analyses, prompts, and
session-state docs — not part of public history, and not to be
referenced from anything in the tree.

## Build + test quick reference

Full setup (toolchain, hosted vs hardware paths, `apps.rs`
bootstrap) is in [BUILDING.md]; pros/cons across the four test
approaches are in [tests/README.md]. Common commands:

| Goal | Command |
|---|---|
| Hosted build | `cargo run --release` |
| Hardware bundle | `bash tests/precursor/build-and-bundle.sh` |
| Flash via Pi rig | `bash tests/precursor/flash-via-pi.sh` |
| Renode smoke | `bash tests/renode/run-renode-tests.sh` |
| Hosted integration | `bash tests/hosted/test_xas_round_trip.sh` |

## Vendored + frozen branches

Two things in or beside this tree are not for in-place editing:

- **`vendor/`** is read-only. The directory-vendored forks
  (`presage`, `libsignal-service-rs`, `curve25519-dalek`) and the
  patch-vendored forks pulled in via `[patch.crates-io]`
  (`ring`, `sha2`, smol-rs primitives) all require explicit
  maintainer approval for any change. File an issue with the
  rationale and wait before editing.

- **`xous-core@xas-vX.Y` is a frozen pin.** Each xas release
  (starting with v0.2) tags a companion `xas-vX.Y` branch on the
  kernel-side fork carrying the exact kernel state that release
  was built against. Never push to a frozen `xas-vX.Y` branch.
  Hardware builds against released xas must use the matching pin.
  v0.1 predates the convention and does not have an
  `xas-v0.1` branch — the pin model applies forward from v0.2.

Active kernel-side development happens on
`tunnell/xous-core@xas`, which carries `apps/manifest.json`'s xas
registration and the `apps/xas/` subtree — neither is on
`betrusted-io/xous-core@dev`. Hardware bundles built off `dev`
boot without Signal in the launcher (see trap 3 below and
[BUILDING.md] §2.1).

## Commit + PR hygiene

- **Author and committer email** for commits made on the
  maintainer's behalf: `2406627+tunnell@users.noreply.github.com`
  (the GitHub-anonymized form). Pass via `git -c user.email=...
  -c user.name=tunnell commit --author=...` so it sticks
  regardless of local config.
- **AI disclosure via `Assisted-by:` trailer** on AI-assisted
  commits, model name only — e.g. `Assisted-by: Opus 4.7`.
  Vendor / product names ("Claude", "Cursor", "ChatGPT") stay
  out of trailers and code comments. Do **not** use the GitHub
  `Co-Authored-By:` form for AI authors — that auto-links a
  separate entity in the GitHub UI (avatar, profile, contributor
  count), which is not the disclosure model we want. PR
  descriptions may add prose disclosure per the README's
  contribution policy. Cross-repo PRs to upstream (e.g.
  `betrusted-io/xous-core`) also need a DCO `Signed-off-by:`
  trailer.
- **No references to out-of-repo files.** Working docs in the
  maintainer's `notes/` workspace are not in the public repo. A
  commit message that says "see CHORES.md" or a comment pointing
  at `../STATE.md` is a dangling reference for anyone cloning
  the public repo. Self-contain commit messages: explain
  rationale inline. If a reference must exist, use a public URL
  or an in-repo path.
- **Local commits only — never `git push` without explicit
  permission.** Applies on every branch in every repo in this
  workspace, not just `main`/`dev`.
- **Protected branches** (do not push to directly): `main`,
  `dev`, `v*` tags, and every `xas-vX.Y` branch on `xous-core`.
  Force-push to a feature branch is fine if the branch is yours.
- **Cross-repo PRs**: agent PATs typically lack
  `Pull requests: Write` on upstream `betrusted-io/xous-core`,
  `whisperfish/*`, and `rust-lang/*`. Push the branch and stage
  a PR body under `notes/reply/`; the maintainer files the
  upstream PR.

## Recurring traps

Four traps have caused real bricked builds or release misses. Read before any flash cycle or release cut:

1. `XOUS_TARGET` default differs between `build-and-bundle.sh`
   (cargo triple) and `flash-via-pi.sh` (legacy SoC alias).
2. `PI_HOST` requires a `pi@` user prefix; bare IP falls through
   to a password prompt.
3. Hardware builds need the `xas`-family kernel branch, not `dev`.
   Verify with `grep APP_NAME_XAS xous-core/services/gam/src/apps.rs`
   before flashing.
4. Snapshot-and-squash recipes must use `git read-tree`, not
   `git checkout <ref> -- .` — the latter doesn't propagate
   deletions, breaking tree-equivalence during release prep.

## Verification

What "done" means depends on what was changed:

- **Doc-only**: render the markdown locally, check
  cross-references resolve, confirm no `notes/` paths leaked in.
- **Code**: hosted PASS is a sanity check, not a ship gate. Real
  verification is **rv32 hardware** (Pi rig flash + UART tail) or
  **Renode** (`tests/renode/run-renode-tests.sh`). Cold-path
  timing, WS idle-close behavior, and the Xous custom allocator
  all diverge from hosted x86_64. When framing a fix as ready,
  name the target it was verified on.
- **Hardware**: the brick-prevention checklist in
  [tests/precursor/README.md] is non-negotiable; read it before
  any flash command.

Before asserting "the bug is X, fix is Y", read the file:line
being measured. Log line names are not contracts — past briefs
inferred bugs from UART shape that the source didn't match, and
agents had to flag the gap before the work was useful.

## House style

Terse and engineering-direct, matching the upstream
`betrusted-io/xous-core` PR convention — short sentences, no
marketing voice, no headings inside PR bodies under ~10 lines.

Per-item rustdoc on `pub fn` in the security-sensitive crates
(`xous-net-bridge`, `xous-signal-worker`, `xous-pddb-ipc`,
`presage-store-pddb`) uses four section headers modeled on rustls
and RustCrypto practice: `# Trust boundary` (what trust state is
crossed), `# Security` (sensitive data touched; log discipline),
`# Errors` (failure modes the caller must handle), `# Platform
constraints` (rv32imac / 16 MiB / single-thread facts). Don't
introduce new convention names; extend these.

Implementation discipline for these crates: secrets derive
`Zeroize` / `ZeroizeOnDrop` with a redacting `Debug`; compare
tokens / MACs / hashes via `subtle::ConstantTimeEq`, never `==`;
public errors are `#[non_exhaustive]` + `thiserror::Error`; no
`.unwrap()` / `.expect()` in non-test code. Tighten, don't bypass.

When a doc is superseded, delete it — git is the archive, not
`docs/history/`. Don't ship parallel "AGENT-USAGE.md" files
duplicating user docs; one README per folder, safety constraints
in a clearly-named section.

## When to ask, when to proceed

The workflow is think → propose → confirm → execute for anything
costly or destructive. Concretely:

- **Proceed without asking**: read code, run tests, render docs,
  draft text, write commits to a local feature branch, file
  research notes under `notes/reply/`.
- **Propose, wait for explicit go**: any `git push`, flash cycle
  (~25 min hardware time), edit to `vendor/`, cross-repo PR, or
  shared-history rewrite (`git rebase -i`, force-push to a shared
  branch).
- **Stop and ask**: anything that would touch `main` or `dev`
  directly, anything that requires deleting state on the device,
  anything that would publish identifying material (PCAPs of real
  Signal traffic carry phone numbers and ACI UUIDs and are not
  committable).

A one- or two-line status update at end-of-turn is enough — the
maintainer reads the diff.

## Out-of-tree workspace pointer

The maintainer's `notes/` workspace (typically `~/code/xas/notes/`)
is not in the repo and not visible to agents on other machines.
Conventions for work that spans sessions:

- Date-prefixed names (`YYYY-MM-DD-topic.md`) for dated artifacts;
  topic-only for evergreen docs.
- `SESSION-STATE-YYYY-MM-DD.md` at the workspace top for
  resumption context.
- Prompts for sub-delegated agents must be **self-contained**:
  embed log excerpts, file paths, and issue URLs inline. A fresh
  agent on another machine cannot read paths under `notes/`.

Leave session markers in the workspace, not in commits or code
comments.

## Extending this file

Keep it short. New traps or rules go inline if one line; if they
need detail, extend an in-tree doc (e.g. `tests/precursor/README.md`
for hardware traps) and link here. Move sections past ~25 lines
out to a linked in-tree doc.

<!-- Cross-references -->

[BUILDING.md]: ./BUILDING.md
[RELEASING.md]: ./RELEASING.md
[docs/ARCHITECTURE.md]: ./docs/ARCHITECTURE.md
[tests/README.md]: ./tests/README.md
[tests/precursor/README.md]: ./tests/precursor/README.md
