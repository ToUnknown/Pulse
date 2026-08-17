# Pulse

Pulse is a Rust/Tauri tray utility using native menus on Windows and macOS.

The current prototype provides:

- a native tray/menu-bar icon and pull-down menu;
- no webview or application window;
- a native Start at Login toggle on Windows and macOS;
- automatic signed update checks and downloads on startup in release builds;
- a native `Check for Updates…` action that becomes `Restart to Update` when ready;
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

## Releases and updates

Pulse publishes signed updater artifacts through GitHub Releases. Before the first release:

1. Generate the updater signing keys and keep the private key backed up somewhere secure:

   ```sh
   pnpm tauri signer generate -w ~/.tauri/pulse.key
   ```

2. Replace `REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY` in `src-tauri/tauri.conf.json` with the one-line content of `~/.tauri/pulse.key.pub`.
3. In GitHub, open **Settings → Secrets and variables → Actions** and add the content of `~/.tauri/pulse.key` as `TAURI_SIGNING_PRIVATE_KEY`.
4. If the key has a password, add it as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

To publish, update the version in `package.json`, `src-tauri/Cargo.toml`, and
`src-tauri/tauri.conf.json`, then push a matching tag such as `v0.2.0`. The
`release.yml` workflow builds macOS Apple Silicon, macOS Intel, and Windows
installers, then attaches the updater signatures and `latest.json` to the GitHub
Release.

Updater signatures prove that an update was produced with the Pulse updater key.
They do not replace Apple notarization or Windows code signing, which should be
configured before distributing production builds broadly.
