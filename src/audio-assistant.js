const invoke = window.__TAURI__.core.invoke;
const answer = document.querySelector("#answer");
let generation = 0;
async function render() {
  const current = ++generation;
  const payload = await invoke("audio_answer_ready");
  if (current !== generation || !payload.text) return;
  answer.classList.remove("reveal"); answer.textContent = payload.text;
  answer.setAttribute("dir", "auto");
  await new Promise(requestAnimationFrame);
  if (current !== generation) return;
  answer.classList.add("reveal");
  await invoke("show_audio_answer", { id: payload.id, height: document.body.scrollHeight });
}
window.addEventListener("pulse-answer", () => render().catch(() => {}));
window.addEventListener("pulse-answer-clear", () => { generation++; answer.textContent = ""; });
document.addEventListener("keydown", event => { if (event.key === "Escape") invoke("dismiss_audio_answer").catch(() => {}); });
render().catch(() => {});
