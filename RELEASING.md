# Releasing xas

xas releases are pinned to specific snapshots of `tunnell/xous-core` so a
given xas version always builds against the same kernel + services state.
Each release gets its own `xas-vX.Y` branch on xous-core; that branch is
**frozen** after the release ships and never updated again. Future xas
releases create new `xas-vX.Y+1` branches — they don't reuse old ones.

## Procedure for cutting `vX.Y`

1. **Decide what's in the release.** The xous-core changes needed by xas
   for this release should already be on the floating `tunnell/xous-core@xas`
   integration branch (or tracked in open PRs against it).

2. **Create the pinned xous-core branch.**

   ```sh
   cd path/to/xous-core
   git fetch origin xas
   git checkout -b xas-vX.Y origin/xas
   # If any in-flight PRs against xous-core need to be in this release,
   # cherry-pick their commits here:
   #   git cherry-pick <sha>...
   git push origin xas-vX.Y
   ```

   This branch is now frozen — do not push to it again.

3. **Bump xas's version.** In the xas repo:

   ```sh
   cd path/to/xous-app-signal
   # Edit Cargo.toml: bump [workspace.package] version to "X.Y.0"
   # Edit BUILDING.md §1: change `git clone -b xas-vX.{Y-1}` ->
   #                              `git clone -b xas-vX.Y`
   # Edit BUILDING.md §3.1: same change in the "Branch selection" note.
   ```

4. **Hardware verify.** Build, flash to Precursor PVT2, drive a send to
   a single-device peer. The send should complete in <60 s end-to-end
   (target was set during v0.2 design). If the send regresses vs the
   prior release, **stop** and investigate before tagging.

5. **Open the release PR.** Branch named `vX.Y-candidate`, base `dev`.
   PR body includes:
   - Hardware verification UART excerpts.
   - Companion `tunnell/xous-core` PR(s) that landed in `xas-vX.Y`.
   - Issues closed.
   - Known issues that ship anyway.

6. **Merge the PR.** Squash-merge is acceptable for a release branch.
   Merge-commit also works if the PR's history is already clean.

7. **Tag the release.**

   ```sh
   cd path/to/xous-app-signal
   git checkout dev
   git pull --ff-only
   git tag -a vX.Y -m "xas vX.Y"
   git push origin vX.Y
   ```

8. **Don't delete `xas-vX.Y` on xous-core.** Past release branches stay
   frozen and accessible forever — anyone re-building an old xas version
   from source needs them.

## Why this branch model

Decoupling the release pin from a floating xas-integration branch means:

- Reproducible builds: `xas vX.Y` always builds against the same kernel.
- No coordination dance: in-flight xous-core PRs can land on
  `tunnell/xous-core@dev` (and eventually upstream `betrusted-io/xous-core@dev`)
  on their own timeline. They get rolled into xas releases when the next
  xas release decides to pin them in.
- Clean fallback: if a maintainer reports a bug on `xas vX.Y`, that
  bug-fix path is well-defined — clone `xous-app-signal@vX.Y`, clone
  `xous-core@xas-vX.Y`, reproduce, fix on a hot-fix branch off `vX.Y`.
