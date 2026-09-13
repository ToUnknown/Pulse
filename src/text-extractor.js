import { physicalCrop, selectionRect } from "./extraction-geometry.js";

const invoke = window.__TAURI__.core.invoke;
const $ = (selector) => document.querySelector(selector);
const screen = $("#screen");
const surface = $("#selection-surface");
const selection = $("#selection");
const result = $("#result");
const frame = $("#capture-frame");
const cropImage = $("#crop-image");
const editor = $("#extracted-text");
const status = $("#status");
const errorMessage = $("#extraction-error");
const errorRow = $(".error-row");
const copy = $("#copy");
const translate = $("#translate");
const languages = $("#languages");
const retry = $("#retry");
const reading = $("#reading");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let capture;
let start;
let pointerId;
let outsidePointer;
let rect;
let crop;
let generation = 0;
let closing = false;
let shown = false;
let busy = false;
let retryAction;

function phase(value) { document.body.dataset.phase = value; }
function updateActions() {
  const disabled = busy || closing || !editor.value.trim();
  copy.disabled = disabled;
  translate.disabled = disabled;
}
function hideLanguages() { languages.hidden = true; translate.setAttribute("aria-expanded", "false"); }
function clearError() { errorRow.hidden = true; retry.hidden = true; retryAction = null; }
function setError(error, action) {
  busy = false;
  phase("error");
  reading.hidden = true;
  editor.disabled = false;
  editor.readOnly = false;
  editor.setAttribute("aria-busy", "false");
  status.textContent = "Request failed";
  errorMessage.textContent = String(error);
  errorRow.hidden = false;
  retryAction = action;
  retry.hidden = !action;
  updateActions();
}

async function dismiss() {
  if (closing) return;
  closing = true;
  generation += 1;
  hideLanguages();
  updateActions();
  document.body.dataset.closing = "true";
  if (!reducedMotion.matches) await new Promise((resolve) => setTimeout(resolve, 180));
  try { await invoke("close_text_extractor"); }
  catch (error) {
    closing = false;
    delete document.body.dataset.closing;
    surface.hidden = true;
    result.hidden = false;
    setError(error);
  }
}

function updateSelection(event) {
  rect = selectionRect(start, { x: event.clientX, y: event.clientY }, innerWidth, innerHeight);
  Object.assign(selection.style, { left: `${rect.x}px`, top: `${rect.y}px`, width: `${rect.width}px`, height: `${rect.height}px` });
}

surface.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || !capture || closing || pointerId !== undefined) return;
  start = { x: event.clientX, y: event.clientY };
  pointerId = event.pointerId;
  surface.setPointerCapture(pointerId);
  selection.hidden = false;
  updateSelection(event);
});
surface.addEventListener("pointermove", (event) => { if (event.pointerId === pointerId) updateSelection(event); });
surface.addEventListener("pointercancel", () => { pointerId = undefined; start = null; selection.hidden = true; });
surface.addEventListener("pointerup", async (event) => {
  if (event.pointerId !== pointerId) return;
  updateSelection(event);
  surface.releasePointerCapture(pointerId);
  pointerId = undefined;
  start = null;
  if (rect.width < 4 || rect.height < 4) { selection.hidden = true; return; }
  crop = physicalCrop(rect, { width: innerWidth, height: innerHeight }, capture);
  try {
    const canvas = document.createElement("canvas");
    canvas.width = crop.width;
    canvas.height = crop.height;
    canvas.getContext("2d").drawImage(screen, crop.x, crop.y, crop.width, crop.height, 0, 0, crop.width, crop.height);
    cropImage.src = canvas.toDataURL("image/png");
    await cropImage.decode();
    if (closing) return;
    surface.hidden = true;
    result.hidden = false;
    frame.hidden = false;
    await runExtraction(true);
  } catch (error) {
    if (closing) return;
    surface.hidden = true;
    result.hidden = false;
    frame.hidden = true;
    setError(error);
  }
});

function revealText(text) {
  busy = false;
  phase("ready");
  reading.hidden = true;
  editor.disabled = false;
  editor.readOnly = false;
  editor.setAttribute("aria-busy", "false");
  editor.value = text;
  status.textContent = text ? "Text ready to edit" : "No text found";
  updateActions();
  if (!reducedMotion.matches) editor.animate([{ opacity: 0, translate: "0 4px" }, { opacity: 1, translate: "0 0" }], { duration: 280, easing: "ease-out" });
  editor.focus({ preventScroll: true });
}

async function runExtraction(animate = false) {
  if (busy || closing) return;
  const current = ++generation;
  busy = true;
  phase("scanning");
  clearError();
  hideLanguages();
  editor.disabled = true;
  editor.value = "";
  editor.setAttribute("aria-busy", "true");
  reading.hidden = false;
  status.textContent = "Reading selection";
  updateActions();
  result.focus({ preventScroll: true });
  if (animate && !reducedMotion.matches) {
    const target = frame.getBoundingClientRect();
    frame.animate([
      { transform: `translate(${rect.x - target.x}px, ${rect.y - target.y}px) scale(${rect.width / target.width}, ${rect.height / target.height})`, borderRadius: "2px" },
      { transform: "none", borderRadius: getComputedStyle(frame).borderRadius },
    ], { duration: 600, easing: "cubic-bezier(.2,.8,.2,1)" });
    for (const element of [$(".text-panel"), $(".result-actions")]) {
      element.animate([{ opacity: 0, translate: "0 14px" }, { opacity: 1, translate: "0 0" }], { duration: 400, delay: 240, easing: "cubic-bezier(.2,.8,.2,1)", fill: "backwards" });
    }
  }
  try {
    const text = await invoke("extract_screen_text", { crop });
    if (current !== generation || closing) return;
    revealText(text);
  } catch (error) { if (current === generation && !closing) setError(error, () => runExtraction()); }
}

async function translateText(language) {
  if (busy || closing || !editor.value.trim()) return;
  const current = ++generation;
  busy = true;
  hideLanguages();
  clearError();
  phase("translating");
  editor.readOnly = true;
  editor.setAttribute("aria-busy", "true");
  status.textContent = "Translating text";
  updateActions();
  editor.focus({ preventScroll: true });
  try {
    const text = await invoke("translate_extracted_text", { text: editor.value, language });
    if (current !== generation || closing) return;
    if (!text.trim()) throw "No translation was returned. Your text is unchanged.";
    revealText(text);
  } catch (error) { if (current === generation && !closing) setError(error, () => translateText(language)); }
}

copy.addEventListener("click", async () => {
  if (busy || closing) return;
  busy = true;
  hideLanguages();
  updateActions();
  try { await invoke("copy_extracted_text", { text: editor.value }); await dismiss(); }
  catch (error) { if (!closing) setError(error); }
});
editor.addEventListener("input", updateActions);
retry.addEventListener("click", () => retryAction?.());
translate.addEventListener("click", () => {
  if (!languages.hidden) { hideLanguages(); return; }
  languages.hidden = false;
  translate.setAttribute("aria-expanded", "true");
  const preferred = navigator.language.split("-")[0];
  const items = [...languages.querySelectorAll("button")];
  (items.find((item) => item.dataset.language === preferred) || items[0]).focus({ preventScroll: true });
});
languages.addEventListener("click", (event) => {
  const item = event.target.closest("[data-language]");
  if (item) translateText(item.dataset.language);
});
languages.addEventListener("keydown", (event) => {
  const items = [...languages.querySelectorAll("button")];
  const index = items.indexOf(document.activeElement);
  let next;
  if (event.key === "ArrowDown") next = (index + 1) % items.length;
  if (event.key === "ArrowUp") next = (index - 1 + items.length) % items.length;
  if (event.key === "Home") next = 0;
  if (event.key === "End") next = items.length - 1;
  if (next !== undefined) { event.preventDefault(); items[next].focus(); }
});
// Require the click to start and end outside, so selecting text beyond the editor
// or releasing the initial screenshot drag cannot accidentally dismiss the window.
document.addEventListener("pointerdown", (event) => {
  outsidePointer = event.button === 0 && !result.hidden && !result.contains(event.target) ? event.pointerId : undefined;
  if (!event.target.closest(".translate-control")) hideLanguages();
});
document.addEventListener("pointerup", (event) => {
  if (outsidePointer === event.pointerId && !result.contains(event.target)) dismiss();
  outsidePointer = undefined;
});
document.addEventListener("pointercancel", () => { outsidePointer = undefined; });
document.addEventListener("contextmenu", (event) => { if (event.target !== editor) event.preventDefault(); });
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { event.preventDefault(); dismiss(); }
  if (event.key === "Tab" && !result.hidden) {
    if (!languages.hidden) { hideLanguages(); translate.focus(); }
    const controls = [...result.querySelectorAll("button:not(:disabled), textarea:not(:disabled)")].filter((el) => el.getClientRects().length);
    const index = controls.indexOf(document.activeElement);
    if (!controls.length || index === -1 || (event.shiftKey && index === 0) || (!event.shiftKey && index === controls.length - 1)) {
      event.preventDefault();
      (event.shiftKey ? controls.at(-1) : controls[0])?.focus();
    }
  }
});
// A display change invalidates pixel-to-screen mapping; never send a misaligned crop.
window.addEventListener("resize", () => { if (shown && document.body.dataset.phase === "selecting") dismiss(); });

try {
  capture = await invoke("text_extractor_capture");
  if (!closing) {
    screen.src = capture.imageUrl;
    await screen.decode();
    if (!closing) {
      phase("selecting");
      await invoke("text_extractor_show");
      shown = true;
    }
  }
} catch (error) {
  if (!closing) {
    surface.hidden = true;
    result.hidden = false;
    frame.hidden = true;
    setError(error);
    await invoke("text_extractor_show").catch(() => dismiss());
  }
}
