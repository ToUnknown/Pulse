# Pulse

Pulse is a Rust/Tauri tray utility using native menus on Windows and macOS.

The current prototype provides:

- a native tray/menu-bar icon and pull-down menu;
- no webview or application window;
- a native Start at Login toggle on Windows and macOS;
- a native `Appearance` submenu on Windows;
- Windows-only Auto, Light, and Dark appearance choices;
- automatic light mode at 07:00 and dark mode at 19:00, persisted across restarts.

## Development

Requirements: Node.js, pnpm, and Rust.

```sh
pnpm install
pnpm tauri dev
```

Click the tray/menu-bar icon to open the native menu. Every available control is inside it.
