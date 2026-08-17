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
