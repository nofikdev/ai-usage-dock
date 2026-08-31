import { readFile, writeFile, rename } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const feedPath = path.join(root, "feed", "announcements.json");
const handle = "thsottiaux";
const maxItems = 20;
const retentionSeconds = 30 * 24 * 60 * 60;
const relevantText = /\b(codex|chatgpt|openai|usage|quota|limit|rate[- ]?limit|reset|weekly|daily)\b/i;

const token = process.env.X_BEARER_TOKEN?.trim();
if (!token) throw new Error("X_BEARER_TOKEN is required");

async function getJson(url) {
  const response = await fetch(url, {
    headers: { accept: "application/json", authorization: `Bearer ${token}` },
  });
  if (!response.ok) throw new Error(`X API returned HTTP ${response.status}`);
  return response.json();
}

function categoryFor(text) {
  if (/\b(limit|quota|rate[- ]?limit|reset|usage)\b/i.test(text)) return "usage_limits";
  if (/\bcodex\b/i.test(text)) return "codex";
  return "announcement";
}

function normalizePost(post) {
  const text = post.text?.trim();
  if (!post.id || !text || !relevantText.test(text)) return null;
  return {
    id: String(post.id),
    publishedAt: post.created_at ?? null,
    text: text.slice(0, 600),
    url: `https://x.com/${handle}/status/${post.id}`,
    category: categoryFor(text),
  };
}

const previous = JSON.parse(await readFile(feedPath, "utf8"));
const user = await getJson(`https://api.x.com/2/users/by/username/${handle}`);
if (!user.data?.id) throw new Error("X user could not be resolved");

const posts = await getJson(`https://api.x.com/2/users/${user.data.id}/tweets?max_results=10&exclude=retweets,replies&tweet.fields=created_at,lang`);
const cutoff = Date.now() / 1000 - retentionSeconds;
const fresh = (posts.data ?? []).map(normalizePost).filter(Boolean);
const old = (previous.items ?? []).filter((item) => {
  const timestamp = Date.parse(item.publishedAt ?? "") / 1000;
  return item.id && item.text && (Number.isNaN(timestamp) || timestamp >= cutoff);
});
const byId = new Map([...fresh, ...old].map((item) => [item.id, item]));
const items = [...byId.values()]
  .sort((a, b) => Date.parse(b.publishedAt ?? "") - Date.parse(a.publishedAt ?? ""))
  .slice(0, maxItems);

if (JSON.stringify(previous.items ?? []) === JSON.stringify(items)) {
  console.log("Announcement feed unchanged");
  process.exit(0);
}

const payload = {
  version: 1,
  source: { handle, profileUrl: `https://x.com/${handle}` },
  fetchedAt: new Date().toISOString(),
  items,
};
const temporaryPath = `${feedPath}.tmp`;
await writeFile(temporaryPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
await rename(temporaryPath, feedPath);
console.log(`Announcement feed updated with ${items.length} item(s)`);
