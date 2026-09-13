const invoke = window.__TAURI__.core.invoke;
const $ = (selector) => document.querySelector(selector);
const section = $("#text-extractor-section");
const enabled = $("#text-extractor-enabled");
const shortcutButton = $("#extractor-shortcut");
const keyButton = $("#extractor-key");
const dialog = $("#openai-key-dialog");
const keyInput = $("#openai-api-key");
const keySave = $("#openai-key-save");
const keyError = $("#openai-key-error");
const settingsError = $("#extractor-settings-error");
let state;
let enableAfterSave = false;
let recording = false;
let busy = false;

function displayShortcut(value) {
  return value.replace(/Key(?=[A-Z](?:\+|$))/g, "").replace(/Digit(?=\d)/g, "")
    .replace(/Control/g, "Ctrl").replace(/Super|Meta/g, "Win").split("+").join(" + ");
}
function error(reason) { settingsError.textContent = String(reason); settingsError.hidden = false; }
function lock(value) {
  busy = value;
  enabled.disabled = value;
  shortcutButton.disabled = value;
  keyButton.disabled = value;
}
function render() {
  enabled.checked = state.enabled;
  shortcutButton.textContent = displayShortcut(state.shortcut);
  $("#extractor-key-status").textContent = state.apiKeyConfigured ? "Shared OpenAI key saved" : "OpenAI API key required";
  keyButton.textContent = state.apiKeyConfigured ? "Update key" : "Add key";
  if (state.error) error(state.error);
}
async function load() { state = await invoke("text_extractor_state"); render(); }
async function save(nextEnabled, nextShortcut = state.shortcut) {
  lock(true);
  settingsError.hidden = true;
  try { await invoke("set_text_extractor", { enabled: nextEnabled, shortcutValue: nextShortcut }); await load(); }
  catch (reason) { render(); error(reason); }
  finally { lock(false); }
}
function openKeyDialog(enable) {
  enableAfterSave = enable;
  enabled.checked = state.enabled;
  keyInput.value = "";
  keyError.hidden = true;
  keySave.textContent = enable ? "Save & enable" : "Save key";
  dialog.showModal();
  keyInput.focus();
}
enabled.addEventListener("change", () => {
  if (enabled.checked && !state.apiKeyConfigured) openKeyDialog(true);
  else save(enabled.checked);
});
keyButton.addEventListener("click", () => openKeyDialog(false));
$("#openai-key-cancel").addEventListener("click", () => dialog.close());
dialog.addEventListener("close", () => { keyInput.value = ""; enableAfterSave = false; });
$("#openai-key-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const shouldEnable = enableAfterSave;
  keySave.disabled = true;
  keyInput.disabled = true;
  $("#openai-key-cancel").disabled = true;
  keyError.hidden = true;
  try {
    await invoke("save_openai_api_key", { apiKey: keyInput.value });
    keyInput.value = "";
    await load();
    dialog.close();
    if (shouldEnable) await save(true);
  } catch (reason) { keyInput.value = ""; keyError.textContent = String(reason); keyError.hidden = false; }
  finally { keySave.disabled = false; keyInput.disabled = false; $("#openai-key-cancel").disabled = false; }
});
dialog.addEventListener("cancel", (event) => { if (keySave.disabled) event.preventDefault(); });
shortcutButton.addEventListener("click", () => {
  recording = true;
  shortcutButton.textContent = "Press a combination…";
  shortcutButton.setAttribute("aria-pressed", "true");
  invoke("record_text_extractor_shortcut", { recording: true }).catch((reason) => { stopRecording(); error(reason); });
});
function stopRecording() {
  if (recording) invoke("record_text_extractor_shortcut", { recording: false }).catch(error);
  recording = false;
  shortcutButton.setAttribute("aria-pressed", "false");
  if (state) shortcutButton.textContent = displayShortcut(state.shortcut);
}
shortcutButton.addEventListener("blur", stopRecording);
shortcutButton.addEventListener("keydown", (event) => {
  if (!recording) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.key === "Escape") { stopRecording(); return; }
  if (/^(Control|Shift|Alt|Meta)/.test(event.code)) return;
  if (!event.ctrlKey && !event.altKey && !event.metaKey) { error("Include Ctrl, Alt, or Windows in your shortcut."); return; }
  if (!/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Space|Arrow(Up|Down|Left|Right))$/.test(event.code)) {
    error("Use a letter, number, function key, arrow, or Space."); return;
  }
  const combination = [event.ctrlKey && "Control", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super", event.code].filter(Boolean).join("+");
  stopRecording();
  save(state.enabled, combination);
});
window.addEventListener("focus", () => {
  if (!section.hidden && !busy && !recording && !dialog.open) load().catch(error);
});
try {
  const settings = await invoke("settings_state");
  if (settings.platform === "windows") {
    section.hidden = false;
    await load();
    lock(false);
  }
} catch (reason) { if (!section.hidden) error(reason); }
