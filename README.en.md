# SignalHub

[中文](README.md) · **English**

A cross-platform floating status light for your desktop. A stack of always-on-top, draggable transparent signs that show the current state with colour and rhythm; click one to collapse the whole thing into a single plain light.

Built for AI coding agents first: **Claude Code, Codex, Cursor, Gemini CLI and Goose connect in one click** (SignalHub writes hooks into their own config files), while **Pi Agent reports from the command line only** (call `signalhub` once from your own script). But it is **not tied to them** — any program that can send one HTTP request can report its state.

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

**Grouping** is decided by the reporter — pass an optional `group` (the `group` field in the HTTP body, or `--group` on the command line):

```bash
curl -s -X POST ... -d '{"state":"working","session":"a","group":"work"}'
curl -s -X POST ... -d '{"state":"error","session":"b","group":"ci"}'

signalhub working --session a --group work
signalhub error --session b --group ci
```

You then get two signs on the lamp (the most urgent one first), and the global state is the highest of the two. Omit `group` and it goes to `default`, which behaves exactly like a single-group setup. The hook commands we install carry no `--group`, so every agent you connect today still lands in `default` — grouping is for when you explicitly want more than one.

**A reporter can also declare what kind of source it is** (`--source agent|system|custom`; otherwise it is inferred from the name): the Connect page uses it to split everything that is not a known agent into **System** and **Custom**, so a script watching the machine does not get mixed up with your own small tools. `signalhub hook` reports `agent`; commands wrapped by `signalhub run`, and a hand-written `signalhub working`, land in `custom`; `--source system` puts you in the system group (the built-in collectors will use it later). It only affects how the Connect page groups things — aggregation, grouping, TTL and notifications are unaffected, so getting it wrong, or leaving it out, changes nothing about the lamp.

If you would rather not type `group` every time, Settings → Connect has a **Group by working directory** switch: turn it on and reports that carry no `group` are grouped by their `cwd` instead — one directory per sign, one project per row. A report that does carry a `group` still wins, so the two can be mixed. The group name is that path; rename it under Appearance → Per-group display if it is too long. It is **off by default**, so upgrading never makes anyone's lamp sprout extra signs.

**Each group's label and position are yours to set** (Settings → Appearance → Per-group display): a group name is an identifier the reporter chose (`build-1`), and you can rename it to something readable, or **pin** it so it always sits first on the lamp. Unpinned groups still sort by urgency, so a red one can never be pushed below a green one. Both are display-only: the group names in the API, and hide/mute matching, are unaffected.

Three channels that do not rely on colour:

- **Shape**: by default `permission` and `error` are drawn as rounded squares, everything else as circles. Red/green is the hardest pair for colour blindness, and those are exactly the two states that most need your attention.
- **Thin ring**: a `stale` dot carries a thin ring that shortens as its grace period runs out, so you can see at a glance how long it has been quiet.
- **Brightness**: "quiet mode" is on by default — when idle, quiet or offline the whole sign fades to almost transparent, and only becomes fully visible when something is actually happening or needs you. Hover restores it. Can be turned off.

**Appearance is yours** (Settings → Appearance): four presets (Single light / Compact / Standard / Bold) set the whole look in one click, or tune each item — shape (expanded signs vs. a single light), size, dot shape, palette, background opacity, blink speed, and which parts of the sign to show (group name / state / duration / message).

**Every state can be styled individually** (Settings → Effects): colour, shape and rhythm can each be overridden per state; anything you leave alone follows the global defaults on the Appearance page. Four more presets (Follow global / Distinguish by shape / Rhythm by state / Blink only when needed) set the whole table at once.

**A system notification when something needs you**: fired when the state newly becomes "needs permission / needs attention / error", with the project name and the agent's own explanation. It only fires on the *transition*, not on every change — a single agent turn pushes state dozens of times, and a notification each time would just be noise. The state also has to **persist** (5 seconds by default, configurable), which filters out the `permission` prompts that come and go within a second. You can add a sound (works on macOS and Windows), **mute a group** (affects notifications and sound only — the lamp still shows it), and set a **quiet-hours window** (hourly granularity, handles crossing midnight; nothing fires or makes a sound during it, and nothing is delivered late — waking up to a pile of notifications from the night is exactly what quiet hours exist to prevent). Notifications also carry two buttons: **Open project folder** and **Mute this group**, so you do not have to go find the project yourself.

**Or forward it somewhere else**: configure any number of outbound hooks in the Hooks tab — on a state change, POST to a URL (ntfy, Slack, your own endpoint) or run a local command, each filtered to the states you care about. They run asynchronously with a timeout, so a dead target cannot affect anything locally. Format and security notes: [docs/api.md §4.9](docs/api.md).

**And there is a light beyond the screen**: at the bottom of the Hooks tab, enter a WLED address and the state colour is POSTed straight to it (`POST /json/state`) — a lamp next to your desk follows the state, so you can tell what is going on without looking at the screen. Only the colour is sent: no group names, no session names, no message text. Saving lights it up once with the current state — that is the answer to "does it work?" — and the result lands in the same log as the outbound hooks (`ok 200 success` means it does). Nothing is configured by default.

**Watch it from your phone**: turn on Settings → Connect → Read-only web panel and you get a link (token included, ready to copy) that shows the lamp in any phone or tablet browser — the state, what each group is doing, how long it has been that way. Read-only, nothing to install. It is **off by default**, and it shares the reporting service's listening address: only with "Allow LAN access" on can devices on the same WiFi see it (the settings page then warns you not to pass the link around, and not to leave it on over public WiFi).

**Left-click** toggles between the stack of signs and a single light, and the choice is remembered. **Right-click a sign** to hide that group, mute it, clear its state, change the shape, or open settings. The lamp remembers where you dragged it. The settings panel header shows today's cumulative time per state, and the Status tab has a 30-minute timeline where each segment's width *is* its duration.

**Every state has a silence timeout** (`working` 90s, `thinking` 60s, `idle` 600s, …). When it expires the session does not vanish — it turns `stale` (grey, steady) and is deleted five minutes later. That way "the agent crashed / stopped talking" and "the agent is idle and waiting" look different, and you never get a lamp that keeps blinking for an agent that is long gone.

## Installing

Grab a build from [Releases](https://github.com/iFishin/SignalHub/releases):

- **macOS** — the `.dmg`; drag SignalHub into Applications. Pick `aarch64` on Apple Silicon, `x64` on Intel.
- **Windows** — the `-setup.exe` (NSIS installer) or the `.msi`.

**The builds are unsigned**, so the first launch is blocked once. On macOS, run this in a terminal:

```bash
xattr -dr com.apple.quarantine /Applications/SignalHub.app
```

That clears the quarantine flag recursively for the whole bundle — both the app and the `signalhub` command bundled inside it. (Right-clicking the icon → Open → Open also works, but may not cover that inner command.) On Windows, click "More info" → "Run anyway" in SmartScreen.

The `signalhub` command **ships inside the app** — no separate install. The one-click setup under Settings → Connect uses it. To use it in a terminal, put it on your PATH:

```bash
ln -sfn /Applications/SignalHub.app/Contents/MacOS/signalhub /usr/local/bin/signalhub
```

Pushing a tag starting with `v` builds all three packages automatically — see [.github/workflows/release.yml](.github/workflows/release.yml).

## Running from source

You need Node.js and a Rust toolchain.

```bash
npm install
npm run tauri dev
```

**Run `npm run stage-cli` before `cargo test`.** The CLI ships as a sidecar inside the app bundle (see `externalBin` in [tauri.conf.json](src-tauri/tauri.conf.json)), and `tauri-build` checks that the file exists at **build-script time** — without it, even `cargo test` fails to compile. `tauri dev` / `tauri build` run that step for you; only direct `cargo` invocations need it (and again after `cargo clean`).

On first launch it creates `port` and `token` under `~/.signalhub/` (`%USERPROFILE%\.signalhub` on Windows). The window is borderless, is not in the taskbar and does not take a Dock icon — **quit it from the tray icon**.

## Reporting state

HTTP API (defaults to `127.0.0.1:41320`; the real port is in `~/.signalhub/port`). **It only listens on this machine by default** — other machines cannot reach it, and that is its default security boundary:

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

**Reporting from another machine**: Settings → Connect has an **Allow LAN access** switch (off by default). Turned on, the server binds `0.0.0.0` and machines, containers or WSL on the same network can reach it — at the cost that **anyone on that network who has the token can report**, so do not turn it on over public WiFi. It takes effect after restarting the app.

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

Done: the floating lamp, state aggregation and TTL, the HTTP reporting API, the CLI (including `watch`), the settings panel, automatic hook installation (Claude Code / Codex / Cursor / Gemini CLI / Goose), Pi Agent (command-line reporting), `signalhub run` (wraps any command and reports for it), CLI text that follows the UI language (`--help`, errors and command output all have Chinese and English), system notifications with sound and action buttons, window position memory, the expanded/collapsed shapes, per-group right-click menus, today's duration stats and a 30-minute timeline, outbound hooks, per-state colour / shape / rhythm, per-group mute, quiet hours, a persistence threshold before alerting, repeat reminders while something stays stuck (again at 5 / 15 / 30 minutes, saying how long it has been waiting), a tray icon that follows the state, single-instance, launch at login, a global shortcut, grouping (an explicit `group` or `--group`, or an automatic fallback per working directory or per session — one sign per session), per-day stats persisted to disk (kept for 30 days), configuration export / import, an opt-in LAN access switch (off by default), click-through mode, source kinds on the Connect page (AI agents / system / custom, declared with `--source`), a read-only web panel (open a link on your phone to watch the lamp; off by default), and physical lights (WLED — the state colour is POSTed to the light, colour only, nothing configured by default).

To do: code signing and notarization (the steps have a place reserved in [.github/workflows/release.yml](.github/workflows/release.yml) — fill them in once you have certificates).

**Auto-update is not wired up yet** — not for lack of time, but because there is one thing I could not find an answer to and that needs testing on a real machine: whether Gatekeeper blocks an *unsigned* macOS app from installing an update after its bundle is replaced. The wiring steps (how to generate the key, and why the public key goes in as contents rather than a path) are written at the top of [.github/workflows/release.yml](.github/workflows/release.yml), ready to switch on once that has been verified.

## License

MIT — see [LICENSE](LICENSE).
