# Dictation maintenance

## Pipeline

- `src-tauri/src/dictation/shortcut.rs` interprets physical Right Option/Right Alt events as tap-to-toggle or hold-to-record. Other input does not stop hands-free recording; Windows preserves AltGr.
- `mod.rs` owns one recording session, bounded audio buffering, the OpenAI connection, final delivery, and overlay commands. Session IDs prevent old network/UI callbacks from completing a newer recording.
- `protocol.rs` configures PCM16 mono at 24 kHz and explicit commit on stop. Default uses `gpt-transcribe` after the commit and keeps the transcript box hidden; Live uses `gpt-live-transcribe` with high delay and shows partial words. Final text is delivered once in either mode.
- `native.m` and `windows.cpp` provide platform shortcuts, capture, clipboard copy, one Paste shortcut, and window behavior.
- `src/dictation.js` and `src/dictation.css` render the passive bottom-center overlay. A shared height spring grows the box to five lines and scrolls older text; the waveform uses only the current microphone level.

There is no field locking, live editor mutation, or transcript history. After recording ends and modifiers are released, delivery copies the complete transcript to the clipboard and sends one platform Paste shortcut to the active app. A failed clipboard write is retried once; if both writes fail, the bottom overlay shows an error. macOS resolves the Paste key through the active keyboard layout. The clipboard retains the transcript even if the app rejects Paste. Pulse cannot guarantee acceptance by every custom editor or elevated Windows app, and it does not retry the shortcut because that could insert duplicate text.

macOS uses native clear glass where available. The untinted outer blur additionally depends on optional private compositor classes/functions, guarded at runtime; if unavailable, the halo is omitted. This needs review on new macOS releases and for any future App Store distribution. Windows currently has CSS styling rather than a desktop backdrop blur.

macOS bundles require both `NSMicrophoneUsageDescription` in `Info.plist` and `com.apple.security.device.audio-input` in the signing entitlements. `bundle.macOS.entitlements` points to `src-tauri/Entitlements.plist`. Without that entitlement, hardened-runtime builds are denied before the permission prompt and do not appear in the Microphone privacy list. Validate the installed bundle's signature with `codesign -d --entitlements - /Applications/Pulse.app`; a working dev process alone does not establish that packaged permissions work.

## Verification inventory

These are available checks, not a claim that the current branch passed them:

| Area | Existing coverage |
| --- | --- |
| Shortcut semantics | Rust unit tests in `shortcut.rs`; macOS adapter harness `shortcut-native-test.m` |
| Protocol and session completion | Rust tests in `protocol.rs` and `mod.rs`, including local WebSocket failure scenarios |
| Async microphone startup | `capture-startup-test.m`, using a simulated engine without opening the microphone |
| macOS final delivery | `paste-native-test.m` checks that delivery waits for modifier release, copies the transcript, and sends one Paste sequence with a private pasteboard and suppressed keyboard events |
| Renderer state and growth | `overlay-test.mjs`, using the real renderer with a minimal DOM; `overlay-layout.html` is a browser layout fixture |
| Native blur geometry | `backdrop-native-test.m`, using an unshown macOS window; explicitly skips when private backdrop support is absent |
| Settings access | `settings-test.mjs` covers grant/denial, stale messages, action errors, overlapping refreshes, repeated requests, and the API-key gate using simulated IPC; `../text-extractor/qa/settings-access.html` covers the shared Advanced Access UI |

Paste acceptance still needs real-editor validation; a private-pasteboard fixture cannot establish acceptance by Codex, Spotlight, or Windows editors. Windows hook/AltGr behavior also needs native verification.

Before a PR to `main`, repository policy requires `cargo fmt -- --check`, `cargo clippy -- -D warnings`, and `cargo test` from `src-tauri`, followed by `git diff --check`. Native acceptance should cover tap/hold, hands-free navigation, Unicode text, one Paste attempt, retained clipboard transcript, and both themes. Do not run these native checks against a user's active editor or clipboard without authorization.
