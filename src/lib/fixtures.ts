import type { DockSettings, PanelState, ProviderId, UsageSnapshot, UsageWindow } from "../types";

const now = Math.floor(Date.now() / 1000);

const settings: DockSettings = {
  startWithWindows: true,
  alwaysOnTop: true,
  labels: {
    "codex-account-1": "Codex Main",
    "codex-account-2": "Codex Extra",
    claude: "Claude",
  },
};

function windowSnapshot(type: "five_hour" | "weekly", remainingPercent: number, resetsAt: number): UsageWindow {
  return {
    type,
    durationMinutes: type === "five_hour" ? 300 : 10080,
    usedPercent: 100 - remainingPercent,
    remainingPercent,
    resetsAt,
  };
}

const fixtureSnapshots: UsageSnapshot[] = [
  {
    provider: "codex",
    accountId: "codex-account-1",
    displayName: "Codex Main",
    plan: "ChatGPT Plus",
    fetchedAt: now,
    status: "healthy",
    error: null,
    windows: [
      windowSnapshot("five_hour", 72, now + 60 * 60 * 2 + 42 * 60),
      windowSnapshot("weekly", 48, now + 60 * 60 * 22),
    ],
  },
  {
    provider: "codex",
    accountId: "codex-account-2",
    displayName: "Codex Extra",
    plan: "ChatGPT Plus",
    fetchedAt: now,
    status: "healthy",
    error: null,
    windows: [
      windowSnapshot("five_hour", 88, now + 60 * 60 * 4 + 5 * 60),
      windowSnapshot("weekly", 31, now + 60 * 60 * 70),
    ],
  },
  {
    provider: "claude",
    accountId: "claude",
    displayName: "Claude",
    plan: "Claude Pro",
    fetchedAt: now,
    status: "healthy",
    error: null,
    windows: [
      windowSnapshot("five_hour", 63, now + 60 * 60 * 1 + 15 * 60),
      windowSnapshot("weekly", 74, now + 60 * 60 * 46),
    ],
  },
];

export function createFixtureState(): PanelState {
  return {
    snapshots: fixtureSnapshots,
    settings,
    hasFetched: true,
    lastUpdatedAt: now,
  };
}

export function emptySnapshot(accountId: ProviderId): UsageSnapshot {
  const isClaude = accountId === "claude";
  return {
    provider: isClaude ? "claude" : "codex",
    accountId,
    displayName: settings.labels[accountId],
    plan: isClaude ? "Claude Pro" : "ChatGPT Plus",
    fetchedAt: null,
    status: "auth_required",
    error: null,
    windows: [],
  };
}
