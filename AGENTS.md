# AGENTS.md

## Validation

Before a PR to `main`, run `cargo fmt -- --check`, `cargo clippy -- -D warnings`, and `cargo test` in `src-tauri`, plus `git diff --check`. Validate changed workflow YAML and report any platform not verified locally.

## Releases

- PRs are squash-merged. `fix:` and `perf:` release a patch, `feat:` releases a minor, and a Conventional Commit breaking marker releases a major. `ci:`, `docs:`, and `chore:` do not release.
- Tags are the version source. Build runners inject versions, so keep release-only version bumps and generated changelogs out of commits.
- Publish only the macOS DMG and updater archive, Windows NSIS executable, and `latest.json`. Signatures belong in the manifest; omit MSI and standalone `.sig` files.
- Publish and clean draft releases by numeric release ID.
- Before changing release or updater code, verify the public `latest.json`, platform downloads, signatures, tag target, and GitHub Actions permissions. Use manual workflow dispatch only for recovery or an intentional version override.
