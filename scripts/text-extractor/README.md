# Text Extractor prototype

Windows only. In Pulse Settings, enable **Text Extractor**, save an OpenAI API key if prompted, and optionally record another shortcut. The default is **Ctrl + Shift + E**. Put the pointer on the monitor to capture, press the shortcut, then drag and release. Edit the returned text and choose **Copy** to copy and close. **Translate** opens a language picker and replaces the editor content with a translation of its current text. The screenshot stays visible. Press **Escape** or click outside the result to dismiss; invoke the shortcut again to take a fresh capture.

The result contains only the screenshot, a full-area editor, and centered Copy and Translate pills. Screenshot and editor use squircle corners on recent WebView2 versions, with rounded corners as a fallback. The selection animates into place, scanning stops when text arrives, and translation animates the editor while retaining its content. Accessible status announcements are kept offscreen. Visible messages appear only for errors. The translation symbol comes from `src/translation-icons/translate-icon.svg` on the Live Translate branch, with the background circle removed. Both pills use the same colors, and both icons inherit the button foreground color.

The overlay uses a monochrome palette that follows Pulse's Appearance selection, including the resolved Auto schedule. The native window is created with Pulse's current visual theme and updates through the same controller as the tray, including transition rollbacks. WebView2 exposes that theme through `prefers-color-scheme`. The blurred backdrop is grayscale; the selected screenshot retains its original colors. Scanning uses a broad, full-height shimmer moving from left to right over the entire screenshot, with no scan line. Once extraction completes, the current 2.4-second pass finishes at the right edge before the text or error appears. Dismissal remains immediate. Reduced motion uses a static highlight and reveals the result without the extra wait.

The feature starts disabled and only enables after secure key storage succeeds and Windows registers the shortcut. Shortcut conflicts leave the previous combination intact. No paid request is made when saving a key or enabling the feature. An invalid or unauthorized key is reported when an extraction is attempted.

## Credential compatibility

`src-tauri/src/openai_credentials.rs` uses the exact Live Translate credential identity from the `live-translate` branch: service `app.pulse.desktop`, account `openai-api-key`, with `keyring`'s native store. Keys are shared, never kept in the JSON preferences or returned to JavaScript. Updating the key updates it for both features. No key removal control is included, to avoid disabling Live Translate unexpectedly. Live Translate itself is not merged into this branch; when combining the branches, its existing helpers can delegate to this module without migrating credentials. Text Extractor uses a separate `text-extractor.json` preferences file and independent commands.

## Capture and request lifecycle

The Win32 backend captures the monitor under the pointer before creating the overlay. The overlay occupies exactly that monitor in physical pixels, including monitors left of or above the primary display. A frozen screenshot is blurred after selection; other monitors are untouched. The crop is validated and produced again in Rust from the original pixels. Only that crop is sent to `https://api.openai.com/v1/responses`, using `gpt-5.6-luna`, `reasoning.effort: low`, and `store: false`. The prompt requests only the intended main text, in its original language. Screenshot instructions are treated as source text. Images stay in memory and are discarded when the capture closes; no screenshot files are written.

Translation sends only the current edited text and uses the same model, low reasoning, shared credentials, and request cancellation. The picker supports English, Ukrainian, German, Spanish, French, Italian, Polish, Portuguese, Japanese, Korean, Simplified Chinese, and Arabic. The backend validates the selected language against this list and caps input at 100,000 UTF-8 bytes. While a translation is pending, the text remains visible and temporarily read-only. Failed or empty translations leave it intact and allow retrying, editing, or copying.

The request has a two-minute timeout, a bounded response size, and cancellation when its window is closed or the feature is disabled. Stale sessions cannot write into a subsequent capture. Clipboard errors preserve the edited text and keep the window open. Native screenshots of protected content may be blank. HDR colors and exclusive fullscreen applications need verification on a Windows desktop.

API contract sources: [Luna model](https://developers.openai.com/api/docs/models/gpt-5.6-luna), [image input](https://developers.openai.com/api/docs/guides/images-vision).

## UI preview without API calls

Run `node scripts/text-extractor/preview.mjs` and open `http://127.0.0.1:4178`. This injects an isolated fixture bridge into the actual frontend. It has no real API, key storage, clipboard, or capture access, and is outside the packaged `src` directory.

- `/` — select an area and receive sample text.
- `/?scenario=demo` — hold the simulated extraction for eight seconds to preview the shimmer. Emulate light/dark color schemes in a desktop browser to preview Pulse's two palettes.
- `/?scenario=error`, `empty`, `pending`, or `clipboard-error` — exercise those result states.
- `/?scenario=translation-error`, `translation-empty`, or `translation-pending` — exercise translation failures and cancellation. Successful fixtures provide German and Ukrainian sample translations; other languages echo the input.
- `/settings.html` — Windows settings with no key; `?key` simulates a shared saved key.
- `/settings.html?scenario=key-error` or `shortcut-error` — simulate saving failures.
- `/settings.html?platform=macos` — confirm that the Windows feature is hidden.

`node --test tests/extraction-geometry.test.mjs` checks drag directions, monitor bounds, and fractional DPI. Rust unit tests verify crop bounds and request/response data structures with local fixtures. These never contact OpenAI or access a saved key. The desktop verification workflow compiles and tests on Windows and macOS without API credentials.

## Windows acceptance pass still required

On a Windows desktop, verify native hotkey registration and conflicts, Windows Credential Manager sharing, repeat capture/dismissal, focus restoration, clipboard ownership, Pulse Appearance changes and Auto scheduling while the overlay is open, protected content, mixed 100/125/150/200% DPI, portrait displays, negative monitor origins, and disconnecting a monitor during capture. Browser fixtures and compilation do not prove these native behaviors. Live API behavior was deliberately not tested for this prototype.
