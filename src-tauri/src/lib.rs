use chrono::DateTime;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

const ACCOUNT_IDS: [&str; 3] = ["codex-account-1", "codex-account-2", "claude"];
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const ANNOUNCEMENTS_FEED_URL: &str = "https://raw.githubusercontent.com/nofikdev/ai-usage-dock/main/feed/announcements.json";
const BACKGROUND_REFRESH_SECONDS: u64 = 300;
const ANNOUNCEMENTS_REFRESH_SECONDS: u64 = 900;
const CODEX_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const CODEX_BACKOFF_SECONDS: [u64; 5] = [60, 120, 240, 480, 900];
const ANNOUNCEMENTS_BACKOFF_SECONDS: [u64; 4] = [60, 300, 900, 3600];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    #[serde(rename = "type")]
    pub window_type: String,
    pub duration_minutes: u32,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub provider: String,
    pub account_id: String,
    pub display_name: String,
    pub plan: String,
    #[serde(default)]
    pub account_identity: Option<String>,
    #[serde(default)]
    pub rate_limit_reached_type: Option<String>,
    pub fetched_at: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub windows: Vec<UsageWindow>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockSettings {
    pub start_with_windows: bool,
    pub always_on_top: bool,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementItem {
    pub id: String,
    pub published_at: Option<i64>,
    pub text: String,
    pub url: String,
    pub category: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementFeed {
    pub status: String,
    pub fetched_at: Option<i64>,
    pub error: Option<String>,
    pub items: Vec<AnnouncementItem>,
    pub last_seen_id: Option<String>,
}

impl Default for AnnouncementFeed {
    fn default() -> Self {
        Self { status: "unavailable".to_string(), fetched_at: None, error: None, items: Vec::new(), last_seen_id: None }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAnnouncementFeed {
    #[serde(default)]
    fetched_at: Option<String>,
    #[serde(default)]
    items: Vec<RemoteAnnouncementItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAnnouncementItem {
    id: String,
    #[serde(default)]
    published_at: Option<String>,
    text: String,
    url: String,
    #[serde(default = "default_announcement_category")]
    category: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub snapshots: Vec<UsageSnapshot>,
    pub settings: DockSettings,
    pub has_fetched: bool,
    pub last_updated_at: Option<i64>,
    pub announcements: AnnouncementFeed,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WindowPosition {
    x: i32,
    y: i32,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WindowSize {
    width: u32,
    height: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    snapshots: Vec<UsageSnapshot>,
    #[serde(default)]
    window_position: Option<WindowPosition>,
    #[serde(default)]
    window_size: Option<WindowSize>,
    settings: DockSettings,
    last_updated_at: Option<i64>,
    #[serde(default)]
    announcements: AnnouncementFeed,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            snapshots: Vec::new(),
            window_position: None,
            window_size: None,
            settings: DockSettings::default(),
            last_updated_at: None,
            announcements: AnnouncementFeed::default(),
        }
    }
}

impl Default for DockSettings {
    fn default() -> Self {
        let mut labels = HashMap::new();
        labels.insert("codex-account-1".to_string(), "Codex 1".to_string());
        labels.insert("codex-account-2".to_string(), "Codex 2".to_string());
        labels.insert("claude".to_string(), "Claude".to_string());
        Self { start_with_windows: true, always_on_top: true, labels }
    }
}

#[derive(Clone)]
pub struct AppState {
    coordinator: Arc<UsageCoordinator>,
}

struct UsageCoordinator {
    app_data_dir: PathBuf,
    state_path: PathBuf,
    persisted: Mutex<PersistedState>,
    snapshots: Mutex<HashMap<String, UsageSnapshot>>,
    sessions: Mutex<HashMap<String, Arc<Mutex<CodexSessionSlot>>>>,
    codex_provider_lock: Mutex<()>,
    refreshing: Mutex<HashSet<String>>,
    codex_backoff: Mutex<HashMap<String, BackoffState>>,
    claude_backoff: Mutex<BackoffState>,
    announcements_backoff: Mutex<BackoffState>,
}

#[derive(Default)]
struct BackoffState {
    failures: usize,
    retry_at: Option<Instant>,
}

struct CodexSessionSlot {
    session: Option<CodexSession>,
}

struct CodexSession {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl UsageCoordinator {
    fn new() -> Arc<Self> {
        let app_data_dir = app_data_dir();
        let _ = fs::create_dir_all(app_data_dir.join("codex"));
        let state_path = app_data_dir.join("state.json");
        let persisted = load_persisted_state(&state_path);
        let mut snapshots = HashMap::new();

        for account_id in ACCOUNT_IDS {
            let snapshot = persisted
                .snapshots
                .iter()
                .find(|snapshot| snapshot.account_id == account_id)
                .cloned()
                .unwrap_or_else(|| initial_snapshot(account_id, &persisted.settings));
            snapshots.insert(account_id.to_string(), snapshot);
        }

        Arc::new(Self {
            app_data_dir,
            state_path,
            persisted: Mutex::new(persisted),
            snapshots: Mutex::new(snapshots),
            sessions: Mutex::new(HashMap::new()),
            codex_provider_lock: Mutex::new(()),
            refreshing: Mutex::new(HashSet::new()),
            codex_backoff: Mutex::new(HashMap::new()),
            claude_backoff: Mutex::new(BackoffState::default()),
            announcements_backoff: Mutex::new(BackoffState::default()),
        })
    }

    fn panel_state(&self) -> PanelState {
        let persisted = self.persisted.lock().expect("state lock poisoned");
        let snapshots = self.snapshots.lock().expect("snapshot lock poisoned");
        PanelState {
            snapshots: ACCOUNT_IDS.iter().filter_map(|id| snapshots.get(*id).cloned()).collect(),
            settings: persisted.settings.clone(),
            has_fetched: persisted.last_updated_at.is_some(),
            last_updated_at: persisted.last_updated_at,
            announcements: persisted.announcements.clone(),
        }
    }

    fn refresh_all(self: &Arc<Self>) -> PanelState {
        let mut workers = Vec::with_capacity(3);
        for account_id in ["codex-account-1", "codex-account-2"] {
            let coordinator = Arc::clone(self);
            workers.push(thread::spawn(move || coordinator.refresh_codex(account_id)));
        }
        let coordinator = Arc::clone(self);
        workers.push(thread::spawn(move || coordinator.refresh_claude()));

        let mut any_success = false;
        for worker in workers {
            if worker.join().unwrap_or(false) { any_success = true; }
        }

        if any_success {
            let now = unix_now();
            if let Ok(mut persisted) = self.persisted.lock() {
                persisted.last_updated_at = Some(now);
                self.persist_snapshots_locked(&mut persisted);
            }
        }

        self.panel_state()
    }

    fn refresh_announcements(&self) -> bool {
        if !self.begin_refresh("announcements") {
            self.log_event("announcements refresh skipped; already running");
            return false;
        }
        let result = self.refresh_announcements_inner();
        self.end_refresh("announcements");
        result
    }

    fn refresh_announcements_inner(&self) -> bool {
        if let Some(seconds) = self.announcements_backoff_remaining() {
            self.set_announcements_failure(&format!("Tijdelijk niet beschikbaar · nieuwe poging over {}", format_retry_delay(seconds)), true);
            return false;
        }

        let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
            Ok(client) => client,
            Err(_) => {
                self.register_announcements_backoff();
                self.set_announcements_failure("Aankondigingen tijdelijk niet beschikbaar", true);
                return false;
            }
        };
        let response = match client.get(ANNOUNCEMENTS_FEED_URL).header("accept", "application/json").send() {
            Ok(response) => response,
            Err(_) => {
                self.register_announcements_backoff();
                self.set_announcements_failure("Aankondigingen offline · cache behouden", true);
                self.log_event("announcements network failure; backoff active");
                return false;
            }
        };
        let status = response.status().as_u16();
        if status == 429 || (500..=599).contains(&status) {
            self.register_announcements_backoff();
            self.set_announcements_failure(&format!("Aankondigingen tijdelijk niet beschikbaar · HTTP {}", status), true);
            self.log_event(&format!("announcements HTTP {}; backoff active", status));
            return false;
        }
        if !response.status().is_success() {
            self.set_announcements_failure("Aankondigingen-feed kan niet worden geladen", false);
            self.log_event(&format!("announcements HTTP {}", status));
            return false;
        }

        let payload: RemoteAnnouncementFeed = match response.json() {
            Ok(payload) => payload,
            Err(_) => {
                self.set_announcements_failure("Aankondigingen-feed heeft een ongeldig formaat", false);
                return false;
            }
        };
        let items = payload.items.into_iter().filter_map(normalize_announcement).take(20).collect::<Vec<_>>();
        let fetched_at = payload.fetched_at.as_deref().and_then(|value| parse_timestamp(&Value::String(value.to_string()))).or_else(|| Some(unix_now()));
        self.reset_announcements_backoff();
        self.set_announcements_success(items, fetched_at);
        self.log_event("announcements fetch success");
        true
    }

    fn set_announcements_success(&self, items: Vec<AnnouncementItem>, fetched_at: Option<i64>) {
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.announcements.status = "healthy".to_string();
            persisted.announcements.fetched_at = fetched_at;
            persisted.announcements.error = None;
            persisted.announcements.items = items;
            self.persist_snapshots_locked(&mut persisted);
        }
    }

    fn set_announcements_failure(&self, message: &str, retryable: bool) {
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.announcements.status = if persisted.announcements.items.is_empty() { "unavailable" } else { "stale" }.to_string();
            persisted.announcements.error = Some(if retryable { message.to_string() } else { "Aankondigingen niet beschikbaar".to_string() });
            self.persist_snapshots_locked(&mut persisted);
        }
    }

    fn mark_announcements_read(&self, id: String) -> PanelState {
        if let Ok(mut persisted) = self.persisted.lock() {
            if persisted.announcements.items.iter().any(|item| item.id == id) {
                persisted.announcements.last_seen_id = Some(id);
                self.persist_snapshots_locked(&mut persisted);
            }
        }
        self.panel_state()
    }

    fn refresh_codex(&self, account_id: &str) -> bool {
        if !self.begin_refresh(account_id) {
            self.log_event(&format!("{} refresh skipped; already running", account_id));
            return false;
        }
        let _provider_guard = self.codex_provider_lock.lock().expect("Codex provider lock poisoned");
        let result = self.refresh_codex_inner(account_id);
        self.end_refresh(account_id);
        result
    }

    fn refresh_codex_inner(&self, account_id: &str) -> bool {
        if let Some(seconds) = self.codex_backoff_remaining(account_id) {
            self.set_failure(account_id, &format!("Tijdelijk beperkt · nieuwe poging over {}", format_retry_delay(seconds)), false);
            return false;
        }

        let slot = {
            let mut sessions = self.sessions.lock().expect("session map lock poisoned");
            sessions
                .entry(account_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(CodexSessionSlot { session: None })))
                .clone()
        };
        let home = self.codex_home(account_id);
        let mut slot = slot.lock().expect("session lock poisoned");
        // app-server keeps a network connection alive. Start each poll cleanly so a
        // connection that went stale cannot poison every later usage check.
        slot.session = None;
        let mut result = Err("Codex CLI niet gevonden".to_string());

        for attempt in 0..2 {
            if slot.session.as_mut().map(|session| session.is_alive()).unwrap_or(false) == false {
                slot.session = None;
                match CodexSession::start(&home) {
                    Ok(session) => slot.session = Some(session),
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                }
            }

            if let Some(session) = slot.session.as_mut() {
                result = session.fetch_usage();
                let retryable = result.as_ref().err().map(|error| is_temporary_codex_error(error)).unwrap_or(false);
                if result.is_ok() || !retryable || attempt == 1 { break; }
                slot.session = None;
                thread::sleep(Duration::from_secs(1_u64 << attempt));
            }
        }

        let success = match result {
            Ok((identity, plan, rate_limit_reached_type, windows)) => {
                self.reset_codex_backoff(account_id);
                self.set_success(account_id, identity, plan, rate_limit_reached_type, windows);
                self.log_event(&format!("{} fetch success", account_id));
                true
            }
            Err(error) => {
                self.record_codex_failure(account_id, &error);
                false
            }
        };

        // Do not retain an app-server process between polls. This also makes the
        // retry path deterministic after a provider-side disconnect.
        slot.session = None;
        success
    }

    fn refresh_claude(&self) -> bool {
        if !self.begin_refresh("claude") {
            self.log_event("claude refresh skipped; already running");
            return false;
        }
        let result = self.refresh_claude_inner();
        self.end_refresh("claude");
        result
    }

    fn refresh_claude_inner(&self) -> bool {
        {
            let backoff = self.claude_backoff.lock().expect("backoff lock poisoned");
            if backoff.retry_at.map(|retry_at| Instant::now() < retry_at).unwrap_or(false) {
                drop(backoff);
                self.set_failure("claude", "Tijdelijk beperkt · wachten op nieuwe poging", false);
                return false;
            }
        }

        let credentials_path = match claude_credentials_path() {
            Some(path) => path,
            None => {
                self.set_failure("claude", "Claude Code-login niet gevonden", true);
                return false;
            }
        };
        let credentials = match fs::read_to_string(credentials_path) {
            Ok(contents) => contents,
            Err(_) => {
                self.set_failure("claude", "Claude Code-login niet gevonden", true);
                return false;
            }
        };
        let credentials_json: Value = match serde_json::from_str(&credentials) {
            Ok(value) => value,
            Err(_) => {
                self.set_failure("claude", "Claude-login kan niet worden gelezen", true);
                return false;
            }
        };
        let token = match claude_access_token(&credentials_json) {
            Some(token) => token,
            None => {
                self.set_failure("claude", "Claude subscription-login niet gevonden", true);
                return false;
            }
        };

        let client = match Client::builder().timeout(Duration::from_secs(15)).build() {
            Ok(client) => client,
            Err(_) => {
                self.set_failure("claude", "Tijdelijk niet beschikbaar", false);
                return false;
            }
        };
        let response = client
            .get(CLAUDE_USAGE_URL)
            .bearer_auth(token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("accept", "application/json")
            .send();

        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.set_failure("claude", "Offline · laatste geldige data behouden", false);
                return false;
            }
        };
        let status = response.status().as_u16();
        if status == 429 {
            self.register_claude_backoff();
            self.set_failure("claude", "Tijdelijk beperkt · laatste geldige data behouden", false);
            self.log_event("claude HTTP 429; backoff active");
            return false;
        }
        if status == 401 || status == 403 {
            self.set_failure("claude", "Claude-login verlopen", true);
            self.log_event("claude authentication state");
            return false;
        }
        if !response.status().is_success() {
            self.set_failure("claude", "Tijdelijk niet beschikbaar", false);
            self.log_event(&format!("claude HTTP {}", status));
            return false;
        }

        let payload: Value = match response.json() {
            Ok(payload) => payload,
            Err(_) => {
                self.set_failure("claude", "Usage-response kan niet worden gelezen", false);
                return false;
            }
        };
        let windows = match parse_claude_windows(&payload) {
            Some(windows) if !windows.is_empty() => windows,
            _ => {
                self.set_failure("claude", "Usage-response heeft een onbekend formaat", false);
                return false;
            }
        };
        self.reset_claude_backoff();
        self.set_success("claude", None, None, None, windows);
        self.log_event("claude fetch success");
        true
    }

    fn set_success(&self, account_id: &str, identity: Option<String>, plan: Option<String>, rate_limit_reached_type: Option<String>, windows: Vec<UsageWindow>) {
        let settings = self.persisted.lock().expect("state lock poisoned").settings.clone();
        let mut snapshots = self.snapshots.lock().expect("snapshot lock poisoned");
        let previous = snapshots.get(account_id).cloned();
        let label = settings.labels.get(account_id).cloned().unwrap_or_default();
        let display_name = if !label.trim().is_empty() { label } else { default_display_name(account_id, identity.as_deref()) };
        snapshots.insert(account_id.to_string(), UsageSnapshot {
            provider: if account_id == "claude" { "claude" } else { "codex" }.to_string(),
            account_id: account_id.to_string(),
            display_name,
            plan: plan.unwrap_or_else(|| if account_id == "claude" { "Claude Pro" } else { "ChatGPT Plus" }.to_string()),
            account_identity: identity.clone(),
            rate_limit_reached_type,
            fetched_at: Some(unix_now()),
            status: "healthy".to_string(),
            error: None,
            windows: if windows.is_empty() { previous.map(|snapshot| snapshot.windows).unwrap_or_default() } else { windows },
        });
    }

    fn set_failure(&self, account_id: &str, message: &str, auth_required: bool) {
        let mut snapshots = self.snapshots.lock().expect("snapshot lock poisoned");
        let current = snapshots.entry(account_id.to_string()).or_insert_with(|| initial_snapshot(account_id, &DockSettings::default()));
        current.status = if auth_required { "auth_required" } else if current.windows.is_empty() { "unavailable" } else { "stale" }.to_string();
        current.error = Some(message.to_string());
    }

    fn begin_refresh(&self, account_id: &str) -> bool {
        self.refreshing.lock().map(|mut refreshing| refreshing.insert(account_id.to_string())).unwrap_or(false)
    }

    fn end_refresh(&self, account_id: &str) {
        if let Ok(mut refreshing) = self.refreshing.lock() {
            refreshing.remove(account_id);
        }
    }

    fn finish_account_refresh(&self, success: bool) -> PanelState {
        if success {
            if let Ok(mut persisted) = self.persisted.lock() {
                persisted.last_updated_at = Some(unix_now());
                self.persist_snapshots_locked(&mut persisted);
            }
        }
        self.panel_state()
    }

    fn record_codex_failure(&self, account_id: &str, error: &str) {
        let message = friendly_codex_error(error);
        if is_temporary_codex_error(error) {
            let delay = self.register_codex_backoff(account_id);
            self.set_failure(account_id, &format!("{} · nieuwe poging over {}", message, format_retry_delay(delay)), false);
        } else {
            self.set_failure(account_id, &message, is_auth_error(error));
        }
        self.log_event(&format!("{} fetch failure ({})", account_id, codex_error_category(error)));
    }

    fn update_settings(&self, settings: DockSettings) -> PanelState {
        if let Ok(mut snapshots) = self.snapshots.lock() {
            for snapshot in snapshots.values_mut() {
                snapshot.display_name = settings
                    .labels
                    .get(&snapshot.account_id)
                    .filter(|label| !label.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| default_display_name(&snapshot.account_id, snapshot.account_identity.as_deref()));
            }
        }
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.settings = settings;
            self.persist_snapshots_locked(&mut persisted);
        }
        self.panel_state()
    }

    fn apply_always_on_top(&self, app: &AppHandle, enabled: bool) -> Result<(), String> {
        let window = app.get_webview_window("main").ok_or_else(|| "Hoofdvenster niet gevonden".to_string())?;
        window.set_always_on_top(enabled).map_err(|error| error.to_string())
    }

    fn toggle_always_on_top(&self, app: &AppHandle) {
        let current = self.persisted.lock().map(|state| state.settings.always_on_top).unwrap_or(true);
        let enabled = !current;
        let _ = self.apply_always_on_top(app, enabled);
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.settings.always_on_top = enabled;
            self.persist_snapshots_locked(&mut persisted);
        }
    }

    fn restore_window_position(&self, app: &AppHandle) {
        let Some(window) = app.get_webview_window("main") else { return; };
        let saved = self.persisted.lock().ok().and_then(|state| state.window_position.clone());
        let Some(position) = saved else { return; };

        let visible = window.available_monitors().ok().map(|monitors| monitors.iter().any(|monitor| {
            let monitor_position = monitor.position();
            let monitor_size = monitor.size();
            position.x < monitor_position.x + monitor_size.width as i32
                && position.x + 80 > monitor_position.x
                && position.y < monitor_position.y + monitor_size.height as i32
                && position.y + 80 > monitor_position.y
        })).unwrap_or(true);
        if visible {
            let _ = window.set_position(Position::Physical(PhysicalPosition::new(position.x, position.y)));
        } else if let Ok(monitors) = window.available_monitors() {
            if let Some(monitor) = monitors.first() {
                let target = monitor.position();
                let _ = window.set_position(Position::Physical(PhysicalPosition::new(target.x, target.y)));
            }
        }
    }

    fn restore_window_size(&self, app: &AppHandle) {
        let Some(window) = app.get_webview_window("main") else { return; };
        let saved = self.persisted.lock().ok().and_then(|state| state.window_size.clone());
        let Some(size) = saved else { return; };
        let _ = window.set_size(Size::Physical(PhysicalSize::new(size.width, size.height)));
    }

    fn save_window_position(&self, position: PhysicalPosition<i32>) {
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.window_position = Some(WindowPosition { x: position.x, y: position.y });
            self.persist_snapshots_locked(&mut persisted);
        }
    }

    fn save_window_size(&self, size: PhysicalSize<u32>) {
        if let Ok(mut persisted) = self.persisted.lock() {
            persisted.window_size = Some(WindowSize { width: size.width, height: size.height });
            self.persist_snapshots_locked(&mut persisted);
        }
    }

    fn persist_snapshots_locked(&self, persisted: &mut PersistedState) {
        persisted.snapshots = self.snapshots.lock().map(|snapshots| snapshots.values().cloned().collect()).unwrap_or_default();
        if let Ok(contents) = serde_json::to_string_pretty(persisted) {
            let temporary_path = self.state_path.with_extension("json.tmp");
            if fs::write(&temporary_path, contents).is_ok() {
                let _ = fs::rename(temporary_path, &self.state_path);
            }
        }
    }

    fn codex_home(&self, account_id: &str) -> PathBuf {
        self.app_data_dir.join("codex").join(account_id)
    }

    fn log_event(&self, event: &str) {
        let logs_dir = self.app_data_dir.join("logs");
        if fs::create_dir_all(&logs_dir).is_err() { return; }
        let log_path = logs_dir.join("ai-usage-dock.log");
        if fs::metadata(&log_path).map(|metadata| metadata.len() > 256 * 1024).unwrap_or(false) {
            let _ = fs::rename(&log_path, logs_dir.join("ai-usage-dock.log.1"));
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "{} {}", unix_now(), event);
        }
    }

    fn register_claude_backoff(&self) {
        if let Ok(mut backoff) = self.claude_backoff.lock() {
            let delays = [60_u64, 120, 300, 900];
            let delay = delays[backoff.failures.min(delays.len() - 1)];
            backoff.failures += 1;
            backoff.retry_at = Some(Instant::now() + Duration::from_secs(delay));
        }
    }

    fn reset_claude_backoff(&self) {
        if let Ok(mut backoff) = self.claude_backoff.lock() {
            *backoff = BackoffState::default();
        }
    }

    fn announcements_backoff_remaining(&self) -> Option<u64> {
        self.announcements_backoff.lock().ok().and_then(|state| state.retry_at).and_then(|retry_at| {
            let remaining = retry_at.saturating_duration_since(Instant::now());
            if remaining.is_zero() { None } else { Some(remaining.as_secs().max(1)) }
        })
    }

    fn register_announcements_backoff(&self) -> u64 {
        if let Ok(mut state) = self.announcements_backoff.lock() {
            let delay = ANNOUNCEMENTS_BACKOFF_SECONDS[state.failures.min(ANNOUNCEMENTS_BACKOFF_SECONDS.len() - 1)];
            state.failures += 1;
            state.retry_at = Some(Instant::now() + Duration::from_secs(delay));
            return delay;
        }
        ANNOUNCEMENTS_BACKOFF_SECONDS[0]
    }

    fn reset_announcements_backoff(&self) {
        if let Ok(mut state) = self.announcements_backoff.lock() {
            *state = BackoffState::default();
        }
    }

    fn codex_backoff_remaining(&self, account_id: &str) -> Option<u64> {
        self.codex_backoff.lock().ok().and_then(|backoff| {
            backoff.get(account_id).and_then(|state| state.retry_at).and_then(|retry_at| {
                let remaining = retry_at.saturating_duration_since(Instant::now());
                if remaining.is_zero() { None } else { Some(remaining.as_secs().max(1)) }
            })
        })
    }

    fn register_codex_backoff(&self, account_id: &str) -> u64 {
        if let Ok(mut backoffs) = self.codex_backoff.lock() {
            let state = backoffs.entry(account_id.to_string()).or_default();
            let delay = CODEX_BACKOFF_SECONDS[state.failures.min(CODEX_BACKOFF_SECONDS.len() - 1)];
            state.failures += 1;
            state.retry_at = Some(Instant::now() + Duration::from_secs(delay));
            return delay;
        }
        CODEX_BACKOFF_SECONDS[0]
    }

    fn reset_codex_backoff(&self, account_id: &str) {
        if let Ok(mut backoffs) = self.codex_backoff.lock() {
            backoffs.remove(account_id);
        }
    }
}

impl CodexSession {
    fn start(home: &Path) -> Result<Self, String> {
        let _ = fs::create_dir_all(home);
        let mut child = (codex_command(&["app-server", "--stdio"], home)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Codex app-server kon niet starten: {}", error))?;
        let stdout = child.stdout.take().ok_or_else(|| "Codex app-server heeft geen output".to_string())?;
        let stdin = child.stdin.take().ok_or_else(|| "Codex app-server heeft geen input".to_string())?;
        let stderr = child.stderr.take().ok_or_else(|| "Codex app-server heeft geen foutuitvoer".to_string())?;
        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().flatten() {
                if sender.send(line).is_err() { break; }
            }
        });
        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        let stderr_sink = Arc::clone(&stderr_buffer);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().flatten() {
                remember_diagnostic_line(&stderr_sink, &line);
            }
        });

        let mut session = Self { child, stdin, responses, stderr: stderr_buffer, next_id: 1 };
        session.request("initialize", json!({
            "clientInfo": { "name": "ai-usage-dock", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": {}
        }))?;
        session.notify("initialized", json!({}))?;
        Ok(session)
    }

    fn is_alive(&mut self) -> bool {
        self.child.try_wait().map(|status| status.is_none()).unwrap_or(false)
    }

    fn fetch_usage(&mut self) -> Result<(Option<String>, Option<String>, Option<String>, Vec<UsageWindow>), String> {
        let account = self.request("account/read", json!({}))?;
        let limits = self.request("account/rateLimits/read", json!({}))?;
        let identity = find_string(&account, &["email", "emailAddress", "email_address"]);
        let plan = find_string(&account, &["planType", "plan_type", "subscription"]);
        let rate_limit_reached_type = find_string(&limits, &["rateLimitReachedType", "rate_limit_reached_type"]);
        let windows = parse_codex_windows(&limits);
        if windows.is_empty() { return Err("Onbekend rate-limit formaat".to_string()); }
        Ok((identity, plan, rate_limit_reached_type, windows))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        writeln!(self.stdin, "{}", message).map_err(|_| "Codex app-server is gestopt".to_string())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{}", message).map_err(|_| "Codex app-server is gestopt".to_string())?;

        let deadline = Instant::now() + Duration::from_secs(CODEX_REQUEST_TIMEOUT_SECONDS);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(self.with_stderr("Codex app-server reageert niet")); }
            let line = match self.responses.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return Err(self.with_stderr("Codex app-server reageert niet")),
                Err(RecvTimeoutError::Disconnected) => return Err(self.with_stderr("Codex app-server is gestopt")),
            };
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value.get("id") == Some(&Value::from(id)) {
                if let Some(error) = value.get("error") { return Err(self.with_stderr(&error_message(error))); }
                return value.get("result").cloned().ok_or_else(|| "Codex antwoord bevat geen resultaat".to_string());
            }
        }
    }

    fn with_stderr(&self, message: &str) -> String {
        let excerpt = self.stderr.lock().ok().map(|stderr| safe_process_excerpt(&stderr)).unwrap_or_default();
        if excerpt.is_empty() { message.to_string() } else { format!("{}: {}", message, excerpt) }
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tauri::command]
fn get_initial_state(state: State<'_, AppState>) -> PanelState {
    state.coordinator.panel_state()
}

#[tauri::command]
fn refresh_usage(state: State<'_, AppState>) -> PanelState {
    state.coordinator.refresh_all()
}

#[tauri::command]
fn refresh_announcements(state: State<'_, AppState>) -> PanelState {
    state.coordinator.refresh_announcements();
    state.coordinator.panel_state()
}

#[tauri::command]
fn mark_announcements_read(id: String, state: State<'_, AppState>) -> PanelState {
    state.coordinator.mark_announcements_read(id)
}

#[tauri::command]
fn connect_codex(account_id: String, state: State<'_, AppState>) -> Result<PanelState, String> {
    if !["codex-account-1", "codex-account-2"].contains(&account_id.as_str()) {
        return Err("Onbekend Codex-account".to_string());
    }
    if !state.coordinator.begin_refresh(&account_id) {
        return Ok(state.coordinator.panel_state());
    }
    let _provider_guard = state.coordinator.codex_provider_lock.lock().expect("Codex provider lock poisoned");
    let home = state.coordinator.codex_home(&account_id);
    let mut command = match codex_command(&["login"], &home) {
        Ok(command) => command,
        Err(error) => {
            state.coordinator.record_codex_failure(&account_id, &error);
            state.coordinator.end_refresh(&account_id);
            return Ok(state.coordinator.panel_state());
        }
    };
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            state.coordinator.reset_codex_backoff(&account_id);
            let refreshed = state.coordinator.refresh_codex_inner(&account_id);
            state.coordinator.end_refresh(&account_id);
            Ok(state.coordinator.finish_account_refresh(refreshed))
        }
        Ok(output) => {
            let details = process_output_text(&output.stdout, &output.stderr);
            let error = if details.is_empty() { "Codex-login is niet afgerond".to_string() } else { details };
            state.coordinator.record_codex_failure(&account_id, &error);
            state.coordinator.end_refresh(&account_id);
            Ok(state.coordinator.panel_state())
        }
        Err(error) => {
            let message = if error.kind() == std::io::ErrorKind::NotFound {
                "Codex CLI niet gevonden".to_string()
            } else {
                "Codex-login kon niet starten".to_string()
            };
            state.coordinator.record_codex_failure(&account_id, &message);
            state.coordinator.end_refresh(&account_id);
            Ok(state.coordinator.panel_state())
        }
    }
}

#[tauri::command]
fn reconnect_provider(account_id: String) -> Result<(), String> {
    if account_id == "claude" { Ok(()) } else { Err("Deze provider gebruikt geen Claude-login".to_string()) }
}

#[tauri::command]
fn update_settings(app: AppHandle, state: State<'_, AppState>, settings: DockSettings) -> Result<PanelState, String> {
    state.coordinator.apply_always_on_top(&app, settings.always_on_top)?;
    let autolaunch = app.autolaunch();
    if settings.start_with_windows { autolaunch.enable().map_err(|error| error.to_string())?; }
    else { autolaunch.disable().map_err(|error| error.to_string())?; }
    Ok(state.coordinator.update_settings(settings))
}

#[tauri::command]
fn hide_window(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main").ok_or_else(|| "Hoofdvenster niet gevonden".to_string())?.hide().map_err(|error| error.to_string())
}

fn initial_snapshot(account_id: &str, settings: &DockSettings) -> UsageSnapshot {
    UsageSnapshot {
        provider: if account_id == "claude" { "claude" } else { "codex" }.to_string(),
        account_id: account_id.to_string(),
        display_name: settings.labels.get(account_id).cloned().filter(|label| !label.trim().is_empty()).unwrap_or_else(|| default_display_name(account_id, None)),
        plan: if account_id == "claude" { "Claude Pro" } else { "ChatGPT Plus" }.to_string(),
        account_identity: None,
        rate_limit_reached_type: None,
        fetched_at: None,
        status: if account_id == "claude" && claude_credentials_path().is_some() { "loading" } else { "auth_required" }.to_string(),
        error: None,
        windows: Vec::new(),
    }
}

fn default_display_name(account_id: &str, identity: Option<&str>) -> String {
    identity.filter(|value| !value.trim().is_empty()).map(str::to_string).unwrap_or_else(|| match account_id {
        "codex-account-1" => "Codex 1".to_string(),
        "codex-account-2" => "Codex 2".to_string(),
        "claude" => "Claude".to_string(),
        _ => account_id.to_string(),
    })
}

fn default_announcement_category() -> String {
    "announcement".to_string()
}

fn normalize_announcement(item: RemoteAnnouncementItem) -> Option<AnnouncementItem> {
    let id = item.id.trim().to_string();
    let text = item.text.trim().to_string();
    let url = item.url.trim().to_string();
    if id.is_empty() || text.is_empty() || !url.starts_with("https://x.com/") { return None; }
    Some(AnnouncementItem {
        id,
        published_at: item.published_at.as_deref().and_then(|value| parse_timestamp(&Value::String(value.to_string()))),
        text: text.chars().take(600).collect(),
        url,
        category: item.category.trim().chars().take(40).collect(),
    })
}

fn parse_codex_windows(value: &Value) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    collect_windows(value, &mut windows);
    windows
}

fn collect_windows(value: &Value, windows: &mut Vec<UsageWindow>) {
    match value {
        Value::Object(object) => {
            let used = object_value(object, &["usedPercent", "used_percent", "utilization"]);
            let duration = object_value(object, &["windowDurationMins", "window_duration_mins", "durationMinutes"]);
            let reset = object_value(object, &["resetsAt", "resets_at"]);
            if let (Some(used), Some(duration)) = (used.and_then(as_f64), duration.and_then(as_f64)) {
                if let Some(window_type) = known_window_type(duration) {
                    let used_percent = used.clamp(0.0, 100.0);
                    let candidate = UsageWindow {
                        window_type: window_type.to_string(),
                        duration_minutes: duration.round() as u32,
                        used_percent,
                        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
                        resets_at: reset.and_then(parse_timestamp),
                    };
                    if !windows.iter().any(|window| window.window_type == candidate.window_type) { windows.push(candidate); }
                }
            }
            for child in object.values() { collect_windows(child, windows); }
        }
        Value::Array(values) => for child in values { collect_windows(child, windows); },
        _ => {}
    }
}

fn parse_claude_windows(value: &Value) -> Option<Vec<UsageWindow>> {
    let mut windows = Vec::new();
    for (key, window_type, duration) in [("five_hour", "five_hour", 300_u32), ("seven_day", "weekly", 10080_u32)] {
        let object = value.get(key).or_else(|| value.get(if key == "seven_day" { "sevenDay" } else { "fiveHour" }))?;
        let used = object_value(object.as_object()?, &["utilization", "used_percentage", "usedPercent"]).and_then(as_f64)?;
        let reset = object_value(object.as_object()?, &["resets_at", "resetsAt"]).and_then(parse_timestamp);
        let used_percent = used.clamp(0.0, 100.0);
        windows.push(UsageWindow { window_type: window_type.to_string(), duration_minutes: duration, used_percent, remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0), resets_at: reset });
    }
    Some(windows)
}

fn known_window_type(duration: f64) -> Option<&'static str> {
    if (duration - 300.0).abs() <= 30.0 { Some("five_hour") }
    else if (duration - 10080.0).abs() <= 240.0 { Some("weekly") }
    else { None }
}

fn object_value<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn as_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn parse_timestamp(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64().or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok())).or_else(|| value.as_str().and_then(|value| value.parse().ok())) {
        return Some(if number > 20_000_000_000 { number / 1000 } else { number });
    }
    value.as_str().and_then(|value| DateTime::parse_from_rfc3339(value).ok()).map(|value| value.timestamp())
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(string) = object.get(*key).and_then(Value::as_str) { return Some(string.to_string()); }
            }
            object.values().find_map(|child| find_string(child, keys))
        }
        Value::Array(values) => values.iter().find_map(|child| find_string(child, keys)),
        _ => None,
    }
}

fn claude_access_token(value: &Value) -> Option<String> {
    let oauth = value.get("claudeAiOauth").or_else(|| value.get("claude_ai_oauth")).unwrap_or(value);
    find_string(oauth, &["accessToken", "access_token"])
}

fn error_message(error: &Value) -> String {
    error.get("message").and_then(Value::as_str).unwrap_or("JSON-RPC-fout").to_string()
}

fn friendly_codex_error(error: &str) -> String {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("niet gevonden") || normalized.contains("not found") || normalized.contains("cannot find") {
        "Codex CLI niet gevonden · controleer installatie en PATH".to_string()
    } else if normalized.contains("approval_policy") || normalized.contains("untrusted") {
        "Codex-configuratie blokkeert de login · voer codex doctor uit".to_string()
    } else if is_auth_error(error) {
        "Codex-login vereist of niet afgerond".to_string()
    } else if normalized.contains("429") || normalized.contains("rate limit") {
        "Codex tijdelijk beperkt".to_string()
    } else if normalized.contains("kon niet starten") {
        "Codex CLI kon niet starten · controleer installatie en rechten".to_string()
    } else if normalized.contains("reageert niet") || normalized.contains("network") || normalized.contains("connection") || normalized.contains("timeout") {
        "Codex-server tijdelijk niet bereikbaar".to_string()
    } else if normalized.contains("onbekend rate-limit formaat") {
        "Codex usage-formaat wordt niet herkend".to_string()
    } else {
        "Codex tijdelijk niet beschikbaar".to_string()
    }
}

fn is_auth_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("auth") || normalized.contains("login") || normalized.contains("unauthorized")
}

fn is_temporary_codex_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    if is_auth_error(error)
        || normalized.contains("niet gevonden")
        || normalized.contains("not found")
        || normalized.contains("cannot find")
        || normalized.contains("kon niet starten")
        || normalized.contains("approval_policy")
        || normalized.contains("untrusted")
        || normalized.contains("onbekend rate-limit formaat")
    {
        return false;
    }
    ["429", "rate limit", "reageert niet", "gestopt", "network", "connection", "timeout", "eof"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn codex_error_category(error: &str) -> &'static str {
    if is_auth_error(error) { "login" }
    else if error.to_ascii_lowercase().contains("niet gevonden") { "cli" }
    else if is_temporary_codex_error(error) { "temporary" }
    else { "provider" }
}

fn format_retry_delay(seconds: u64) -> String {
    if seconds >= 60 { format!("{}m", seconds.div_ceil(60)) } else { format!("{}s", seconds.max(1)) }
}

fn remember_diagnostic_line(buffer: &Arc<Mutex<String>>, line: &str) {
    let Ok(mut buffer) = buffer.lock() else { return; };
    if buffer.len() >= 4096 { return; }
    let remaining = 4096 - buffer.len();
    let clipped: String = line.chars().take(remaining.saturating_sub(1)).collect();
    buffer.push_str(&clipped);
    buffer.push('\n');
}

fn safe_process_excerpt(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let normalized = trimmed.to_ascii_lowercase();
        if trimmed.is_empty()
            || normalized.contains("http://")
            || normalized.contains("https://")
            || normalized.contains("token")
            || normalized.contains("secret")
            || normalized.contains("password")
            || normalized.contains("authorization")
            || normalized.contains("bearer")
        {
            continue;
        }
        lines.push(trimmed.chars().take(160).collect::<String>());
        if lines.len() == 4 { break; }
    }
    lines.join(" | ").chars().take(500).collect()
}

fn process_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined = format!("{}\n{}", stdout, stderr);
    combined.chars().take(8192).collect()
}

fn path_file_name_is(path: Option<&Path>, expected: &str) -> bool {
    path.and_then(Path::file_name).and_then(|name| name.to_str()).map(|name| name.eq_ignore_ascii_case(expected)).unwrap_or(false)
}

fn find_native_codex_near_launcher(launcher: &Path) -> Option<PathBuf> {
    let root = launcher.parent()?.join("node_modules").join("@openai").join("codex").join("node_modules");
    let mut directories = vec![(root, 0_u8)];
    while let Some((directory, depth)) = directories.pop() {
        if depth > 6 { continue; }
        let Ok(entries) = fs::read_dir(directory) else { continue; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path_file_name_is(Some(path.as_path()), "codex.exe")
                && path_file_name_is(path.parent(), "bin")
                && path.components().any(|component| component.as_os_str().to_string_lossy().eq_ignore_ascii_case("vendor"))
            {
                return Some(path);
            }
            if path.is_dir() { directories.push((path, depth + 1)); }
        }
    }
    None
}

fn app_data_dir() -> PathBuf {
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") { return PathBuf::from(local_app_data).join("AIUsageDock"); }
    if let Some(user_profile) = env::var_os("USERPROFILE") { return PathBuf::from(user_profile).join("AppData").join("Local").join("AIUsageDock"); }
    PathBuf::from("AIUsageDock")
}

fn resolve_codex_cli() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let mut launchers = Vec::new();
        let mut executables = Vec::new();

        for directory in env::var_os("PATH").map(|value| env::split_paths(&value).collect::<Vec<_>>()).unwrap_or_default() {
            launchers.push(directory.join("codex.cmd"));
            executables.push(directory.join("codex.exe"));
        }
        for variable in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(root) = env::var_os(variable) {
                let npm = PathBuf::from(root).join("npm");
                launchers.push(npm.join("codex.cmd"));
                executables.push(npm.join("codex.exe"));
            }
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = env::var_os(variable) {
                let node = PathBuf::from(root).join("nodejs");
                launchers.push(node.join("codex.cmd"));
                executables.push(node.join("codex.exe"));
            }
        }

        let mut seen = HashSet::new();
        for launcher in launchers {
            if launcher.is_file() && seen.insert(launcher.clone()) {
                if let Some(native) = find_native_codex_near_launcher(&launcher) {
                    return Ok(native);
                }
            }
        }
        for executable in executables {
            if executable.is_file() && seen.insert(executable.clone()) {
                return Ok(executable);
            }
        }
        Err("Codex native CLI niet gevonden · installeer of update Codex CLI".to_string())
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from("codex"))
    }
}

fn codex_command(args: &[&str], home: &Path) -> Result<Command, String> {
    let executable = resolve_codex_cli()?;
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut command = Command::new(executable);
        command.args(args);
        command.env("CODEX_HOME", home);
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
        Ok(command)
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(executable);
        command.args(args);
        command.env("CODEX_HOME", home);
        Ok(command)
    }
}

fn claude_credentials_path() -> Option<PathBuf> {
    if let Some(config_dir) = env::var_os("CLAUDE_CONFIG_DIR") { return Some(PathBuf::from(config_dir).join(".credentials.json")); }
    env::var_os("USERPROFILE").map(|profile| PathBuf::from(profile).join(".claude").join(".credentials.json"))
}

fn load_persisted_state(path: &Path) -> PersistedState {
    fs::read_to_string(path).ok().and_then(|contents| serde_json::from_str(&contents).ok()).unwrap_or_default()
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs() as i64).unwrap_or_default()
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show / Hide", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let top = MenuItem::with_id(app, "always-on-top", "Always on top aan / uit", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &refresh, &settings, &top, &exit])?;
    let mut rgba = vec![0_u8; 16 * 16 * 4];
    for y in 0..16 {
        for x in 0..16 {
            if (x >= 3 && x <= 5 && y >= 7) || (x >= 7 && x <= 9 && y >= 4) || (x >= 11 && x <= 13 && y >= 1) {
                let offset = (y * 16 + x) * 4;
                rgba[offset] = 63;
                rgba[offset + 1] = 124;
                rgba[offset + 2] = 111;
                rgba[offset + 3] = 255;
            }
        }
    }
    let tray_icon = tauri::image::Image::new_owned(rgba, 16, 16);
    TrayIconBuilder::new().icon(tray_icon).menu(&menu).on_menu_event(|app, event| {
        let Some(window) = app.get_webview_window("main") else { return; };
        match event.id.as_ref() {
            "show" => {
                if window.is_visible().unwrap_or(false) { let _ = window.hide(); } else { let _ = window.show(); let _ = window.set_focus(); }
            }
            "refresh" => {
                let state = app.state::<AppState>();
                let panel = state.coordinator.refresh_all();
                let _ = app.emit("usage-updated", panel);
            }
            "settings" => { let _ = app.emit("open-settings", ()); }
            "always-on-top" => {
                let state = app.state::<AppState>();
                state.coordinator.toggle_always_on_top(app);
                let _ = app.emit("usage-updated", state.coordinator.panel_state());
            }
            "exit" => { app.exit(0); }
            _ => {}
        }
    }).build(app)?;
    Ok(())
}

pub fn run() {
    let coordinator = UsageCoordinator::new();
    let background_coordinator = Arc::clone(&coordinator);
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(AppState { coordinator: Arc::clone(&coordinator) })
        .setup(move |app| {
            setup_tray(&app.handle())?;
            coordinator.restore_window_size(app.handle());
            coordinator.restore_window_position(app.handle());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(coordinator.persisted.lock().map(|state| state.settings.always_on_top).unwrap_or(true));
            }
            if coordinator.persisted.lock().map(|state| state.settings.start_with_windows).unwrap_or(false) {
                let _ = app.autolaunch().enable();
            }
            let handle = app.handle().clone();
            thread::spawn(move || loop {
                let panel = background_coordinator.refresh_all();
                let _ = handle.emit("usage-updated", panel);
                thread::sleep(Duration::from_secs(BACKGROUND_REFRESH_SECONDS));
            });
            let announcement_coordinator = Arc::clone(&coordinator);
            let announcement_handle = app.handle().clone();
            thread::spawn(move || loop {
                announcement_coordinator.refresh_announcements();
                let _ = announcement_handle.emit("usage-updated", announcement_coordinator.panel_state());
                thread::sleep(Duration::from_secs(ANNOUNCEMENTS_REFRESH_SECONDS));
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            let state = window.state::<AppState>();
            match event {
                WindowEvent::CloseRequested { api, .. } => { api.prevent_close(); let _ = window.hide(); }
                WindowEvent::Moved(position) => state.coordinator.save_window_position(*position),
                WindowEvent::Resized(size) => state.coordinator.save_window_size(*size),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![get_initial_state, refresh_usage, refresh_announcements, mark_announcements_read, connect_codex, reconnect_provider, update_settings, hide_window])
        .run(tauri::generate_context!())
        .expect("error while running AI Usage Dock");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_windows_are_normalized_by_duration() {
        let response = json!({
            "rateLimits": {
                "primary": { "usedPercent": 28, "windowDurationMins": 300, "resetsAt": 1_800_000_000 },
                "secondary": { "usedPercent": 52, "windowDurationMins": 10080, "resetsAt": 1_800_086_400 },
                "futureBucket": { "usedPercent": 99, "windowDurationMins": 60, "resetsAt": 1_800_000_000 }
            }
        });

        let windows = parse_codex_windows(&response);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window_type, "five_hour");
        assert_eq!(windows[0].remaining_percent, 72.0);
        assert_eq!(windows[1].window_type, "weekly");
        assert_eq!(windows[1].remaining_percent, 48.0);
    }

    #[test]
    fn claude_windows_accept_subscription_shape() {
        let response = json!({
            "five_hour": { "utilization": 37, "resets_at": "1800000000" },
            "seven_day": { "utilization": 63, "resets_at": 1800086400_i64 }
        });

        let windows = parse_claude_windows(&response).expect("valid Claude response");
        assert_eq!(windows[0].window_type, "five_hour");
        assert_eq!(windows[0].remaining_percent, 63.0);
        assert_eq!(windows[1].window_type, "weekly");
        assert_eq!(windows[1].remaining_percent, 37.0);
    }

    #[test]
    fn millisecond_reset_timestamps_are_normalized() {
        assert_eq!(parse_timestamp(&json!(1_800_000_000_000_i64)), Some(1_800_000_000));
        assert_eq!(parse_timestamp(&json!(1_800_000_000_i64)), Some(1_800_000_000));
    }

    #[test]
    fn iso_reset_timestamps_are_normalized_for_claude() {
        assert_eq!(parse_timestamp(&json!("2026-08-31T16:42:00Z")), Some(1_788_194_520));
    }

    #[test]
    fn codex_retries_only_transient_failures() {
        assert!(is_temporary_codex_error("Codex app-server reageert niet"));
        assert!(is_temporary_codex_error("HTTP 429 rate limit"));
        assert!(!is_temporary_codex_error("Codex-login vereist"));
        assert!(!is_temporary_codex_error("Codex CLI niet gevonden"));
        assert!(!is_temporary_codex_error("approval_policy untrusted"));
    }

    #[test]
    fn retry_delay_is_compact_and_bounded() {
        assert_eq!(format_retry_delay(1), "1s");
        assert_eq!(format_retry_delay(60), "1m");
        assert_eq!(format_retry_delay(61), "2m");
        assert_eq!(CODEX_BACKOFF_SECONDS, [60, 120, 240, 480, 900]);
    }

    #[test]
    fn diagnostic_excerpt_omits_sensitive_lines() {
        let output = safe_process_excerpt("connection reset\nhttps://example.test/login?token=secret\nAuthorization: Bearer secret\nretry later");
        assert_eq!(output, "connection reset | retry later");
    }

    #[test]
    fn native_codex_is_preferred_next_to_npm_launcher() {
        let root = env::temp_dir().join(format!("ai-usage-dock-resolver-{}-{}", std::process::id(), unix_now()));
        let launcher = root.join("npm").join("codex.cmd");
        let native = root.join("npm").join("node_modules").join("@openai").join("codex").join("node_modules").join("@openai").join("codex-win32-x64").join("vendor").join("x86_64-pc-windows-msvc").join("bin").join("codex.exe");
        fs::create_dir_all(launcher.parent().expect("launcher parent")).expect("launcher directory");
        fs::create_dir_all(native.parent().expect("native parent")).expect("native directory");
        fs::write(&launcher, "@echo off").expect("launcher");
        fs::write(&native, "binary").expect("native");

        assert_eq!(find_native_codex_near_launcher(&launcher), Some(native));
        let _ = fs::remove_dir_all(root);
    }
}
