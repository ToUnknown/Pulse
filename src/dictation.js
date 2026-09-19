const invoke = window.__TAURI__?.core.invoke;
const body = document.body;
const words = document.querySelector("#words");
const canvas = document.querySelector("#waveform");
const shell = document.querySelector("#waveform-shell");
const overlay = document.querySelector("#dictation");
const transcript = document.querySelector("#transcript");
const context = canvas.getContext("2d");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let snapshot = { phase: "idle", levels: [], text: "" };
let shown = "", desired = "", id, lastTyping = 0, lastSamples = 0, waveTime = 0;
let generation = 0, shownSession, voice = 0, lastFrame = 0;
let glassBusy = false, lastGlass = "";
async function syncGlass() {
  if (!invoke || id === undefined) return;
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
  const signature = JSON.stringify([sessionId, ...[...Object.values(frame), ...Object.values(textFrame)].map(value => Math.round(value * 100) / 100)]);
  if (signature === lastGlass) return;
  try {
    const available = await invoke("dictation_glass", { sessionId, frame, transcript: textFrame });
    if (id === sessionId) {
      lastGlass = signature;
      body.dataset.nativeGlass = String(!!available);
    }
  } catch { /* Keep the CSS material if the native effect is unavailable. */ }
}
function setText(value, instant = false) {
  desired = value;
  // Final text is authoritative: corrected prefixes replace partials immediately.
  if (instant || !desired.startsWith(shown) || reducedMotion.matches) { shown = desired; words.textContent = shown; words.scrollTop = words.scrollHeight; }
}
export function render(next) {
  if (next.id !== undefined && id !== undefined && next.id < id) return;
  const freshSession = next.id !== undefined && next.id !== id;
  if (freshSession) {
    body.dataset.placing = "true";
    generation++;
    id = next.id; shown = desired = ""; words.textContent = ""; lastSamples = 0;
    body.dataset.expanded = "false"; voice = 0;
  }
  snapshot = next;
  body.style.setProperty("--bottom", `${next.bottom || 28}px`);
  // All recording and delivery states stay in the same bottom-center overlay.
  if (next.samples !== lastSamples) { waveTime = performance.now(); lastSamples = next.samples; }
  const final = ["sending", "done", "error"].includes(next.phase);
  setText(next.text || "", final);
  if (next.text) body.dataset.expanded = "true";
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
  if (!glassBusy) {
    glassBusy = true;
    syncGlass().finally(() => { glassBusy = false; });
  }
  if (desired !== shown && now - lastTyping > 24) {
    const remaining = Array.from(desired.slice(shown.length));
    const chunk = remaining.slice(0, Math.max(1, Math.ceil(remaining.length / 12))).join("");
    shown += chunk;
    // Keep only the new fragment animated, with a bounded number of DOM nodes.
    words.textContent = shown.slice(0, -chunk.length);
    const span = document.createElement("span"); span.className = "fresh"; span.textContent = chunk; words.append(span);
    words.scrollTop = words.scrollHeight; lastTyping = now;
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
  const level = now - waveTime < 220 ? Math.max(0, snapshot.level ?? snapshot.levels?.at(-1) ?? 0) : 0;
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
    context.globalAlpha = .12 + .88 * envelope;
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
  window.pulseDictationStart = (sessionId, bottom) => render({ id: sessionId, bottom, phase: "listening", text: "", levels: [] });
  poll();
}
// Browser-only visual fixture. Native sessions never accept URL-driven state.
else if (new URLSearchParams(location.search).has("preview")) {
  const levels = Array.from({length:80},(_,i)=>i%17<4?.001:.03+Math.sin(i*1.4)**2*.13);
  let step=0;
  const demo=[
    {phase:"listening",text:"",levels:levels.slice(-14)},
    {phase:"listening",text:"A little thought becomes a sentence. Every word appears while I speak.",levels},
    {phase:"finalizing",text:"A little thought becomes a sentence. Every word appears while I speak.",levels},
    {phase:"sending",text:"A little thought becomes a sentence. Every word appears while I speak.",levels},
    {phase:"done",text:"A little thought becomes a sentence. Every word appears while I speak.",levels}
  ];
  window.previewDictation = render;
  const advance=()=>{render({id:1,bottom:32,samples:step+1,level:.09,...demo[Math.min(step++,demo.length-1)]});};
  const fixture = new URLSearchParams(location.search).get("preview");
  if (fixture === "long") demo[1].text = "This is a longer thought that keeps appearing as I speak. The older words quietly drift toward the top, leaving room for what comes next. I can move through my apps freely while dictation stays here. Only the newest line remains completely clear.";
  const index = { pill: 0, expanded: 1, long: 1, finalizing: 2, sending: 3, done: 4 }[fixture];
  if (index !== undefined) { step = index; advance(); }
  else { advance(); setInterval(advance,3000); }
  setInterval(() => { if (snapshot.phase === "listening") render({...snapshot, samples:(snapshot.samples || 0)+1, level:.025 + .07 * (1+Math.sin(performance.now()/450))/2}); }, 70);
}
