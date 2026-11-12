import { test } from "node:test";
import assert from "node:assert/strict";
import { runInNewContext } from "node:vm";
import { BOOTSTRAP } from "../src/lib/bootstrap.js";

// Execute the real pre-paint bootstrap: the theme must agree with saved
// preferences before first paint, including browsers where storage is denied.
// Language is URL-based since the Astro migration (#363): each locale page
// writes its own <html lang> at build time, so the bootstrap no longer
// touches it.
function load({ theme, systemDark = false, blocked = false } = {}) {
  const root = { dataset: {} };
  const meta = {};
  runInNewContext(BOOTSTRAP, {
    document: { documentElement: root, querySelector: () => meta },
    localStorage: { getItem: () => {
      if (blocked) throw new Error("Storage denied");
      return theme;
    } },
    matchMedia: () => ({ matches: systemDark }),
  });
  return { theme: root.dataset.theme, chrome: meta.content };
}

test("saved light appearance overrides a dark system before paint", () => {
  assert.deepEqual(load({ theme: "light", systemDark: true }), { theme: "light", chrome: "#ffffff" });
});
test("saved dark appearance survives a new document", () => {
  assert.deepEqual(load({ theme: "dark" }), { theme: "dark", chrome: "#1a1c21" });
});
test("system and absent preferences use the device appearance", () => {
  for (const theme of [undefined, "system", "invalid"]) {
    assert.equal(load({ theme, systemDark: true }).theme, "dark");
    assert.equal(load({ theme, systemDark: false }).theme, "light");
  }
});
test("blocked storage does not prevent startup or theme resolution", () => {
  assert.deepEqual(load({ blocked: true, systemDark: true }), { theme: "dark", chrome: "#1a1c21" });
});
