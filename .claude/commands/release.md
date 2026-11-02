---
description: Cut a CalVer release — verify clean tree, run CI gate, update CHANGELOG, tag, push
argument-hint: "[optional release note one-liner]"
allowed-tools: Bash(git status:*), Bash(git diff:*), Bash(git log:*), Bash(git tag:*), Bash(git push:*), Bash(cargo fmt:*), Bash(cargo clippy:*), Bash(cargo build:*), Bash(cargo test:*), Bash(cargo audit:*), Bash(date:*), Bash(gh run list:*)
---

Cut a release for novus2mqtt. The version is **CalVer `YYYY.M.D`** computed by [build.rs](build.rs) from `date`, so the git tag must match today's date.

**Do not push the tag without showing the user the tag name and CHANGELOG diff first.**

## Steps

1. **Compute the target version**: `date +%Y.%-m.%-d` → e.g. `2026.4.17`. Tag will be `v<version>`.

2. **Pre-flight checks** (stop and report on first failure):
   - Working tree clean (`git status --porcelain` empty).
   - On `main`.
   - Tag `v<version>` does not already exist.
   - Last CI run on `main` is green (`gh run list --branch main --limit 1`).

3. **Run the local CI gate** (same as `/check`):
   - `cargo fmt --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo build --release`
   - `cargo test`
   - `cargo audit` (warn but don't block on advisories alone — show them to the user)

4. **Update CHANGELOG.md**:
   - Move entries from the `## [Unreleased]` section into a new `## [<version>] - YYYY-MM-DD` section.
   - Recreate an empty `## [Unreleased]` block at the top.
   - If `$ARGUMENTS` is provided, prepend it as a one-line release summary under the new section.
   - If `## [Unreleased]` is empty, STOP and ask the user whether to release anyway.

5. **Show the user**: the tag name, the CHANGELOG diff, and a one-line summary of what will be released. Ask for go/no-go.

6. **On approval**:
   - Commit the CHANGELOG: `git commit -m "chore: release <version>"` (no Co-Authored-By unless asked).
   - Tag: `git tag -a v<version> -m "<version>"`.
   - Push: `git push origin main` then `git push origin v<version>`.

7. **Verify**: print the new tag and the GitHub release URL (if a release workflow runs on tag push).

## Rules

- Never force-push, never delete tags, never amend the release commit after pushing.
- If `cargo audit` reports a critical advisory, BLOCK the release and ask the user.
- If multiple releases happen on the same day, stop — CalVer collisions need the user to decide
  (manual `.dev` suffix, postpone, or skip the date).
