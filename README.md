# InkPads Desktop

Lightweight Tauri v2 desktop companion for [InkPads](https://inkpads.pages.dev/) designed for distribution via World of Warships modpacks (Aslain's Modpack, WoWS ModStation).

## Phase 1 Capabilities
- **Remote WebView Shell:** Loads `https://inkpads.pages.dev/` so web updates deploy without requiring a new installer.
- **System Tray & Single-Instance:** Closing the window minimizes to the Windows system tray so replay watching continues during Clan Battle sessions.
- **WoWS `replays\` Auto-Detection & Watcher:** Automatically locates World of Warships (`C:\Games\World_of_Warships*`, Steam, or manual selection), watches `replays\` (including new versioned patch subfolders), ignores `temp.wowsreplay` and `tempArenaInfo.json`, and waits for write stabilization.
- **Strict Clan Battle Scope Gate & Golden Invariant 1:** Probes only the 12-byte header + JSON metadata in Rust (`<1ms`), drops non-Clan Battles / scrims / training rooms / KOTS locally, quarantines spectators, and runs `/api/worker/batch-preflight` so victories record stats with 0 bytes uploaded.

## Development
```powershell
npm install
npm run dev
```

## Release Build (NSIS `.exe`)
```powershell
npm run build
```
