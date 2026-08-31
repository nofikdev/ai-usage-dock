export type ProviderId = "codex-account-1" | "codex-account-2" | "claude";
export type ProviderStatus = "loading" | "healthy" | "stale" | "auth_required" | "unavailable";
export type UsageWindowType = "five_hour" | "weekly";
export type AnnouncementFeedStatus = "loading" | "healthy" | "stale" | "unavailable";

export interface UsageWindow {
  type: UsageWindowType;
  durationMinutes: number;
  usedPercent: number;
  remainingPercent: number;
  resetsAt: number | null;
}

export interface UsageSnapshot {
  provider: "codex" | "claude";
  accountId: ProviderId;
  displayName: string;
  plan: string;
  accountIdentity?: string | null;
  rateLimitReachedType?: string | null;
  fetchedAt: number | null;
  status: ProviderStatus;
  error: string | null;
  windows: UsageWindow[];
}

export interface DockSettings {
  startWithWindows: boolean;
  alwaysOnTop: boolean;
  labels: Record<ProviderId, string>;
}

export interface AnnouncementItem {
  id: string;
  publishedAt: number | null;
  text: string;
  url: string;
  category: string;
}

export interface AnnouncementFeed {
  status: AnnouncementFeedStatus;
  fetchedAt: number | null;
  error: string | null;
  items: AnnouncementItem[];
  lastSeenId: string | null;
}

export interface PanelState {
  snapshots: UsageSnapshot[];
  settings: DockSettings;
  hasFetched: boolean;
  lastUpdatedAt: number | null;
  announcements: AnnouncementFeed;
}
