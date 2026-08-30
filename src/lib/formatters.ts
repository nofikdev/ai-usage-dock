import type { ProviderStatus, UsageWindowType } from "../types";

const timeFormatter = new Intl.DateTimeFormat("nl-NL", {
  hour: "2-digit",
  minute: "2-digit",
});

const dayTimeFormatter = new Intl.DateTimeFormat("nl-NL", {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
});

export function formatReset(timestamp: number | null): string {
  if (!timestamp) return "—";

  const date = new Date(timestamp * 1000);
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  return sameDay ? timeFormatter.format(date) : dayTimeFormatter.format(date).replace(".", "");
}

export function formatFetchedAt(timestamp: number | null): string {
  if (!timestamp) return "nog niet bijgewerkt";
  return timeFormatter.format(new Date(timestamp * 1000));
}

export function formatAge(timestamp: number | null): string {
  if (!timestamp) return "";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - timestamp);
  if (seconds < 60) return "zojuist";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m oud`;
  return `${Math.floor(minutes / 60)}u oud`;
}

export function windowLabel(type: UsageWindowType): string {
  return type === "five_hour" ? "5h" : "week";
}

export function usageTone(remainingPercent: number): "normal" | "warning" | "danger" | "limit" {
  if (remainingPercent <= 0) return "limit";
  if (remainingPercent < 10) return "danger";
  if (remainingPercent <= 25) return "warning";
  return "normal";
}

export function statusLabel(status: ProviderStatus): string {
  switch (status) {
    case "healthy":
      return "live";
    case "stale":
      return "stale";
    case "auth_required":
      return "login nodig";
    case "unavailable":
      return "niet beschikbaar";
    case "loading":
      return "laden";
  }
}
