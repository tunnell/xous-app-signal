# SYNC.md — How `tunnell/xous-core` is wired for the xas integration

The `xous-app-signal` (xas) project lives in a feature branch of
`tunnell/xous-core` rather than in its own GitHub fork. This avoids
GitHub's per-account fork-naming friction (you can have multiple
forks of the same upstream, but the UI defaults to one and the
fine-grained PAT scopes for creating extras through the API are
non-default). The branch model below preserves all the visibility
benefits a separate fork would give us, with no tooling overhead.

## The three branches that matter

```
betrusted-io/xous-core/dev  ← upstream
            │
            ▼  (mirror via upstream remote, fast-forward only)
tunnell/xous-core/dev-for-xas
            │
            ▼  (branch off; integration commits land here)
tunnell/xous-core/xas
```

| Branch | Purpose | Updated how |
|---|---|---|
| `betrusted-io/xous-core/dev` | Authoritative upstream HEAD. We never push here. | Upstream maintainers. |
| `tunnell/xous-core/dev-for-xas` | Our local mirror of upstream `dev`. Stays at `upstream/dev`'s tip; never carries our commits. | `git push origin upstream/dev:dev-for-xas` (fast-forward only). |
| `tunnell/xous-core/xas` | Where `apps/xas/` lives. Carries every diff from `dev-for-xas`. | We commit here. PR'd into `dev-for-xas` for visibility. |

`tunnell/xous-core/dev` exists as the original-fork default branch but
isn't load-bearing for xas; we leave it alone. The user's other
projects (`dev-for-xous-signal-client` etc.) use the same naming
convention, so this fork can host multiple parallel
`dev-for-<project>` mirrors without conflict.

## Why two branches and not one

`xas` alone would mix our integration commits with whatever upstream
divergence happened since we forked. With `dev-for-xas` separate:

- The PR `xas → dev-for-xas` shows **only our diff against upstream**,
  which is the question we want answered. Currently that's "everything
  in `apps/xas/` plus the workspace-Cargo-toml edits to register it".
- When upstream advances, `dev-for-xas` fast-forwards to follow.
  Rebasing `xas` onto the new `dev-for-xas` cleanly separates "is
  there a merge conflict with upstream" from "do our Stage-N changes
  still apply".
- The PR's diff stays meaningful (number of changed files, lines)
  rather than ballooning every time upstream merges anything.

A single-branch model where `xas` rebases against `upstream/dev`
directly would also work, but loses the PR-visibility benefit and
makes "what's our delta?" a multi-command answer instead of a single
URL.

## Operations

### One-time setup (already done at this commit)

```sh
cd ~/precursor-signal/repos/xous-core
git remote -v   # origin = tunnell/xous-core, upstream = betrusted-io/xous-core
git fetch upstream
git checkout -b dev-for-xas upstream/dev
git push -u origin dev-for-xas

git checkout -b xas
git push -u origin xas
```

After this, `git branch --show-current` is `xas`, and pushing from
`xas` updates `tunnell/xous-core/xas`.

### Pulling upstream changes into `dev-for-xas`

When `betrusted-io/xous-core/dev` moves:

```sh
cd ~/precursor-signal/repos/xous-core
git fetch upstream
git checkout dev-for-xas
git merge --ff-only upstream/dev    # fast-forward; refuses if there's a divergence
git push origin dev-for-xas
```

`--ff-only` is intentional — `dev-for-xas` should never carry merges
or commits of our own. If `--ff-only` refuses, something has gone
wrong (someone pushed a commit directly to `dev-for-xas` instead of
`xas`); investigate before forcing.

### Picking up upstream changes on `xas`

After the steps above, rebase `xas` on the new `dev-for-xas`:

```sh
git checkout xas
git fetch origin
git rebase dev-for-xas
# Resolve any conflicts in apps/xas/* or workspace Cargo.toml entries
git push --force-with-lease origin xas
```

`--force-with-lease` is safer than `--force`: it refuses if the
remote has commits we haven't seen (caught a stale push). The PR
auto-updates after the push.

### Adding a new commit on `xas`

Same as ordinary feature-branch work:

```sh
git checkout xas
# ... edit ...
git add -A && git commit -m "..."
git push origin xas
```

The PR `xas → dev-for-xas` updates immediately.

### Inspecting our delta

```sh
git diff dev-for-xas..xas --stat   # quick file-count / line-count
git log dev-for-xas..xas           # commits on our side
gh pr view xas --repo tunnell/xous-core   # the same diff in PR form
```

## What lives where

`apps/xas/` (in `tunnell/xous-core/xas` branch only):

```
apps/xas/
├── crates/
│   ├── xous-app-signal/        ← the binary
│   ├── xous-signal-bridge/     ← Manager worker thread + IPC
│   ├── presage-store-pddb/     ← storage trait impls
│   └── xous-net-bridge/        ← TLS + sync HTTP + WS pump
├── vendor/
│   ├── presage/                ← tokio-removed fork (Stage 7)
│   ├── libsignal-service-rs/   ← reqwest-replaced fork (Stage 6)
│   └── curve25519-dalek/       ← betrusted-io fork + lizard port
├── docs/
│   ├── ARCHITECTURE.md, INTEGRATION.md, SYNC.md (this), UI.md
│   └── stage/REPORT-*.md  (historical per-stage execution reports)
└── tests/renode/
    ├── xas-smoke.resc, xas-smoke.robot, run-renode-tests.sh   ← Stage 9b
```

Edits to xous-core's top-level (workspace `Cargo.toml`,
`apps/manifest.json`) are unavoidable and intentional — see
`apps/xas/docs/INTEGRATION.md` for the precise list. Those edits
are the "delta from upstream" the PR shows; they should stay
small and contained.

## Future scenarios

### "We want to send our changes upstream"

Open a PR `xas → betrusted-io/dev` (a real upstream PR). Most of
`apps/xas/` is contained to its own subdirectory; the workspace
`Cargo.toml` edits are the cross-cutting parts. We'd probably split
the upstream PR into:

1. The `apps/xas/` subtree as a single feature commit.
2. The workspace + `apps/manifest.json` registration as a separate
   trivially-reviewable commit.
3. The `[patch.crates-io]` additions, with rationale per crate.

### "betrusted-io merges xas-related work into dev"

`dev-for-xas` fast-forwards to pick it up. `xas` rebases onto the
new `dev-for-xas`; upstream commits that overlap with our patches
become merge-conflict surface (rare since `apps/xas/` is its own
subdirectory).

### "We want to run upstream's test suite against `xas`"

`xas` is a strict superset of `dev-for-xas`. Whatever CI / test
recipe upstream runs on `dev` runs on `xas` too, modulo the
addition of `apps/xas/` as a workspace member. New apps are
expected to add themselves to xous-core's workspace; CI tolerates
that.
