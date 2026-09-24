const invoke = window.__TAURI__?.core.invoke;
const section = document.querySelector("#dictation-section");
const enabled = document.querySelector("#dictation-enabled");
const modeDetails = document.querySelector("#dictation-modes");
const mode = document.querySelector("#dictation-mode");
const modeButtons = [...mode.querySelectorAll("button")];
const error = document.querySelector("#dictation-error");
const microphone = document.querySelector("#dictation-microphone");
const accessibility = document.querySelector("#dictation-accessibility");
let busy = false, revision = 0, accessState, actionError = "", configured = false;
let selectedMode = "default";
function setModeExpanded(value) {
  if (modeDetails.dataset.expanded !== String(value)) modeDetails.dataset.expanded = String(value);
  if (modeDetails.inert !== !value) modeDetails.inert = !value;
}
function renderMode(value) {
  selectedMode = value === "live" ? "live" : "default";
  mode.style.setProperty("--mode-index", selectedMode === "live" ? 1 : 0);
  for (const button of modeButtons) {
    const selected = button.dataset.mode === selectedMode;
    button.setAttribute("aria-checked", String(selected));
    button.tabIndex = selected ? 0 : -1;
  }
}
function showError(message) {
  if (error.textContent !== message) error.textContent = message;
  error.hidden = !message;
}
async function refresh() {
  if (!invoke || busy) return;
  const request = ++revision;
  try {
    const state = await invoke("dictation_settings");
    if (busy || request !== revision) return;
    const currentAccess = JSON.stringify([state.apiKeyConfigured, state.microphone, state.accessibility, state.enabled]);
    if (currentAccess !== accessState) actionError = "";
    accessState = currentAccess;
    configured = state.apiKeyConfigured;
    section.hidden = false;
    enabled.disabled = !state.apiKeyConfigured;
    const checked = state.enabled && state.apiKeyConfigured;
    if (enabled.checked !== checked) enabled.checked = checked;
    setModeExpanded(checked);
    renderMode(state.mode);
    for (const button of modeButtons) button.disabled = false;
    const description = document.querySelector("#dictation-description");
    const copy = state.apiKeyConfigured
      ? `Turn your voice into text. Tap ${state.shortcut} to start or stop, or hold it while speaking.`
      : "Add your OpenAI API key below to enable Dictation.";
    if (description.textContent !== copy) description.textContent = copy;
    microphone.hidden = state.microphone;
    accessibility.hidden = state.accessibility;
    microphone.disabled = accessibility.disabled = false;
    showError(state.apiKeyConfigured ? actionError || state.error || "" : "");
  } catch { /* The backend may still be starting. */ }
}
async function act(command, args) {
  if (!invoke || busy) return;
  busy = true; revision++; actionError = "";
  section.dataset.savingMode = String(command === "set_dictation_mode" && configured);
  enabled.disabled = microphone.disabled = accessibility.disabled = true;
  for (const button of modeButtons) button.disabled = true;
  showError("");
  try { await invoke(command, args); }
  catch (reason) { actionError = String(reason); showError(actionError); }
  finally {
    busy = false;
    enabled.disabled = !configured;
    for (const button of modeButtons) button.disabled = false;
    microphone.disabled = accessibility.disabled = false;
    section.dataset.savingMode = "false";
    await refresh();
  }
}
enabled.addEventListener("change", () => {
  setModeExpanded(enabled.checked);
  return act("set_dictation_enabled", {enabled:enabled.checked});
});
function selectMode(button) {
  if (busy || !enabled.checked || !button || button.dataset.mode === selectedMode) return;
  renderMode(button.dataset.mode);
  return act("set_dictation_mode", {mode:selectedMode});
}
mode.addEventListener("click", event => selectMode(event.target.closest("button")));
mode.addEventListener("keydown", event => {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  if (busy) return;
  const next = ["ArrowLeft", "Home"].includes(event.key) ? "default" : "live";
  const button = modeButtons.find(button => button.dataset.mode === next);
  button.focus();
  selectMode(button);
});
microphone.addEventListener("click", () => act("request_dictation_access", {microphone:true}));
accessibility.addEventListener("click", () => act("request_dictation_access", {microphone:false}));
refresh();
setInterval(()=>{if(!document.hidden) refresh();},2000);

window.addEventListener("pulse-api-key-changed", refresh);
window.addEventListener("focus", refresh);
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
