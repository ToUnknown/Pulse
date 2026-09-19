const invoke = window.__TAURI__?.core.invoke;
const section = document.querySelector("#dictation-section");
const enabled = document.querySelector("#dictation-enabled");
const error = document.querySelector("#dictation-error");
let busy = false;
async function refresh() {
  if (!invoke || busy) return;
  try {
    const state = await invoke("dictation_settings");
    section.hidden = false;
    enabled.checked = state.enabled;
    document.querySelector("#dictation-copy-last").hidden = !state.hasLastTranscript;
    document.querySelector("#dictation-microphone").hidden = state.microphone;
    document.querySelector("#dictation-accessibility").hidden = state.accessibility;
    document.querySelector("#dictation-key-note").textContent = state.apiKeyConfigured
      ? "Uses your saved OpenAI API key. Audio is sent to OpenAI only while dictating."
      : "Add your OpenAI API key below to use Dictation.";
    if (state.error) { error.textContent=state.error; error.hidden=false; }
    document.documentElement.dataset.dictationAvailable = "true";
    // Dictation needs an API key even when extraction uses the Codex provider.
    document.querySelector("#openai-key-form").hidden = false;
  } catch { /* Dictation is currently a macOS feature. */ }
}
enabled.addEventListener("change", async () => {
  busy=true; enabled.disabled=true; error.hidden=true;
  try { await invoke("set_dictation_enabled",{enabled:enabled.checked}); }
  catch(reason) { error.textContent=String(reason); error.hidden=false; }
  finally { busy=false; enabled.disabled=false; await refresh(); }
});
for (const [id,microphone] of [["dictation-microphone",true],["dictation-accessibility",false]]) {
  document.getElementById(id).addEventListener("click",async()=>{
    try { await invoke("request_dictation_access",{microphone}); await refresh(); }
    catch(reason) { error.textContent=String(reason); error.hidden=false; }
  });
}
refresh();
setInterval(()=>{if(!document.hidden) refresh();},2000);

document.querySelector("#dictation-copy-last").addEventListener("click", async (event) => {
  try { await invoke("copy_last_dictation"); event.target.textContent = "Copied"; setTimeout(() => { event.target.textContent = "Copy last transcript"; }, 1500); }
  catch(reason) { error.textContent=String(reason); error.hidden=false; }
});
