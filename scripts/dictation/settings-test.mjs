// Permission/settings lifecycle using the actual controller and simulated IPC.
// No OS permissions, microphone, clipboard, or installed app are modified.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../../src/dictation-settings.js', import.meta.url), 'utf8');
const markup = readFileSync(new URL('../../src/settings.html', import.meta.url), 'utf8');
assert.match(markup, /id="dictation-mode" class="shortcut-mode" role="radiogroup"/, 'dictation uses the shared mode slider');
assert.doesNotMatch(markup, /Default transcribes after you finish speaking/, 'the extra mode explanation is removed');
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return {promise, resolve, reject};
};
const settle = () => new Promise(resolve => setImmediate(resolve));
async function fixture(overrides = {}) {
  const ids = ['section','enabled','mode','error','description','microphone','accessibility'];
  const nodes = Object.fromEntries(ids.map(id => [id, {
    hidden:true, disabled:false, checked:false, value:'', textContent:'', listeners:{},
    addEventListener(name, listener) { this.listeners[name] = listener; },
  }]));
  const modeButtons = ['default', 'live'].map(value => ({
    dataset:{mode:value}, disabled:false, tabIndex:-1, attributes:{},
    setAttribute(name, state) { this.attributes[name] = state; },
    focus() { this.focused = true; },
  }));
  nodes.mode.querySelectorAll = () => modeButtons;
  nodes.mode.style = {setProperty(name, value) { this[name] = value; }};
  const state = {enabled:false, mode:'default', apiKeyConfigured:true, microphone:false,
    accessibility:false, shortcut:'Right Option', error:null, ...overrides};
  const calls = [], events = {}, documentEvents = {};
  let handler;
  const document = {
    hidden:false,
    querySelector(selector) { return nodes[selector.replace('#dictation-', '')]; },
    addEventListener(name, callback) { documentEvents[name] = callback; },
  };
  const context = vm.createContext({document, setInterval(){}, window:{
    addEventListener(name, callback) { events[name] = callback; },
    __TAURI__:{core:{invoke:async(name,args)=>{
      calls.push({name,args});
      if(handler) return handler(name,args);
      if(name==='dictation_settings') return {...state};
      if(name==='set_dictation_enabled') state.enabled=args.enabled;
      if(name==='set_dictation_mode') state.mode=args.mode;
    }}},
  }});
  vm.runInContext(source, context);
  await settle();
  return {nodes,state,calls,events,document,documentEvents,
    handle(value) { handler=value; },
    refresh:()=>vm.runInContext('refresh()', context),
    click:(id, value='live')=>id==='mode'
      ? nodes.mode.listeners.click({target:{closest:()=>modeButtons.find(button=>button.dataset.mode===value)}})
      : nodes[id].listeners[id==='enabled'?'change':'click'](),
    selectedMode:()=>modeButtons.find(button=>button.attributes['aria-checked']==='true')?.dataset.mode,
    key:(key)=>nodes.mode.listeners.keydown({key,preventDefault(){}}),
  };
}

// A successful refresh must clear stale DOM text as well as hide the warning.
const f = await fixture({error:'Allow Accessibility'});
assert.equal(f.selectedMode(),'default');
await f.click('mode');
assert.equal(f.state.mode,'live','model selection is saved');
assert.equal(f.selectedMode(),'live');
assert.equal(f.nodes.mode.style['--mode-index'],1,'selection moves the shared slider');
assert.equal(f.calls.at(-2).name,'set_dictation_mode');
f.state.mode='default';
await f.refresh();
assert.equal(f.selectedMode(),'default','model selection refreshes from persisted state');
assert.equal(f.nodes.mode.style['--mode-index'],0);
f.key('ArrowRight'); await settle();
assert.equal(f.selectedMode(),'live','keyboard navigation selects the second mode');
const rejectedMode = await fixture();
rejectedMode.handle((name) => {
  if (name === 'dictation_settings') return {...rejectedMode.state};
  throw new Error('Could not save model');
});
await rejectedMode.click('mode');
assert.equal(rejectedMode.selectedMode(),'default','failed model changes return the slider to the saved mode');
assert.equal(f.nodes.error.hidden,false);
assert.equal(f.nodes.accessibility.hidden,false);
f.state.accessibility=true; f.state.error=null;
await f.refresh();
assert.equal(f.nodes.accessibility.hidden,true);
assert.equal(f.nodes.error.hidden,true);
assert.equal(f.nodes.error.textContent,'');
assert.equal(f.nodes.enabled.checked,false,'permission alone does not silently enable dictation');
f.state.error='Microphone request denied';
await f.click('microphone');
assert.equal(f.nodes.microphone.hidden,false,'requesting is not the same as granting');
assert.equal(f.nodes.error.textContent,'Microphone request denied');
f.state.microphone=true; f.state.error=null;
await f.events.focus();
assert.equal(f.nodes.microphone.hidden,true);
assert.equal(f.nodes.error.hidden,true);
f.state.error='Network connection failed';
await f.refresh();
assert.equal(f.nodes.error.textContent,'Network connection failed','unrelated diagnostics stay visible');
console.log('PASS: permission changes, denial, focus refresh, stale warning removal, unrelated errors');

// IPC action errors persist while relevant, then clear when access changes.
const local = await fixture();
local.handle((name)=>{
  if(name==='dictation_settings') return {...local.state};
  throw new Error('Allow Accessibility before enabling');
});
local.nodes.enabled.checked=true;
await local.click('enabled');
assert.match(local.nodes.error.textContent,/Allow Accessibility/);
assert.equal(local.nodes.enabled.checked,false,'failed enable restores the backend toggle state');
await local.refresh();
assert.equal(local.nodes.error.hidden,false);
local.state.accessibility=true;
await local.refresh();
assert.equal(local.nodes.error.hidden,true,'resolved local permission error also disappears');
local.handle(null);
local.nodes.enabled.checked=true;
await local.click('enabled');
assert.equal(local.nodes.enabled.checked,true);
console.log('PASS: failed enable, local error recovery, successful enable');

// Newer permission snapshots win even when IPC resolves in reverse order.
const race = await fixture();
const replies=[];
race.handle(()=>{const reply=deferred();replies.push(reply);return reply.promise;});
const older=race.refresh(), newer=race.refresh();
replies[1].resolve({...race.state,accessibility:true}); await newer;
replies[0].resolve({...race.state,accessibility:false,error:'Stale denial'}); await older;
assert.equal(race.nodes.accessibility.hidden,true);
assert.equal(race.nodes.error.hidden,true);

// A snapshot started before an action cannot undo its fresh completion state.
const pending = await fixture();
const old=deferred(), action=deferred();
let reads=0, actions=0;
pending.handle((name)=>{
  if(name==='dictation_settings') return reads++===0?old.promise:{...pending.state};
  actions++; return action.promise;
});
const poll=pending.refresh(), request=pending.click('microphone');
assert.equal(pending.nodes.microphone.disabled,true);
assert.equal(pending.nodes.enabled.disabled,true);
await pending.click('microphone');
assert.equal(actions,1,'repeat clicks cannot create duplicate requests');
pending.state.microphone=true;
action.resolve(); await request;
old.resolve({...pending.state,microphone:false,error:'Old warning'}); await poll;
assert.equal(pending.nodes.microphone.hidden,true);
assert.equal(pending.nodes.error.hidden,true);
assert.equal(pending.nodes.enabled.disabled,false);
console.log('PASS: reversed refresh responses, in-flight action race, duplicate requests, busy controls');

// No key means unavailable, including after a request whose refresh fails.
const gated=await fixture({apiKeyConfigured:false});
assert.equal(gated.nodes.enabled.disabled,true);
gated.handle(name=>{if(name==='dictation_settings')throw new Error('offline');});
await gated.click('microphone');
assert.equal(gated.nodes.enabled.disabled,true);
gated.handle(null);
gated.state.apiKeyConfigured=true;
await gated.events['pulse-api-key-changed']();
assert.equal(gated.nodes.enabled.disabled,false);
gated.state.apiKeyConfigured=false;
await gated.documentEvents.visibilitychange();
await settle();
assert.equal(gated.nodes.enabled.disabled,true);
assert.equal(gated.nodes.enabled.checked,false);
assert.equal(gated.nodes.error.hidden,true);
console.log('PASS: API-key gate, failed refresh recovery, key changes, visibility refresh');
