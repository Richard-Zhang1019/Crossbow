import assert from "node:assert/strict";
import test from "node:test";
import { nextSetupStep } from "./firstConnection.ts";

const fresh = {
  progress: "pending",
  configured: false,
  installed: false,
  coreState: "Stopped",
  takeoverAvailable: true,
  takeoverEnabled: false,
};
const step = (state) => nextSetupStep({ ...fresh, ...state });

test("a fresh data directory starts with import even after an earlier setup", () => {
  assert.equal(step({ progress: "complete" }), "import");
});

test("a missing core needs download, while a bundled core can start", () => {
  assert.equal(step({ configured: true }), "download");
  assert.equal(step({ configured: true, installed: true }), "start");
  assert.equal(step({ configured: true, coreState: "Starting" }), "start");
  assert.equal(step({ configured: true, installed: true, coreState: "Crashed" }), "start");
});

test("verification waits for an explicit takeover on supported platforms", () => {
  const running = { configured: true, coreState: "Running" };
  assert.equal(step(running), "takeover");
  assert.equal(step({ ...running, takeoverEnabled: true }), "verify");
  assert.equal(step({ ...running, takeoverAvailable: false }), "verify");
});

test("established profiles and completed setup keep the regular overview", () => {
  assert.equal(step({ configured: true, progress: null }), null);
  assert.equal(step({ configured: true, progress: "complete" }), null);
});
