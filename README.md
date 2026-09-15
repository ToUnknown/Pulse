# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

### macOS

Text Extractor requires macOS 14 or later. Other menu-bar features remain available on older supported Macs.

- Settings from the menu-bar menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red menu-bar icons
- In-app update checks and restart

- Text Extractor: Option + Shift + T opens the editor; Control + Option + Shift + T copies immediately. Basic uses Apple Vision locally with no model download or API key. Enabling Text Extractor prepares OCR in the background; Settings shows its status until it is ready. Allow Screen Recording in Settings → Advanced, where both shortcuts and their default modes can be customized. A locally installed Codex signed in with ChatGPT is the preferred provider for GPT-5.6 Luna Advanced extraction. The shared OpenAI API key remains an alternative; the Codex option is hidden when Codex is not installed.

### Windows

- Settings from the tray menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red tray icons
- In-app update checks and restart
- Auto, Light, and Dark appearance modes with configurable start times.
- Text Extractor prototype: use Win + Shift + T to open the animated editor, or Ctrl + Win + Shift + T to select and instantly copy local OCR text. Selection happens over the live desktop. Basic uses a small downloaded PP-OCRv5 model locally, with no API key. Enable it and customize either shortcut and its default mode in Settings → Advanced. A locally installed Codex signed in with ChatGPT is the preferred provider for GPT-5.6 Luna Advanced extraction, with the shared OpenAI API key as an alternative. The Codex option is hidden when Codex is not installed. Escape or an outside click dismisses the overlay.

See [Text Extractor setup and prototype notes](scripts/text-extractor/README.md) for architecture, setup, and manual platform verification notes.

The [manual Windows OCR acceptance pipeline](scripts/text-extractor/qa/README.md) prepares English and Ukrainian test cards for a future native verification run. It only launches with an explicit `--run` flag.

Text Extractor’s **Translate** uses Google Translate’s unofficial web endpoint on both platforms, without an API key or account. Choosing a language sends the current editor text to Google; an internet connection is required. If translation fails, the original text stays editable. Retry, or explicitly choose **Use Advanced** to send it through the configured Codex/API provider when available. This prototype accepts up to 5,000 characters per Google request; longer passages can use Advanced. Google may change or block this endpoint.
