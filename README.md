# xous-app-signal (`xas`)

A Signal client for Xous (Precursor) built on the Whisperfish Rust stack —
[`whisperfish/presage`](https://github.com/whisperfish/presage),
[`whisperfish/libsignal-service-rs`](https://github.com/whisperfish/libsignal-service-rs),
and [`signalapp/libsignal`](https://github.com/signalapp/libsignal) — rather
than reimplementing the Signal protocol from primitives.

The on-device binary is named `xas` (pronounceable abbreviation of
**X**ous **a**pp **s**ignal).

The driving project value is end-user verifiability: a user who buys a
Precursor should be able to read every line of Rust that ends up on their
device. The design therefore leans on upstream community-maintained code
(reused as-is or with small, reviewable patches) and minimizes bespoke
Xous-specific glue.

## Documents

- [docs/REPORT.md](./docs/REPORT.md) — design rationale and the six load-bearing decisions.
- [docs/CALL_GRAPH.md](./docs/CALL_GRAPH.md) — call graph from each `presage-cli` subcommand through the stack, with Mermaid diagrams.
- [docs/ROADMAP.md](./docs/ROADMAP.md) — staged plan from empty workspace to MVP.

## Layout

```
xous-app-signal/
├── Cargo.toml                  # workspace root with [profile.release], [patch.crates-io]
├── rust-toolchain.toml         # 1.95.0 stable
├── crates/
│   ├── presage-store-pddb/     # 9 storage trait impls over PDDB (Decision 1)
│   ├── xous-net-bridge/        # sync TLS + WS pump + channel bridge (Decision 3)
│   ├── xous-signal-bridge/     # Manager-on-worker + IPC forwarder (Decision 4)
│   └── xous-app-signal/        # binary entry point (binary name: `xas`)
└── stage/                      # per-stage execution reports
```

## Build

```sh
cargo build --workspace
cargo build --workspace --release
cargo build --workspace --profile=release-small   # for binary-size measurement
```

## Test

```sh
cargo test --workspace
```

## Status

Stage 0 (workspace scaffolding) complete — see `stage/REPORT-0.md`. Subsequent
stages tracked in [docs/ROADMAP.md](./docs/ROADMAP.md).
