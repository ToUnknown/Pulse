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

### Build installers in Codex

After the Codex environment setup finishes, open the Play menu and run the action for the current computer:

- `Build macOS DMG` creates the Apple Silicon DMG on macOS and copies it to the local Desktop.
- `Build Windows EXE` creates the Windows x64 NSIS installer on Windows and copies it to the local Desktop.

Each action disables updater artifacts because local builds do not have the release signing key. The release workflow still creates and signs updater artifacts.
