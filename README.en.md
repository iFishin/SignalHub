# SignalHub

[中文](README.md) · **English**

A cross-platform floating status light for your desktop. A stack of always-on-top, draggable transparent signs that show the current state with colour and rhythm; click one to collapse the whole thing into a single plain light.

Built for AI coding agents (Claude Code, Codex, Cursor, Gemini CLI, Goose) first, but **not tied to them** — any program that can send one HTTP request can report its state.

> The design docs in [`docs/`](docs/) are written in Chinese. They are maintainer documentation: they record *why* each decision was made and which bugs were actually hit. This README and the app UI are available in English.

## States

| State | Looks like | Means |
|---|---|---|
| `idle` | green, steady | Idle, nothing to do |
| `thinking` | green, fast blink | Reasoning |
| `working` | green, slow blink | Running a tool / task |
| `done` | green, steady + a pulse | Finished (the session disappears after 30s) |
| `attention` | amber, blinking | Wants your attention |
| `permission` | red, smooth blink | Waiting for approval |
| `error` | red, hard strobe | Error / blocked |
| `stale` | grey, steady | There is a session, but it stopped talking |
| `offline` | grey, dimmer | No sessions at all |
| `off` | grey, dimmer | Monitoring is paused |

Priority is **red > amber > green**: while one group is waiting for approval, another group reporting "working" will not override the red; and when the waiting group goes back to work, the red clears on its own. The global state is the highest priority across all groups.

**Each sign** is a dark rounded pill (the same visual language as the app icon): a coloured dot, the group name, the state, and how long it has been that way — plus a second line when there is a message.

**Grouping** is decided by the reporter — pass an optional `group`:

```bash
curl -s -X POST ... -d '{"state":"working","session":"a","group":"work"}'
curl -s -X POST ... -d '{"state":"error","session":"b","group":"ci"}'
```

You then get two signs on the lamp (the most urgent one first), and the global state is the highest of the two. Omit `group` and it goes to `default`, which behaves exactly like a single-group setup.

**Each group's label and position are yours to set** (Settings → Status): a group name is an identifier the reporter chose (`build-1`), and you can rename it to something readable, or **pin** it so it always sits first on the lamp. Unpinned groups still sort by urgency, so a red one can never be pushed below a green one. Both are display-only: the group names in the API, and hide/mute matching, are unaffected.

Three channels that do not rely on colour:

- **Shape**: by default `permission` and `error` are drawn as rounded squares, everything else as circles. Red/green is the hardest pair for colour blindness, and those are exactly the two states that most need your attention.
- **Thin ring**: a `stale` dot carries a thin ring that shortens as its grace period runs out, so you can see at a glance how long it has been quiet.
- **Brightness**: "quiet mode" is on by default — when idle, quiet or offline the whole sign fades to almost transparent, and only becomes fully visible when something is actually happening or needs you. Hover restores it. Can be turned off.

**Appearance is yours** (Settings → Appearance): four presets (Single light / Compact / Standard / Bold) set the whole look in one click, or tune each item — shape (expanded signs vs. a single light), size, dot shape, palette, background opacity, blink speed, and which parts of the sign to show (group name / state / duration / message).

**Every state can be styled individually** (Settings → Effects): colour, shape and rhythm can each be overridden per state; anything you leave alone follows the global defaults on the Appearance page. Four more presets (Follow global / Distinguish by shape / Rhythm by state / Blink only when needed) set the whole table at once.

**A system notification when something needs you**: fired when the state newly becomes "needs permission / needs attention / error", with the project name and the agent's own explanation. It only fires on the *transition*, not on every change — a single agent turn pushes state dozens of times, and a notification each time would just be noise. The state also has to **persist** (5 seconds by default, configurable), which filters out the `permission` prompts that come and go within a second. You can add a sound (works on macOS and Windows), **mute a group** (affects notifications and sound only — the lamp still shows it), and set a **quiet-hours window** (hourly granularity, handles crossing midnight; nothing fires or makes a sound during it, and nothing is delivered late — waking up to a pile of notifications from the night is exactly what quiet hours exist to prevent). Notifications also carry two buttons: **Open project folder** and **Mute this group**, so you do not have to go find the project yourself.

**Or forward it somewhere else**: configure any number of outbound hooks in the Hooks tab — on a state change, POST to a URL (ntfy, Slack, your own endpoint) or run a local command, each filtered to the states you care about. They run asynchronously with a timeout, so a dead target cannot affect anything locally. Format and security notes: [docs/api.md §4.9](docs/api.md).

**Left-click** toggles between the stack of signs and a single light, and the choice is remembered. **Right-click a sign** to hide that group, mute it, clear its state, change the shape, or open settings. The lamp remembers where you dragged it. The settings panel header shows today's cumulative time per state, and the Status tab has a 30-minute timeline where each segment's width *is* its duration.

**Every state has a silence timeout** (`working` 90s, `thinking` 60s, `idle` 600s, …). When it expires the session does not vanish — it turns `stale` (grey, steady) and is deleted five minutes later. That way "the agent crashed / stopped talking" and "the agent is idle and waiting" look different, and you never get a lamp that keeps blinking for an agent that is long gone.

## Running it

You need Node.js and a Rust toolchain.

```bash
npm install
npm run tauri dev
```

On first launch it creates `port` and `token` under `~/.signalhub/` (`%USERPROFILE%\.signalhub` on Windows). The window is borderless, is not in the taskbar and does not take a Dock icon — **quit it from the tray icon**.

## Reporting state

HTTP API (defaults to `127.0.0.1:41320`; the real port is in `~/.signalhub/port`):

```bash
curl -s -X POST "http://127.0.0.1:$(cat ~/.signalhub/port)/v1/state" \
  -H "Authorization: Bearer $(cat ~/.signalhub/token)" \
  -H 'Content-Type: application/json' \
  -d '{"state":"permission","session":"demo","message":"needs approval"}'
```

There is no `$(cat …)` on Windows; the PowerShell equivalent:

```powershell
$dir = "$env:USERPROFILE\.signalhub"
$port = Get-Content "$dir\port"
$token = Get-Content "$dir\token"
Invoke-RestMethod -Method Post "http://127.0.0.1:$port/v1/state" `
  -Headers @{ Authorization = "Bearer $token" } `
  -ContentType 'application/json' `
  -Body '{"state":"permission","session":"demo","message":"needs approval"}'
```

Or use the CLI:

```bash
signalhub working --session build-1 --agent ci --message "compiling"
signalhub status
signalhub watch                                   # follow state changes in a terminal
signalhub watch --until done --timeout 10m        # wait for a state in CI
```

Full API reference: [docs/api.md](docs/api.md).

## Wiring up an agent

```bash
signalhub install hooks claude-code      # also: codex / cursor / gemini / goose
signalhub install hooks claude-code --dry-run   # see what it would change first
```

It writes hooks into the agent's config file (`~/.claude/settings.json` for Claude Code). It touches **only the `hooks` key**, keeps everything else byte for byte, and backs the file up to `*.signalhub-backup` first. Running it twice does not duplicate entries; if the command changed it updates in place; and if the file is not valid JSON it aborts without writing anything. Undo with `signalhub uninstall hooks claude-code`.

**Goose is the exception**: it uses a plugin mechanism, so SignalHub installs a directory of its own (`~/.agents/plugins/signalhub/`) and never touches your shared config — uninstalling removes the whole thing. The others insert an entry into a shared config file.

Restart the agent afterwards. Then: prompt → fast green, tool call → slow green, permission → red, answer → a pulse and back to steady. Per-agent differences (which ones have no "needs permission" event, whose payload field names differ) are in [docs/api.md §4](docs/api.md).

Put the CLI on your PATH first, so what gets written into the config is the portable bare name rather than an absolute path:

```bash
ln -sfn "$(pwd)/src-tauri/target/debug/signalhub" ~/.local/bin/signalhub
signalhub install hooks claude-code
```

On Windows there is no symlink step — append `src-tauri\target\debug` (that is where `signalhub.exe` lives) to your **user** PATH and open a new terminal:

```powershell
$user = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path',
  "$user;$(Resolve-Path src-tauri\target\debug)", 'User')
```

An absolute path works too, but the moment you move the binary the hooks silently stop working (nothing shows up in the agent's session) and that is a painful thing to track down.

## Status

Done: the floating lamp, state aggregation and TTL, the HTTP reporting API, the CLI (including `watch`), the settings panel, automatic hook installation (Claude Code / Codex / Cursor / Gemini CLI / Goose), system notifications with sound and action buttons, window position memory, the expanded/collapsed shapes, per-group right-click menus, today's duration stats and a 30-minute timeline, outbound hooks, per-state colour / shape / rhythm, per-group mute, quiet hours, a persistence threshold before alerting, a tray icon that follows the state, single-instance, launch at login, a global shortcut, and click-through mode.

To do: ship the CLI as a Tauri sidecar inside the app bundle (only needed for distribution).
