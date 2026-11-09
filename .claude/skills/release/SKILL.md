---
name: release
description: Publish a new GitHub release of Taskbar Monitor (version bump, tag, CI build, hash check). Usage /release 0.2.5
disable-model-invocation: true
---

Release version `$ARGUMENTS` (X.Y.Z, no leading v) to GitHub.

1. Preconditions: on `main`, working tree clean, `git fetch` shows nothing new upstream, and
   `$ARGUMENTS` is higher than the version in `Cargo.toml`. Stop and report if any fails.
2. Set `version = "$ARGUMENTS"` in `Cargo.toml`, run `cargo build --release` so `Cargo.lock` follows,
   and check the README and `store/listing.md` describe any user-visible change since the last tag
   (`git log $(git describe --tags --abbrev=0)..HEAD --oneline`).
3. Commit (`Release v$ARGUMENTS`), then show the user the commit and the changes since the last tag,
   and get an explicit yes before pushing: the tag reaches every installed copy within a day.
4. `git push origin main`, then `git tag v$ARGUMENTS && git push origin v$ARGUMENTS`.
5. Wait for the Release workflow (`gh run list --workflow release.yml -L 1`, then
   `gh run watch <id>` in the background). On failure, show the log and stop; never delete or
   move a published tag without asking.
6. Verify: `gh release view v$ARGUMENTS` lists `taskbar-monitor.exe` and `.sha256`; download the exe
   and confirm its SHA-256 matches the `.sha256` file.
7. Tell the user the release URL and remind them that the Store build is separate: run
   `/store-update` to ship the same version there.
