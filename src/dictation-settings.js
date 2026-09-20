const invoke = window.__TAURI__?.core.invoke;
const section = document.querySelector("#dictation-section");
const enabled = document.querySelector("#dictation-enabled");
const error = document.querySelector("#dictation-error");
let busy = false;
async function refresh() {
  if (!invoke || busy) return;
  try {
    const state = await invoke("dictation_settings");
    if (busy) return;
    section.hidden = false;
    enabled.disabled = !state.apiKeyConfigured;
    enabled.checked = state.enabled && state.apiKeyConfigured;
    document.querySelector("#dictation-description").textContent = state.apiKeyConfigured
      ? `Turn your voice into text. Tap ${state.shortcut} to start or stop, or hold it while speaking.`
      : "Add your OpenAI API key below to enable Dictation.";
    document.querySelector("#dictation-microphone").hidden = state.microphone;
    document.querySelector("#dictation-accessibility").hidden = state.accessibility;
    if (state.error && state.apiKeyConfigured) { error.textContent=state.error; error.hidden=false; }
    else if (!state.apiKeyConfigured) error.hidden=true;
  } catch { /* The backend may still be starting. */ }
}
enabled.addEventListener("change", async () => {
  busy=true; enabled.disabled=true; error.hidden=true;
  try { await invoke("set_dictation_enabled",{enabled:enabled.checked}); }
  catch(reason) { error.textContent=String(reason); error.hidden=false; }
  finally { busy=false; await refresh(); }
});
for (const [id,microphone] of [["dictation-microphone",true],["dictation-accessibility",false]]) {
  document.getElementById(id).addEventListener("click",async()=>{
    try { await invoke("request_dictation_access",{microphone}); await refresh(); }
    catch(reason) { error.textContent=String(reason); error.hidden=false; }
  });
}
refresh();
setInterval(()=>{if(!document.hidden) refresh();},2000);

window.addEventListener("pulse-api-key-changed", refresh);
window.addEventListener("focus", refresh);
