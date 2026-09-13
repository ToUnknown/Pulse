# Text Extractor prototype

Windows only. Open **Settings → Advanced**, enable **Text Extractor**, and optionally record another shortcut. The default is **Ctrl + Shift + E**. Enabling, starting, and using Basic do not require an API key. Put the pointer on the monitor to capture, press the shortcut, then drag and release. The screenshot and editor appear together immediately. Windows OCR fills the editor as soon as local recognition finishes, with no artificial scanning delay.

Without a saved key, the overlay contains only the screenshot, editor, and **Copy**. Settings explains that a shared OpenAI API key unlocks **Advanced** and **Translate**, with a separate password field for saving or replacing that key. With a key, the overlay adds a **Basic / Advanced** sliding selector above the screenshot and **Translate** to the left of **Copy**. Basic remains the default for every capture. Availability refreshes when the overlay regains focus.

Switching to Advanced fades away the editor, centers the screenshot and slightly enlarges it, then runs a broad full-height shimmer from left to right. When Luna's result arrives, the current 2.4-second pass finishes at the right edge before the screenshot shrinks and moves upward and the editor and buttons fade in below it. Switching modes retains each mode's edited draft, cancels the previous request, and reuses completed results without another paid call. Reduced motion skips the flight, zoom, and shimmer wait. Errors leave the mode selector usable and preserve existing edits.

Edit the returned text and choose **Copy** to copy and close. **Translate** opens a language picker and replaces the current mode's editor content with a translation of its current text. Press **Escape** or click outside the result to dismiss; invoke the shortcut again to take a fresh capture. A drag that starts inside the editor does not dismiss the overlay when released outside.

Screenshot and editor use squircle corners on recent WebView2 versions, with rounded corners as a fallback. The translation symbol comes from `src/translation-icons/translate-icon.svg` on the Live Translate branch, with the background circle removed. Both centered pills use the same foreground and background colors, with Translate on the left and Copy on the right. Accessible status announcements are offscreen; there are no visible hints, headings, or close/reselect/done controls.

The monochrome overlay follows Pulse's Appearance selection, including the resolved Auto schedule. The native window is created with Pulse's current visual theme and updates through the same controller as the tray, including transition rollbacks. WebView2 exposes that theme through `prefers-color-scheme`. The blurred backdrop is grayscale; the selected screenshot retains its original colors.

Settings now has **General**, **Advanced**, and **Appearance** pages with a sliding tab indicator and keyboard navigation. General contains startup behavior; Appearance contains Auto scheduling and tray/menu-bar icon style. The Advanced page contains Text Extractor and the separate shared API-key field on Windows. The macOS page explains that these advanced features are currently available on Windows. Page changes preserve saved state and close open dropdowns.

## Credential compatibility

`src-tauri/src/openai_credentials.rs` uses the exact Live Translate credential identity from the `live-translate` branch: service `app.pulse.desktop`, account `openai-api-key`, with `keyring`'s native store. Keys are shared, never kept in the JSON preferences or returned to JavaScript. Updating the key updates it for both features. No key removal control is included, to avoid disabling Live Translate unexpectedly. Live Translate itself is not merged into this branch; when combining the branches, its existing helpers can delegate to this module without migrating credentials. Text Extractor uses a separate `text-extractor.json` preferences file and independent commands.

## Capture and request lifecycle

The Win32 backend captures the monitor under the pointer before creating the overlay. The overlay occupies exactly that monitor in physical pixels, including monitors left of or above the primary display. A frozen screenshot is blurred after selection; other monitors are untouched. The crop is validated and produced again in Rust from the original pixels. Basic uses Windows.Media.Ocr with installed recognition languages, preferring the user profile languages and falling back to an installed recognizer. It preserves recognized line breaks and scales images only when Windows OCR requires a smaller image. A missing OCR language produces an actionable error; Basic never falls back to a network call. See [Microsoft OcrEngine documentation](https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine).

Only in Advanced is that crop sent to `https://api.openai.com/v1/responses`, using `gpt-5.6-luna`, `reasoning.effort: low`, and `store: false`. The prompt requests only the intended main text, in its original language. Screenshot instructions are treated as source text. Images stay in memory and are discarded when the capture closes; no screenshot files are written.

Translation sends only the current edited text and uses the same model, low reasoning, shared credentials, and request cancellation. The picker supports English, Ukrainian, German, Spanish, French, Italian, Polish, Portuguese, Japanese, Korean, Simplified Chinese, and Arabic. The backend validates the selected language against this list and caps input at 100,000 UTF-8 bytes. While a translation is pending, the text remains visible and temporarily read-only. Failed or empty translations leave it intact and allow retrying, editing, or copying.

The request has a two-minute timeout, a bounded response size, and cancellation when its window is closed or the feature is disabled. Ordered request IDs prevent late requests, cancellation messages, or responses from overriding a newer mode or capture. Native OCR runs on a blocking worker with balanced WinRT initialization; cancellation discards its result, though Windows may finish recognition already in progress. Clipboard errors preserve the edited text and keep the window open. Native screenshots of protected content may be blank. HDR colors and exclusive fullscreen applications need verification on a Windows desktop.

API contract sources: [Luna model](https://developers.openai.com/api/docs/models/gpt-5.6-luna), [image input](https://developers.openai.com/api/docs/guides/images-vision).

## UI preview without API calls

Run `node scripts/text-extractor/preview.mjs` and open `http://127.0.0.1:4178`. This injects an isolated fixture bridge into the actual frontend. It has no real API, key storage, clipboard, or capture access, and is outside the packaged `src` directory.

- `/` — select an area and receive sample text.
- `/?key&scenario=demo` — complete Basic immediately, then switch to Advanced for a simulated 6.5-second request to preview the full animation. Emulate light/dark color schemes in a desktop browser to preview Pulse's two palettes.
- `/?key&scenario=error`, `empty`, or `pending` — exercise Advanced result states. `/?scenario=basic-error`, `basic-empty`, or `basic-pending` exercise local recognition states; `clipboard-error` simulates a copy failure.
- `/?key&scenario=translation-error`, `translation-empty`, or `translation-pending` — exercise translation failures and cancellation. Successful fixtures provide German and Ukrainian sample translations; other languages echo the input.
- `/settings.html` — Windows settings with no key; `?key` simulates a shared saved key.
- `/settings.html?scenario=key-error` or `shortcut-error` — simulate saving failures.
- `/settings.html?platform=macos` — confirm that the Windows feature is hidden.

`node --test tests/extraction-geometry.test.mjs` checks drag directions, monitor bounds, and fractional DPI. Rust unit tests verify crop bounds and request/response data structures with local fixtures. These never contact OpenAI or access a saved key. The desktop verification workflow compiles and tests on Windows and macOS without API credentials.

## Windows acceptance pass still required

On a Windows desktop, verify native hotkey registration and conflicts, Windows Credential Manager sharing, local OCR with installed and missing language packs, OCR on large/portrait selections, repeat capture/dismissal, focus restoration, clipboard ownership, Pulse Appearance changes and Auto scheduling while the overlay is open, protected content, mixed 100/125/150/200% DPI, portrait displays, negative monitor origins, and disconnecting a monitor during capture. Browser fixtures and compilation do not prove these native behaviors. Live API behavior was deliberately not tested for this prototype.
