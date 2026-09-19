// Actual renderer with a minimal DOM and deferred native snapshot response.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const element = () => ({textContent:'',dataset:{},style:{setProperty(){}},append(){},scrollHeight:0});
const body = element(), nodes = Object.fromEntries(['#words','#waveform','#status'].map(id=>[id,element()]));
nodes['#waveform'].getContext = () => ({});
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
  requestAnimationFrame(){},setTimeout(){},performance:{now:()=>0},
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
