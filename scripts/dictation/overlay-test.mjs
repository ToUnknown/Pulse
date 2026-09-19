// Actual renderer with a minimal DOM and deferred native snapshot response.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const css = readFileSync(new URL('../../src/dictation.css',import.meta.url),'utf8');
assert.match(css, /#text-viewport\s*\{[^}]*max-height:\s*100px/, 'transcript viewport holds five 20px lines');
assert.match(css, /#words\s*\{[^}]*line-height:\s*20px/, 'transcript line height matches the five-line limit');
assert.match(css, /#dictation\s*\{[^}]*bottom:\s*var\(--bottom\)/, 'unanchored preview is at the screen bottom');
const element = () => ({
  children:[], value:'', dataset:{}, classList:{add(){}},
  style:{setProperty(name,value){this[name]=value;}},
  get textContent(){return this.value+this.children.map(node=>node.textContent).join('');},
  set textContent(value){this.value=value;this.children=[];},
  insertBefore(node,anchor){const index=anchor?this.children.indexOf(anchor):this.children.length;this.children.splice(index,0,node);node.parent=this;},
  remove(){if(this.parent)this.parent.children.splice(this.parent.children.indexOf(this),1);},
  append(node){this.insertBefore(node,null);}, scrollHeight:0, offsetHeight:0,
  animate(){return {cancel(){}};},
  scrollTo({top}){this.scrollTop=top;},
  getBoundingClientRect(){return {x:400,y:600,width:64,height:28};},
});
const body = element(), nodes = Object.fromEntries(['#words','#transcript','#text-viewport','#waveform','#status','#waveform-shell','#dictation'].map(id=>[id,element()]));
const bars=[];
let barStart;
const drawing={setTransform(){},clearRect(){bars.length=0;},beginPath(){},moveTo(x,y){barStart=y;},lineTo(x,y){this.amplitude=y-barStart;},stroke(){bars.push({height:this.amplitude,alpha:this.globalAlpha});}};
nodes['#waveform'].getContext = () => drawing;
Object.assign(nodes['#waveform'],{clientWidth:64,clientHeight:28});
let resolveSnapshot;
const calls = [];
let holdGlass = false, failGlass = false, overlayOpacity = "1";
const glassReplies = [];
const invoke = (name,args) => {
  calls.push({name,args});
  if (name === 'dictation_glass' && failGlass) return Promise.reject(new Error('temporarily unavailable'));
  if (name === 'dictation_glass' && holdGlass) return new Promise(resolve => glassReplies.push(resolve));
  return name === 'dictation_snapshot' ? new Promise(resolve=>{resolveSnapshot=resolve;}) : Promise.resolve(name === 'dictation_glass');
};
const motionPreference = {matches:false};
const themePreference = {matches:false};
const sandbox = vm.createContext({
  window:{__TAURI__:{core:{invoke}}},
  document:{body,querySelector:id=>nodes[id],createElement:element},
  matchMedia:query=>query.includes("color-scheme")?themePreference:motionPreference,
  requestAnimationFrame(){},setTimeout(){},performance:{now:()=>0},innerWidth:1440,innerHeight:900,devicePixelRatio:1, getComputedStyle:element=>({opacity:element===nodes["#dictation"]?overlayOpacity:"1",getPropertyValue:()=>"#303237"}),
});
vm.runInContext(readFileSync(new URL('../../src/dictation.js',import.meta.url),'utf8').replace('export function render','function render'),sandbox);
sandbox.window.pulseDictationStart(2,28);
resolveSnapshot({id:1,phase:'done',text:'Old transcript',message:'Inserted'});
await new Promise(resolve=>setImmediate(resolve));
await Promise.resolve();
assert.equal(body.dataset.phase,'listening','late old completion must not replace new recording');
assert.equal(nodes['#words'].textContent,'');
assert.equal(nodes['#status'].textContent,'');
assert.equal(body.dataset.nativeGlass,'true','native glass replaces CSS only after successful acknowledgement');
assert.ok(calls.findIndex(c=>c.name==='dictation_glass') < calls.findIndex(c=>c.name==='dictation_overlay_ready'),'native glass is positioned before revealing the window');
assert.deepEqual(calls.filter(c=>c.name==='dictation_overlay_ready').map(c=>c.args.sessionId),[2], 'hidden overlay must reset and request show without waiting for rAF');
sandbox.render({id:2,phase:'done',text:'Final words',message:'Inserted'});
assert.equal(nodes['#status'].textContent,'','completion has no success tip');
sandbox.window.pulseDictationStart(3,28);
assert.equal(body.dataset.expanded,'false');
assert.equal(nodes['#words'].textContent,'');
sandbox.render({id:2,phase:'done',message:'Inserted'});
assert.equal(body.dataset.phase,'listening','stale session cannot flash completion');
console.log('PASS: fresh-session reset, stale snapshot rejection, hidden-webview readiness, no success tip');

// Even stale field metadata cannot move the simplified overlay or hide text.
for (const phase of ['listening','finalizing','sending']) {
  sandbox.render({id:4,phase,inline:true,text:'Complete words',target:{x:200,y:650,width:1,height:18}});
  assert.equal(body.dataset.anchored,undefined,'overlay always stays at bottom center');
  assert.equal(body.dataset.inline,undefined,'transcript remains visible above the pill');
  assert.equal(body.style['--anchor-x'],undefined,'input geometry is ignored');
}
sandbox.render({id:5,phase:'error',message:'Do not show this',text:''});
assert.equal(nodes['#status'].textContent,'','errors never appear in the overlay');
console.log('PASS: bottom-center recording and delivery, visible transcript, silent errors');

sandbox.render({id:6,phase:'listening',samples:1,level:.12});
sandbox.drawWave(100);
assert.equal(bars.length,9);
assert.ok(bars[4].height>bars[0].height*2 && bars[4].alpha>bars[0].alpha,'voice peaks and opacity are strongest at center');
sandbox.render({id:7,phase:'listening',samples:1,level:0,levels:[1,1,1]});
sandbox.drawWave(100);
assert.ok(bars.every(bar=>bar.height===1.5),'old loud history cannot move a silent live waveform');
console.log('PASS: current voice level only, center peaks, fading sides');

await Promise.resolve();
holdGlass = true;
sandbox.window.pulseDictationStart(8,28);
sandbox.window.pulseDictationStart(9,28);
glassReplies[1](true);
await Promise.resolve();
glassReplies[0](false);
await Promise.resolve();
assert.equal(body.dataset.nativeGlass,'true','stale glass response cannot replace a new session material');
console.log('PASS: stale native-glass response rejected');

holdGlass = false;
sandbox.render({id:9,phase:'done'});
overlayOpacity = '.45';
await sandbox.syncGlass();
assert.equal(calls.filter(c=>c.name==='dictation_glass').at(-1).args.frame.opacity,.45,'native lens follows the completion fade instead of disappearing immediately');
overlayOpacity = '0';
await sandbox.syncGlass();
assert.equal(calls.filter(c=>c.name==='dictation_glass').at(-1).args.frame.opacity,0,'native lens is hidden when the fade finishes');
failGlass = true;
overlayOpacity = '.6';
await sandbox.syncGlass();
const failedCalls = calls.length;
failGlass = false;
await sandbox.syncGlass();
assert.equal(calls.length,failedCalls+1,'an unsuccessful geometry update is retried even when the frame has stopped moving');
console.log('PASS: native glass completion fade and failed-update recovery');

// Measured layout, not painted word-animation overflow, drives one shared spring.
sandbox.window.pulseDictationStart(10,28);
sandbox.render({id:10,phase:'listening',text:'A partial wor'});
const partial = nodes['#words'].children.at(-1);
sandbox.render({id:10,phase:'listening',text:'A partial word'});
assert.equal(nodes['#words'].children.at(-1),partial,'partial words retain their animation node');
nodes['#words'].offsetHeight = 20;
sandbox.layoutText(0);
sandbox.layoutText(16);
assert.ok(parseFloat(nodes['#text-viewport'].style.height)>0 && parseFloat(nodes['#text-viewport'].style.height)<20,'growth begins gradually');
for(let time=32;time<=1000;time+=16)sandbox.layoutText(time);
assert.equal(nodes['#text-viewport'].style.height,'20px');
nodes['#words'].offsetHeight = 140;
const heightBefore = nodes['#text-viewport'].style.height;
sandbox.render({id:10,phase:'listening',text:'A partial word followed by many more lines'});
assert.equal(nodes['#text-viewport'].style.height,heightBefore,'text updates cannot snap the current height');
for(let time=1008;time<=2200;time+=16)sandbox.layoutText(time);
assert.equal(nodes['#text-viewport'].style.height,'100px');
assert.equal(nodes['#words'].style.transform,'translateY(-40px)','overflow and growth settle together');
motionPreference.matches = true;
nodes['#words'].offsetHeight = 40;
sandbox.layoutText(2216);
assert.equal(nodes['#text-viewport'].style.height,'40px');
assert.equal(nodes['#words'].style.transform,'translateY(0px)','reduced motion settles both values immediately');
sandbox.window.pulseDictationStart(11,28);
assert.equal(nodes['#text-viewport'].style.height,'0px','fresh recording clears the previous height');
console.log('PASS: partial-word identity, continuous growth, shared overflow, reduced motion and session layout reset');

await sandbox.syncGlass();
const themeCalls = calls.length;
themePreference.matches = true;
await sandbox.syncGlass();
assert.equal(calls.length,themeCalls+1,'theme change updates native glass even if geometry is unchanged');
assert.equal(calls.at(-1).args.darkMode,true);
themePreference.matches = false;
await sandbox.syncGlass();
assert.equal(calls.at(-1).args.darkMode,false);
console.log('PASS: native glass tracks appearance changes without geometry changes');
