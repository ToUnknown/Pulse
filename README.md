# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

### macOS

The Mac app requires macOS 12 or later; Text Extractor requires macOS 14 or later.

- Settings from the menu-bar menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red menu-bar icons
- In-app update checks and restart

- Text Extractor: Option + Shift + T opens the editor; Control + Option + Shift + T copies immediately. Basic uses Apple Vision locally with no model download or API key. Allow Screen Recording in Settings → Advanced, where both shortcuts and their default modes can be customized. Advanced extraction and Translate use the provider selected in Settings: Apple Intelligence by default, or OpenAI with a saved API key. Apple’s cloud model requires macOS 27, a build made with Xcode 27, an eligible Apple Intelligence device, and Apple-approved Private Cloud Compute access. Unavailable access appears beside the provider with Retry; Basic stays usable.

### Windows

- Settings from the tray menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red tray icons
- In-app update checks and restart
- Auto, Light, and Dark appearance modes with configurable start times.
- Text Extractor prototype: use Win + Shift + T to open the animated editor, or Ctrl + Win + Shift + T to select and instantly copy local OCR text. Selection happens over the live desktop. Basic uses a small downloaded PP-OCRv5 model locally, with no API key. Enable it and customize either shortcut and its default mode in Settings → Advanced. Adding the optional shared OpenAI API key unlocks Advanced extraction with GPT-5.6 Luna (no reasoning) and Translate; the key is checked before it is saved. Escape or an outside click dismisses the overlay.

See [Text Extractor setup and prototype notes](scripts/text-extractor/README.md) for architecture, setup, and manual platform verification notes.

The [manual Windows OCR acceptance pipeline](scripts/text-extractor/qa/README.md) prepares English and Ukrainian test cards for a future native verification run. It only launches with an explicit `--run` flag.
