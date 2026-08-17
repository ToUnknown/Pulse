# Pulse

Pulse is a windowless Rust/Tauri tray utility for macOS and Windows. Keep every
control in the native tray menu; do not add a webview or application window.

## Structure

- Runtime and native menus: `src-tauri/src/lib.rs`
- Tauri/updater configuration: `src-tauri/tauri.conf.json`
- Tray assets: `src-tauri/icons/tray/`
- Release workflow and scripts: `.github/workflows/release.yml` and `.github/scripts/`

Windows owns the Appearance submenu (Auto, Light, and Dark); Auto uses Light
from 07:00–19:00. macOS uses the system appearance controls.

## Build and validation

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```

Before opening a PR to `main`, run `cargo fmt -- --check`,
`cargo clippy -- -D warnings`, and `cargo test` from `src-tauri`. Also run
`git diff --check`, validate edited workflow YAML, and report any supported
platform not verified locally.

## Releases

- PRs are squash-merged. Use `fix:`/`perf:` for patch, `feat:` for minor, and a Conventional Commit breaking marker for major releases. `ci:`, `docs:`, and `chore:` do not release.
- Git tags are the version source of truth. Versions are injected only on build runners; do not commit release-only version bumps or generated changelogs.
- The workflow builds macOS Apple Silicon and Windows x64, caches `src-tauri/target`, signs updater artifacts, finalizes public updater URLs, and publishes `latest.json`.
- Draft releases must be published and cleaned up by numeric release ID. Do not replace this with tag-based draft lookup.
- Keep `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in GitHub Actions secrets. Never commit the private key, and keep its backup safe.
- Before changing release or updater code, verify the public `latest.json`, platform downloads, signatures, tag target, and GitHub Actions permissions. Use manual workflow dispatch only for recovery or an intentional version override.

Updater signatures do not replace Apple notarization or Windows code signing.
