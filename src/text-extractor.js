import { physicalCrop, selectionRect } from "./extraction-geometry.js";

const invoke = window.__TAURI__.core.invoke;
const $ = (selector) => document.querySelector(selector);
const screen = $("#screen");
const surface = $("#selection-surface");
const selection = $("#selection");
const result = $("#result");
const details = $("#result-details");
const frame = $("#capture-frame");
const stage = $("#capture-stage");
const modeControl = $("#extraction-mode");
const modeButtons = [...modeControl.querySelectorAll("button")];
const translateControl = $(".translate-control");
const cropImage = $("#crop-image");
const editor = $("#extracted-text");
const status = $("#status");
const errorMessage = $("#extraction-error");
const errorRow = $(".error-row");
const copy = $("#copy");
const translate = $("#translate");
const languages = $("#languages");
const retry = $("#retry");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
const motions = new Set();
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
let mode = "basic";
let advancedAvailable = false;
const drafts = { basic: null, advanced: null };

function phase(value) { document.body.dataset.phase = value; }
function motion(element, keyframes, options) {
  const animation = element.animate(keyframes, options);
  motions.add(animation);
  animation.finished.then(() => { motions.delete(animation); animation.cancel(); }, () => motions.delete(animation));
  return animation.finished.catch(() => {});
}
function cancelMotions() { for (const animation of motions) animation.cancel(); }
reducedMotion.addEventListener("change", () => { if (reducedMotion.matches) cancelMotions(); });

function moveStage(origin, duration = 580) {
  if (!origin || reducedMotion.matches) return Promise.resolve();
  stage.style.transform = "none";
  const base = stage.getBoundingClientRect();
  stage.style.transform = "";
  const targetTransform = getComputedStyle(stage).transform;
  const x = origin.x + origin.width / 2 - base.x - base.width / 2;
  const y = origin.y + origin.height / 2 - base.y - base.height / 2;
  return motion(stage, [
    { transform: `translate(${x}px, ${y}px) scale(${origin.width / base.width})` },
    { transform: targetTransform },
  ], { duration, easing: "cubic-bezier(.2,.8,.2,1)" });
}

async function showDetails(current, finalPhase, origin = stage.getBoundingClientRect()) {
  const opening = details.hidden;
  details.hidden = false;
  document.body.dataset.zoomed = "false";
  if (opening && !reducedMotion.matches) {
    details.inert = true;
    phase("revealing");
    await Promise.all([
      moveStage(origin),
      motion(details, [
        { opacity: 0, transform: "translateY(24px)" },
        { opacity: 1, transform: "none" },
      ], { duration: 440, delay: 140, easing: "cubic-bezier(.2,.8,.2,1)", fill: "backwards" }),
    ]);
  }
  if (closing || current !== generation) return;
  details.inert = false;
  phase(finalPhase);
  editor.focus({ preventScroll: true });
}

function renderMode() {
  modeControl.dataset.mode = mode;
  modeButtons.forEach((button) => {
    const selected = button.dataset.mode === mode;
    button.setAttribute("aria-checked", String(selected));
    button.tabIndex = selected ? 0 : -1;
  });
}
function setCapabilities(available) {
  advancedAvailable = available;
  modeControl.hidden = !available;
  translateControl.hidden = !available;
  if (!available) {
    hideLanguages();
    if (mode === "advanced") switchMode("basic");
  }
}
async function switchMode(next) {
  if (closing || !crop || next === mode || (next === "advanced" && !advancedAvailable)) return;
  mode = next;
  renderMode();
  await runExtraction();
}
modeButtons.forEach((button) => button.addEventListener("click", () => switchMode(button.dataset.mode)));
modeControl.addEventListener("keydown", (event) => {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const next = event.key === "Home" ? "basic" : event.key === "End" ? "advanced" : mode === "basic" ? "advanced" : "basic";
  modeButtons.find((button) => button.dataset.mode === next).focus();
  switchMode(next);
});

function updateActions() {
  const disabled = busy || closing || !editor.value.trim();
  copy.disabled = disabled;
  translate.disabled = disabled || !advancedAvailable;
}
function hideLanguages() { languages.hidden = true; translate.setAttribute("aria-expanded", "false"); }
function clearError() { errorRow.hidden = true; retry.hidden = true; retryAction = null; }
async function setError(error, action) {
  const current = generation;
  busy = false;
  editor.disabled = false;
  editor.readOnly = false;
  editor.setAttribute("aria-busy", "false");
  status.textContent = "Request failed";
  errorMessage.textContent = String(error);
  errorRow.hidden = false;
  retryAction = action;
  retry.hidden = !action;
  updateActions();
  await showDetails(current, "error");
}

async function dismiss() {
  if (closing) return;
  closing = true;
  generation += 1;
  hideLanguages();
  updateActions();
  document.body.dataset.closing = "true";
  cancelMotions();
  if (!reducedMotion.matches) await new Promise((resolve) => setTimeout(resolve, 180));
  try { await invoke("close_text_extractor"); }
  catch (error) {
    closing = false;
    delete document.body.dataset.closing;
    surface.hidden = true;
    result.hidden = false;
    await setError(error);
  }
}

function updateSelection(event) {
  rect = selectionRect(start, { x: event.clientX, y: event.clientY }, innerWidth, innerHeight);
  Object.assign(selection.style, { left: `${rect.x}px`, top: `${rect.y}px`, width: `${rect.width}px`, height: `${rect.height}px` });
}

surface.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || !capture || closing || pointerId !== undefined || document.body.dataset.phase !== "selecting") return;
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
  if (capture.mode === "quick") {
    phase("copying");
    surface.hidden = true;
    closing = true;
    // Native local OCR writes the clipboard and closes this window. No result UI.
    await invoke("text_extractor_quick_copy", { crop }).catch(() => invoke("close_text_extractor").catch(() => {}));
    return;
  }
  phase("capturing");
  try {
    const images = await invoke("text_extractor_capture_selection", { crop });
    if (closing) return;
    cropImage.src = images.imageUrl;
    screen.src = images.backdropUrl;
    await Promise.all([cropImage.decode(), screen.decode()]);
    if (closing) return;
    surface.hidden = true;
    screen.hidden = false;
    result.hidden = false;
    frame.hidden = false;
    await runExtraction(true);
  } catch (error) {
    if (closing) return;
    surface.hidden = true;
    result.hidden = false;
    stage.hidden = true;
    await setError(error);
  }
});

async function revealText(text, current) {
  busy = false;
  editor.disabled = false;
  editor.readOnly = false;
  editor.setAttribute("aria-busy", "false");
  editor.value = text;
  drafts[mode] = text;
  status.textContent = text ? "Text ready to edit" : "No text found";
  updateActions();
  if (!details.hidden && !reducedMotion.matches) {
    motion(editor, [{ opacity: 0, translate: "0 4px" }, { opacity: 1, translate: "0 0" }], { duration: 280, easing: "ease-out" });
  }
  await showDetails(current, "ready");
}

async function finishShimmerCycle() {
  if (reducedMotion.matches) return;
  const animation = frame.getAnimations({ subtree: true })
    .find((animation) => animation.animationName === "screenshot-shimmer");
  if (!animation?.effect) return;
  const { currentIteration } = animation.effect.getComputedTiming();
  // End at the right edge of this pass, even if the API returns halfway through.
  // Keeping the same animation avoids a jump or an extra pass on a quick response.
  animation.effect.updateTiming({ iterations: (currentIteration ?? 0) + 1 });
  await animation.finished.catch(() => {});
}

async function runExtraction(initial = false, force = false) {
  if (closing) return;
  const current = ++generation;
  const origin = stage.getBoundingClientRect();
  cancelMotions();
  busy = true;
  clearError();
  hideLanguages();
  editor.readOnly = false;
  editor.value = drafts[mode] ?? "";
  editor.setAttribute("aria-busy", "true");
  status.textContent = "Reading selection";
  updateActions();
  const cached = !force && drafts[mode] !== null;
  // One ordered request ID covers requests and cancellation, including quick toggles.
  const request = cached
    ? invoke("cancel_text_extraction", { requestId: current }).then(() => ({ text: drafts[mode] }), (error) => ({ error }))
    : invoke("extract_screen_text", { crop, mode, requestId: current }).then((text) => ({ text }), (error) => ({ error }));
  if (initial) {
    phase("flying");
    details.hidden = true;
    details.inert = true;
    modeControl.inert = true;
    editor.disabled = true;
    document.body.dataset.zoomed = "false";
    if (!reducedMotion.matches) {
      const target = frame.getBoundingClientRect();
      await motion(frame, [
        { transform: `translate(${rect.x - target.x}px, ${rect.y - target.y}px) scale(${rect.width / target.width}, ${rect.height / target.height})`, borderRadius: "0px", boxShadow: "0 0 0 transparent" },
        { transform: "none", borderRadius: getComputedStyle(frame).borderRadius, boxShadow: getComputedStyle(frame).boxShadow },
      ], { duration: 540, easing: "cubic-bezier(.2,.8,.2,1)" });
    }
    if (current !== generation || closing) return;
    modeControl.inert = false;
    phase("basic-reading");
  } else if (mode === "basic" || cached) {
    phase("basic-reading");
    editor.disabled = !cached;
    await showDetails(current, "basic-reading", origin);
  } else {
    phase("centering");
    result.focus({ preventScroll: true });
    editor.disabled = true;
    details.inert = true;
    if (!details.hidden && !reducedMotion.matches) {
      await motion(details, [{ opacity: 1 }, { opacity: 0, transform: "translateY(12px)" }], { duration: 160, easing: "ease-in", fill: "forwards" });
    }
    if (current !== generation || closing) return;
    details.hidden = true;
    document.body.dataset.zoomed = "true";
    await moveStage(origin, 600);
    if (current !== generation || closing) return;
    phase("scanning");
  }
  if (current !== generation || closing) return;
  const response = await request;
  if (current !== generation || closing) return;
  if (mode === "advanced" && !cached) await finishShimmerCycle();
  if (current !== generation || closing) return;
  if ("error" in response) await setError(response.error, () => runExtraction(false, true));
  else await revealText(response.text, current);
}

async function translateText(language) {
  if (busy || closing || !advancedAvailable || !editor.value.trim()) return;
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
    const text = await invoke("translate_extracted_text", { text: editor.value, language, requestId: current });
    if (current !== generation || closing) return;
    if (!text.trim()) throw "No translation was returned. Your text is unchanged.";
    await revealText(text, current);
  } catch (error) { if (current === generation && !closing) await setError(error, () => translateText(language)); }
}

copy.addEventListener("click", async () => {
  if (busy || closing) return;
  busy = true;
  hideLanguages();
  updateActions();
  try { await invoke("copy_extracted_text", { text: editor.value }); await dismiss(); }
  catch (error) { if (!closing) await setError(error); }
});
editor.addEventListener("input", () => { drafts[mode] = editor.value; updateActions(); });
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
function insideResult(target) { return (details.hidden ? stage : result).contains(target); }
document.addEventListener("pointerdown", (event) => {
  outsidePointer = event.button === 0 && !result.hidden && !insideResult(event.target) ? event.pointerId : undefined;
  if (!event.target.closest(".translate-control")) hideLanguages();
});
document.addEventListener("pointerup", (event) => {
  if (outsidePointer === event.pointerId && !insideResult(event.target)) dismiss();
  outsidePointer = undefined;
});
document.addEventListener("pointercancel", () => { outsidePointer = undefined; });
document.addEventListener("contextmenu", (event) => { if (event.target !== editor) event.preventDefault(); });
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { event.preventDefault(); dismiss(); }
  if (event.key === "Tab" && !result.hidden) {
    if (!languages.hidden) { hideLanguages(); translate.focus(); }
    const controls = [...result.querySelectorAll("button:not(:disabled), textarea:not(:disabled)")].filter((el) => el.getClientRects().length && el.tabIndex !== -1 && !el.closest("[inert]"));
    const index = controls.indexOf(document.activeElement);
    if (!controls.length || index === -1 || (event.shiftKey && index === 0) || (!event.shiftKey && index === controls.length - 1)) {
      event.preventDefault();
      (event.shiftKey ? controls.at(-1) : controls[0])?.focus();
    }
  }
});
window.addEventListener("focus", async () => {
  if (!shown || closing || capture?.mode === "quick") return;
  try { setCapabilities(await invoke("text_extractor_capabilities")); } catch { /* The capture may be closing. */ }
});
// A display change invalidates pixel-to-screen mapping; never send a misaligned crop.
window.addEventListener("resize", () => { if (shown && ["selecting", "capturing"].includes(document.body.dataset.phase)) dismiss(); });

async function beginCapture() {
  if (capture || closing) return;
  try {
    capture = await invoke("text_extractor_capture");
    if (closing) return;
    phase("selecting");
    await invoke("text_extractor_show");
    // Hidden webviews may suspend animation frames; only wait after showing.
    await new Promise(requestAnimationFrame);
    shown = true;
    if (capture.mode !== "quick") {
      invoke("text_extractor_capabilities").then((available) => { if (!closing) setCapabilities(available); }).catch(() => {});
    }
  } catch (error) {
    if (!closing) {
      surface.hidden = true;
      result.hidden = false;
      stage.hidden = true;
      await setError(error);
      await invoke("text_extractor_show").catch(() => dismiss());
    }
  }
}
window.addEventListener("pulse-capture-start", beginCapture);
// Hidden warm windows remain idle until the shortcut publishes a capture session.
if (await invoke("text_extractor_ready")) await beginCapture();
