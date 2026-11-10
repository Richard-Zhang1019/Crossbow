import { test } from "node:test";
import assert from "node:assert/strict";
import {
  sortConnections,
  parseNavigation,
  notificationDestination,
  DIAG_STEPS,
} from "./workflow.ts";
test("connection sort is stable, non-mutating and puts unknown ages last", () => {
  const rows = [
    { id: "b", down: 1, up: 0, elapsed_ms: null },
    { id: "a", down: 3, up: 8, elapsed_ms: 100 },
    { id: "c", down: 3, up: 1, elapsed_ms: 500 },
  ];
  assert.deepEqual(
    sortConnections(rows, "down").map((r) => r.id),
    ["a", "c", "b"],
  );
  assert.deepEqual(
    sortConnections(rows, "age").map((r) => r.id),
    ["c", "a", "b"],
  );
  assert.equal(rows[0].id, "b");
});
test("notification navigation accepts known pages and controls only", () => {
  assert.equal(parseNavigation('{"page":"https://example.com"}'), null);
  assert.equal(parseNavigation("broken"), null);
  assert.equal(
    parseNavigation('{"page":"settings","control":"danger","settingSection":"network"}').control,
    undefined,
  );
  assert.equal(
    parseNavigation('{"page":"profiles","profileId":"test","control":"cb-profile-detail"}')
      .profileId,
    "test",
  );
  assert.equal(
    parseNavigation('{"page":"diagnostics","checkId":"core_running"}').checkId,
    "core_running",
  );
});
test("failure actions route to the operation rather than generic diagnostics", () => {
  assert.deepEqual(notificationDestination("setting-port"), ["settings"]);
  assert.deepEqual(notificationDestination("profile-update-abc"), ["profiles"]);
  assert.deepEqual(notificationDestination("logs-copy"), []);
  assert.equal(DIAG_STEPS.length, 7);
});
