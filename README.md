# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

- A compact Settings window that opens from the menu-bar or tray menu
- Start at login on macOS and Windows
- Default and Red menu-bar or tray icons
- Windows appearance controls with Auto, Light, and Dark modes
- Configurable light and dark start times for Windows Auto mode

Closing Settings hides the window without quitting Pulse. Opening it again brings it to the front.

## Development

Requires Node.js, pnpm, and Rust.

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```

The Codex environment adds two Play actions:

- `Build macOS DMG` runs on macOS and saves the DMG to the local Desktop.
- `Build Windows EXE` runs on Windows and saves the NSIS installer to the local Desktop.
