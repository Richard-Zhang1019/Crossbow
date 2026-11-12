import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { pruneActivity, receiveActivity } from "../src/activity.mjs";

const dayMs = 86_400_000;
const midnight = Date.parse("2026-11-10T16:00:00Z");
const report = {
  day: Math.floor((midnight + 8 * 3600_000) / dayMs),
  daily_id: "1234567890abcdef1234567890abcdef",
  platform: "windows",
  version: "0.14.2",
};

function activityDatabase(t) {
  const sqlite = new DatabaseSync(":memory:");
  t.after(() => sqlite.close());
  sqlite.exec(readFileSync(new URL("../migrations/0002_activity_reports.sql", import.meta.url), "utf8"));
  return {
    prepare(sql) {
      return {
        bind(...values) {
          return { run: async () => sqlite.prepare(sql).run(...values) };
        },
      };
    },
    rawCount: () => sqlite.prepare("SELECT count(*) AS n FROM activity_reports").get().n,
    totals: () => sqlite.prepare("SELECT devices FROM activity_daily_totals ORDER BY day").all().map(row => row.devices),
  };
}

function post(payload) {
  return new Request("https://example.com/api/v1/active", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
}

test("only the first report for a day and ID counts, even if retries use different metadata", async (t) => {
  const db = activityDatabase(t);
  for (const payload of [report, report, { ...report, platform: "macos" }]) {
    assert.equal((await receiveActivity(post(payload), db, midnight)).status, 204);
  }
  const other = { ...report, daily_id: "11111111111111111111111111111111" };
  assert.equal((await receiveActivity(post(other), db, midnight)).status, 204);
  assert.equal(db.rawCount(), 2);
  assert.deepEqual(db.totals(), [2]);

  await pruneActivity(db, midnight + 7 * dayMs);
  assert.equal(db.rawCount(), 0);
  assert.deepEqual(db.totals(), [2]);
});

test("rejects invalid bodies and dates before touching the database", async () => {
  for (const invalid of [
    { ...report, ip: "127.0.0.1" },
    { ...report, daily_id: "a".repeat(1100) },
    { ...report, day: report.day + 1 },
    { ...report, day: report.day - 2 },
    { ...report, daily_id: "not-an-id" },
    { ...report, platform: "ios" },
    { ...report, version: "not-a-version" },
  ]) {
    assert.equal((await receiveActivity(post(invalid), null, midnight)).status, 400);
  }
});

test("allows an in-flight report to arrive just after Beijing midnight", async (t) => {
  const db = activityDatabase(t);
  const previous = { ...report, day: report.day - 1 };
  assert.equal((await receiveActivity(post(previous), db, midnight + 60_000)).status, 204);
  assert.equal((await receiveActivity(post(previous), db, midnight + 300_000)).status, 400);
  assert.equal((await receiveActivity(post(report), null, midnight)).status, 503);
  assert.deepEqual(db.totals(), [1]);
});
