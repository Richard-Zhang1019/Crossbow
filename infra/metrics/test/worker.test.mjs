import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import worker, { collectSnapshot } from "../src/worker.mjs";

const schema = [
  "0001_github_daily_snapshots.sql",
  "0002_activity_reports.sql",
  "0003_github_snapshot_cooldown.sql",
].map(name => readFileSync(new URL(`../migrations/${name}`, import.meta.url), "utf8")).join("\n");
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
            async first() {
              return sqlite.prepare(sql).get(...values) ?? null;
            },
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

function fixture(at = tick) {
  const events = [];
  const delays = [];
  const capture = event => events.push(event);
  const options = {
    now: () => at,
    wait: async ms => { delays.push(ms); at += ms; },
    logger: { log: capture, warn: capture, error: capture },
  };
  return { options, events, delays, setTime: next => { at = next; } };
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

test("sends the configured GitHub token as a bearer header", async (t) => {
  const db = testDatabase(t);
  const seen = [];
  const github = async (url, init) => {
    seen.push({ url, authorization: init?.headers?.authorization });
    if (!url.includes("/releases")) return Response.json({ stargazers_count: 123 });
    return Response.json([{
      tag_name: "v0.14.1",
      assets: [asset(1, "Crossbow_0.14.1_aarch64.dmg")],
    }]);
  };
  const { options } = fixture();
  const result = await collectSnapshot(db, tick, github, { ...options, token: "secret-token" });
  assert.equal(result.inserted, true);
  assert.deepEqual(seen.map(({ authorization }) => authorization),
    ["Bearer secret-token", "Bearer secret-token"]);
});

test("collects stable installation assets across pages in one Beijing-day snapshot", async (t) => {
  const db = testDatabase(t);
  await collectSnapshot(db, tick, github(), fixture().options);
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
  const { options, setTime } = fixture();
  assert.equal((await collectSnapshot(db, tick, github(), options)).inserted, true);
  const backup = tick + 5 * 60_000;
  setTime(backup);
  const unexpectedFetch = () => assert.fail("Existing snapshots must skip GitHub");
  assert.equal((await collectSnapshot(db, backup, unexpectedFetch, options)).reason, "already_collected");
  const tomorrow = tick + 86_400_000;
  setTime(tomorrow);
  await collectSnapshot(db, tomorrow, github({ stars: 125 }), options);
  assert.deepEqual(db.rows().map(({ stars }) => stars), [123, 125]);
});

test("failed GitHub page does not write a partial snapshot", async (t) => {
  const db = testDatabase(t);
  const { options, delays, events } = fixture();
  await assert.rejects(collectSnapshot(db, tick, github({ failPageTwo: true }), options), /HTTP 503/);
  assert.deepEqual(delays, [1_000, 2_000]);
  assert.equal(events.at(-1).stage, "github_releases");
  assert.equal(events.at(-1).httpStatus, 503);
  assert.deepEqual(db.rows(), []);
});

test("network and D1 transient failures retry while preserving the actual collection time", async t => {
  const db = testDatabase(t);
  const { options, delays, events } = fixture();
  const request = github();
  let githubCalls = 0;
  let writes = 0;
  const prepare = db.prepare;
  db.prepare = sql => {
    const statement = prepare(sql);
    if (!sql.includes("INSERT INTO github_daily_snapshots")) return statement;
    return { bind(...args) {
      const bound = statement.bind(...args);
      return { async run() {
        if (++writes === 1) throw new Error("D1_ERROR", { cause: new Error("Network connection lost.") });
        return bound.run();
      } };
    } };
  };

  const result = await collectSnapshot(db, tick, async (...args) => {
    if (++githubCalls === 1) throw new DOMException("Timed out", "TimeoutError");
    return request(...args);
  }, options);

  assert.equal(result.inserted, true);
  assert.equal(writes, 2);
  assert.equal(githubCalls, 4);
  assert.deepEqual(delays, [1_000, 1_000]);
  assert.equal(db.rows()[0].collected_at, new Date(tick + 1_000).toISOString());
  assert.deepEqual(events.filter(event => event.outcome === "retry").map(event => event.stage), ["github_stars", "write_snapshot"]);
});

test("GitHub cooldown survives the next Cron and uses the later limit header", async t => {
  const db = testDatabase(t);
  const { options, setTime, delays } = fixture();
  const backup = tick + 5 * 60_000;
  const lastBackup = tick + 10 * 60_000;
  await assert.rejects(collectSnapshot(db, tick, async () => new Response(null, {
    status: 403,
    headers: {
      "retry-after": "60",
      "x-ratelimit-remaining": "0",
      "x-ratelimit-reset": String((backup + 60_000) / 1000),
    },
  }), options), /github_rate_limited/);
  assert.deepEqual(delays, []);
  assert.deepEqual(db.rows(), []);
  setTime(backup);
  const noRequests = () => assert.fail("Cooldown must prevent GitHub calls");
  assert.equal((await collectSnapshot(db, backup, noRequests, options)).reason, "github_cooldown");
  setTime(lastBackup);
  assert.equal((await collectSnapshot(db, lastBackup, github(), options)).inserted, true);
  assert.equal(db.rows()[0].day, "2026-11-11");
  assert.equal(db.rows()[0].collected_at, new Date(lastBackup).toISOString());
});

test("a late execution and a response arriving after cutoff cannot write historical snapshots", async t => {
  const db = testDatabase(t);
  const cutoff = tick + 12 * 60_000;
  const { options, setTime } = fixture(cutoff);
  const noRequests = () => assert.fail("Past-window execution must skip GitHub");
  assert.equal((await collectSnapshot(db, tick, noRequests, options)).reason, "outside_window");
  setTime(cutoff - 100);
  const request = github();
  await assert.rejects(collectSnapshot(db, tick + 10 * 60_000, async (...args) => {
    if (args[0].includes("page=2")) setTime(cutoff);
    return request(...args);
  }, options), /window_closed/);
  assert.deepEqual(db.rows(), []);
});

test("backup Cron does not rerun activity pruning", async t => {
  t.mock.method(Date, "now", () => tick + 12 * 60_000);
  for (const [cron, expectedTasks] of [["5 16 * * *", 2], ["10 16 * * *", 1], ["15 16 * * *", 1]]) {
    const tasks = [];
    worker.scheduled({ cron, scheduledTime: tick }, {
      DB: { prepare: () => ({ bind: () => ({ run: async () => {} }) }) },
    }, { waitUntil: task => tasks.push(task) });
    assert.equal(tasks.length, expectedTasks);
    await Promise.all(tasks);
  }
});
