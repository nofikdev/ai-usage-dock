# AI Usage Dock

Small local Windows utility for seeing the remaining Codex and Claude Code subscription windows at a glance.

## Installeren

Download bij [Releases](../../releases/latest) `AI-Usage-Dock-Setup.exe` en start de installer. Dit bestand is altijd de nieuwste Windows-release. Sluit een oudere dock-versie eerst via het systeemvak. De dock draait lokaal op Windows; er is geen PHP-, Apache-, localhost- of cloudserver nodig.

De huidige release bevat:

- één dock-instantie tegelijk;
- compacte, versleepbare en resizable UI;
- opgeslagen vensterpositie en -afmetingen;
- Codex-usage per account en Claude Code-usage;
- automatische refresh met een rustige interval en handmatige refresh.

## Gebouwd met

- **Rust** — de native Windows-laag via Tauri: venster, systeemvak, lokale opslag, processen en provider-requests;
- **TypeScript + React** — de zichtbare dock en instellingen;
- **CSS** — compacte en responsive layout;
- **Node.js/npm** — development- en build tooling.

Er wordt geen PHP gebruikt in dit project. De Codex- en Claude-login blijven lokaal op de computer; tokens worden niet naar de React-state of het opgeslagen dock-statebestand gekopieerd.

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

If Codex cannot connect, check the local CLI once in PowerShell:

```powershell
codex --version
where.exe codex.cmd
codex doctor
```

Then restart the dock and use **Connect** for the required Codex account. Codex 1 and Codex 2 have separate local logins.
