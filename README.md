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

Pulse publishes signed updater artifacts through GitHub Releases. The updater
public key is committed in `src-tauri/tauri.conf.json`. Before the first release:

1. Keep the matching `~/.tauri/pulse.key` and its password backed up securely; replacing or losing them prevents updates to installed copies.
2. In GitHub, open **Settings → Secrets and variables → Actions** and add the content of `~/.tauri/pulse.key` as `TAURI_SIGNING_PRIVATE_KEY`.
3. If the key has a password, add it as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Keep **Settings → Actions → General → Workflow permissions** set to **Read and write permissions** so the workflow can create tags and releases.

Git tags are the release version source of truth. PR titles determine whether a
merge publishes a new version:

- `fix:` creates a patch release such as `0.1.1`;
- `feat:` creates a minor release such as `0.2.0`;
- `feat!:` creates a breaking release.

After an eligible PR is squash-merged into `main`, the release workflow calculates
the next version from the latest `v*` tag. It temporarily injects that version only
on the build runners, builds macOS Apple Silicon and Windows installers, and
publishes the signed artifacts and `latest.json`. It does not add a release PR,
version commit, or generated changelog to `main`.

For recovery or an intentional version override, run **Build and publish release**
from the GitHub Actions page and choose `auto`, `patch`, `minor`, or `major`. A
failed build removes its incomplete draft release and tag so the same version can
be retried safely.

Updater signatures prove that an update was produced with the Pulse updater key.
They do not replace Apple notarization or Windows code signing, which should be
configured before distributing production builds broadly.
