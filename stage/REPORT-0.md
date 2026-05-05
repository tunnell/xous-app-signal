# Stage 0 — Repository scaffolding

Status: **complete**.
Workspace: `/home/tunnell/precursor-signal/xous-app-signal/`.
Commit: `af65bc4` (local, single commit).

## What was done

1. Created the workspace directory layout per `docs/ROADMAP.md` Stage 0:
   ```
   xous-app-signal/
   ├── Cargo.toml
   ├── rust-toolchain.toml
   ├── README.md
   ├── AGENTS.md
   ├── .gitignore
   ├── docs/
   │   ├── REPORT.md      -> ../../REPORT.md         (symlink)
   │   ├── CALL_GRAPH.md  -> ../../CALL_GRAPH.md     (symlink)
   │   └── ROADMAP.md     -> ../../ROADMAP.md        (symlink)
   ├── crates/
   │   ├── presage-store-pddb/{Cargo.toml, src/lib.rs}
   │   ├── xous-net-bridge/{Cargo.toml, src/lib.rs}
   │   ├── xous-signal-bridge/{Cargo.toml, src/lib.rs}
   │   └── xous-app-signal/{Cargo.toml, src/main.rs}
   └── stage/
       └── REPORT-0.md
   ```

2. Workspace `Cargo.toml`:
   - `resolver = "2"`, `edition = "2024"`, `rust-version = "1.85"`.
   - `[profile.release]` mirrors xous-core's `[profile.release]` from
     [`xous-core/Cargo.toml:154-161`](https://github.com/betrusted-io/xous-core/blob/main/Cargo.toml#L154-L161):
     `codegen-units = 1`, `lto = "fat"`, `opt-level = "s"`, `incremental = true`,
     `debug = true`, `strip = false`. The `debug = true` setting keeps DWARF
     in the on-disk binary for hardware crash analysis (xous-core's choice).
   - `[profile.release-small]` is a new profile inheriting from `release` with
     `debug = false` and `strip = "symbols"` — for binary-size measurements
     per `docs/REPORT.md` "Binary size strategy" section.

3. `[patch.crates-io]` mirrors the **git-based** patches from
   [xous-core's lines 165-196](https://github.com/betrusted-io/xous-core/blob/main/Cargo.toml#L164-L196):
   `sha2` (`betrusted-io/hashes`, branch `sha2-v0.10.8-xous`) and `ring`
   (`betrusted-io/ring-xous` at the pinned rev). The path-based patches
   (`aes` -> `services/aes`, `getrandom` -> `imports/getrandom`) are
   intentionally not mirrored — those paths only exist inside xous-core's
   tree and don't apply to a standalone workspace. Documented as a TODO
   for the rv32 hardware integration stage. The `curve25519-dalek` patch
   is also intentionally not mirrored, pending resolution of the
   xous-core-fork-vs-libsignal-fork conflict (see `docs/REPORT.md` Risk #3).

4. Each of the four crates has a stub `Cargo.toml` (workspace-inheriting
   metadata, empty `[dependencies]`) and a stub `src/lib.rs` (or
   `src/main.rs`) with only a doc comment describing the crate's eventual
   responsibility per `docs/REPORT.md`.

5. The `xous-app-signal` crate's binary is named `xas` per the user's
   request: a pronounceable abbreviation of **X**ous **a**pp **s**ignal.
   The crate name itself is `xous-app-signal`.

6. `rust-toolchain.toml` pins `channel = "1.95.0"` — stable Rust 1.95 is
   the active default on the development machine (verified via
   `rustup toolchain list`). 1.95 is well above the 1.85 minimum that
   `edition = "2024"` (libsignal v0.91's edition) requires. xous-core
   itself does not pin a toolchain at the workspace level (verified
   2026-05); this pin is downstream-of-xous-core and may need to be
   reconciled later with whatever the `betrusted-io/rust` Xous fork
   currently produces.

7. `AGENTS.md` written with project conventions oriented around the
   verifiability principle from `docs/REPORT.md` §1: cite source for
   non-trivial claims; check `[patch.crates-io]` in xous-core before
   adding deps; run `cargo fmt + cargo clippy` before committing; no
   emojis; one stage = one commit.

8. `README.md` written: project description, document index, layout,
   build instructions, status.

9. Initialized a local git repo and committed the scaffolding as a single
   commit (`af65bc4`). Per the user's instruction, all commits remain local;
   no remote is configured.

## Verification

All verification commands from the ROADMAP Stage 0 spec passed.

```sh
$ cargo build --workspace
   Compiling presage-store-pddb v0.0.1
   Compiling xous-net-bridge v0.0.1
   Compiling xous-signal-bridge v0.0.1
   Compiling xous-app-signal v0.0.1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.36s

$ cargo build --workspace --release
    Finished `release` profile [optimized + debuginfo] target(s) in 3.02s

$ cargo build --workspace --profile=release-small
    Finished `release-small` profile [optimized] target(s) in 2.90s

$ cargo tree --workspace -d
warning: nothing to print.
( = no duplicate dep versions across the workspace)
```

Two warnings appear during every build:

```
warning: patch `ring v0.17.7 (https://github.com/betrusted-io/ring-xous?...)` was not used in the crate graph
warning: patch `sha2 v0.10.8 (https://github.com/betrusted-io/hashes.git?...)` was not used in the crate graph
```

These are expected and harmless: no crate in our workspace pulls `sha2`
or `ring` yet. The patches will activate as soon as Stage 4+ pulls in
`presage` (which transitively uses `signal-crypto`, which uses `sha2`)
or anything that uses rustls (which uses `ring`).

Additional sanity checks beyond the ROADMAP spec:

```sh
$ cargo run --bin xas
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
     Running `target/debug/xas`
( = clean exit)

$ cargo fmt --all -- --check
( = clean)

$ cargo clippy --workspace --all-targets -- -D warnings
    Checking xous-app-signal v0.0.1
    Checking presage-store-pddb v0.0.1
    Checking xous-net-bridge v0.0.1
    Checking xous-signal-bridge v0.0.1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
( = no warnings)
```

## Binary sizes (hosted x86_64, baseline empty workspace)

| Profile | Stripped? | Size | Note |
|---|---|---|---|
| `dev` (debug) | no | 4.34 MB | full debug info |
| `release` | no (matches xous-core) | 1.95 MB | optimized + DWARF |
| `release-small` | yes | **287 KB** | optimized + stripped |

The `release-small` profile correctly strips DWARF — useful as a baseline
when measuring growth as deps are added in subsequent stages.

These numbers are for hosted x86_64; rv32 cross-compile sizes will be
measured in Stage 9.

## Deviations from the ROADMAP

1. **Workspace name changed.** ROADMAP Stage 0 deliverables said the
   workspace name should be `xous-signal-app`. User requested mid-stage
   that it be renamed to `xous-app-signal` (with the binary named `xas`
   for a pronounceable abbreviation). Made the change before committing.

2. **`[patch.crates-io]` partially mirrored.** ROADMAP step 3 says to
   mirror entries for `sha2`, `aes`, `ring`, `getrandom`. Only `sha2`
   and `ring` were mirrored — both are git-based and work standalone.
   `aes` and `getrandom` are path-based in xous-core and cannot be
   mirrored 1:1 without either (a) merging this workspace into
   xous-core's tree, or (b) vendoring those forks here. Surfaced as a
   TODO with a comment in the workspace `Cargo.toml`.

## Suggested ROADMAP refinements

The following are minor corrections / clarifications that should be folded
back into `docs/ROADMAP.md`:

1. **Stage 0 Step 3 — caveat on path-based patches.** The current text
   says "Add `[patch.crates-io]` entries that mirror xous-core's lines
   165-196: `sha2`, `aes`, `ring`, `getrandom`." Reality: only `sha2`
   and `ring` are git-based and mirrorable standalone. The path-based
   ones (`aes` and `getrandom`) need to either be deferred or be
   vendored into a `vendor/` subdirectory now. Suggested rewrite:

   > 3. Add `[patch.crates-io]` entries mirroring xous-core's git-based
   > patches: `sha2` and `ring`. The path-based patches in xous-core
   > (`aes` -> `services/aes`, `getrandom` -> `imports/getrandom`) cannot
   > be mirrored 1:1 in a standalone workspace and are deferred to
   > Stage 9 (rv32 hardware integration), at which point either we merge
   > into xous-core's tree or vendor the forks here. Document as a TODO
   > comment in the workspace `Cargo.toml`. Do not copy xous-core's
   > `curve25519-dalek` patch — see Risk #3 in `REPORT.md`.

2. **Stage 0 binary name.** The example app crate's binary should be
   named `xas` (pronounceable abbreviation) per the user's preference
   established mid-stage. Update the deliverables example to reflect
   this.

3. **`cargo run --bin xas` could be added to the verification step.**
   It's a tiny additional check that catches binary-name typos in
   `Cargo.toml`. Worth adding.

4. **`cargo fmt --check` and `cargo clippy --workspace --all-targets
   -- -D warnings` could also be in the verification step.** Both are
   in `AGENTS.md` as required-before-commit hygiene; making them part
   of the stage verification is consistent.

None of these are blocking — Stage 0 is complete as-is. They are
suggestions to make the ROADMAP slightly more precise for the next
agent who runs through it.

## Open questions / things to revisit

1. **Toolchain pin (1.95.0) vs Xous fork.** The `betrusted-io/rust`
   Xous toolchain fork tracks Rust upstream at its own cadence. Stage 9
   (rv32 build) is the first time we'll know whether 1.95 is available
   on the Xous side; if not, we'll need to either bump the Xous fork
   or downgrade to a version it supports. xous-core itself has no
   `rust-toolchain.toml` at the workspace level — surfaced in
   `docs/REPORT.md` §New Findings N9.

2. **`aes` and `getrandom` patch wiring.** Decision deferred to
   Stage 9 per (1) above. The two options — merge into xous-core's
   workspace vs. vendor `aes`/`getrandom` forks here — should be
   discussed before that stage starts.

3. **`curve25519-dalek` fork conflict.** Documented as `REPORT.md`
   Risk #3. Resolution likely needs to happen before Stage 4 (when
   `presage-store-pddb` first pulls in `libsignal` transitively, which
   activates the libsignal pin to `signalapp/curve25519-dalek`). Worth
   a separate brief investigation: diff `tunnell/curve25519-dalek` (the
   xous-core pin) against `signalapp/curve25519-dalek`
   (`signal-curve25519-4.1.3` tag) to determine if the patches are
   additive or conflicting.

## Files committed

```
.gitignore
AGENTS.md
Cargo.lock
Cargo.toml
README.md
crates/presage-store-pddb/Cargo.toml
crates/presage-store-pddb/src/lib.rs
crates/xous-app-signal/Cargo.toml
crates/xous-app-signal/src/main.rs
crates/xous-net-bridge/Cargo.toml
crates/xous-net-bridge/src/lib.rs
crates/xous-signal-bridge/Cargo.toml
crates/xous-signal-bridge/src/lib.rs
docs/CALL_GRAPH.md  (symlink)
docs/REPORT.md      (symlink)
docs/ROADMAP.md     (symlink)
rust-toolchain.toml
```

`stage/REPORT-0.md` (this file) is committed as a separate followup
since git diff is more useful that way.
