import { useEffect, useMemo, useState, type PointerEvent } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { createFixtureState, emptySnapshot } from "./lib/fixtures";
import { compactStatusLabel, formatAge, formatReset, statusLabel, usageTone, windowLabel } from "./lib/formatters";
import { invokeNative, isNativeRuntime, listenNative, startNativeDragging } from "./lib/tauri";
import type { DockSettings, PanelState, ProviderId, ProviderStatus, UsageSnapshot, UsageWindow } from "./types";

const providerOrder: ProviderId[] = ["codex-account-1", "codex-account-2", "claude"];

function handleDragStart(event: PointerEvent<HTMLDivElement>) {
  if (event.button !== 0 || !isNativeRuntime) return;
  event.preventDefault();
  void startNativeDragging();
}

function App() {
  const [panel, setPanel] = useState<PanelState>(() => createFixtureState());
  const [view, setView] = useState<"dock" | "settings">("dock");
  const [onboardingDismissed, setOnboardingDismissed] = useState(false);
  const [isLoading, setIsLoading] = useState(isNativeRuntime);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [actionAccount, setActionAccount] = useState<ProviderId | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    async function loadNativeState() {
      if (!isNativeRuntime) return;

      try {
        const initial = await invokeNative<PanelState>("get_initial_state");
        if (!disposed) {
          setPanel(initial);
          setIsLoading(false);
        }

        const stopListening = await listenNative<PanelState>("usage-updated", (next) => {
          if (!disposed) setPanel(next);
        });
        const stopSettings = await listenNative<void>("open-settings", () => {
          if (!disposed) setView("settings");
        });

        if (disposed) {
          stopListening();
          stopSettings();
        } else {
          unlisten = () => {
            stopListening();
            stopSettings();
          };
        }
      } catch {
        if (!disposed) {
          setIsLoading(false);
          setLoadError("De dock kan de lokale providerlaag niet starten.");
        }
      }
    }

    void loadNativeState();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const snapshots = useMemo(
    () => providerOrder.map((accountId) => panel.snapshots.find((snapshot) => snapshot.accountId === accountId) ?? emptySnapshot(accountId)),
    [panel.snapshots],
  );
  const hasConnectedProvider = snapshots.some((snapshot) => snapshot.windows.length > 0 || snapshot.status === "loading");
  const showOnboarding = !onboardingDismissed && !isLoading && !hasConnectedProvider && snapshots.every((snapshot) => snapshot.status === "auth_required");

  async function refresh() {
    setIsRefreshing(true);
    setLoadError(null);
    try {
      if (isNativeRuntime) {
        setPanel(await invokeNative<PanelState>("refresh_usage"));
      } else {
        setPanel(createFixtureState());
      }
    } catch {
      setLoadError("Vernieuwen lukte niet. De laatste geldige waarden blijven staan.");
    } finally {
      setIsRefreshing(false);
    }
  }

  async function connect(accountId: ProviderId) {
    setActionAccount(accountId);
    setLoadError(null);
    try {
      if (isNativeRuntime) {
        if (accountId === "claude") await invokeNative("reconnect_provider", { accountId });
        else await invokeNative("connect_codex", { accountId });
        setPanel(await invokeNative<PanelState>("refresh_usage"));
      } else {
        setPanel((current) => ({
          ...current,
          snapshots: current.snapshots.map((snapshot) =>
            snapshot.accountId === accountId ? { ...snapshot, status: "loading" } : snapshot,
          ),
        }));
      }
    } catch {
      setLoadError("Verbinden lukte niet. Controleer de lokale login en probeer opnieuw.");
    } finally {
      setActionAccount(null);
    }
  }

  async function saveSettings(settings: DockSettings) {
    try {
      if (isNativeRuntime) setPanel(await invokeNative<PanelState>("update_settings", { settings }));
      else setPanel((current) => ({ ...current, settings }));
      setView("dock");
    } catch {
      setLoadError("Instellingen konden niet worden opgeslagen.");
    }
  }

  async function hideWindow() {
    if (isNativeRuntime) await invokeNative("hide_window");
  }

  if (view === "settings") {
    return (
      <SettingsView
        settings={panel.settings}
        snapshots={snapshots}
        actionAccount={actionAccount}
        onBack={() => setView("dock")}
        onSave={saveSettings}
        onReconnect={connect}
      />
    );
  }

  if (showOnboarding) {
    return (
      <OnboardingView
        snapshots={snapshots}
        actionAccount={actionAccount}
        onConnect={connect}
        onStart={() => setOnboardingDismissed(true)}
      />
    );
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="drag-handle" data-tauri-drag-region onPointerDown={handleDragStart}>
          <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
          <h1>AI usage</h1>
        </div>
        <div className="top-actions">
          <button className="icon-button" type="button" aria-label="Vernieuwen" title="Vernieuwen" onClick={() => void refresh()} disabled={isRefreshing}>
            <RefreshIcon spinning={isRefreshing} />
          </button>
          <button className="icon-button" type="button" aria-label="Instellingen" title="Instellingen" onClick={() => setView("settings")}>
            <SettingsIcon />
          </button>
          <button className="icon-button close-button" type="button" aria-label="Naar systeemvak verbergen" title="Naar systeemvak verbergen" onClick={() => void hideWindow()}>
            <CloseIcon />
          </button>
        </div>
      </header>

      {loadError ? <div className="notice notice-error" role="status">{loadError}</div> : null}

      <section className="provider-list" aria-label="AI usage per provider">
        {snapshots.map((snapshot) => (
          <ProviderSection
            key={snapshot.accountId}
            snapshot={snapshot}
            onConnect={connect}
            actionAccount={actionAccount}
          />
        ))}
      </section>

    </main>
  );
}

function ProviderSection({
  snapshot,
  onConnect,
  actionAccount,
}: {
  snapshot: UsageSnapshot;
  onConnect: (accountId: ProviderId) => Promise<void>;
  actionAccount: ProviderId | null;
}) {
  const identity = snapshot.displayName || (snapshot.accountId === "claude" ? "Claude" : "Codex");
  const actionLabel = snapshot.accountId === "claude" ? "Reconnect" : "Connect";
  const isConnecting = actionAccount === snapshot.accountId;
  const isActionable = snapshot.status === "auth_required" || snapshot.status === "unavailable";
  const statusText = compactStatusLabel(snapshot.status);
  const details = [snapshot.plan, snapshot.accountIdentity, snapshot.rateLimitReachedType ? `limit: ${snapshot.rateLimitReachedType}` : null, snapshot.error].filter(Boolean).join(" · ");
  const statusDescription = snapshot.status === "healthy" ? "live" : statusLabel(snapshot.status);

  return (
    <article className="provider-section">
      <div className="provider-heading">
        <span className="provider-name">{identity}</span>
        <span className={`provider-status provider-status-${snapshot.status}`} title={details || statusDescription} aria-label={`${identity}: ${statusDescription}`}>
          <span className="status-dot" aria-hidden="true" />
          {statusText}
        </span>
      </div>

      {snapshot.windows.length > 0 ? (
        <div className="usage-window-list">
          {snapshot.windows.map((usageWindow) => <UsageLine key={usageWindow.type} usageWindow={usageWindow} />)}
        </div>
      ) : (
        <div className="provider-empty">
          <span>{providerMessage(snapshot.status)}</span>
          {isActionable ? (
            <button className="inline-button" type="button" onClick={() => void onConnect(snapshot.accountId)} disabled={isConnecting}>
              {isConnecting ? "Bezig…" : actionLabel}
            </button>
          ) : null}
        </div>
      )}

      {snapshot.windows.length > 0 && snapshot.status === "stale" ? (
        <span className="sr-only">Laatste geldige data: {formatAge(snapshot.fetchedAt)}</span>
      ) : null}
    </article>
  );
}

function UsageLine({ usageWindow }: { usageWindow: UsageWindow }) {
  const remaining = Math.max(0, Math.min(100, Math.round(usageWindow.remainingPercent)));
  const tone = usageTone(remaining);
  return (
    <div className="usage-line">
      <span className="window-label">{windowLabel(usageWindow.type)}</span>
      <div className={`usage-bar usage-bar-${tone}`} role="progressbar" aria-label={`${windowLabel(usageWindow.type)} resterend`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={remaining}>
        <span style={{ width: `${remaining}%` }} />
      </div>
      <span className={`remaining-value remaining-${tone}`}>{tone === "limit" ? "LIMIT" : `${remaining}%`}</span>
      <time className="reset-time" dateTime={usageWindow.resetsAt ? new Date(usageWindow.resetsAt * 1000).toISOString() : undefined} title={usageWindow.resetsAt ? new Date(usageWindow.resetsAt * 1000).toLocaleString("nl-NL") : undefined}>
        {formatReset(usageWindow.resetsAt)}
      </time>
    </div>
  );
}

function OnboardingView({
  snapshots,
  actionAccount,
  onConnect,
  onStart,
}: {
  snapshots: UsageSnapshot[];
  actionAccount: ProviderId | null;
  onConnect: (accountId: ProviderId) => Promise<void>;
  onStart: () => void;
}) {
  return (
    <main className="app-shell onboarding-view">
      <header className="topbar">
        <div className="drag-handle" data-tauri-drag-region onPointerDown={handleDragStart}>
          <div className="brand-mark" aria-hidden="true"><span /><span /><span /></div>
          <h1>AI usage</h1>
        </div>
      </header>
      <div className="onboarding-copy">
        <h2>Je usage, altijd in beeld.</h2>
        <p>Koppel je lokale Codex-accounts. Claude Code wordt automatisch herkend wanneer de bestaande login beschikbaar is.</p>
      </div>
      <div className="connection-list">
        {snapshots.map((snapshot) => {
          const isConnected = snapshot.windows.length > 0 || snapshot.status === "loading";
          const isConnecting = actionAccount === snapshot.accountId;
          return (
            <div className="connection-row" key={snapshot.accountId}>
              <div><span className="connection-name">{snapshot.accountId === "claude" ? "Claude Code" : snapshot.displayName}</span><span className="connection-plan">{snapshot.plan}</span></div>
              {isConnected ? <span className="connected-label"><span className="status-dot status-dot-success" /> {snapshot.status === "loading" ? "Verbinden…" : "Login gevonden"}</span> : <button className="inline-button" type="button" onClick={() => void onConnect(snapshot.accountId)} disabled={isConnecting}>{isConnecting ? "Bezig…" : snapshot.accountId === "claude" ? "Reconnect" : "Connect"}</button>}
            </div>
          );
        })}
      </div>
      <button className="primary-button" type="button" onClick={onStart}>Open usage dock <ArrowIcon /></button>
    </main>
  );
}

function SettingsView({
  settings,
  snapshots,
  actionAccount,
  onBack,
  onSave,
  onReconnect,
}: {
  settings: DockSettings;
  snapshots: UsageSnapshot[];
  actionAccount: ProviderId | null;
  onBack: () => void;
  onSave: (settings: DockSettings) => Promise<void>;
  onReconnect: (accountId: ProviderId) => Promise<void>;
}) {
  const [draft, setDraft] = useState<DockSettings>(settings);
  const [isSaving, setIsSaving] = useState(false);

  async function save() {
    setIsSaving(true);
    await onSave(draft);
    setIsSaving(false);
  }

  function updateLabel(accountId: ProviderId, label: string) {
    setDraft((current) => ({ ...current, labels: { ...current.labels, [accountId]: label } }));
  }

  return (
    <main className="app-shell settings-view">
      <header className="settings-header">
        <button className="back-button" type="button" onClick={onBack}><ArrowIcon direction="back" /> Terug</button>
        <span className="eyebrow">INSTELLINGEN</span>
      </header>
      <section className="settings-section">
        <div className="setting-toggle-row"><div><span className="setting-label">Start with Windows</span><span className="setting-help">Open de dock automatisch na aanmelden.</span></div><Toggle checked={draft.startWithWindows} onChange={(checked) => setDraft((current) => ({ ...current, startWithWindows: checked }))} /></div>
        <div className="setting-toggle-row"><div><span className="setting-label">Always on top</span><span className="setting-help">Houd de dock boven andere vensters.</span></div><Toggle checked={draft.alwaysOnTop} onChange={(checked) => setDraft((current) => ({ ...current, alwaysOnTop: checked }))} /></div>
      </section>
      <section className="settings-section">
        <div className="section-caption">ACCOUNTS</div>
        {snapshots.map((snapshot) => (
          <div className="account-setting-row" key={snapshot.accountId}>
            <label htmlFor={`label-${snapshot.accountId}`}>
              <span className="setting-label">{snapshot.accountId === "claude" ? "Claude" : snapshot.accountId === "codex-account-1" ? "Codex account 1" : "Codex account 2"}</span>
              <span className="setting-help">{snapshot.status === "healthy" ? "Connected" : statusLabel(snapshot.status)}</span>
            </label>
            <div className="account-setting-controls"><input id={`label-${snapshot.accountId}`} value={draft.labels[snapshot.accountId]} maxLength={24} onChange={(event) => updateLabel(snapshot.accountId, event.target.value)} /><button className="inline-button" type="button" onClick={() => void onReconnect(snapshot.accountId)} disabled={actionAccount === snapshot.accountId}>{actionAccount === snapshot.accountId ? "…" : "Reconnect"}</button></div>
          </div>
        ))}
      </section>
      <section className="settings-section refresh-setting"><div><span className="setting-label">Refresh interval</span><span className="setting-help">Vast ingesteld om Claude niet te overspoelen.</span></div><span className="setting-value">5 minuten</span></section>
      <button className="primary-button settings-save" type="button" onClick={() => void save()} disabled={isSaving}>{isSaving ? "Opslaan…" : "Instellingen opslaan"}</button>
    </main>
  );
}

function Toggle({ checked, onChange }: { checked: boolean; onChange: (checked: boolean) => void }) {
  return <button className={`toggle ${checked ? "toggle-on" : ""}`} type="button" role="switch" aria-checked={checked} onClick={() => onChange(!checked)}><span /></button>;
}

function providerMessage(status: ProviderStatus): string {
  switch (status) {
    case "auth_required": return "Login vereist voor actuele usage.";
    case "unavailable": return "Tijdelijk niet beschikbaar.";
    case "loading": return "Usage wordt opgehaald…";
    default: return "Geen usage window beschikbaar.";
  }
}

function RefreshIcon({ spinning = false }: { spinning?: boolean }) {
  return <svg className={spinning ? "icon spinning" : "icon"} viewBox="0 0 20 20" aria-hidden="true"><path d="M16.5 7.5A6.5 6.5 0 1 0 17 12" /><path d="M16.5 3.5v4h-4" /></svg>;
}

function SettingsIcon() {
  return <svg className="icon" viewBox="0 0 20 20" aria-hidden="true"><path d="M4 5h12M4 10h12M4 15h12" /><circle cx="8" cy="5" r="1.5" /><circle cx="12" cy="10" r="1.5" /><circle cx="7" cy="15" r="1.5" /></svg>;
}

function CloseIcon() {
  return <svg className="icon" viewBox="0 0 20 20" aria-hidden="true"><path d="m5 5 10 10M15 5 5 15" /></svg>;
}

function ArrowIcon({ direction = "forward" }: { direction?: "forward" | "back" }) {
  return <svg className={`icon arrow-icon ${direction === "back" ? "arrow-back" : ""}`} viewBox="0 0 20 20" aria-hidden="true"><path d="M4 10h11M11 5l5 5-5 5" /></svg>;
}

export default App;
