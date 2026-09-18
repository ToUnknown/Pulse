// One recorder for every Pulse shortcut, including already registered native keys.
export function displayShortcut(value, platform = "windows") {
  const labels = { control: "Ctrl", ctrl: "Ctrl", super: platform === "macos" ? "⌘" : "Win", meta: platform === "macos" ? "⌘" : "Win", shift: "Shift", alt: platform === "macos" ? "Option" : "Alt" };
  return value.split("+").map(part => labels[part.toLowerCase()] || part.replace(/^Key(?=[A-Z]$)/, "").replace(/^Digit(?=\d$)/, "")).join(" + ");
}
export function shortcutFromEvent(event) {
  if (event.key === "Escape" || /^(Control|Shift|Alt|Meta)/.test(event.code)) return null;
  if (!event.ctrlKey && !event.altKey && !event.metaKey) throw new Error("Include Ctrl, Alt, or Command / Windows in your shortcut.");
  if (!/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Space|Arrow(Up|Down|Left|Right))$/.test(event.code)) throw new Error("Use a letter, number, function key, arrow, or Space.");
  return [event.ctrlKey && "Control", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super", event.code].filter(Boolean).join("+");
}
export function attachShortcutRecorder({ buttons, value, platform, accept, error, changed = () => {} }) {
  const invoke = window.__TAURI__.core.invoke;
  let recording;
  function stop() {
    if (recording) invoke("record_text_extractor_shortcut", { recording: false }).catch(error);
    recording = null; changed(false);
    for (const [field, button] of Object.entries(buttons)) {
      button.setAttribute("aria-pressed", "false");
      if (value(field)) button.textContent = displayShortcut(value(field), platform());
    }
  }
  function receive(combination) {
    if (!recording) return;
    const field = recording; stop(); accept(field, combination);
  }
  window.addEventListener("pulse-shortcut-recording-start", stop);
  for (const [field, button] of Object.entries(buttons)) {
    button.addEventListener("click", () => {
      window.dispatchEvent(new Event("pulse-shortcut-recording-start"));
      button.focus(); recording = field; changed(true);
      button.textContent = "Press a combination…"; button.setAttribute("aria-pressed", "true");
      invoke("record_text_extractor_shortcut", { recording: true }).catch(reason => { stop(); error(reason); });
    });
    button.addEventListener("blur", stop);
    button.addEventListener("keydown", event => {
      if (recording !== field) return;
      event.preventDefault(); event.stopPropagation();
      if (event.key === "Escape") { stop(); return; }
      try { const key = shortcutFromEvent(event); if (key) receive(key); } catch (reason) { error(reason.message); }
    });
  }
  window.addEventListener("pulse-extractor-shortcut", event => { if (typeof event.detail === "string") receive(event.detail); });
  window.addEventListener("blur", stop);
  return { stop };
}
