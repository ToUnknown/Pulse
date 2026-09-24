# Pulse

Pulse is a tray/menu-bar utility for macOS and Windows.

## Features

### macOS

Text Extractor requires macOS 14 or later. Other menu-bar features remain available on older supported Macs.

- Settings from the menu-bar menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red menu-bar icons
- In-app update checks and restart

- Dictation: tap **Right Option** to start or finish hands-free recording, or hold it for at least half a second and release. See [Dictation](#dictation) for setup and delivery behavior.

- Text Extractor: Option + Shift + T opens the editor; Control + Option + Shift + T copies immediately. Basic uses Apple Vision locally with no model download or API key. Enabling Text Extractor prepares OCR in the background; Settings shows its status until it is ready. Allow Screen Recording in Settings → Advanced, where both shortcuts and their default modes can be customized. GPT-6 Luna Advanced extraction uses the shared OpenAI API key.

### Windows

- Settings from the tray menu, organized into General, Advanced, and Appearance pages
- Start at login
- Default and Red tray icons
- In-app update checks and restart
- Auto, Light, and Dark appearance modes with configurable start times.
- Dictation: tap **Right Alt** to start or finish hands-free recording, or hold it for at least half a second and release. AltGr remains available for normal typing.
- Text Extractor prototype: use Win + Shift + T to open the animated editor, or Ctrl + Win + Shift + T to select and instantly copy local OCR text. Selection happens over the live desktop. Basic uses a small downloaded PP-OCRv5 model locally, with no API key. Enable it and customize either shortcut and its default mode in Settings → Advanced. GPT-6 Luna Advanced extraction uses the shared OpenAI API key. Escape or an outside click dismisses the overlay.

See [Text Extractor setup and prototype notes](scripts/text-extractor/README.md) for architecture, setup, and manual platform verification notes.

The [manual Windows OCR acceptance pipeline](scripts/text-extractor/qa/README.md) prepares English and Ukrainian test cards for a future native verification run. It only launches with an explicit `--run` flag.

Text Extractor’s **Translate** uses Google Translate’s unofficial web endpoint on both platforms, without an API key or account. Choosing a language sends the current editor text to Google; an internet connection is required. If translation fails, the original text stays editable. Retry, or explicitly choose **Use Advanced** to send it to GPT-6 Luna with your OpenAI API key. Translate is unavailable when the editor contains more than 5,000 characters; shorten the text to enable it again. Google may change or block this endpoint.

## Dictation

Add your OpenAI API key and enable **Tap or hold to dictate** in Settings → Advanced. Choose a **Transcription model** there: **Default** uses `gpt-transcribe` and is selected when no preference has been saved; **Live** uses `gpt-live-transcribe`. The choice is saved for future recordings. Dictation requires microphone access; macOS also requires Accessibility access.

Recording always shows a small voice-reactive pill at the bottom center. **Default** keeps the transcript box hidden while you speak and transcribes after you release or stop the recording. **Live** shows incoming words in a box above the pill, with up to five visible lines, and uses high delay for transcription. Both modes stream audio to OpenAI and commit the recording on release or stop before delivering the final text. Hands-free recording continues while you type, click, or switch apps. No input field is tracked or edited during recording.

Pulse sends the completed transcript to the currently selected supported input using Unicode input events, without reading or changing the clipboard. If no suitable input is found or no text can be dispatched, Pulse copies the transcript and briefly shows **Copied to clipboard**. Once input events have been submitted, Pulse does not retry or overwrite the clipboard based on an unreliable editor readback. Some custom editors and Windows apps running with higher privileges may reject synthetic input; submission is not a universal insertion guarantee.

The macOS pill uses native clear glass on macOS 26 or a system material on older versions. Its outer blur uses optional private compositor APIs and has no tinted fallback when unavailable. Windows uses CSS styling; it does not currently blur other desktop windows behind the overlay. Both themes keep contrasting text and waveform colors.

Pulse does not save dictation recordings or transcripts to disk or offer a last-transcript history. Session text is cleared after delivery; operational errors appear in Settings. See [dictation maintenance notes](scripts/dictation/README.md) for architecture and verification coverage.
