const invoke = window.__TAURI__.core.invoke;
const $ = (selector) => document.querySelector(selector);
const section = $("#text-extractor-section");
const enabled = $("#text-extractor-enabled");
const shortcutButtons = { shortcut: $("#extractor-shortcut"), quickShortcut: $("#extractor-quick-shortcut") };
const keySection = $("#openai-key-section");
const keyInput = $("#openai-api-key");
const keySave = $("#openai-key-save");
const keyRemove = $("#openai-key-remove");
const keyMask = $("#openai-key-mask");
const keyStatus = $("#extractor-key-status");
const keyError = $("#openai-key-error");
const settingsError = $("#extractor-settings-error");
const shortcutRows = $("#extractor-shortcuts");
const modeControls = [...document.querySelectorAll(".shortcut-mode")];
const ocrSetup = $("#ocr-setup");
const ocrLabel = $("#ocr-setup-label");
const ocrProgress = $("#ocr-setup-progress");
const ocrRetry = $("#ocr-setup-retry");
let ocrPoll;
let state;
let platform = "windows";
let recording = null;
let busy = false;
let checkingKey = false;

function displayShortcut(value) {
  return value.split("+").map((part) => {
    const labels = { control: "Ctrl", ctrl: "Ctrl", super: platform === "macos" ? "⌘" : "Win", meta: platform === "macos" ? "⌘" : "Win", shift: "Shift", alt: platform === "macos" ? "Option" : "Alt" };
    return labels[part.toLowerCase()] || part.replace(/^Key(?=[A-Z]$)/, "").replace(/^Digit(?=\d$)/, "");
  }).join(" + ");
}
function error(reason) { settingsError.textContent = String(reason); settingsError.hidden = false; $("#tab-advanced").click(); }
function lock(value) {
  busy = value;
  enabled.disabled = value || state?.captureAccess?.supported === false;
  for (const button of Object.values(shortcutButtons)) button.disabled = value;
  keyInput.disabled = value;
  keyRemove.disabled = value;
  for (const group of modeControls) for (const button of group.querySelectorAll("button")) button.disabled = value;
  renderKeyControls();
}
function setExpanded(value) {
  shortcutRows.dataset.expanded = String(value);
  shortcutRows.inert = !value;
}
function renderModes() {
  for (const group of modeControls) {
    group.hidden = !state.apiKeyConfigured;
    const mode = state[group.dataset.field] || "basic";
    group.style.setProperty("--mode-index", mode === "advanced" ? 1 : 0);
    for (const button of group.querySelectorAll("button")) {
      const selected = button.dataset.mode === mode;
      button.setAttribute("aria-checked", String(selected));
      button.tabIndex = selected ? 0 : -1;
    }
  }
}
function render() {
  enabled.checked = state.enabled;
  const access = state.captureAccess;
  $("#capture-access").hidden = platform !== "macos" || !state.enabled || access?.granted;
  $("#extractor-description").textContent = platform === "macos"
    ? (access?.supported === false ? "Requires macOS 14 or later." : "Offline. Uses built-in Apple text recognition.")
    : "Offline. Downloads a small model on first use.";
  setExpanded(state.enabled);
  renderOcrStatus(state.localOcr);
  renderModes();
  for (const [field, button] of Object.entries(shortcutButtons)) { button.textContent = displayShortcut(state[field]); button.title = button.textContent; }
  keyStatus.textContent = state.apiKeyConfigured
    ? "Key saved."
    : "Optional. Unlocks Advanced and Translate.";
  delete keyStatus.dataset.tone;
  keyInput.removeAttribute("aria-invalid");
  keyInput.dataset.configured = String(state.apiKeyConfigured);
  keyInput.placeholder = state.apiKeyConfigured ? "" : "sk-…";
  keyRemove.hidden = !state.apiKeyConfigured;
  renderKeyControls();
  if (state.error) error(state.error);
  else settingsError.hidden = true;
}
async function load() { state = await invoke("text_extractor_state"); render(); }
function renderOcrStatus(status) {
  if (state) state.localOcr = status;
  const phase = status?.phase || "idle";
  ocrSetup.hidden = !state?.enabled || ["idle", "ready"].includes(phase);
  ocrRetry.hidden = phase !== "error";
  ocrSetup.dataset.error = String(phase === "error");
  ocrLabel.textContent = phase === "downloading"
    ? `Downloading offline OCR · ${status.modelIndex}/2`
    : phase === "error" ? status.error
    : "Preparing offline OCR…";
  ocrProgress.hidden = phase !== "downloading";
  if (status?.totalBytes) {
    ocrProgress.value = Math.min(1, status.downloadedBytes / status.totalBytes);
  } else { ocrProgress.removeAttribute("value"); }
}
async function pollOcrStatus() {
  try {
    if (state?.enabled && !document.hidden) renderOcrStatus(await invoke("local_ocr_state"));
  } catch { /* The window may be closing; the next visible poll can refresh it. */ }
  const pending = ["preparing", "downloading", "loading"].includes(state?.localOcr?.phase);
  ocrPoll = window.setTimeout(pollOcrStatus, pending ? 500 : 2500);
}
ocrRetry.addEventListener("click", async () => {
  ocrRetry.disabled = true;
  try {
    await invoke("retry_local_ocr_setup");
    renderOcrStatus(await invoke("local_ocr_state"));
  } catch (reason) { ocrLabel.textContent = String(reason); }
  finally { ocrRetry.disabled = false; }
});
window.addEventListener("pagehide", () => window.clearTimeout(ocrPoll));
async function save(nextEnabled, nextShortcut = state.shortcut, nextQuickShortcut = state.quickShortcut, nextModes = {}) {
  lock(true);
  settingsError.hidden = true;
  setExpanded(nextEnabled);
  try { await invoke("set_text_extractor", { enabled: nextEnabled, shortcutValue: nextShortcut, quickShortcutValue: nextQuickShortcut, editorMode: nextModes.editorMode || state.editorMode || "basic", quickMode: nextModes.quickMode || state.quickMode || "basic" }); await load(); }
  catch (reason) { render(); error(reason); }
  finally { lock(false); }
}
enabled.addEventListener("change", () => save(enabled.checked));
for (const group of modeControls) {
  async function selectMode(button) {
    if (busy || !state.enabled || !state.apiKeyConfigured) return;
    const field = group.dataset.field;
    const previous = state[field] || "basic";
    if (previous === button.dataset.mode) return;
    state[field] = button.dataset.mode;
    renderModes();
    // Keep the previous saved value for rollback if saving fails.
    state[field] = previous;
    await save(state.enabled, state.shortcut, state.quickShortcut, { [field]: button.dataset.mode });
  }
  group.addEventListener("click", event => {
    const button = event.target.closest("button");
    if (button) selectMode(button);
  });
  group.addEventListener("keydown", event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    if (busy) return;
    const current = state[group.dataset.field] || "basic";
    const next = event.key === "Home" ? "basic" : event.key === "End" ? "advanced" : current === "basic" ? "advanced" : "basic";
    const button = group.querySelector(`[data-mode="${next}"]`);
    button.focus();
    selectMode(button);
  });
}
function renderKeyControls() {
  keySave.disabled = busy || !keyInput.value.trim();
  keySave.textContent = checkingKey ? "Checking…" : "Save";
  keyMask.hidden = keyInput.dataset.configured !== "true" || keyInput.value.trim() !== "";
}
keyInput.addEventListener("input", () => {
  keyError.hidden = true;
  delete keyStatus.dataset.tone;
  keyInput.removeAttribute("aria-invalid");
  if (state) render();
  renderKeyControls();
});
$("#openai-key-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (busy || !keyInput.value.trim()) return;
  checkingKey = true;
  lock(true);
  keyError.hidden = true;
  keyStatus.textContent = "Checking key…";
  delete keyStatus.dataset.tone;
  try {
    await invoke("save_openai_api_key", { apiKey: keyInput.value });
    keyInput.value = "";
    await load();
  } catch (reason) {
    keyStatus.textContent = String(reason);
    keyStatus.dataset.tone = "error";
    keyInput.setAttribute("aria-invalid", "true");
  } finally { checkingKey = false; lock(false); }
});
keyRemove.addEventListener("click", async () => {
  if (busy) return;
  lock(true);
  keyError.hidden = true;
  try {
    await invoke("clear_openai_api_key");
    keyInput.value = "";
    await load();
  } catch (reason) { keyError.textContent = String(reason); keyError.hidden = false; }
  finally { lock(false); }
});
function stopRecording() {
  if (recording) invoke("record_text_extractor_shortcut", { recording: false }).catch(error);
  recording = null;
  for (const [field, button] of Object.entries(shortcutButtons)) {
    button.setAttribute("aria-pressed", "false");
    if (state) button.textContent = displayShortcut(state[field]);
  }
}
function acceptShortcut(combination) {
  const field = recording;
  if (!field) return;
  stopRecording();
  save(state.enabled, field === "shortcut" ? combination : state.shortcut,
    field === "quickShortcut" ? combination : state.quickShortcut);
}
for (const [field, button] of Object.entries(shortcutButtons)) {
  button.addEventListener("click", () => {
    button.focus();
    recording = field;
    button.textContent = "Press a combination…";
    button.setAttribute("aria-pressed", "true");
    invoke("record_text_extractor_shortcut", { recording: true }).catch((reason) => { stopRecording(); error(reason); });
  });
  button.addEventListener("blur", stopRecording);
  button.addEventListener("keydown", (event) => {
    if (recording !== field) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") { stopRecording(); return; }
    if (/^(Control|Shift|Alt|Meta)/.test(event.code)) return;
    if (!event.ctrlKey && !event.altKey && !event.metaKey) { error(platform === "macos" ? "Include Control, Option, or Command in your shortcut." : "Include Ctrl, Alt, or Windows in your shortcut."); return; }
    if (!/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Space|Arrow(Up|Down|Left|Right))$/.test(event.code)) {
      error("Use a letter, number, function key, arrow, or Space."); return;
    }
    acceptShortcut([event.ctrlKey && "Control", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super", event.code].filter(Boolean).join("+"));
  });
}
window.addEventListener("pulse-extractor-shortcut", (event) => {
  if (typeof event.detail === "string") acceptShortcut(event.detail);
});
window.addEventListener("focus", () => {
  if (!section.hidden && !busy && !recording) load().catch(error);
});
try {
  const settings = await invoke("settings_state");
  platform = settings.platform;
  if (["windows", "macos"].includes(platform)) {
    section.hidden = false;
    keySection.hidden = false;
    await load();
    lock(false);
    pollOcrStatus();
  } else { $("#advanced-unavailable").hidden = false; }
} catch (reason) { if (!section.hidden) error(reason); }

$("#capture-access-request").addEventListener("click", async () => {
  const button = $("#capture-access-request");
  button.disabled = true;
  try { await invoke("request_text_extractor_access"); await load(); }
  catch (reason) { $("#capture-access-label").textContent = String(reason); button.textContent = "Retry"; }
  finally { button.disabled = false; }
});
