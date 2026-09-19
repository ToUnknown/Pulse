const invoke = window.__TAURI__?.core.invoke;
const body = document.body;
const words = document.querySelector("#words");
const transcript = document.querySelector("#transcript");
const canvas = document.querySelector("#waveform");
const context = canvas.getContext("2d");
const status = document.querySelector("#status");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
let snapshot = { phase: "idle", levels: [], text: "" };
let shown = "", desired = "", id, phase, lastTyping = 0, lastSamples = 0, waveTime = 0;
let flightAnimation;
function clearFlight() { flightAnimation?.cancel(); flightAnimation = null; document.querySelector("#flight")?.remove(); transcript.style.opacity = ""; }
function setText(value, instant = false) {
  desired = value;
  // Final text is authoritative: corrected prefixes replace partials immediately.
  if (instant || !desired.startsWith(shown) || reducedMotion.matches) { shown = desired; words.textContent = shown; words.scrollTop = words.scrollHeight; }
}
function fly(target) {
  clearFlight();
  if (!shown || reducedMotion.matches) { transcript.style.opacity = "0"; return; }
  const rect = transcript.getBoundingClientRect();
  const flight = document.createElement("div");
  flight.id = "flight";
  flight.textContent = shown;
  Object.assign(flight.style, { left: `${rect.left}px`, top: `${rect.top}px`, width: `${rect.width}px`, height: `${rect.height}px` });
  document.body.append(flight);
  transcript.style.opacity = "0";
  // A destination on another screen has no honest position in this canvas.
  const visible = target && target.x >= 0 && target.x < innerWidth && target.y >= 0 && target.y < innerHeight;
  const x = visible ? target.x - rect.left : 0;
  const y = visible ? target.y - rect.top : -22;
  const scale = visible ? Math.max(.07, Math.min(.4, target.height / rect.height)) : .97;
  flightAnimation = flight.animate([
    { transform: "translate(0,0) scale(1)", opacity: 1 },
    { transform: `translate(${x * .78}px,${y * .78}px) scale(${Math.max(scale,.45)})`, opacity: .8, offset: .7 },
    { transform: `translate(${x}px,${y}px) scale(${scale})`, opacity: 0 }
  ], { duration: 580, easing: "cubic-bezier(.32,.02,.22,1)", fill: "forwards" });
  flightAnimation.finished.then(() => flight.remove(), () => flight.remove());
}
export function render(next) {
  if (next.id !== id) {
    id = next.id; shown = desired = ""; words.textContent = ""; phase = "idle"; lastSamples = 0;
    clearFlight(); body.dataset.expanded = "false";
  }
  snapshot = next;
  body.style.setProperty("--bottom", `${next.bottom || 28}px`);
  if (next.samples !== lastSamples) { waveTime = performance.now(); lastSamples = next.samples; }
  const final = ["sending", "done", "error"].includes(next.phase);
  setText(next.text || "", final);
  if (next.text) body.dataset.expanded = "true";
  body.dataset.phase = next.phase;
  status.textContent = next.message || "";
  if (next.phase === "sending" && phase !== "sending") fly(next.target);
  if (next.phase === "idle") clearFlight();
  phase = next.phase;
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
  context.strokeStyle = getComputedStyle(body).getPropertyValue("--wave"); context.lineWidth = 4.5; context.lineCap = "round";
  const levels = snapshot.levels || [], step = 10;
  const drift = reducedMotion.matches ? 0 : Math.min(1,(now-waveTime)/85)*step;
  const count = Math.ceil(width/step);
  for (let i=0;i<count;i++) {
    const level = levels[levels.length - 1 - i] || 0;
    const amplitude = Math.max(1, Math.min(1, Math.sqrt(level)*2.8)*(height-24));
    const x = width - 10 - i*step - drift;
    if (x < 8) continue;
    context.beginPath(); context.moveTo(x,height/2-amplitude/2); context.lineTo(x,height/2+amplitude/2); context.stroke();
  }
}
async function poll() {
  try { render(await invoke("dictation_snapshot")); } catch { /* A hidden/recreated overlay can reconnect without interrupting capture. */ }
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
