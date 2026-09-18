import test from "node:test";
import assert from "node:assert/strict";
import { shortcutFromEvent, displayShortcut, attachShortcutRecorder } from "../src/shortcut-recorder.js";

test("Space chords, modifier-only keys, Escape and invalid shortcuts", () => {
  assert.equal(shortcutFromEvent({ code: "Space", ctrlKey: true, altKey: true }), "Control+Alt+Space");
  assert.equal(shortcutFromEvent({ code: "Space", ctrlKey: true, shiftKey: true }), "Control+Shift+Space");
  assert.equal(shortcutFromEvent({ code: "ShiftLeft" }), null);
  assert.equal(shortcutFromEvent({ key: "Escape" }), null);
  assert.throws(() => shortcutFromEvent({ code: "Space", shiftKey: true }));
  assert.throws(() => shortcutFromEvent({ code: "Enter", ctrlKey: true }));
  assert.equal(displayShortcut("Control+Alt+Space"), "Ctrl + Alt + Space");
  assert.equal(displayShortcut("Super+KeyT", "macos"), "⌘ + T");
});
test("the shared recorder replaces a shortcut and releases native recording on blur", () => {
  const root = new EventTarget(); const calls = []; const accepted = [];
  root.__TAURI__ = { core: { invoke: async (command, args) => { calls.push([command, args]); } } };
  globalThis.window = root;
  const button = new EventTarget(); button.focus = () => {}; button.setAttribute = () => {};
  attachShortcutRecorder({ buttons: { listenShortcut: button }, value: () => "Control+Alt+Space", platform: () => "windows", accept: (...args) => accepted.push(args), error: assert.fail });
  button.dispatchEvent(new Event("click"));
  const registered = new Event("pulse-extractor-shortcut"); registered.detail = "Control+Shift+Space"; root.dispatchEvent(registered);
  assert.deepEqual(accepted, [["listenShortcut", "Control+Shift+Space"]]);
  assert.deepEqual(calls.map(call => call[1].recording), [true, false]);
  button.dispatchEvent(new Event("click")); button.dispatchEvent(new Event("blur"));
  assert.equal(calls.at(-1)[1].recording, false);
  assert.equal(button.textContent, "Ctrl + Alt + Space");
  delete globalThis.window;
});
