import assert from "node:assert/strict";
import test from "node:test";
import { subscriptionCleanup } from "./unlisten.ts";

const tick = () => new Promise(resolve => setImmediate(resolve));

test("subscription rejection is handled while the component remains mounted", async () => {
  const cleanup = subscriptionCleanup(Promise.reject(new Error("registration failed")));
  await tick(); // node:test fails on unhandled rejection before cleanup.
  cleanup();
  await tick();
});

test("a listener registered after unmount is still removed exactly once", async () => {
  let registered;
  let removed = 0;
  const cleanup = subscriptionCleanup(new Promise(resolve => { registered = resolve; }));
  cleanup();
  registered(() => { removed += 1; });
  await tick();
  assert.equal(removed, 1);
});

test("unlisten exceptions do not become unhandled rejections", async () => {
  const cleanup = subscriptionCleanup(Promise.resolve(() => { throw new Error("channel closed"); }));
  cleanup();
  await tick();
});
