import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const source = readFileSync(new URL('../src/text-extractor.js', import.meta.url), 'utf8');
const start = source.includes('async function finishShimmerCycle()')
  ? source.indexOf('async function finishShimmerCycle()')
  : source.indexOf('async function runExtraction(');
const extraction = source.slice(start, source.indexOf('async function translateText('));
const detailsSource = source.slice(source.indexOf('async function showDetails('), source.indexOf('function renderMode()'));
const tick = () => new Promise(resolve => setImmediate(resolve));
function fixture() {
  let resolveRequest;
  const request = new Promise(resolve => { resolveRequest = resolve; });
  const answers = [], errors = [];
  const state = {
    closing: false, generation: 0, busy: false, mode: 'advanced', crop: {},
    drafts: { advanced: null }, reducedMotion: { matches: false },
    stage: { getBoundingClientRect: () => ({ x: 0, y: 0, width: 500, height: 200 }) },
    frame: { getAnimations: () => [{ animationName: 'screenshot-shimmer', effect: { getComputedTiming: () => ({ currentIteration: 0 }), updateTiming() {} }, finished: new Promise(() => {}) }] },
    details: { hidden: true, inert: true }, editor: { setAttribute() {} },
    status: {}, result: { focus() {} }, document: { body: { dataset: {} } },
    cancelMotions() {}, clearError() {}, hideLanguages() {}, updateActions() {},
    phase: value => { state.document.body.dataset.phase = value; },
    moveStage: async () => {}, invoke: () => request,
    revealText: async text => { answers.push(text); },
    setError: async error => { errors.push(error); },
  };
  vm.createContext(state); vm.runInContext(extraction, state);
  return { state, resolveRequest, answers, errors };
}

test('an Advanced response is revealed without waiting for the active shimmer to finish', async () => {
  const f = fixture();
  const running = f.state.runExtraction();
  await tick();
  assert.equal(f.state.document.body.dataset.phase, 'scanning');
  f.resolveRequest('Ready text');
  await tick();
  assert.deepEqual(f.answers, ['Ready text']);
  await running;
});

test('Advanced errors are shown immediately and stale results remain ignored', async () => {
  const error = fixture();
  error.state.invoke = async () => { throw 'Offline'; };
  const running = error.state.runExtraction();
  await tick();
  assert.deepEqual(error.errors, ['Offline']);
  await running;
  const stale = fixture();
  const obsolete = stale.state.runExtraction();
  await tick();
  stale.state.generation++;
  stale.resolveRequest('Old text');
  await obsolete;
  assert.deepEqual(stale.answers, []);
});

test('the result panel starts its reveal with no delayed fade', async () => {
  const animations = [];
  const state = {
    details: { hidden: true, inert: true }, document: { body: { dataset: {} } },
    stage: { getBoundingClientRect: () => ({}) }, reducedMotion: { matches: false },
    generation: 1, closing: false, editor: { focus() {} },
    phase() {}, moveStage: async () => {},
    motion: async (_element, _frames, options) => { animations.push(options); },
  };
  vm.createContext(state); vm.runInContext(detailsSource, state);
  await state.showDetails(1, 'ready');
  assert.equal(animations.length, 1);
  assert.equal(animations[0].delay ?? 0, 0);
  assert.equal(state.details.hidden, false);
  assert.equal(state.details.inert, false);
});
