## Structure

- Runtime and native menus: `src-tauri/src/lib.rs`
- Tauri/updater configuration: `src-tauri/tauri.conf.json`
- Tray assets: `src-tauri/icons/tray/`
- Settings UI: `src/settings.html`, `src/settings.css`, and `src/settings.js`
- Local Codex build actions: `.codex/environments/environment.toml` and `scripts/codex/`
- Release workflow and scripts: `.github/workflows/release.yml` and `.github/scripts/`

## Settings

- Keep Start at login and the Default or Red icon choice in Settings. macOS uses the label `Menu-bar icon`; Windows uses `Tray icon`.
- Windows owns the Appearance submenu. Auto, Light, and Dark are radio choices, so exactly one item is checked.
- Windows Auto uses the schedule from Settings. The default light period is 07:00–19:00.
- The Settings window is fixed-size and unscrollable. Opening it brings it to the front; closing it hides the window without quitting Pulse.
- Time and icon controls use custom pill dropdowns. The whole time pill is clickable, and a menu opens below when its rendered height fits.

## Build and validation

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```

The Codex Play actions build a macOS DMG on macOS or a Windows x64 NSIS executable on Windows. Each action copies its installer to that machine's Desktop.

Before opening a PR to `main`, run `cargo fmt -- --check`,
`cargo clippy -- -D warnings`, and `cargo test` from `src-tauri`. Also run
`git diff --check`, validate edited workflow YAML, and report any supported
platform not verified locally.

## Releases

- PRs are squash-merged. Use `fix:`/`perf:` for patch, `feat:` for minor, and a Conventional Commit breaking marker for major releases. `ci:`, `docs:`, and `chore:` do not release.
- Git tags are the version source of truth. Versions are injected only on build runners; do not commit release-only version bumps or generated changelogs.
- The workflow builds a macOS Apple Silicon DMG and Windows x64 NSIS executable, caches `src-tauri/target`, signs updater artifacts, finalizes public updater URLs, and publishes `latest.json`.
- Keep release assets minimal: DMG plus updater archive for macOS, NSIS executable for Windows, and `latest.json`. Do not upload MSI or standalone `.sig` files; signatures are embedded in the manifest.
- Draft releases must be published and cleaned up by numeric release ID. Do not replace this with tag-based draft lookup.
- Before changing release or updater code, verify the public `latest.json`, platform downloads, signatures, tag target, and GitHub Actions permissions. Use manual workflow dispatch only for recovery or an intentional version override.
