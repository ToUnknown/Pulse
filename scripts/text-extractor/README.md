# Text Extractor prototype

Windows only. In Pulse Settings, enable **Text Extractor**, save an OpenAI API key if prompted, and optionally record another shortcut. The default is **Ctrl + Shift + E**. Put the pointer on the monitor to capture, press the shortcut, then drag and release. Edit the returned text and choose **Copy text** (copies and closes), **Done**, or **Escape**. **Select again** uses the same frozen screen; close and invoke the shortcut again to take a fresh capture.

The feature starts disabled and only enables after secure key storage succeeds and Windows registers the shortcut. Shortcut conflicts leave the previous combination intact. No paid request is made when saving a key or enabling the feature. An invalid or unauthorized key is reported when an extraction is attempted.

## Credential compatibility

`src-tauri/src/openai_credentials.rs` uses the exact Live Translate credential identity from the `live-translate` branch: service `app.pulse.desktop`, account `openai-api-key`, with `keyring`'s native store. Keys are shared, never kept in the JSON preferences or returned to JavaScript. Updating the key updates it for both features. No key removal control is included, to avoid disabling Live Translate unexpectedly. Live Translate itself is not merged into this branch; when combining the branches, its existing helpers can delegate to this module without migrating credentials. Text Extractor uses a separate `text-extractor.json` preferences file and independent commands.

## Capture and request lifecycle

The Win32 backend captures the monitor under the pointer before creating the overlay. The overlay occupies exactly that monitor in physical pixels, including monitors left of or above the primary display. A frozen screenshot is blurred after selection; other monitors are untouched. The crop is validated and produced again in Rust from the original pixels. Only that crop is sent to `https://api.openai.com/v1/responses`, using `gpt-5.6-luna`, `reasoning.effort: medium`, and `store: false`. The prompt requests only the intended main text, in its original language. Screenshot instructions are treated as source text. Images stay in memory and are discarded when the capture closes; no screenshot files are written.

The request has a two-minute timeout, a bounded response size, and cancellation when its window is closed or the feature is disabled. Stale sessions cannot write into a subsequent capture. Clipboard errors preserve the edited text and keep the window open. Native screenshots of protected content may be blank. HDR colors and exclusive fullscreen applications need verification on a Windows desktop.

API contract sources: [Luna model](https://developers.openai.com/api/docs/models/gpt-5.6-luna), [image input](https://developers.openai.com/api/docs/guides/images-vision).

## UI preview without API calls

Run `node scripts/text-extractor/preview.mjs` and open `http://127.0.0.1:4178`. This injects an isolated fixture bridge into the actual frontend. It has no real API, key storage, clipboard, or capture access, and is outside the packaged `src` directory.

- `/` — select an area and receive sample text.
- `/?scenario=error`, `empty`, `pending`, or `clipboard-error` — exercise those result states.
- `/settings.html` — Windows settings with no key; `?key` simulates a shared saved key.
- `/settings.html?scenario=key-error` or `shortcut-error` — simulate saving failures.
- `/settings.html?platform=macos` — confirm that the Windows feature is hidden.

`node --test tests/extraction-geometry.test.mjs` checks drag directions, monitor bounds, and fractional DPI. Rust unit tests verify crop bounds and request/response data structures with local fixtures. These never contact OpenAI or access a saved key. The desktop verification workflow compiles and tests on Windows and macOS without API credentials.

## Windows acceptance pass still required

On a Windows desktop, verify native hotkey registration and conflicts, Windows Credential Manager sharing, repeat capture/dismissal, focus restoration, clipboard ownership, protected content, mixed 100/125/150/200% DPI, portrait displays, negative monitor origins, and disconnecting a monitor during capture. Browser fixtures and compilation do not prove these native behaviors. Live API behavior was deliberately not tested for this prototype.
