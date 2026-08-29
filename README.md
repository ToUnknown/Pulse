# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

### macOS

- Settings from the menu-bar menu
- Start at login
- Default and Red menu-bar icons
- In-app update checks and restart
- Experimental live speech translation to a virtual microphone

### Windows

- Settings from the tray menu
- Start at login
- Default and Red tray icons
- In-app update checks and restart
- Auto, Light, and Dark appearance modes with configurable start times

Closing Settings hides the window without quitting Pulse. Opening it again brings it to the front.

## Live Translate (experimental)

Live Translate captures a physical microphone, sends 24 kHz PCM audio to OpenAI's
`gpt-realtime-translate` model, and writes the translated speech to a virtual microphone.
When translation is stopped, that same `Pulse` microphone carries the selected physical
microphone unchanged, so call apps can leave it selected. When translation is active, Pulse
outputs only translated speech. It never plays translated speech through the user's speakers.

Live Translate is a macOS-only feature and is off by default. Turning it on installs Pulse's
bundled, input-only Core Audio driver after administrator approval. It publishes exactly one
system audio endpoint named `Pulse`, with no public render side. Select `Pulse` as the microphone
in the call app.

In Pulse Settings, save an OpenAI API key and choose the physical microphone. Output routing
is fixed internally to the Pulse virtual microphone and has no user-selectable destination.
The key is stored in macOS Keychain, not in Pulse's config file. From the menu-bar menu, choose
`Translate to`, select one of the 13 output languages, and click
`Start Translation`.

The dedicated Realtime Translation API currently selects its own native translation voice.
Its session schema does not expose the named voice choices available to standard Realtime
voice-agent sessions. Pulse therefore uses the model's returned audio directly instead of
adding a second text-to-speech pass that would increase latency and change the audio path.

The General `Live Translate` switch controls the feature globally. Turning it off stops
translation, removes its controls from the menu-bar menu, and removes the `Pulse` input. It also
removes the Pulse driver. Upgrading from the earlier experiment removes the Pulse-owned VB-CABLE
driver and aggregate device.

Pulse cannot make translated audio appear under the original hardware microphone's name.
Desktop applications bind to operating-system audio endpoints, so the call app must use
the virtual input. Already-running apps may need their microphone selection refreshed.
See [Live Translate architecture and limitations](docs/live-translate.md).

## Development

Requires Node.js, pnpm, and Rust.

```sh
pnpm install
pnpm tauri dev
pnpm tauri build
```

### Build installers in Codex

After the Codex environment setup finishes, open the Play menu and run the action for the current computer:

- `Build DMG` creates the Apple Silicon DMG on macOS and copies it to the local Desktop.
- `Build EXE` creates the Windows x64 NSIS installer on Windows and copies it to the local Desktop.

Each action disables updater artifacts because local builds do not have the release signing key. The release workflow still creates and signs updater artifacts.
