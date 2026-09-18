import { featureDisclosure } from "./feature-disclosure.js";
import { attachShortcutRecorder, displayShortcut } from "./shortcut-recorder.js";
const invoke = window.__TAURI__.core.invoke;
const section = document.querySelector("#audio-assistant-section");
const listening = document.querySelector("#audio-listening");
const buttons = { listenShortcut: document.querySelector("#audio-listen-shortcut"), answerShortcut: document.querySelector("#audio-answer-shortcut") };
const choose = document.querySelector("#audio-context-choose");
const clear = document.querySelector("#audio-context-clear");
const errorNode = document.querySelector("#audio-settings-error");
const disclosure = featureDisclosure(document.querySelector("#audio-assistant-details"), document.querySelector("#audio-disclosure"), "Audio assistant");
let state, busy = false, recording = false, loadGeneration = 0;
function error(reason) { errorNode.textContent = String(reason); errorNode.hidden = false; document.querySelector("#tab-advanced").click(); }
async function load() {
  if (section.hidden) return;
  const generation = ++loadGeneration;
  const next = await invoke("audio_assistant_state");
  if (generation !== loadGeneration) return;
  state = next;
  listening.checked = state.listening;
  disclosure.enabled(state.listening);
  if (!recording) for (const [field, button] of Object.entries(buttons)) button.textContent = displayShortcut(state[field]);
  document.querySelector("#audio-context-name").textContent = state.contextFile?.split(/[\\/]/).pop() || "No file selected";
  clear.hidden = !state.contextFile;
  document.querySelector("#audio-provider-status").textContent = `Answers: ${state.answerProvider}. Uses the Advanced access selection below.`;
  errorNode.hidden = !state.error; errorNode.textContent = state.error || "";
  if (state.error) document.querySelector("#tab-advanced").click();
}
async function change(command, args) {
  if (busy) return;
  busy = true; [listening, choose, clear, ...Object.values(buttons)].forEach(el => { el.disabled = true; });
  try { await invoke(command, args); await load(); }
  catch (reason) { await load(); error(reason); }
  finally { busy = false; [listening, choose, clear, ...Object.values(buttons)].forEach(el => { el.disabled = false; }); }
}
listening.addEventListener("change", () => change("set_audio_listening", { listening: listening.checked }));
choose.addEventListener("click", () => change("audio_context_file", { clear: false }));
clear.addEventListener("click", () => change("audio_context_file", { clear: true }));
attachShortcutRecorder({ buttons, value: field => state?.[field], platform: () => "windows", error,
  changed: value => { recording = value; },
  accept: (field, combination) => change("set_audio_shortcuts", {
    listenShortcut: field === "listenShortcut" ? combination : state.listenShortcut,
    answerShortcut: field === "answerShortcut" ? combination : state.answerShortcut
  })
});
for (const event of ["pulse-audio-assistant-changed", "pulse-answer-provider-changed", "focus"]) window.addEventListener(event, () => load().catch(error));
if ((await invoke("settings_state")).platform === "windows") { section.hidden = false; await load(); }
