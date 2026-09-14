import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import test from 'node:test';

// Execute the production floating-window handlers with a controlled animation
// clock. The unrelated settings widgets do not need a browser for this race.
const source = readFileSync(new URL('../src/settings.js', import.meta.url), 'utf8');
const marker = 'if (window.__PULSE_FLOATING_SETTINGS__) {';
assert.ok(source.includes(marker));
const handlers = source.slice(source.indexOf(marker));

function setup({ reducedMotion = false } = {}) {
  const callbacks = new Map(), timers = [], actions = [], errors = [];
  const dataset = {};
  const done = { addEventListener: (name, callback) => callbacks.set(`done:${name}`, callback) };
  const header = { addEventListener() {} };
  const selectedTab = { focus() {} };
  runInNewContext(handlers, {
    window: {
      __PULSE_FLOATING_SETTINGS__: true,
      addEventListener: (name, callback) => callbacks.set(name, callback),
    },
    document: {
      documentElement: { dataset },
      querySelector: selector => selector === '#settings-done' ? done : selector === '.settings-header' ? header : selectedTab,
      addEventListener: (name, callback) => callbacks.set(name, callback),
    },
    invoke: async (_command, { action }) => { actions.push(action); },
    showError: error => errors.push(error),
    openCustomSelect: null,
    matchMedia: () => ({ matches: reducedMotion }),
    setTimeout: callback => timers.push(callback),
  });
  return {
    dataset, actions, errors, timers,
    close: () => callbacks.get('done:click')(),
    reopen: () => callbacks.get('pulse-settings-open')(),
    finishAnimation: () => { assert.ok(timers.length); timers.shift()(); },
  };
}

test('Done hides Settings only after its closing animation', async () => {
  const panel = setup();
  const closing = panel.close();
  assert.equal(panel.dataset.closing, 'true');
  assert.deepEqual(panel.actions, ['ready']);
  panel.finishAnimation();
  await closing;
  assert.deepEqual(panel.actions, ['ready', 'done']);
  assert.equal(panel.dataset.closing, undefined);
});

test('reopening Settings cancels the pending native hide', async () => {
  const panel = setup();
  const closing = panel.close();
  panel.reopen();
  panel.finishAnimation();
  await closing;
  assert.deepEqual(panel.actions, ['ready']);
  assert.equal(panel.dataset.closing, undefined);
});

test('an older close cannot hide or reset a newer dismissal', async () => {
  const panel = setup();
  const oldClose = panel.close();
  panel.reopen();
  const newClose = panel.close();
  panel.finishAnimation();
  await oldClose;
  assert.deepEqual(panel.actions, ['ready']);
  assert.equal(panel.dataset.closing, 'true');
  panel.finishAnimation();
  await newClose;
  assert.deepEqual(panel.actions, ['ready', 'done']);
  assert.equal(panel.dataset.closing, undefined);
});

test('repeated Done clicks request only one hide', async () => {
  const panel = setup();
  const first = panel.close();
  await panel.close();
  assert.equal(panel.timers.length, 1);
  panel.finishAnimation();
  await first;
  assert.deepEqual(panel.actions, ['ready', 'done']);
});

test('reduced motion dismisses without an animation timer', async () => {
  const panel = setup({ reducedMotion: true });
  await panel.close();
  assert.equal(panel.timers.length, 0);
  assert.deepEqual(panel.actions, ['ready', 'done']);
  assert.equal(panel.dataset.closing, undefined);
});
