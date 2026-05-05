# Project conventions for AI agents

These conventions apply to any agent (Claude Code, etc.) working in this repo.
The driving value is **end-user verifiability** — see `docs/REPORT.md` §1.
Every convention below serves that goal.

## When working from a ROADMAP stage

- Read the stage section in `docs/ROADMAP.md` end-to-end before starting.
- Prerequisites in the stage are mandatory; verify them before doing work.
- Run the verification step exactly as written. If it fails, do not move on;
  diagnose, fix, or surface to the user.
- Stop conditions are non-negotiable. If a stage's scope grows beyond
  ~1.5× the original estimate, stop and surface — re-scoping is the user's call.
- Write a `stage/REPORT-{stage_number}.md` per stage with: what was done,
  verification output, deviations from the ROADMAP, and any suggested
  ROADMAP refinements.

## Citations

- Cite source for non-trivial claims with `repo/path/to/file.rs:line`. For
  upstream code, use full GitHub permalinks where useful.
- Don't invent file paths or line numbers. Verify before citing.
- If a fact in `docs/REPORT.md` or `docs/ROADMAP.md` doesn't match the
  current source, surface the discrepancy in the stage report; don't
  silently change the design doc.

## Dependencies

- **Before adding any dep**, run `cargo tree -d` to ensure no duplicate
  versions exist after your change.
- Check `[patch.crates-io]` in xous-core's `Cargo.toml`
  (https://github.com/betrusted-io/xous-core/blob/main/Cargo.toml#L164-L196)
  for an existing fork. If xous-core has one, mirror it in our workspace
  Cargo.toml's `[patch.crates-io]`.
- Don't pull a transitively-large dep when a small one will do. Tokio is
  excluded by design (see `docs/REPORT.md` Decision 2). Use the smol-rs
  primitives instead.

## Code style

- No emojis in code, comments, or commit messages.
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`
  must pass before committing.
- Default to no comments. Add a comment only when the *why* is non-obvious
  — a hidden constraint, a subtle invariant, a workaround. Don't comment
  what well-named code already says.
- Keep functions short. Mutable state should have one clear owner.

## Commits

- All commits are local for now (no push to any remote).
- Commit messages: imperative mood, ≤72 char first line, body explains
  *why* not *what*. Reference the stage number when relevant
  (e.g., "Stage 0: scaffold workspace and four crates").
- One stage = one commit (or a small series). Don't lump multiple stages
  into one commit.

## Design discipline

- `docs/REPORT.md` is the source of truth for design decisions. If you
  think a decision is wrong, surface to the user with reasoning before
  changing anything.
- Every change should map to a stage in `docs/ROADMAP.md`. If you find
  yourself doing work that doesn't fit a stage, that's a sign the ROADMAP
  needs updating — surface it.
