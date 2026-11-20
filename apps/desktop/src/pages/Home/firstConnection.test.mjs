import assert from "node:assert/strict";
import test from "node:test";
import { isPortUnavailable, nextSetupStep } from "./firstConnection.ts";

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


test("only a typed bind failure for the current port shows the port guide", () => {
  const conflict = { kind: "portUnavailable", message: "any localized details", port: 7897 };
  assert.equal(isPortUnavailable(conflict, 7897), true);
  assert.equal(isPortUnavailable(conflict, 7898), false);
  assert.equal(isPortUnavailable({ kind: "startFailed", message: "无法绑定", port: null }, 7897), false);
  assert.equal(isPortUnavailable("无法绑定", 7897), false);
  assert.equal(isPortUnavailable(null, 7897), false);
});
