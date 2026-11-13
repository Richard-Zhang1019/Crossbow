import { pruneActivity, receiveActivity } from "./activity.mjs";

const GITHUB_API = "https://api.github.com";
const REPOSITORY = `${GITHUB_API}/repos/Richard-Zhang1019/Crossbow`;
const SHANGHAI_OFFSET_MS = 8 * 60 * 60 * 1000;
const RELEASE_PAGE_LIMIT = 10;
const DAY_MS = 24 * 60 * 60_000;
const CUTOFF_MS = 17 * 60_000; // Beijing 00:17, leaving time for the 00:15 run.
const RETRY_DELAYS = [1_000, 2_000];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

class SnapshotError extends Error {
  constructor(code, { status, retryable = false, retryAt } = {}) {
    super(status ? `${code} (HTTP ${status})` : code);
    this.code = code;
    this.status = status;
    this.retryable = retryable;
    this.retryAt = retryAt;
  }
}

async function snapshotStage(stage, context, operation) {
  const { day, deadline, now, wait, logger } = context;
  for (let attempt = 1; ; attempt++) {
    try {
      if (now() >= deadline) throw new SnapshotError("window_closed");
      return await operation();
    } catch (error) {
      const failure = error instanceof SnapshotError ? error : new SnapshotError("unexpected_error");
      const delay = RETRY_DELAYS[attempt - 1];
      const retry = failure.retryable && delay !== undefined && now() + delay < deadline;
      const event = { day, stage, attempt, outcome: retry ? "retry" : "failed",
        code: failure.code, httpStatus: failure.status, retryAt: failure.retryAt };
      if (!retry) {
        logger.error(event);
        throw failure;
      }
      logger.warn(event);
      await wait(delay);
    }
  }
}

function rateLimitUntil(response, now) {
  const retryAfter = response.headers.get("retry-after");
  const reset = response.headers.get("x-ratelimit-reset");
  const exhausted = response.headers.get("x-ratelimit-remaining") === "0";
  const minimum = retryAfter && /^\d+$/.test(retryAfter) ? now + Number(retryAfter) * 1000 : Date.parse(retryAfter);
  const resetAt = exhausted && reset && /^\d+$/.test(reset) ? Number(reset) * 1000 : 0;
  const until = Math.max(Number.isSafeInteger(minimum) ? minimum : 0, Number.isSafeInteger(resetAt) ? resetAt : 0);
  return until > now ? until : now + 60_000;
}

async function d1(operation) {
  try {
    return await operation();
  } catch (error) {
    const message = `${error?.message ?? ""} ${error?.cause?.message ?? ""}`;
    const retryable = /network connection lost|storage caused object to be reset|reset because its code was updated|replica disconnected from primary|transient issue on remote node/i.test(message);
    throw new SnapshotError(retryable ? "d1_transient" : "d1_error", { retryable });
  }
}

function platformFor(name) {
  if (/^Crossbow_.+\.dmg$/i.test(name)) return "macos";
  if (/^Crossbow_.+-setup\.exe$/i.test(name)) return "windows";
  if (/^Crossbow_.+_linux_[\w-]+\.tar\.gz$/i.test(name)) return "linux";
  return null;
}

function isCount(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

async function githubJson(url, request, context) {
  const { now, deadline, db } = context;
  if (now() >= deadline) throw new SnapshotError("window_closed");
  let response;
  try {
    response = await request(url, {
      headers: {
        accept: "application/vnd.github+json",
        "user-agent": "crossbow-metrics",
        "x-github-api-version": "2026-03-10",
      },
      signal: AbortSignal.timeout(Math.max(1, Math.min(10_000, deadline - now()))),
    });
  } catch (error) {
    const code = error?.name === "TimeoutError" ? "github_timeout" : "github_network";
    throw new SnapshotError(code, { retryable: true });
  }

  if (!response.ok) {
    const status = response.status;
    const rateLimited = status === 429 || (status === 403 && (
      response.headers.has("retry-after") || response.headers.get("x-ratelimit-remaining") === "0" ||
      /rate limit/i.test(await response.text().catch(() => ""))
    ));
    if (rateLimited) {
      const retryAt = rateLimitUntil(response, now());
      context.logger.warn({ day: context.day, stage: "github", outcome: "rate_limited", httpStatus: status, retryAt });
      // Persist a rate limit even when the response arrived at the collection cutoff.
      await snapshotStage("write_cooldown", { ...context, deadline: now() + 10_000 }, () => d1(() => db.prepare(
        `INSERT INTO github_snapshot_cooldown (id, retry_at) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET retry_at = MAX(retry_at, excluded.retry_at)`,
      ).bind(retryAt).run()));
      throw new SnapshotError("github_rate_limited", { status, retryAt });
    }
    throw new SnapshotError("github_http", { status, retryable: status >= 500 });
  }

  try {
    return { data: await response.json(), link: response.headers.get("link") };
  } catch (error) {
    if (error instanceof SyntaxError) throw new SnapshotError("github_invalid_json");
    throw new SnapshotError("github_network", { retryable: true });
  }
}

async function releaseAssets(request, context) {
  const assets = new Map();
  let url = `${REPOSITORY}/releases?per_page=50`;
  for (let page = 0; url; page++) {
    if (page === RELEASE_PAGE_LIMIT || new URL(url).origin !== GITHUB_API) {
      throw new SnapshotError("github_release_pagination");
    }
    const { data, link } = await githubJson(url, request, context);
    if (!Array.isArray(data)) throw new SnapshotError("github_invalid_releases");

    for (const release of data) {
      if (release?.draft || release?.prerelease) continue;
      if (typeof release?.tag_name !== "string" || !Array.isArray(release.assets)) {
        throw new SnapshotError("github_incomplete_release");
      }
      for (const asset of release.assets) {
        const platform = typeof asset?.name === "string" && platformFor(asset.name);
        if (!platform) continue;
        if (!isCount(asset.id) || asset.id === 0 || !isCount(asset.download_count) ||
            !Number.isFinite(Date.parse(asset.created_at))) {
          throw new SnapshotError("github_incomplete_asset");
        }
        assets.set(asset.id, {
          id: asset.id,
          name: asset.name,
          tag: release.tag_name,
          platform,
          created_at: asset.created_at,
          download_count: asset.download_count,
        });
      }
    }
    url = link?.match(/<([^>]+)>;\s*rel="next"/)?.[1] ?? null;
  }
  if (assets.size === 0) throw new SnapshotError("github_no_installers");
  return [...assets.values()];
}

export async function collectSnapshot(db, scheduledTime, request = fetch, {
  now = Date.now, wait = sleep, logger = console,
} = {}) {
  const midnight = Math.floor((scheduledTime + SHANGHAI_OFFSET_MS) / DAY_MS) * DAY_MS - SHANGHAI_OFFSET_MS;
  const day = new Date(midnight + SHANGHAI_OFFSET_MS).toISOString().slice(0, 10);
  const deadline = midnight + CUTOFF_MS;
  const context = { db, day, deadline, now, wait, logger };

  function skip(reason, fields = {}) {
    logger.log({ day, stage: "snapshot", outcome: "skipped", reason, ...fields });
    return { day, inserted: false, reason };
  }

  if (now() < midnight || now() >= deadline) return skip("outside_window");

  const state = await snapshotStage("check_snapshot", context, () => d1(() => db.prepare(
    `SELECT EXISTS (SELECT 1 FROM github_daily_snapshots WHERE day = ?) AS present,
            (SELECT retry_at FROM github_snapshot_cooldown WHERE id = 1) AS retry_at`,
  ).bind(day).first()));
  if (state.present) return skip("already_collected");
  if (state.retry_at > now()) return skip("github_cooldown", { retryAt: state.retry_at });

  const stars = await snapshotStage("github_stars", context, async () => {
    const repo = await githubJson(REPOSITORY, request, context);
    if (!isCount(repo.data?.stargazers_count)) throw new SnapshotError("github_invalid_stars");
    return repo.data.stargazers_count;
  });
  logger.log({ day, stage: "github_stars", outcome: "ok" });
  const assets = await snapshotStage("github_releases", context, () => releaseAssets(request, context));
  logger.log({ day, stage: "github_releases", outcome: "ok", assetCount: assets.length });

  const collectedAt = new Date(now()).toISOString();
  const { meta } = await snapshotStage("write_snapshot", context, () => d1(() => db.prepare(
    `INSERT INTO github_daily_snapshots (day, collected_at, stars, assets_json)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(day) DO NOTHING`,
  ).bind(day, collectedAt, stars, JSON.stringify(assets)).run()));
  const inserted = meta.changes === 1;
  logger.log({ day, stage: "write_snapshot", outcome: inserted ? "inserted" : "already_collected", collectedAt });
  return { day, assetCount: assets.length, inserted };
}

export default {
  fetch(request, env) {
    const { pathname } = new URL(request.url);
    if (pathname === "/api/v1/active") return receiveActivity(request, env.DB);
    if (pathname !== "/" && pathname !== "/healthz") {
      return new Response(null, { status: 404 });
    }
    if (request.method !== "GET") {
      return new Response(null, { status: 405, headers: { allow: "GET" } });
    }
    return Response.json({ service: "crossbow-metrics", status: "ok" });
  },
  scheduled(controller, env, ctx) {
    const time = controller.scheduledTime;
    ctx.waitUntil(collectSnapshot(env.DB, time));
    if (controller.cron === "5 16 * * *") {
      ctx.waitUntil(pruneActivity(env.DB, time).catch(() => {
        console.error({ stage: "prune_activity", outcome: "failed", code: "d1_error" });
        throw new SnapshotError("prune_activity_failed");
      }));
    }
  },
};
