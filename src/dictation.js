const invoke = window.__TAURI__?.core.invoke;
const body = document.body;
const words = document.querySelector("#words");
const canvas = document.querySelector("#waveform");
const context = canvas.getContext("2d");
const status = document.querySelector("#status");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let snapshot = { phase: "idle", levels: [], text: "" };
let shown = "", desired = "", id, lastTyping = 0, lastSamples = 0, waveTime = 0;
let generation = 0, shownSession;
function setText(value, instant = false) {
  desired = value;
  // Final text is authoritative: corrected prefixes replace partials immediately.
  if (instant || !desired.startsWith(shown) || reducedMotion.matches) { shown = desired; words.textContent = shown; words.scrollTop = words.scrollHeight; }
}
export function render(next) {
  if (next.id !== undefined && id !== undefined && next.id < id) return;
  if (next.id !== undefined && next.id !== id) {
    generation++;
    id = next.id; shown = desired = ""; words.textContent = ""; lastSamples = 0;
    body.dataset.expanded = "false";
  }
  snapshot = next;
  body.style.setProperty("--bottom", `${next.bottom || 28}px`);
  if (next.samples !== lastSamples) { waveTime = performance.now(); lastSamples = next.samples; }
  const final = ["sending", "done", "error"].includes(next.phase);
  setText(next.text || "", final);
  if (next.text) body.dataset.expanded = "true";
  body.dataset.phase = next.phase;
  status.textContent = next.phase === "error" ? next.message || "" : "";
  if (invoke && next.id !== undefined && shownSession !== next.id && ["listening", "finalizing", "error"].includes(next.phase)) {
    shownSession = next.id;
    // Reset the DOM before showing a reused webview. Do not wait for an
    // animation frame: hidden native webviews may suspend frame callbacks.
    invoke("dictation_overlay_ready", { sessionId: next.id }).catch(() => {});
  }
}
function frame(now) {
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
  const levels = snapshot.levels || [], step = 7;
  const drift = reducedMotion.matches ? 0 : Math.min(1,(now-waveTime)/85)*step;
  const count = Math.ceil(width/step);
  for (let i=0;i<count;i++) {
    const level = levels[levels.length - 1 - i] || 0;
    const amplitude = Math.max(1, Math.min(1, Math.sqrt(level)*2.8)*(height-12));
    const x = width - 10 - i*step - drift;
    if (x < 8) continue;
    context.beginPath(); context.moveTo(x,height/2-amplitude/2); context.lineTo(x,height/2+amplitude/2); context.stroke();
  }
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
    {phase:"sending",text:"A little thought becomes a sentence. Every word appears while I speak.",target:{x:innerWidth*.65,y:innerHeight*.35,height:24},levels},
    {phase:"done",text:"A little thought becomes a sentence. Every word appears while I speak.",message:"Inserted",levels}
  ];
  window.previewDictation = render;
  const advance=()=>{render({id:1,bottom:32,samples:step+1,...demo[Math.min(step++,demo.length-1)]});};
  const fixture = new URLSearchParams(location.search).get("preview");
  const index = { pill: 0, expanded: 1, finalizing: 2, sending: 3, done: 4 }[fixture];
  if (index !== undefined) { step = index; advance(); }
  else { advance(); setInterval(advance,3000); }
}
