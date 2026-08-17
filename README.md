# Pulse

Pulse is a windowless Rust/Tauri tray utility for macOS and Windows. Every
control lives in the native tray or menu-bar menu; the app does not open a
webview or application window.

## Features

- Native tray/menu-bar menu and icon
- Start at Login on macOS and Windows
- Signed update checks on startup and from the menu
- Windows-only Appearance menu with Auto, Light, and Dark modes
- Automatic Windows light mode from 07:00–19:00 and dark mode otherwise

macOS appearance is managed by the operating system, so Pulse does not add a
separate appearance control there.

## Development

Requires Node.js, pnpm, and Rust.

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```
