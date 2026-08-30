export type ProviderId = "codex-account-1" | "codex-account-2" | "claude";
export type ProviderStatus = "loading" | "healthy" | "stale" | "auth_required" | "unavailable";
export type UsageWindowType = "five_hour" | "weekly";

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

export interface PanelState {
  snapshots: UsageSnapshot[];
  settings: DockSettings;
  hasFetched: boolean;
  lastUpdatedAt: number | null;
}
