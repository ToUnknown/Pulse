# Pulse

Pulse is a Rust/Tauri tray utility for macOS and Windows. It uses native tray menus only: keep it windowless and webview-free.

- macOS: status and quit menu items.
- Windows: native Appearance submenu with Auto, Light, and Dark. Auto uses Light from 07:00–19:00 and Dark otherwise.
- Runtime code: `src-tauri/src/lib.rs`.
- Tray assets: `src-tauri/icons/tray/`.

## Build

Requires Rust, Node.js, and pnpm.

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```

## Validation and releases

- Before opening a PR to `main`, run `cargo fmt -- --check`, `cargo clippy -- -D warnings`, and `cargo test` from `src-tauri`; report any supported platform not verified locally.
- Releases are created by `.github/workflows/release.yml`. Read **Releases and updates** in `README.md` before changing versions, updater configuration, signing, or release automation.
- For releases, keep versions aligned in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`; keep the updater private key out of the repository.
