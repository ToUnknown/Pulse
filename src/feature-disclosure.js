// Enabled state expands once; subsequent refreshes preserve the user's choice.
export function featureDisclosure(panel, button, name) {
  let enabled;
  function expand(value) {
    panel.dataset.expanded = String(value);
    panel.inert = !value;
    panel.setAttribute("aria-hidden", String(!value));
    button.setAttribute("aria-expanded", String(value));
    button.setAttribute("aria-label", `${value ? "Collapse" : "Expand"} ${name} settings`);
    button.title = `${value ? "Collapse" : "Expand"} settings`;
  }
  button.addEventListener("click", () => expand(panel.dataset.expanded !== "true"));
  expand(false);
  return { enabled(value) { if (value !== enabled) { enabled = value; expand(value); } } };
}
