const invoke = window.__TAURI__?.core.invoke;
const section = document.querySelector("#dictation-section");
const enabled = document.querySelector("#dictation-enabled");
const error = document.querySelector("#dictation-error");
const microphone = document.querySelector("#dictation-microphone");
const accessibility = document.querySelector("#dictation-accessibility");
let busy = false, revision = 0, accessState, actionError = "", configured = false;
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
    enabled.checked = state.enabled && state.apiKeyConfigured;
    document.querySelector("#dictation-description").textContent = state.apiKeyConfigured
      ? `Turn your voice into text. Tap ${state.shortcut} to start or stop, or hold it while speaking.`
      : "Add your OpenAI API key below to enable Dictation.";
    microphone.hidden = state.microphone;
    accessibility.hidden = state.accessibility;
    microphone.disabled = accessibility.disabled = false;
    showError(state.apiKeyConfigured ? actionError || state.error || "" : "");
  } catch { /* The backend may still be starting. */ }
}
async function act(command, args) {
  if (!invoke || busy) return;
  busy = true; revision++; actionError = "";
  enabled.disabled = microphone.disabled = accessibility.disabled = true;
  showError("");
  try { await invoke(command, args); }
  catch (reason) { actionError = String(reason); showError(actionError); }
  finally {
    busy = false;
    enabled.disabled = !configured;
    microphone.disabled = accessibility.disabled = false;
    await refresh();
  }
}
enabled.addEventListener("change", () => act("set_dictation_enabled", {enabled:enabled.checked}));
microphone.addEventListener("click", () => act("request_dictation_access", {microphone:true}));
accessibility.addEventListener("click", () => act("request_dictation_access", {microphone:false}));
refresh();
setInterval(()=>{if(!document.hidden) refresh();},2000);

window.addEventListener("pulse-api-key-changed", refresh);
window.addEventListener("focus", refresh);
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
