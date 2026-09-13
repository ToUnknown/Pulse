import { test } from 'node:test';
import assert from 'node:assert/strict';
import { physicalCrop, selectionRect } from '../src/extraction-geometry.js';

test('selection normalizes every drag direction', () => {
  for (const [start, end] of [
    [{ x: 10, y: 20 }, { x: 60, y: 80 }],
    [{ x: 60, y: 80 }, { x: 10, y: 20 }],
    [{ x: 10, y: 80 }, { x: 60, y: 20 }],
    [{ x: 60, y: 20 }, { x: 10, y: 80 }],
  ]) assert.deepEqual(selectionRect(start, end, 100, 100), { x: 10, y: 20, width: 50, height: 60 });
});
test('pointer leaving the selected monitor cannot enlarge the crop', () => {
  assert.deepEqual(selectionRect({ x: 20, y: 30 }, { x: -50, y: 300 }, 100, 100), { x: 0, y: 30, width: 20, height: 70 });
});
test('125%, 150%, 200% DPI and fractional edges retain selected pixels', () => {
  for (const scale of [1, 1.25, 1.5, 2]) {
    const crop = physicalCrop({ x: 10.2, y: 20.2, width: 60.2, height: 30.2 }, { width: 1440, height: 900 }, { width: 1440 * scale, height: 900 * scale });
    assert.equal(crop.x, Math.floor(10.2 * scale));
    assert.equal(crop.y, Math.floor(20.2 * scale));
    assert.equal(crop.x + crop.width, Math.ceil(70.4 * scale));
    assert.equal(crop.y + crop.height, Math.ceil(50.4 * scale));
  }
});
test('whole-screen selection never runs past the last physical pixel', () => {
  assert.deepEqual(physicalCrop({ x: 0, y: 0, width: 1536, height: 864 }, { width: 1536, height: 864 }, { width: 1920, height: 1080 }), { x: 0, y: 0, width: 1920, height: 1080 });
});
