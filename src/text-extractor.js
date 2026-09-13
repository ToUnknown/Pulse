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
const copy = $("#copy");
const retry = $("#retry");
const reselect = $("#reselect");
const reading = $("#reading");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let capture;
let start;
let pointerId;
let rect;
let crop;
let generation = 0;
let closing = false;
let shown = false;

function phase(value) { document.body.dataset.phase = value; }
function setError(error) {
  phase("error");
  reading.hidden = true;
  editor.disabled = false;
  status.textContent = "Something went wrong";
  errorMessage.textContent = String(error);
  errorMessage.hidden = false;
  retry.hidden = !crop;
  reselect.disabled = !capture;
  $("#edit-hint").textContent = "Try again, or select another area.";
}

async function dismiss() {
  if (closing) return;
  closing = true;
  generation += 1;
  document.body.dataset.closing = "true";
  if (!reducedMotion.matches) await new Promise((resolve) => setTimeout(resolve, 150));
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
  const pixels = physicalCrop(rect, { width: innerWidth, height: innerHeight }, capture);
  $("#dimensions").textContent = `${pixels.width} × ${pixels.height}`;
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
});

async function runExtraction(animate = false) {
  const current = ++generation;
  phase("scanning");
  errorMessage.hidden = true;
  retry.hidden = true;
  reselect.disabled = true;
  copy.disabled = true;
  editor.disabled = true;
  editor.value = "";
  reading.hidden = false;
  status.textContent = "Reading your selection…";
  $("#edit-hint").textContent = "A little clarity, in a moment.";
  $("#done").focus({ preventScroll: true });
  if (animate && !reducedMotion.matches) {
    const target = frame.getBoundingClientRect();
    frame.animate([
      { transform: `translate(${rect.x - target.x}px, ${rect.y - target.y}px) scale(${rect.width / target.width}, ${rect.height / target.height})`, borderRadius: "2px" },
      { transform: "none", borderRadius: "12px" },
    ], { duration: 560, easing: "cubic-bezier(.2,.8,.2,1)" });
    for (const element of [$(".text-panel"), $(".result-heading"), $(".result-hint")]) {
      element.animate([{ opacity: 0, translate: "0 12px" }, { opacity: 1, translate: "0 0" }], { duration: 350, delay: 220, fill: "backwards" });
    }
  }
  try {
    const text = await invoke("extract_screen_text", { crop });
    if (current !== generation || closing) return;
    phase("ready");
    reading.hidden = true;
    editor.disabled = false;
    editor.value = text;
    editor.placeholder = text ? "" : "No readable text found. Try another area, or type here.";
    reselect.disabled = false;
    status.textContent = text ? "Ready to edit" : "No text found";
    $("#edit-hint").textContent = "Make it yours. Edit, then copy.";
    copy.disabled = !text.trim();
    editor.focus({ preventScroll: true });
  } catch (error) { if (current === generation && !closing) setError(error); }
}

copy.addEventListener("click", async () => {
  copy.disabled = true;
  try { await invoke("copy_extracted_text", { text: editor.value }); await dismiss(); }
  catch (error) { errorMessage.textContent = String(error); errorMessage.hidden = false; copy.disabled = !editor.value.trim(); }
});
editor.addEventListener("input", () => { copy.disabled = !editor.value.trim(); });
retry.addEventListener("click", () => runExtraction());
reselect.addEventListener("click", () => {
  generation += 1;
  result.hidden = true;
  surface.hidden = false;
  selection.hidden = true;
  crop = null;
  rect = null;
  phase("selecting");
});
$("#close").addEventListener("click", dismiss);
$("#done").addEventListener("click", dismiss);
document.addEventListener("contextmenu", (event) => { if (event.target !== editor) event.preventDefault(); });
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { event.preventDefault(); dismiss(); }
  if (event.key === "Tab" && !result.hidden) {
    const controls = [...result.querySelectorAll("button:not(:disabled), textarea:not(:disabled)")].filter((el) => !el.hidden);
    if (event.shiftKey && document.activeElement === controls[0]) { event.preventDefault(); controls.at(-1)?.focus(); }
    else if (!event.shiftKey && document.activeElement === controls.at(-1)) { event.preventDefault(); controls[0]?.focus(); }
  }
});
// A display change invalidates pixel-to-screen mapping; never send a misaligned crop.
window.addEventListener("resize", () => { if (shown && document.body.dataset.phase === "selecting") dismiss(); });

try {
  capture = await invoke("text_extractor_capture");
  screen.src = capture.imageUrl;
  await screen.decode();
  phase("selecting");
  await invoke("text_extractor_show");
  shown = true;
} catch (error) {
  surface.hidden = true;
  result.hidden = false;
  frame.hidden = true;
  setError(error);
  await invoke("text_extractor_show").catch(() => dismiss());
}
