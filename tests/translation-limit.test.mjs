import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../src/text-extractor.js', import.meta.url), 'utf8');
// Exercise the production action/dispatch functions with a small editor fixture.
const actions = source.slice(source.indexOf('function exceedsTranslationLimit()'), source.indexOf('function clearError()'));
const dispatch = source.slice(source.indexOf('async function translateText('), source.indexOf('copy.addEventListener('));
function editorFixture() {
  const calls = [];
  const element = () => ({ disabled: false, title: '', hidden: false, setAttribute() {} });
  const state = {
    editor: { value: 'Editable draft', readOnly: false, setAttribute() {}, focus() {} },
    copy: element(), translate: element(), translationFallback: element(), languages: element(),
    status: {}, busy: false, closing: false, advancedAvailable: true, generation: 0,
    clearError() {}, phase() {},
    invoke: async (command, payload) => { calls.push({ command, payload }); return 'Translated'; },
    revealText: async () => {},
  };
  vm.createContext(state);
  vm.runInContext(actions + dispatch, state);
  return { state, calls };
}

test('pasting an oversized draft after failure disables both translation actions but preserves Copy', () => {
  const { state } = editorFixture();
  state.translationFallback.hidden = false;
  state.editor.value = 'a'.repeat(5001);
  state.updateActions();
  assert.equal(state.translate.disabled, true);
  assert.equal(state.translationFallback.disabled, true);
  assert.equal(state.copy.disabled, false);
  assert.equal(state.languages.hidden, true);
  state.editor.value = 'Shortened draft';
  state.updateActions();
  assert.equal(state.translate.disabled, false);
  assert.equal(state.translationFallback.disabled, false);
});

test('5,000 Unicode characters remain usable and 5,001 disable translation', () => {
  const { state } = editorFixture();
  for (const character of ['a', 'і', '🙂']) {
    state.editor.value = character.repeat(5000);
    state.updateActions();
    assert.equal(state.translate.disabled, false);
    state.editor.value += character;
    state.updateActions();
    assert.equal(state.translate.disabled, true);
    assert.equal(state.translationFallback.disabled, true);
  }
});

test('stale retry and Advanced callbacks cannot send an oversized edited draft', async () => {
  const { state, calls } = editorFixture();
  state.editor.value = 'і'.repeat(5001);
  await state.translateText('en');
  await state.translateText('en', true);
  assert.deepEqual(calls, []);
  assert.equal(state.editor.readOnly, false);
});

test('valid drafts can explicitly use either provider', async () => {
  for (const advanced of [false, true]) {
    const { state, calls } = editorFixture();
    await state.translateText('uk', advanced);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].command, 'translate_extracted_text');
    assert.equal(calls[0].payload.useAdvanced, advanced);
    assert.equal(calls[0].payload.text, 'Editable draft');
  }
});
