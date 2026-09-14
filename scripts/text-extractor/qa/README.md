# Native Windows OCR acceptance pipeline

This pipeline runs only when explicitly requested. It is not connected to CI or the app's startup. Initial Windows attempts on September 14 reached the card screen but stopped before OCR on harness integration errors; see [run status](RUN_STATUS.md). The complete matrix has not passed. Manual testing is currently selected at the user's request.

The runner opens synthetic English and Ukrainian cards in a separate Edge window and uses Pulse's real Windows shortcuts, native mouse selection, capture, downloaded Basic recognizer, editor, and clipboard. It observes the actual webview through local WebView2 debugging; it does not substitute recognition results or call the OCR command directly. Advanced extraction and translation are outside this suite.

## Run when ready

For a single screen you can test yourself, start Pulse and run `pnpm ocr:cards`. This opens English and Ukrainian text in one Edge window, with a card picker and a light/dark toggle. Use your own Pulse shortcuts to select any portion. It does not send input, change preferences, touch the clipboard, or run OCR automatically. Close the Edge window when finished; its local card server exits with it.

Use a terminal on the unlocked Windows desktop, in the Pulse checkout. Close Pulse through its tray menu and stop any running Tauri dev process first. The runner refuses to replace an existing Pulse process. Keep the desktop unlocked and leave the mouse and keyboard to the runner until it finishes; F8 or Ctrl+C stops it.

Prerequisites: Windows 10 version 2004+ or Windows 11, Node 22+, pnpm, Microsoft Edge, the project's Rust/MSVC build dependencies, and both OCR models already downloaded by Pulse. The runner verifies the cached files against the production SHA-256 manifest before starting Pulse. It does not install packages, download models, run Rust tests, or contact the OpenAI API. A cached dependency build is required unless `--skip-build` is supplied.

```powershell
cd C:\Users\maxga\Codex\Pulse
pnpm ocr:qa --run
```

`pnpm ocr:qa` without `--run` only displays help. This is deliberately an explicit opt-in, even on Windows. Do not add `--run` to CI or an unattended scheduler: the suite controls a visible desktop.

To run a smaller spacing regression later:

```powershell
pnpm ocr:qa --run --cases=english-spacing,ukrainian-spacing,small-english-line,small-ukrainian-line --themes=light --modes=quick,editor
```

Options:

| Option | Default | Meaning |
| --- | --- | --- |
| `--cases=id,...` | All 13 fixtures | IDs from `fixtures.json` |
| `--themes=light,dark` | Both | Source card colors; does not change Windows or Pulse Appearance |
| `--modes=quick,editor` | Both | Actual native shortcut paths, forced to Basic |
| `--repeat=3` | 1 | Repeat the selected matrix, up to 20 times |
| `--monitor=0` | Primary monitor | Zero-based monitor in the native enumeration; recorded in the report |
| `--skip-build` | Off | Use `src-tauri/target/debug/pulse.exe`; report explicitly marks its source revision unverified |

The default run has **52 cases: 13 cards × 2 source themes × 2 shortcut paths**. It builds the current debug app with `cargo build --locked --offline --no-default-features`, starts its own process, and waits for the cached OCR sessions to initialize without inference. Real recognition begins only when the native selection is released. A failed build or missing model stops setup instead of silently using another executable or downloading anything.

## Cards and expectations

| Card | Coverage |
| --- | --- |
| `english-spacing` | Word boundaries, repeated letters, punctuation, editing |
| `ukrainian-spacing` | Ukrainian word boundaries, Ї/Є/ґ and apostrophe, editing |
| `mixed-languages` | English and Ukrainian on the same lines |
| `english-document` | Heading, paragraphs, subheading, line order |
| `ukrainian-outline` | Numbered list with indented subitems |
| `english-table` | Invoice table, cells, prices and total |
| `ukrainian-table` | Order table with Ukrainian labels and decimal commas |
| `two-columns` | Separate columns; expected reading order is the left column, then the right |
| `code-and-symbols` | Code, quotes, operators, braces, URL and email |
| `small-english-line` | A single 12px line with only 2px padding |
| `small-ukrainian-line` | A single 12px Ukrainian line, selected in reverse direction |
| `numbers-and-dates` | Identifiers, leading zeros, percentages, dates and spaced amounts |
| `blank-card` | Shapes without text; unchanged clipboard and the No text found notice |

These are acceptance expectations, not a claim that every layout already works. Columns and code deliberately challenge the recognizer and current row grouping. Failures remain visible; the runner does not rearrange columns, fix punctuation, or insert missing spaces to get a passing result. Table cells are expected in row order, separated by a single space. Decorative indentation and blank visual gaps between blocks are not part of the expected plain text.

`fixtures.json` holds both the rendering instructions and independently written expected strings. The expected strings are never generated from OCR output. The cards use locally installed Segoe UI and Consolas, with no web fonts, images from the internet, or language model calls.

## Checks and artifacts

Every nonempty case compares the recognized text with its expected string. Normalization is limited to Unicode NFC, CRLF line endings, nonbreaking spaces, and whitespace at line edges. Internal spaces, punctuation, case, blank lines, and reading order remain significant. The report includes character and word error rates, expected/actual space and line counts, and a specific flag when removing spaces is the only difference. The pass condition is exact normalized equality, not an average accuracy threshold.

Quick Copy checks clipboard output, observation of the quick path, absence of a result panel, closure of the selector, and a green, top-centered **Text copied** notice. The blank card requires the clipboard sentinel to remain intact and a visible, top-centered **No text found** notice. Opening Settings is an unexpected failure. The editor path checks that the field becomes editable, tests replacing text on the two spacing cards, clicks the real Copy button, and checks exact clipboard content and dismissal. Blank editor results must disable Copy and preserve the clipboard. The original OCR text is compared before editing.

The runner verifies fullscreen browser/native pixel geometry and records monitor bounds and DPI. If a card overflows the desktop or the coordinate mapping is ambiguous, it stops without guessing where to select. Source images are captured only from the synthetic card rectangle; editor evidence is limited to the result panel. The original clipboard is kept privately in the STA bridge's memory, never in reports.

Results go into the gitignored folder `out/text-extractor-qa/<timestamp>/`:

- `report.html`: readable comparison with visible space markers, per-check outcomes, and source/editor images.
- `report.json`: full matrix, status, build revision and binary hash, verified model hashes, native display geometry, text comparisons, phase observations, and timings.
- `report.md`: compact summary.
- A folder per case containing `source.png`, `expected.txt`, `actual.txt`, and, for editor cases, `editor.png`.
- `build.log`, `pulse.log`, and `edge.log` for setup/runtime failures.

Text mismatches continue through the remaining matrix. A failed native interaction, unexpected Settings window, crash, timeout, or abort stops the run and leaves remaining cases marked **pending**. These runs are **incomplete**, never passing. Any failure or cleanup problem returns a nonzero exit code. Shortcut-to-selector and recognition timings are observations with automation overhead, not performance benchmarks or animation video.

This suite covers the selected monitor and its current DPI on synthetic cards. It does not certify every application, multi-monitor transition, protected video/HDR behavior, physical keyboard hardware, first-download handling, or Advanced/API behavior.

## Cleanup and recovery

The harness temporarily enables extraction, sets both defaults to Basic, and uses Ctrl+Win+Shift+T for Quick Copy and Win+Shift+T for the editor. It checks the active selector's mode before releasing the mouse. It does not read or replace the shared API key. On exit, it stops only its own build/app/browser processes, closes the local server, restores the original extraction preferences and clipboard, and returns the cursor and focus. It does not restart the user's normal dev app.

The Edge instance uses a separate disposable profile in the report directory and a loopback debugging port. Your normal browser profile is not used. Reports stay local and are not uploaded or opened automatically. Delete the run directory when you no longer need its artifacts and browser profile.

Preference restoration is guarded: if the file changed concurrently or Pulse could not stop, the runner leaves it untouched and prints a recovery message. An interrupted run can leave `preferences-backup.json` inside its output directory; that file contains only extraction preferences, not the API key. To restore it manually, first quit Pulse, then review that file and use its exact path below. This deliberately overwrites the current extraction preferences with the pre-run copy:

```powershell
$ocrBackupPath = 'C:\Users\maxga\Codex\Pulse\out\text-extractor-qa\<timestamp>\preferences-backup.json'
$ocrBackup = Get-Content -Raw -LiteralPath $ocrBackupPath | ConvertFrom-Json
if ($null -eq $ocrBackup.originalBase64) {
  Remove-Item -LiteralPath $ocrBackup.path -ErrorAction SilentlyContinue
} else {
  [IO.File]::WriteAllBytes($ocrBackup.path, [Convert]::FromBase64String($ocrBackup.originalBase64))
}
Remove-Item -LiteralPath $ocrBackupPath
```

Normal abort/EOF restores the clipboard through the persistent PowerShell bridge. Force-killing that bridge or ending the Windows session can prevent restoration, so use F8 or Ctrl+C. If a clipboard format cannot be safely preserved, setup fails before replacing it; copy plain text before trying again.

## Maintaining this harness

Keep it outside the packaged `src` directory. Changes to production target URLs, DOM IDs, startup readiness text, or preference schema may require an update here. A stale integration contract should produce a blocked run, not a fabricated OCR pass. Static syntax inspection is possible without starting anything using `node --check` on the `.mjs` files; parsing `desktop.ps1` with PowerShell's AST parser likewise does not execute it. Actual native acceptance remains deferred until explicitly requested.
