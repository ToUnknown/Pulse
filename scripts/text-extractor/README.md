# Text Extractor prototype

Windows only. Open **Settings → Advanced** and enable **Text Extractor**. **Win + Shift + T** selects an area over the live desktop, reads it locally, copies the text, and closes without showing a screenshot, editor, or buttons. **Win + Shift + Ctrl + T** opens the editor flow: after release, the selected crop flies from its original rectangle into the screen's center, then settles above the editor as the text appears. Basic OCR runs during the flight. The editor shortcut can be changed in Settings; Win + Shift + T is reserved for Quick Copy. The former Ctrl + Shift + E default migrates to the new editor shortcut, while other custom combinations are preserved.

Both shortcuts are intercepted on a dedicated Windows keyboard-hook thread while enabled, consuming the T press, repeat, and release before Windows' Snipping Tool sees the chord. Other keys and modifier releases pass through. Modifier state is tracked directly, with no typed text recorded. A bounded non-blocking queue sends actions off the hook thread. Disabling the feature restores Windows' normal shortcut handling. Shortcut recording consumes these reserved chords and reports the chosen combination to Settings without starting either extractor.

In the editor, without a saved key, the overlay contains only the screenshot, editor, and **Copy**. Settings explains that a shared OpenAI API key unlocks **Advanced** and **Translate**, with a separate password field for saving or replacing that key. With a key, the overlay adds a **Basic / Advanced** sliding selector above the screenshot and **Translate** to the left of **Copy**. Basic remains the default for every capture. Availability refreshes when the overlay regains focus.

Switching to Advanced fades away the editor, centers the screenshot and slightly enlarges it, then runs a broad full-height shimmer from left to right. When Luna's result arrives, the current 2.4-second pass finishes at the right edge before the screenshot shrinks and moves upward and the editor and buttons fade in below it. Switching modes retains each mode's edited draft, cancels the previous request, and reuses completed results without another paid call. Reduced motion skips the flight, zoom, and shimmer wait. Errors leave the mode selector usable and preserve existing edits.

Edit the returned text and choose **Copy** to copy and close. **Translate** opens a language picker and replaces the current mode's editor content with a translation of its current text. Press **Escape** or click outside the result to dismiss; invoke the shortcut again to take a fresh capture. A drag that starts inside the editor does not dismiss the overlay when released outside.

Screenshot and editor use squircle corners on recent WebView2 versions, with rounded corners as a fallback. The translation symbol comes from `src/translation-icons/translate-icon.svg` on the Live Translate branch, with the background circle removed. Both centered pills use the same foreground and background colors, with Translate on the left and Copy on the right. Accessible status announcements are offscreen; there are no visible hints, headings, or close/reselect/done controls.

The monochrome overlay follows Pulse's Appearance selection, including the resolved Auto schedule. The native window is created with Pulse's current visual theme and updates through the same controller as the tray, including transition rollbacks. WebView2 exposes that theme through `prefers-color-scheme`. The blurred backdrop is grayscale; the selected screenshot retains its original colors.

Settings now has **General**, **Advanced**, and **Appearance** pages with a sliding tab indicator and keyboard navigation. General contains startup behavior; Appearance contains Auto scheduling and tray/menu-bar icon style. The Advanced page contains Text Extractor and the separate shared API-key field on Windows. The macOS page explains that these advanced features are currently available on Windows. Page changes preserve saved state and close open dropdowns. The selected settings pill is black in light mode and white in dark mode. Text uses difference blending so its color inverts exactly where the moving pill passes beneath each letter; reduced motion and forced colors have accessible fallbacks.

## Credential compatibility

`src-tauri/src/openai_credentials.rs` uses the exact Live Translate credential identity from the `live-translate` branch: service `app.pulse.desktop`, account `openai-api-key`, with `keyring`'s native store. Keys are shared, never kept in the JSON preferences or returned to JavaScript. Updating the key updates it for both features. No key removal control is included, to avoid disabling Live Translate unexpectedly. Live Translate itself is not merged into this branch; when combining the branches, its existing helpers can delegate to this module without migrating credentials. Text Extractor uses a separate `text-extractor.json` preferences file and independent commands.

## Capture and request lifecycle

Pulse prepares a hidden transparent webview when the feature is enabled and after a capture closes. The shortcut path only reads monitor geometry, positions that preloaded window, and shows the live selector; it does not capture/encode a screenshot, transfer pixels, or read credentials. Native DWM window transitions are disabled. The selection surface intercepts the mouse over the live desktop, including monitors left of or above the primary display.

Pixels are captured only on release. Quick Copy captures only the crop, hides the selector, and runs native OCR without returning images or text to the webview; failed or empty recognition leaves the clipboard intact and reports the error in Settings. The editor captures the selected monitor once and retains Windows' native BGRA buffer. Only the bound crop is converted and PNG-encoded before its flight starts. A separate cancellable command prepares a small backdrop while the crop is already moving; neither backdrop encoding nor decoding gates the animation. Only after selection is the frozen background blurred. Other monitors are untouched. The selector is excluded from capture with `WDA_EXCLUDEFROMCAPTURE` (Windows 10 version 2004+).

Closing the last selector does not quit the Windows tray app; explicit Quit and updater restart still work. The replacement selector is prepared in the background. The keyboard hook only updates shortcut state and sends a non-blocking action; Windows-menu input masking runs on the action thread. Remapped/injected shortcut keys are supported, and only the tagged unused mask key is ignored. When a capture closes, the next hook event resynchronizes modifier/T state with Windows, recovering releases consumed while the selector was focused and preserving repeat suppression if the original chord is still held.

Every active and preloaded window has a unique label. Ready/start handshakes avoid cold-webview startup work, and matching-label cleanup prevents a delayed watchdog or cancelled Quick Copy from closing a newer session. Display changes invalidate the selected monitor geometry before capture. Basic uses Windows.Media.Ocr with installed recognition languages, preferring the user profile languages and falling back to an installed recognizer. It preserves recognized line breaks and scales images only when Windows OCR requires a smaller image. A missing OCR language produces an actionable error; Basic never falls back to a network call. See [Microsoft OcrEngine documentation](https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine).

Only in Advanced is that crop sent to `https://api.openai.com/v1/responses`, using `gpt-5.6-luna`, `reasoning.effort: low`, and `store: false`. The prompt requests only the intended main text, in its original language. Screenshot instructions are treated as source text. Images stay in memory and are discarded when the capture closes; no screenshot files are written.

Translation sends only the current edited text and uses the same model, low reasoning, shared credentials, and request cancellation. The picker supports English, Ukrainian, German, Spanish, French, Italian, Polish, Portuguese, Japanese, Korean, Simplified Chinese, and Arabic. The backend validates the selected language against this list and caps input at 100,000 UTF-8 bytes. While a translation is pending, the text remains visible and temporarily read-only. Failed or empty translations leave it intact and allow retrying, editing, or copying.

The request has a two-minute timeout, a bounded response size, and cancellation when its window is closed or the feature is disabled. Ordered request IDs prevent late requests, cancellation messages, or responses from overriding a newer mode or capture. Native OCR runs on a blocking worker with balanced WinRT initialization; cancellation discards its result, though Windows may finish recognition already in progress. Clipboard errors preserve the edited text and keep the window open. Native screenshots of protected content may be blank. HDR colors and exclusive fullscreen applications need verification on a Windows desktop.

API contract sources: [Luna model](https://developers.openai.com/api/docs/models/gpt-5.6-luna), [image input](https://developers.openai.com/api/docs/guides/images-vision).

## UI preview without API calls

Run `node scripts/text-extractor/preview.mjs` and open `http://127.0.0.1:4178`. This injects an isolated fixture bridge into the actual frontend. It has no real API, key storage, clipboard, or capture access, and is outside the packaged `src` directory.

- `/` — select an area and preview its flight into the editor. A fixture desktop canvas continues animating during selection.
- `/?quick` — select and copy with no result UI or API-key access.
- `/?warm` — stay preloaded until a `pulse-capture-start` DOM event starts the fixture session.
- `/?scenario=capture-pending` or `capture-error` — exercise delayed capture cancellation and errors.
- `/?scenario=backdrop-pending` or `backdrop-error` — verify that backdrop preparation cannot hold up the crop flight or usable editor.
- `/?key&scenario=demo` — preview the Basic crop flight, then switch to Advanced for a simulated 6.5-second request to preview the full animation. Emulate light/dark color schemes in a desktop browser to preview Pulse's two palettes.
- `/?key&scenario=error`, `empty`, or `pending` — exercise Advanced result states. `/?scenario=basic-error`, `basic-empty`, or `basic-pending` exercise local recognition states; `clipboard-error` simulates a copy failure.
- `/?key&scenario=translation-error`, `translation-empty`, or `translation-pending` — exercise translation failures and cancellation. Successful fixtures provide German and Ukrainian sample translations; other languages echo the input.
- `/settings.html` — Windows settings with no key; `?key` simulates a shared saved key.
- `/settings.html?scenario=key-error` or `shortcut-error` — simulate saving failures.
- `/settings.html?platform=macos` — confirm that the Windows feature is hidden.

`node --test tests/extraction-geometry.test.mjs` checks drag directions, monitor bounds, and fractional DPI. Rust unit tests verify crop bounds, both shortcut chords, repeats, modifier releases, disabled/recording behavior, stale session cleanup, and request/response data structures with local fixtures. These never contact OpenAI or access a saved key. The desktop verification workflow compiles and tests on Windows and macOS without API credentials.

## Windows verification and remaining acceptance checks

### Native capture and shortcut checks on 2026-09-13

Changes were applied and checked in `C:\Users\maxga\Codex\Pulse`. The native last-window regression initially exited Pulse; after the lifecycle fix, Pulse remained alive and created one replacement selector. Alternating held Quick Copy/editor shortcuts initially skipped every second activation after the focused overlay consumed key releases. After state resynchronization, four consecutive `SendInput` shortcut/Escape cycles opened and dismissed Pulse without opening Snipping Tool. These are native automated input checks, not physical keyboard testing.

Native Basic OCR recognized the known sentence `Pulse native text extraction test`, and Quick Copy wrote that exact sentence to the clipboard and closed with no result window. No live OpenAI calls were made. Windows formatting, Clippy with warnings denied, and all 31 Rust tests passed; macOS passed its corresponding 27 tests.

WebView2 timing on the 2560×1440 Windows desktop initially measured 1205 ms from pointer release to the `flying` phase. Separating backdrop work reduced a 700×300 capture to 244 ms; retaining native BGRA pixels reduced it further to 91 ms. A 1300×300 text-fixture selection measured 127 ms and reached the editable result. The initial desktop capture and later fixture captures contain different pixels, so these are diagnostic measurements, not a controlled benchmark or a guarantee for all resolutions. Timings end when the frontend starts its flight animation; they do not measure camera-visible display latency.

### Native selector startup measured on 2026-09-13

The Windows debug build at `c2ad44a` still captured and PNG-encoded the full monitor before showing the selection window. Rebuilding the preview from `28c4b0c` removed that startup work and used the preloaded transparent selector. Three native editor activations on Windows 11 build 26200 measured:

| Build | Activation to visible selector, milliseconds |
| --- | --- |
| `c2ad44a`, screenshot before selection | 2291, 2225, 2226 |
| `28c4b0c`, preloaded live selector | 38, 31, 16 |

The isolated probes called the native editor entry point and waited for the session's `shown` flag, which is set after the frontend enables selection and the native window is shown and focused. The feature had three seconds to initialize before the first activation and two seconds between activations. A hidden blank window kept the older probe alive between captures. Temporary preferences enabled only the probe, credential availability was stubbed off, and the probes dismissed without selecting an area, running OCR, or calling OpenAI. These timings cover an initialized app; they do not establish cold application launch time, immediate activation after enabling, physical keyboard-hook behavior, or first-paint/input latency measured by a camera.

The corrected clean debug executable was built separately so the Windows checkout's uncommitted files could remain intact. When reproducing a startup report, check the executable's build revision as well as the checked-out branch: an older running executable can still have the screenshot-before-selection pipeline after the branch has been updated.

On a Windows desktop, verify both shortcuts suppress Snipping Tool, key-up/repeat behavior, enable/disable and shortcut recording, cold/warm launch latency, transparency and mouse capture over live video, Quick Copy with no result UI, editor crop alignment on release, native custom-hotkey registration and conflicts, Windows Credential Manager sharing, local OCR with installed and missing language packs, OCR on large/portrait selections, repeat capture/dismissal, focus restoration, clipboard ownership, Pulse Appearance changes and Auto scheduling while the overlay is open, protected content, mixed 100/125/150/200% DPI, portrait displays, negative monitor origins, and disconnecting a monitor during capture. Browser fixtures and compilation do not prove these native behaviors. Live API behavior was deliberately not tested for this prototype.
