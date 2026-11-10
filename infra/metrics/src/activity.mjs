const DAY_MS = 86_400_000;
const SHANGHAI_OFFSET_MS = 8 * 3600_000;
const MAX_BODY_BYTES = 1024;
const MIDNIGHT_GRACE_MS = 5 * 60_000;
const PLATFORMS = new Set(["macos", "windows", "linux"]);
const VERSION = /^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-z0-9.-]+)?(?:\+[a-z0-9.-]+)?$/i;

function beijingDay(time) {
  return Math.floor((time + SHANGHAI_OFFSET_MS) / DAY_MS);
}

async function readBody(request) {
  if (!request.body) return null;
  const reader = request.body.getReader();
  const bytes = new Uint8Array(MAX_BODY_BYTES);
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, size)));
      if (size + value.length > bytes.length) {
        await reader.cancel();
        return null;
      }
      bytes.set(value, size);
      size += value.length;
    }
  } catch {
    return null;
  } finally {
    reader.releaseLock();
  }
}

function validReport(report, now) {
  if (!report || typeof report !== "object" || Object.keys(report).length !== 4) return false;
  const { day, daily_id, platform, version } = report;
  return Number.isSafeInteger(day) &&
    day >= beijingDay(now - MIDNIGHT_GRACE_MS) && day <= beijingDay(now) &&
    typeof daily_id === "string" && daily_id.length === 32 && /^[a-f0-9]+$/.test(daily_id) &&
    PLATFORMS.has(platform) &&
    typeof version === "string" && version.length <= 48 && VERSION.test(version) && version.trim() === version;
}

export async function receiveActivity(request, db, now = Date.now()) {
  if (request.method !== "POST") {
    return new Response(null, { status: 405, headers: { allow: "POST" } });
  }
  if (request.headers.get("content-type")?.split(";")[0].trim().toLowerCase() !== "application/json") {
    return new Response(null, { status: 415 });
  }
  const report = await readBody(request);
  if (!validReport(report, now)) return new Response(null, { status: 400 });

  const { day, daily_id, platform, version } = report;
  try {
    await db.prepare(
      `INSERT INTO activity_reports (day, daily_id, platform, version)
       VALUES (?, ?, ?, ?)
       ON CONFLICT (day, daily_id) DO NOTHING`,
    ).bind(day, daily_id, platform, version).run();
  } catch {
    return new Response(null, { status: 503 });
  }
  return new Response(null, { status: 204 });
}

export async function pruneActivity(db, time) {
  // Keep seven Beijing calendar days of deduplication records. Totals are independent.
  await db.prepare("DELETE FROM activity_reports WHERE day < ?").bind(beijingDay(time) - 6).run();
}
