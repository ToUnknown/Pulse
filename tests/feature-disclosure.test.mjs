import test from "node:test";
import assert from "node:assert/strict";
import { featureDisclosure } from "../src/feature-disclosure.js";
test("features collapse while off, expand on activation, and preserve manual collapse across refreshes", () => {
  const panel = { dataset: {}, setAttribute() {} };
  const button = new EventTarget(); const attrs = {};
  button.setAttribute = (key, value) => { attrs[key] = value; };
  const disclosure = featureDisclosure(panel, button, "Audio assistant");
  disclosure.enabled(false); assert.equal(panel.inert, true);
  button.dispatchEvent(new Event("click")); assert.equal(panel.inert, false);
  disclosure.enabled(false); assert.equal(panel.inert, false);
  disclosure.enabled(true); assert.equal(attrs["aria-expanded"], "true");
  button.dispatchEvent(new Event("click")); disclosure.enabled(true);
  assert.equal(panel.inert, true); assert.equal(attrs["aria-label"], "Expand Audio assistant settings");
  disclosure.enabled(false); disclosure.enabled(true); assert.equal(panel.inert, false);
});
