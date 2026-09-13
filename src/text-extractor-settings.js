const invoke = window.__TAURI__.core.invoke;
const $ = (selector) => document.querySelector(selector);
const section = $("#text-extractor-section");
const enabled = $("#text-extractor-enabled");
const shortcutButton = $("#extractor-shortcut");
const keySection = $("#openai-key-section");
const keyInput = $("#openai-api-key");
const keySave = $("#openai-key-save");
const keyError = $("#openai-key-error");
const settingsError = $("#extractor-settings-error");
let state;
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
  keyInput.disabled = value;
  keySave.disabled = value || !keyInput.value.trim();
}
function render() {
  enabled.checked = state.enabled;
  shortcutButton.textContent = displayShortcut(state.shortcut);
  $("#extractor-key-status").textContent = state.apiKeyConfigured ? "Shared key saved securely. Advanced and Translate are available." : "No key saved. Basic text recognition is available.";
  keyInput.placeholder = state.apiKeyConfigured ? "Enter a replacement key" : "sk-…";
  keySave.textContent = state.apiKeyConfigured ? "Update key" : "Save key";
  const availability = $("#extractor-availability");
  availability.hidden = !state.enabled;
  availability.textContent = state.apiKeyConfigured ? "Basic is the default. Advanced and Translate are ready to use." : "Basic works on your PC. Add an API key below to unlock Advanced and Translate.";
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
enabled.addEventListener("change", () => save(enabled.checked));
keyInput.addEventListener("input", () => { keySave.disabled = busy || !keyInput.value.trim(); });
$("#openai-key-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (busy || !keyInput.value.trim()) return;
  lock(true);
  keyError.hidden = true;
  try {
    await invoke("save_openai_api_key", { apiKey: keyInput.value });
    keyInput.value = "";
    await load();
  } catch (reason) { keyInput.value = ""; keyError.textContent = String(reason); keyError.hidden = false; }
  finally { lock(false); }
});
shortcutButton.addEventListener("click", () => {
  shortcutButton.focus();
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
window.addEventListener("pulse-extractor-shortcut", (event) => {
  if (!recording) return;
  stopRecording();
  if (event.detail === "Super+Shift+KeyT") { error("Win + Shift + T is reserved for quick copy. Choose another editor shortcut."); return; }
  if (event.detail === "Control+Super+Shift+KeyT") save(state.enabled, event.detail);
});
window.addEventListener("focus", () => {
  if (!section.hidden && !busy && !recording) load().catch(error);
});
try {
  const settings = await invoke("settings_state");
  if (settings.platform === "windows") {
    section.hidden = false;
    keySection.hidden = false;
    await load();
    lock(false);
  } else { $("#advanced-unavailable").hidden = false; }
} catch (reason) { if (!section.hidden) error(reason); }
