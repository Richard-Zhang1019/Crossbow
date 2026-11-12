import { pruneActivity, receiveActivity } from "./activity.mjs";

const GITHUB_API = "https://api.github.com";
const REPOSITORY = `${GITHUB_API}/repos/Richard-Zhang1019/Crossbow`;
const SHANGHAI_OFFSET_MS = 8 * 60 * 60 * 1000;
const RELEASE_PAGE_LIMIT = 10;

function platformFor(name) {
  if (/^Crossbow_.+\.dmg$/i.test(name)) return "macos";
  if (/^Crossbow_.+-setup\.exe$/i.test(name)) return "windows";
  if (/^Crossbow_.+_linux_[\w-]+\.tar\.gz$/i.test(name)) return "linux";
  return null;
}

function isCount(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

async function githubJson(url, request) {
  const response = await request(url, {
    headers: {
      accept: "application/vnd.github+json",
      "user-agent": "crossbow-metrics",
      "x-github-api-version": "2026-03-10",
    },
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    throw new Error(`GitHub returned HTTP ${response.status} for ${new URL(url).pathname}`);
  }
  return { data: await response.json(), link: response.headers.get("link") };
}

async function releaseAssets(request) {
  const assets = new Map();
  let url = `${REPOSITORY}/releases?per_page=50`;
  for (let page = 0; url; page++) {
    if (page === RELEASE_PAGE_LIMIT || new URL(url).origin !== GITHUB_API) {
      throw new Error("GitHub release pagination exceeded the collection limit");
    }
    const { data, link } = await githubJson(url, request);
    if (!Array.isArray(data)) throw new Error("GitHub releases response was not an array");

    for (const release of data) {
      if (release.draft || release.prerelease) continue;
      if (typeof release.tag_name !== "string" || !Array.isArray(release.assets)) {
        throw new Error("GitHub returned an incomplete release");
      }
      for (const asset of release.assets) {
        const platform = platformFor(asset.name);
        if (!platform) continue;
        if (!isCount(asset.id) || asset.id === 0 || !isCount(asset.download_count) ||
            !Number.isFinite(Date.parse(asset.created_at))) {
          throw new Error("GitHub returned an incomplete installation asset");
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
  if (assets.size === 0) throw new Error("No Crossbow installation assets found");
  return [...assets.values()];
}

export async function collectSnapshot(db, scheduledTime, request = fetch) {
  const day = new Date(scheduledTime + SHANGHAI_OFFSET_MS).toISOString().slice(0, 10);
  const [repo, assets] = await Promise.all([
    githubJson(REPOSITORY, request),
    releaseAssets(request),
  ]);
  if (!isCount(repo.data?.stargazers_count)) {
    throw new Error("GitHub returned an invalid repository star count");
  }

  // One statement commits the complete snapshot. Re-running a successful
  // schedule preserves the original observation for that day.
  const { meta } = await db.prepare(
    `INSERT INTO github_daily_snapshots (day, collected_at, stars, assets_json)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(day) DO NOTHING`,
  ).bind(day, new Date().toISOString(), repo.data.stargazers_count, JSON.stringify(assets)).run();
  return { day, assetCount: assets.length, inserted: meta.changes === 1 };
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
    ctx.waitUntil(collectSnapshot(env.DB, time).then(snapshot => console.log("GitHub snapshot", snapshot)));
    ctx.waitUntil(pruneActivity(env.DB, time));
  },
};
