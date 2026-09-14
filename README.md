# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

### macOS

- Settings from the menu-bar menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red menu-bar icons
- In-app update checks and restart

### Windows

- Settings from the tray menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red tray icons
- In-app update checks and restart
- Auto, Light, and Dark appearance modes with configurable start times.
- Text Extractor prototype: use Win + Shift + T to open the animated editor, or Ctrl + Win + Shift + T to select and instantly copy local OCR text. Selection happens over the live desktop. Basic uses a small downloaded PP-OCRv5 model locally, with no API key. Enable it and customize either shortcut and its default mode in Settings → Advanced. Adding the optional shared OpenAI API key unlocks Advanced extraction with GPT-5.6 Luna (no reasoning) and Translate; the key is checked before it is saved. Escape or an outside click dismisses the overlay.

See [Text Extractor setup and prototype notes](scripts/text-extractor/README.md) for the API-free UI preview and Windows verification checklist.

The [manual Windows OCR acceptance pipeline](scripts/text-extractor/qa/README.md) prepares English and Ukrainian test cards for a future native verification run. It only launches with an explicit `--run` flag.
