# AGENTS.md

## References

- Codex setup and build actions: `.codex/environments/environment.toml`; installer logic: `scripts/codex/build-desktop-installer.mjs`.
- GitHub release workflow: `.github/workflows/release.yml`; release helpers: `.github/scripts/`.

## Git

- Commit completed task changes without waiting to be asked. Preserve meaningful development steps and split large tasks when useful. Local commits need no release prefix because PRs are squash-merged.
- Format the PR title as `type(scope)!: summary`; it becomes the squash commit subject read by release automation. `type` is lowercase; `scope` optionally names the affected area.
  - `fix:` repairs existing behavior and triggers a patch release.
  - `perf:` improves existing performance and triggers a patch release.
  - `feat:` adds behavior and triggers a minor release.
  - `!` after the type or scope triggers a major release. A `BREAKING CHANGE:` or `BREAKING-CHANGE:` commit footer does the same.
  - Other types do not trigger a release. Common types are `build:` for builds or dependencies, `chore:` for maintenance, `ci:` for automation, `docs:` for documentation, `refactor:` for internal restructuring, `revert:` for reversals, `style:` for formatting, and `test:` for tests.
- Release automation scans every commit since the latest `vX.Y.Z` tag and uses the highest bump. Manual runs may force `patch`, `minor`, or `major`. Build runners inject versions, so omit release-only version bumps and generated changelogs.
- Write PR descriptions as one plain-language paragraph explaining what changes for people, not a code summary or bullet list.

## Verification

- Before a PR to `main`, run `cargo fmt -- --check`, `cargo clippy -- -D warnings`, and `cargo test` in `src-tauri`, then `git diff --check`.
- For GitHub automation changes, syntax-check changed scripts and validate changed workflow YAML.
- Browser previews can help with webview UI changes. Native menu, tray, and OS integration may need human verification.
- Before diagnosing or changing release or updater behavior, inspect the current GitHub release and `latest.json`; repository files do not show deployed state.
- If `AGENTS.md` or `README.md` is outdated, tell the developer that an update is needed.
