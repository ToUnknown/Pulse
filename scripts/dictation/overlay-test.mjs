// Actual renderer with a minimal DOM and deferred native snapshot response.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const element = () => ({textContent:'',dataset:{},style:{setProperty(name,value){this[name]=value;}},append(){},scrollHeight:0});
const body = element(), nodes = Object.fromEntries(['#words','#waveform','#status'].map(id=>[id,element()]));
const bars=[];
let barStart;
const drawing={setTransform(){},clearRect(){bars.length=0;},beginPath(){},moveTo(x,y){barStart=y;},lineTo(x,y){this.amplitude=y-barStart;},stroke(){bars.push({height:this.amplitude,alpha:this.globalAlpha});}};
nodes['#waveform'].getContext = () => drawing;
Object.assign(nodes['#waveform'],{clientWidth:104,clientHeight:32});
let resolveSnapshot;
const calls = [];
const invoke = (name,args) => {
  calls.push({name,args});
  return name === 'dictation_snapshot' ? new Promise(resolve=>{resolveSnapshot=resolve;}) : Promise.resolve();
};
const sandbox = vm.createContext({
  window:{__TAURI__:{core:{invoke}}},
  document:{body,querySelector:id=>nodes[id]},
  matchMedia:()=>({matches:false}),
  requestAnimationFrame(){},setTimeout(){},performance:{now:()=>0},innerWidth:1440,innerHeight:900,devicePixelRatio:1, getComputedStyle:()=>({getPropertyValue:()=>"#888"}),
});
vm.runInContext(readFileSync(new URL('../../src/dictation.js',import.meta.url),'utf8').replace('export function render','function render'),sandbox);
sandbox.window.pulseDictationStart(2,28);
resolveSnapshot({id:1,phase:'done',text:'Old transcript',message:'Inserted'});
await new Promise(resolve=>setImmediate(resolve));
assert.equal(body.dataset.phase,'listening','late old completion must not replace new recording');
assert.equal(nodes['#words'].textContent,'');
assert.equal(nodes['#status'].textContent,'');
assert.deepEqual(calls.filter(c=>c.name==='dictation_overlay_ready').map(c=>c.args.sessionId),[2], 'hidden overlay must reset and request show without waiting for rAF');
sandbox.render({id:2,phase:'done',text:'Final words',message:'Inserted'});
assert.equal(nodes['#status'].textContent,'','completion has no success tip');
sandbox.window.pulseDictationStart(3,28);
assert.equal(body.dataset.expanded,'false');
assert.equal(nodes['#words'].textContent,'');
sandbox.render({id:2,phase:'done',message:'Inserted'});
assert.equal(body.dataset.phase,'listening','stale session cannot flash completion');
console.log('PASS: fresh-session reset, stale snapshot rejection, hidden-webview readiness, no success tip');

sandbox.render({id:4,phase:'listening',inline:true,text:'Live words',target:{x:200,y:650,width:600,height:60}});
assert.equal(body.style['--anchor-x'],'500px','pill centered across original field');
assert.equal(body.style['--anchor-bottom'],'258px');
assert.equal(body.dataset.inline,'true');
sandbox.render({id:4,phase:'listening',inline:true,text:'More words',target:{x:200,y:570,width:600,height:140}});
assert.equal(body.style['--anchor-x'],'500px');
assert.equal(body.style['--anchor-bottom'],'338px','pill follows expanding field top edge');
sandbox.render({id:5,phase:'listening',inline:false,text:'Standalone words',target:null});
assert.equal(body.dataset.anchored,'false','no input uses bottom-center output');
assert.equal(body.dataset.inline,'false');
console.log('PASS: field centering, growing-input positioning, standalone output');

sandbox.render({id:6,phase:'listening',samples:1,level:.12});
sandbox.drawWave(100);
assert.equal(bars.length,15);
assert.ok(bars[7].height>bars[0].height*2 && bars[7].alpha>bars[0].alpha,'voice peaks and opacity are strongest at center');
sandbox.render({id:7,phase:'listening',samples:1,level:0,levels:[1,1,1]});
sandbox.drawWave(100);
assert.ok(bars.every(bar=>bar.height===1.5),'old loud history cannot move a silent live waveform');
console.log('PASS: current voice level only, center peaks, fading sides');
