# InkPads Desktop

**InkPads Desktop** is a lightweight Windows companion app for [InkPads](https://inkpads.pages.dev/)—the World of Warships Clan Battles replay vault and tactical minimap analyzer.

Instead of manually dragging and dropping `.wowsreplay` files into your browser after every match, InkPads Desktop runs quietly in your Windows system tray and handles Clan Battle replay intake automatically.

---

## What It Does
- **Automatic `replays\` Watcher:** Automatically finds your World of Warships installation (WGC, Steam, or Epic) and watches for newly finished `.wowsreplay` files—including new subfolders created after game patches.
- **Strictly Clan Battles Only:** Randoms, Co-op, Ranked, Brawl, Operations, Training Rooms, and intra-clan Scrims are filtered out locally on your PC before any upload happens.
- **Fast & Bandwidth-Friendly:** Only reads the tiny replay header locally (`<1ms`). Clan Battle **victories** record stats with **0 bytes** of replay video uploaded; only **defeats** are uploaded for minimap rendering.
- **Runs in the Background:** Closing the window minimizes InkPads to your system tray so it keeps watching throughout your Clan Battle session.
- **Always Up to Date:** Web dashboard updates appear automatically, and desktop app updates are delivered via signed releases.

---

## How to Install
1. Download **`InkPads_0.1.2_x64-setup.exe`** (or the latest version) from the [**Releases**](https://github.com/the-mongols/InkPads-Desktop/releases/latest) page.
2. Double-click the `.exe` to run the installer.
   - *Note:* Because early builds are not yet Authenticode-signed, Windows SmartScreen may show *"Windows protected your PC"*. Click **More info → Run anyway** to continue.
3. Launch **InkPads** from your Start Menu.

*(For modpacks / silent install: `InkPads_0.1.2_x64-setup.exe /S`)*

---

## How to Use
1. **Link Your Wargaming Account:**
   - Right-click the **InkPads** icon in your Windows system tray (bottom-right near the clock).
   - Hover over **Link WG Account** and choose your region (**North America (NA)**, **Europe (EU)**, or **Asia (ASIA)**).
   - Sign in with Wargaming OpenID in your browser—the InkPads window will automatically log in and link your clan to the background watcher.
2. **Verify Your WoWS Folder (Optional):**
   - InkPads auto-detects common install paths like `C:\Games\World_of_Warships\replays` and Steam.
   - If your game is installed somewhere custom, right-click the tray icon → **Select WoWS Folder...** and pick your `World_of_Warships` or `replays` folder.
3. **Play Clan Battles:**
   - You can close the InkPads window anytime—it stays active in the system tray.
   - Right-click the tray icon to see your current status (`Watcher: Watching`, `Account: [CLAN] Name`), **Pause / Resume Watcher**, **Check for Updates**, **Sign Out**, or **Quit InkPads**.

---

## Building from Source (Developers)
Requirements: Node.js 22+, Rust toolchain, and Windows WebView2.

```powershell
npm install
npm run dev      # Run in development mode
npm run build    # Build NSIS installer (.exe)
```
