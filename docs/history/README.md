# docs/history — historical development records

This folder is **archival**. The files here describe how xas got
built, not how it currently works. Read them only if you need to
understand the rationale behind a specific design decision that
emerged during a particular development stage; for the current
architecture see [`../ARCHITECTURE.md`](../ARCHITECTURE.md).

## Contents

```
docs/history/
└── stage/            ← per-stage execution reports from the original buildout
    ├── REPORT-0.md   ← Stage 0: workspace layout, sysroot, repo-symlink decisions
    ├── REPORT-1.md   ← Stage 1: ...
    ├── ...
    └── REPORT-14a.md ← Stage 14a: final pre-MVP work
```

Each `REPORT-N.md` corresponds to one stage of the original
buildout (Stages 0 through 14a). At the time these were written,
they were checkpoints — what got done, what got punted, what the
next stage needed to assume. They reference doc files that no
longer exist in this checkout (`docs/REPORT.md`,
`docs/ROADMAP.md`, `docs/CALL_GRAPH.md` — all removed in the
release-polish pass) and use stage-internal vocabulary.

## What they're useful for

- Reading the rationale for an unusual design choice (e.g.,
  "why did we vendor curve25519-dalek?" — REPORT-0 Risk #3
  documents the original conflict).
- Tracing when a particular crate appeared (e.g., when
  `xous-net-bridge` was split out from `xous-signal-worker`).
- Understanding the order in which the project's risks were
  retired.

## What they're NOT useful for

- Onboarding. Read [`../../README.md`](../../README.md) and
  [`../ARCHITECTURE.md`](../ARCHITECTURE.md) instead.
- Current build instructions. Use
  [`../../BUILDING.md`](../../BUILDING.md).
- Current state of any feature. Reports are frozen at the time
  they were written; the code has moved on.

## Why we keep them at all

Deleting them would erase the design rationale that led to the
current shape of the project. Keeping them in `docs/history/`
clearly archives them — anyone landing in this directory knows
they're looking at past, not present.
