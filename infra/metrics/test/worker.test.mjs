import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { collectSnapshot } from "../src/worker.mjs";

const schema = readFileSync(
  new URL("../migrations/0001_github_daily_snapshots.sql", import.meta.url),
  "utf8",
);
const tick = Date.parse("2026-11-10T16:05:00Z");

function testDatabase(t) {
  const sqlite = new DatabaseSync(":memory:");
  t.after(() => sqlite.close());
  sqlite.exec(schema);
  return {
    prepare(sql) {
      return {
        bind(...values) {
          return {
            async run() {
              return { meta: { changes: sqlite.prepare(sql).run(...values).changes } };
            },
          };
        },
      };
    },
    rows: () => sqlite.prepare("SELECT * FROM github_daily_snapshots").all(),
  };
}

function asset(id, name, count = 10) {
  return {
    id, name, download_count: count, created_at: "2026-11-01T00:00:00Z",
  };
}

function github({ stars = 123, failPageTwo = false } = {}) {
  return async (url) => {
    if (!url.includes("/releases")) return Response.json({ stargazers_count: stars });
    if (url.includes("page=2")) {
      if (failPageTwo) return new Response(null, { status: 503 });
      return Response.json([
        { tag_name: "v0.14.1", assets: [asset(3, "Crossbow_0.14.1_linux_x86_64.tar.gz")] },
        { tag_name: "v0.14.2-rc", prerelease: true, assets: [asset(8, "Crossbow_0.14.2-rc_aarch64.dmg")] },
      ]);
    }
    return Response.json(
      [{
        tag_name: "v0.14.1",
        assets: [
          asset(1, "Crossbow_0.14.1_aarch64.dmg"),
          asset(2, "Crossbow_0.14.1_x64-setup.exe"),
          asset(4, "Crossbow_0.14.1_x64-setup.exe.sig"),
          asset(5, "Crossbow.app.tar.gz"),
          asset(6, "latest.json"),
          asset(7, "SHA256SUMS"),
        ],
      }],
      { headers: { link: '<https://api.github.com/repositories/1/releases?page=2>; rel="next"' } },
    );
  };
}

test("collects stable installation assets across pages in one Beijing-day snapshot", async (t) => {
  const db = testDatabase(t);
  await collectSnapshot(db, tick, github());
  const [row] = db.rows();
  assert.equal(row.day, "2026-11-11");
  assert.equal(row.stars, 123);
  const assets = JSON.parse(row.assets_json);
  assert.deepEqual(assets.map(({ id, platform, download_count }) => [id, platform, download_count]), [
    [1, "macos", 10], [2, "windows", 10], [3, "linux", 10],
  ]);
});

test("retry preserves the first snapshot; the next day stores a separate one", async (t) => {
  const db = testDatabase(t);
  assert.equal((await collectSnapshot(db, tick, github())).inserted, true);
  assert.equal((await collectSnapshot(db, tick + 60_000, github({ stars: 124 }))).inserted, false);
  await collectSnapshot(db, tick + 86_400_000, github({ stars: 125 }));
  assert.deepEqual(db.rows().map(({ stars }) => stars), [123, 125]);
});

test("failed GitHub page does not write a partial snapshot", async (t) => {
  const db = testDatabase(t);
  await assert.rejects(collectSnapshot(db, tick, github({ failPageTwo: true })), /HTTP 503/);
  assert.deepEqual(db.rows(), []);
});
