# AI Usage Dock

Small local Windows utility for seeing the remaining Codex and Claude Code subscription windows at a glance.

## Current shape

- Tauri 2 + React + TypeScript + Rust
- no PHP, Apache, localhost backend, Electron, cloud sync or telemetry
- one normalized `UsageSnapshot` contract between native providers and the UI
- two isolated Codex homes under `%LOCALAPPDATA%\AIUsageDock\codex\account-*`
- Claude credentials are read only from Claude Code's existing credentials file
- cached snapshots and window preferences live in `%LOCALAPPDATA%\AIUsageDock\state.json`

The dock's primary task is scanning current remaining usage. Settings and connection actions are deliberately kept in the same small utility window.

## Development

```powershell
npm install
npm run dev
```

For the native app, install Rust and the Tauri Windows prerequisites first, then run:

```powershell
npm run tauri dev
```

The browser development fallback renders safe fixture data so the layout can be inspected without exposing credentials or starting provider processes.

## Native provider notes

Codex is queried through two long-lived `codex app-server --stdio` processes. Claude uses the existing Claude Code OAuth credentials and its compatibility usage endpoint behind `ClaudeUsageProvider`. Provider failures preserve the last valid snapshot and never put provider credentials into React state or the state file.
