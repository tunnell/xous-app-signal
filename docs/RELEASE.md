# Release procedure

How to cut a new release of xas. Concise: just the commands and the order.

## 0. Pre-flight

- Confirm `dev` and `main` are at the same HEAD on `tunnell/xous-app-signal`.
  If `dev` is ahead, fast-forward `main` first.
- Confirm there are no open `WIP` / `[do not merge]` commits on `dev`.
- Confirm the BUILDING.md workflow has been tested by a blank agent (or by you
  on a fresh checkout) within the past week. Drift in BUILDING.md is the most
  common silent regression — see the prior fresh-agent test reports.
- Confirm hardware validation has been done end-to-end for major releases:
  link → send → receive on a real Precursor PVT2.

```sh
cd ~/code/xas/xous-app-signal
git rev-parse --short dev
git rev-parse --short main      # should match dev
git log --oneline -5            # sanity check tip
```

## 1. Bump the workspace version

Edit `Cargo.toml`, change `[workspace.package].version`. Follow semver:

- Patch (0.1.0 → 0.1.1): doc fixes, small bug fixes, no API change
- Minor (0.1.0 → 0.2.0): user-visible new features or API additions
- Major (0.x.y → 1.0.0): breaking changes or "we consider this production"

Then regen `Cargo.lock` so it picks up the new version:

```sh
cargo metadata --format-version 1 >/dev/null
```

Commit:

```sh
git add Cargo.toml Cargo.lock
git commit -m "release: bump workspace version to X.Y.Z for vX.Y tag"
```

## 2. Sync dev and main

```sh
git push origin dev
git checkout main
git merge --ff-only dev
git push origin main
git checkout dev
```

If the fast-forward fails, `main` has commits `dev` doesn't — investigate
before continuing. Don't force-push `main`.

## 3. Tag the release

Use an annotated tag with release notes in the message:

```sh
git tag -a vX.Y -m "$(cat <<'EOF'
xas vX.Y — <one-line summary>

What's new since previous tag (or "first release"):
- ...

Required upstream patches still in review (carry locally until merged):
- betrusted-io/xous-core#877 (kernel byte-1 mirror)
- whisperfish/libsignal-service-rs#431 (keepalive tolerance)
- rust-lang/rust#156414 (std recv decode)

Known limitations:
- ... (link to GitHub issues)

Hardware-validated YYYY-MM-DD on Precursor PVT2.
EOF
)"

git push origin vX.Y
```

## 4. Create a GitHub release

Bundles the annotated tag's message into a release page that GitHub renders
nicely. Mirror the tag message into the release body so the GH UI matches the
git tag.

```sh
gh release create vX.Y \
    --title "xas vX.Y" \
    --notes-file <(git tag -l --format='%(contents)' vX.Y)
```

If you want to attach prebuilt binaries (e.g. a `xous.img` for direct flash
without rebuilding), add `path/to/xous.img#xous-img-vX.Y` as positional args
after the tag name. For 0.x releases we don't bundle binaries — readers build
from source per BUILDING.md.

## 5. Post-release housekeeping

- Update the version in `BUILDING.md` `--git-describe` example if the SoC
  version pin has changed.
- Move any "after release X" items in CHORES.md / GitHub issues forward to
  the next milestone.
- Tweet / post / whatever the release communication is.

---

## Notes for v0.1 specifically (the first release)

Tagged 2026-05-11. First hardware-validated release.

Validation done this session:
- xas registered as GAM context on Precursor PVT2 (after the `submenu: 1`
  fix on `tunnell/xous-core` `xous-app-signal` branch)
- Link flow completed end-to-end (QR scan → primary device approval → key
  rotation → contact import)
- Keepalive tolerance exercised (UART showed `ka outstanding (within
  tolerance, continuing) outstanding=1 threshold=3` repeatedly — PR #431's
  effective fix working)
- Send / receive exercised; sends survive server-initiated WS rotation via
  worker's exponential-backoff retry (sometimes 1-4 minute latency; tracked
  as #1)
- "Press Backspace to cancel" replaces the unactionable "Press Esc to
  cancel" on the Linking screen (Precursor has no Esc key)

Companion upstream branches that gate clean future releases:
- `tunnell/xous-core/xous-app-signal` — what BUILDING.md tells you to clone
- `tunnell/libsignal-service-rs/pr-keepalive-tolerance` — PR #431's source
- `tunnell/rust/pr-xous-net-recv-byte-offset` — PR #156414's source (build
  doesn't consume this; kernel-side mirror in xous-core#877 compensates)
