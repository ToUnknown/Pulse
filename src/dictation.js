const invoke = window.__TAURI__?.core.invoke;
const body = document.body;
const words = document.querySelector("#words");
const canvas = document.querySelector("#waveform");
const shell = document.querySelector("#waveform-shell");
const overlay = document.querySelector("#dictation");
const deliveryError = document.querySelector("#delivery-error");
const transcript = document.querySelector("#transcript");
const textViewport = document.querySelector("#text-viewport");
const transcriptBackdrop = document.querySelector("#transcript-backdrop");
const waveformBackdrop = document.querySelector("#waveform-backdrop");
const context = canvas.getContext("2d");
const darkMode = matchMedia("(prefers-color-scheme: dark)");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let snapshot = { phase: "idle", level: 0, text: "" };
let shown = "", id, lastSamples = 0, waveTime = 0;
let wordTokens = [], wordNodes = [];
let textHeight = 0, textVelocity = 0, layoutTime;
const wordMoves = new WeakMap();
// One spring drives growth and overflow together. The text's screen position
// stays continuous as the box reaches five lines and starts following the tail.
function layoutText(now) {
  const target = words.offsetHeight;
  const dt = Math.min(64, Math.max(0, now - (layoutTime ?? now))) / 1000;
  layoutTime = now;
  if (reducedMotion.matches) {
    textHeight = target; textVelocity = 0;
    for (const node of wordNodes) wordMoves.get(node)?.cancel();
  }
  else {
    const distance = textHeight - target, rate = 20;
    const combined = textVelocity + rate * distance, decay = Math.exp(-rate * dt);
    textHeight = target + (distance + combined * dt) * decay;
    textVelocity = (textVelocity - rate * combined * dt) * decay;
    if (Math.abs(textHeight - target) < .02 && Math.abs(textVelocity) < .2) {
      textHeight = target; textVelocity = 0;
    }
  }
  textViewport.style.height = `${Math.min(100, Math.max(0, textHeight))}px`;
  words.style.transform = `translateY(${-Math.max(0, textHeight - 100)}px)`;
}
let generation = 0, shownSession, voice = 0, lastFrame = 0;
let glassBusy = false, lastGlass = "";
function placeBackdrop(element, rect, opacity) {
  const padding = 24;
  element.style.left = `${rect.x - padding}px`;
  element.style.top = `${rect.y - padding}px`;
  element.style.width = `${rect.width + padding * 2}px`;
  element.style.height = `${rect.height + padding * 2}px`;
  element.style.opacity = String(opacity * .8);
}
async function syncGlass() {
  if (id === undefined) return;
  const sessionId = id;
  const rect = shell.getBoundingClientRect();
  // Keep the native lens through the DOM's completion fade; removing it on
  // the first "done" snapshot makes the glass pop away under the fading bars.
  const visible = ["listening", "finalizing", "sending", "done"].includes(snapshot.phase);
  const frame = { x: rect.x, y: rect.y, width: rect.width, height: rect.height,
    opacity: visible ? Number(getComputedStyle(shell).opacity) * Number(getComputedStyle(overlay).opacity) : 0 };
  const textRect = transcript.getBoundingClientRect();
  const textFrame = { x: textRect.x, y: textRect.y, width: textRect.width, height: textRect.height,
    opacity: visible ? Number(getComputedStyle(transcript).opacity) * Number(getComputedStyle(overlay).opacity) : 0 };
  // Browser fallback shares the exact measured frame/fade used by AppKit.
  // Keep it outside the blocks so their clipping cannot cut off the halo.
  placeBackdrop(waveformBackdrop, frame, frame.opacity);
  placeBackdrop(transcriptBackdrop, textFrame, textFrame.opacity);
  if (!invoke) return;
  const signature = JSON.stringify([sessionId, darkMode.matches, ...[...Object.values(frame), ...Object.values(textFrame)].map(value => Math.round(value * 100) / 100)]);
  if (signature === lastGlass) return;
  try {
    const available = await invoke("dictation_glass", { sessionId, frame, transcript: textFrame, darkMode: darkMode.matches });
    if (id === sessionId) {
      lastGlass = signature;
      body.dataset.nativeGlass = String(!!available);
    }
  } catch { /* Keep the CSS material if the native effect is unavailable. */ }
}
function setText(value) {
  if (value === shown) return;
  const parentRect = words.getBoundingClientRect();
  const positions = new Map();
  if (!reducedMotion.matches) for (const node of wordNodes) {
    if (!/\S/u.test(node.textContent)) continue;
    const rect = node.getBoundingClientRect();
    positions.set(node, { x: rect.x - parentRect.x, y: rect.y - parentRect.y });
  }
  shown = value;
  const tokens = value.match(/\s+|\S+/gu) || [];
  let prefix = 0, suffix = 0;
  while (prefix < tokens.length && prefix < wordTokens.length && tokens[prefix] === wordTokens[prefix]) prefix++;
  // Keep a partially revealed word's existing node as more letters arrive.
  if (prefix < tokens.length && prefix < wordTokens.length && /\S/u.test(wordTokens[prefix]) &&
    tokens[prefix].startsWith(wordTokens[prefix])) {
    wordNodes[prefix].textContent = tokens[prefix];
    prefix++;
  }
  while (suffix < tokens.length - prefix && suffix < wordTokens.length - prefix &&
    tokens[tokens.length - 1 - suffix] === wordTokens[wordTokens.length - 1 - suffix]) suffix++;
  // Preserve unchanged words and their animations when a model corrects text.
  const tail = wordNodes.slice(wordNodes.length - suffix);
  const anchor = tail[0] || null;
  for (const node of wordNodes.slice(prefix, wordNodes.length - suffix)) node.remove();
  const added = [];
  for (let i = prefix; i < tokens.length - suffix; i++) {
    const token = tokens[i];
    const node = document.createElement("span");
    node.textContent = token;
    if (/\S/u.test(token)) {
      node.className = "word";
      // Extending a partial word should not restart its entrance on every delta.
      const extending = wordTokens[i] && token.startsWith(wordTokens[i]);
      if (!extending && !reducedMotion.matches) {
        node.classList.add("new-word");
        node.style.animationDelay = `${Math.min(added.length, 5) * 25}ms`;
      }
    }
    words.insertBefore(node, anchor);
    added.push(node);
  }
  wordNodes = [...wordNodes.slice(0, prefix), ...added, ...tail];
  wordTokens = tokens;
  // Retarget from the currently painted position, including an unfinished move.
  // Opacity/blur entrances use different properties and never affect measurement.
  for (const node of wordNodes) {
    const previous = positions.get(node);
    if (!previous) continue;
    wordMoves.get(node)?.cancel();
    const rect = node.getBoundingClientRect();
    const x = previous.x - (rect.x - parentRect.x);
    const y = previous.y - (rect.y - parentRect.y);
    if (Math.abs(x) > .1 || Math.abs(y) > .1) {
      wordMoves.set(node, node.animate([
        { translate: `${x}px ${y}px` }, { translate: '0px 0px' },
      ], { duration: 220, easing: 'cubic-bezier(.22,.8,.25,1)' }));
    }
  }
  if (reducedMotion.matches) layoutText(performance.now());
}
export function render(next) {
  if (next.id !== undefined && id !== undefined && next.id < id) return;
  const freshSession = next.id !== undefined && next.id !== id;
  if (freshSession) {
    body.dataset.placing = "true";
    generation++;
    id = next.id; shown = ""; words.textContent = ""; lastSamples = 0;
    wordTokens = []; wordNodes = [];
    textHeight = 0; textVelocity = 0; layoutTime = undefined;
    textViewport.style.height = '0px'; words.style.transform = 'translateY(0px)';
    textViewport.scrollTop = 0;
    body.dataset.expanded = "false"; voice = 0;
  }
  snapshot = next;
  deliveryError.textContent = next.phase === "error" ? next.message || "Could not copy the transcript." : "";
  body.dataset.mode = next.mode || (freshSession ? "live" : body.dataset.mode || "live");
  body.style.setProperty("--bottom", `${next.bottom || 28}px`);
  // All recording and delivery states stay in the same bottom-center overlay.
  if (next.samples !== lastSamples) { waveTime = performance.now(); lastSamples = next.samples; }
  setText(next.text || "");
  body.dataset.expanded = String(body.dataset.mode === "live" && !!next.text);
  body.dataset.phase = next.phase;
  if (freshSession) {
    // Resolve the bottom position before revealing a reused overlay.
    void body.offsetHeight;
    body.dataset.placing = "false";
  }
  if (invoke && next.id !== undefined && shownSession !== next.id && ["listening", "finalizing", "error"].includes(next.phase)) {
    shownSession = next.id;
    // Reset the DOM before showing a reused webview. Do not wait for an
    // animation frame: hidden native webviews may suspend frame callbacks.
    // Place native glass before revealing the window, including a reused one.
    syncGlass().finally(() => {
      if (id === next.id) invoke("dictation_overlay_ready", { sessionId: next.id }).catch(() => {});
    });
  }
}
function frame(now) {
  layoutText(now);
  if (!glassBusy) {
    glassBusy = true;
    syncGlass().finally(() => { glassBusy = false; });
  }
  if (snapshot.phase === "listening") drawWave(now);
  requestAnimationFrame(frame);
}
function drawWave(now) {
  const width = canvas.clientWidth, height = canvas.clientHeight;
  if (!width || !height) return;
  const dpr = devicePixelRatio || 1;
  if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) { canvas.width = Math.round(width*dpr); canvas.height = Math.round(height*dpr); }
  context.setTransform(dpr,0,0,dpr,0,0); context.clearRect(0,0,width,height);
  context.strokeStyle = getComputedStyle(body).getPropertyValue("--wave"); context.lineWidth = 3; context.lineCap = "round";
  // Only the current microphone level drives the wave; there is no history.
  const level = now - waveTime < 220 ? Math.max(0, snapshot.level ?? 0) : 0;
  const target = Math.min(1, Math.sqrt(level) * 4.2);
  const elapsed = Math.min(64, Math.max(1, now - lastFrame)); lastFrame = now;
  voice += (target - voice) * (1 - Math.exp(-elapsed / (target > voice ? 45 : 130)));
  const count = 9, step = 5, center = (count - 1) / 2;
  for (let i = 0; i < count; i++) {
    const distance = Math.abs(i - center) / (center + 1);
    const envelope = Math.cos(distance * Math.PI / 2) ** 1.8;
    const movement = reducedMotion.matches ? 1 : .72 + .28 * Math.cos(now * .014 + i * 1.9);
    const amplitude = 1.5 + voice * envelope * movement * (height - 7);
    const x = width / 2 + (i - center) * step;
    context.globalAlpha = .3 + .7 * envelope;
    context.beginPath(); context.moveTo(x,height/2-amplitude/2); context.lineTo(x,height/2+amplitude/2); context.stroke();
  }
  context.globalAlpha = 1;
}
async function poll() {
  const startedGeneration = generation;
  try {
    const next = await invoke("dictation_snapshot");
    if (startedGeneration === generation) render(next);
  } catch { /* A hidden/recreated overlay can reconnect without interrupting capture. */ }
  setTimeout(poll, snapshot.phase === "idle" ? 100 : 33);
}
requestAnimationFrame(frame);
if (invoke) {
  window.pulseDictationStart = (sessionId, bottom, mode = "live") => render({ id: sessionId, bottom, mode, phase: "listening", text: "", level: 0 });
  poll();
}
// Browser-only visual fixture. Native sessions never accept URL-driven state.
else if (new URLSearchParams(location.search).has("preview")) {
  let step=0;
  const demo=[
    {phase:"listening",text:"",level:.09},
    {phase:"listening",text:"A little thought becomes a sentence. Every word appears while I speak.",level:.09},
    {phase:"finalizing",text:"A little thought becomes a sentence. Every word appears while I speak.",level:.09},
    {phase:"sending",text:"A little thought becomes a sentence. Every word appears while I speak.",level:.09},
    {phase:"done",text:"A little thought becomes a sentence. Every word appears while I speak.",level:.09}
  ];
  window.previewDictation = render;
  const advance=()=>{render({id:1,bottom:32,samples:step+1,level:.09,...demo[Math.min(step++,demo.length-1)]});};
  const fixture = new URLSearchParams(location.search).get("preview");
  if (fixture === "long") demo[1].text = "A thought becomes a sentence. New words appear gently while I speak, and the box keeps the latest five lines in view. I can keep talking without losing the full transcript, then send everything to the selected input when I finish.";
  const index = { pill: 0, expanded: 1, long: 1, finalizing: 2, sending: 3, done: 4 }[fixture];
  if (index !== undefined) { step = index; advance(); }
  else { advance(); setInterval(advance,3000); }
  setInterval(() => { if (snapshot.phase === "listening") render({...snapshot, samples:(snapshot.samples || 0)+1, level:.025 + .07 * (1+Math.sin(performance.now()/450))/2}); }, 70);
}
