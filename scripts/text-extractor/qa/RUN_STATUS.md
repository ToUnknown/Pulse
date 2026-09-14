# Windows run status — September 14, 2026

The user authorized the automated suite, then switched to testing a single visible card manually. No native OCR accuracy result is claimed from the initial automated attempts.

The pipeline built the Windows app, verified both cached model hashes, loaded the CPU sessions, opened the Edge card page, and restored preferences and clipboard without reported cleanup errors. The 2560×1440 primary monitor reported 100% scaling.

| Run directory under `out/text-extractor-qa` | Result |
| --- | --- |
| `2026-09-14T12-08-56-184Z` | Stopped before selection: native fullscreen bounds changed before Edge's viewport settled. Added an explicit viewport wait. |
| `2026-09-14T12-10-37-737Z` | Viewport reached 2560×1440 at DPR 1. Stopped before the shortcut: Windows PowerShell could not resolve the abbreviated generic list type. Changed it to the fully qualified .NET type. |

The comparison helper passed local checks for missing Ukrainian/English spaces, punctuation loss, reading order, CRLF normalization, and empty text. The desktop bridge passed PowerShell parsing, and the fully qualified generic list now resolves on Windows. Neither proves native OCR accuracy. The corrected shortcut path and new success-notice checks await an automated rerun; automation was stopped as requested.

The updated Windows app passed formatting, Clippy with warnings denied, and a debug build. Tauri dev was restarted, the cached OCR sessions reported ready, and the manual English/Ukrainian card was visibly confirmed on the Windows desktop. The user then completed hands-on testing and confirmed: “Everything is working.” This records manual acceptance of the current Windows changes; it does not claim that the automated 52-case matrix completed.

Use `pnpm ocr:cards` for manual acceptance. The full 52-case suite remains available through `pnpm ocr:qa --run` when requested again.

During PR preparation, formatting and Clippy passed on both platforms, together with all 49 Windows and 33 macOS Rust unit tests. These include the OCR dictionary/space decoder regressions on Windows. The automated native card matrix was not restarted.
