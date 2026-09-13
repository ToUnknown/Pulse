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
let state;
let recording = null;
let busy = false;
let checkingKey = false;

function displayShortcut(value) {
  return value.split("+").map((part) => {
    const labels = { control: "Ctrl", ctrl: "Ctrl", super: "Win", meta: "Win", shift: "Shift", alt: "Alt" };
    return labels[part.toLowerCase()] || part.replace(/^Key(?=[A-Z]$)/, "").replace(/^Digit(?=\d$)/, "");
  }).join(" + ");
}
function error(reason) { settingsError.textContent = String(reason); settingsError.hidden = false; $("#tab-advanced").click(); }
function lock(value) {
  busy = value;
  enabled.disabled = value;
  for (const button of Object.values(shortcutButtons)) button.disabled = value;
  keyInput.disabled = value;
  keyRemove.disabled = value;
  renderKeyControls();
}
function render() {
  enabled.checked = state.enabled;
  for (const [field, button] of Object.entries(shortcutButtons)) button.textContent = displayShortcut(state[field]);
  keyStatus.textContent = state.apiKeyConfigured
    ? "Stored in Windows Credential Manager. Enter a new key to replace it."
    : "Add a key to use Advanced extraction and Translate.";
  delete keyStatus.dataset.tone;
  keyInput.removeAttribute("aria-invalid");
  keyInput.dataset.configured = String(state.apiKeyConfigured);
  keyInput.placeholder = state.apiKeyConfigured ? "" : "sk-…";
  keyRemove.hidden = !state.apiKeyConfigured;
  renderKeyControls();
  const availability = $("#extractor-availability");
  availability.hidden = !state.enabled;
  availability.textContent = state.apiKeyConfigured ? "Basic is the default. Advanced and Translate are ready to use." : "Basic works on your PC. Add an API key below to unlock Advanced and Translate.";
  if (state.error) error(state.error);
  else settingsError.hidden = true;
}
async function load() { state = await invoke("text_extractor_state"); render(); }
async function save(nextEnabled, nextShortcut = state.shortcut, nextQuickShortcut = state.quickShortcut) {
  lock(true);
  settingsError.hidden = true;
  try { await invoke("set_text_extractor", { enabled: nextEnabled, shortcutValue: nextShortcut, quickShortcutValue: nextQuickShortcut }); await load(); }
  catch (reason) { render(); error(reason); }
  finally { lock(false); }
}
enabled.addEventListener("change", () => save(enabled.checked));
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
    if (!event.ctrlKey && !event.altKey && !event.metaKey) { error("Include Ctrl, Alt, or Windows in your shortcut."); return; }
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
  if (settings.platform === "windows") {
    section.hidden = false;
    keySection.hidden = false;
    await load();
    lock(false);
  } else { $("#advanced-unavailable").hidden = false; }
} catch (reason) { if (!section.hidden) error(reason); }
