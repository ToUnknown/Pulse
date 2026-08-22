const invoke = window.__TAURI__.core.invoke;
document.querySelector("#close").addEventListener("click", async () => {
  await invoke("set_pip_enabled", { enabled: false });
});
// Controls are enabled when the native browser-media monitor supplies a session.
for (const button of document.querySelectorAll("[data-action]")) button.disabled = true;
