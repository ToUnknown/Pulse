import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import test from 'node:test';

const source = readFileSync(new URL('../src/text-extractor-settings.js', import.meta.url), 'utf8');
const render = source.slice(source.indexOf('function renderOcrStatus('), source.indexOf('async function pollOcrStatus('));

function setup(platform) {
  const context = {
    platform, state: { enabled: true },
    ocrSetup: { dataset: {} }, ocrRetry: {}, ocrLabel: {},
    ocrProgress: { removeAttribute() {} },
  };
  runInNewContext(render, context);
  return context;
}

test('macOS preparation shows a centered message until native OCR is ready', () => {
  const ui = setup('macos');
  ui.renderOcrStatus({ phase: 'preparing' });
  assert.equal(ui.ocrSetup.hidden, false);
  assert.equal(ui.ocrSetup.dataset.centered, 'true');
  assert.equal(ui.ocrLabel.textContent, 'On-device OCR is compiling. It will be available shortly.');
  assert.equal(ui.ocrRetry.hidden, true);
  assert.equal(ui.ocrProgress.hidden, true);
  ui.renderOcrStatus({ phase: 'ready' });
  assert.equal(ui.ocrSetup.hidden, true);
  ui.state.enabled = false;
  ui.renderOcrStatus({ phase: 'preparing' });
  assert.equal(ui.ocrSetup.hidden, true);
});

test('preparation failure offers Retry without claiming it is still compiling', () => {
  const ui = setup('macos');
  ui.renderOcrStatus({ phase: 'error', error: 'Could not prepare on-device OCR. Try again.' });
  assert.equal(ui.ocrSetup.hidden, false);
  assert.equal(ui.ocrRetry.hidden, false);
  assert.equal(ui.ocrSetup.dataset.error, 'true');
  assert.equal(ui.ocrLabel.textContent, 'Could not prepare on-device OCR. Try again.');
});

test('Windows keeps its download status and progress', () => {
  const ui = setup('windows');
  ui.renderOcrStatus({ phase: 'downloading', modelIndex: 1, downloadedBytes: 50, totalBytes: 100 });
  assert.equal(ui.ocrLabel.textContent, 'Downloading offline OCR · 1/2');
  assert.equal(ui.ocrSetup.dataset.centered, 'false');
  assert.equal(ui.ocrProgress.hidden, false);
  assert.equal(ui.ocrProgress.value, 0.5);
});
