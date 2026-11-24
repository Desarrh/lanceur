# Lanceur

Keyboard launcher bar for Windows: press **Alt+Space**, type two letters, press Enter.
Rust + egui, CPU rendering, single portable exe, runs in the background (tray icon, starts with Windows), ~0 % CPU at rest.

What it finds and does:
- **Apps** (Start menu + `C:\Dev\*\dist\*.exe`), **games** (Steam, Epic, GOG via the Ultimate launcher's library, or Steam/Epic directly), **files and folders** (the Explorateur index if present, otherwise its own light index). Enter opens, Shift+Enter opens the containing folder.
- Learned ranking (frequency + recency, and your last pick for the exact same query).
- **Calculator and conversions**: `2,5*4`, `200 + 10%`, `10 km en mi`, `100 usd en eur` (ECB rates cached daily), `#2E5E9E` (color preview).
- **Windows commands**: lock, sleep, empty/open recycle bin, `vol 30`, mute, `sortie` (switch audio output).
- **Prefixes**: `g` Google, `yt` YouTube, `gh` GitHub, `y` Tube, `v` Mémoire search, `k <name|:port>` stop a dev server (Vigie), `ram`, `mode jeu`, `p <game>`, `bilan`, `d <project> [t|gh]` (VS Code, terminal, GitHub), `;snippet`, `:3000` (what listens on a port: open or kill), `c` (resume a Claude Code session), `!` (repeat last action).
- **Selected text** in the foreground app (read through UI Automation, never by simulating Ctrl+C): translate, web search, format/compact JSON, compute/convert, open link or path.
- Wrong keyboard layout tolerated (AZERTY ↔ QWERTY).
- Headless: `Lanceur.exe --action "<query>"` runs the first result without UI (for a Stream Deck).

Integrations with the author's other apps are optional: when an app is missing or too old, its entries are hidden.

License: MIT.
