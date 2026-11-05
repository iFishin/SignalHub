# SignalHub

[中文](README.md) · **English**

A cross-platform floating status light for your desktop. A stack of always-on-top, draggable transparent signs that show the current state with colour and rhythm; click one to open the inbox, double-click to collapse the whole thing into a single plain light.

Built for AI coding agents first: **Claude Code, Codex, Cursor, Gemini CLI, Goose, WorkBuddy, opencode, Pi and Hermes all connect in one click** (most get hooks written into their own config files; opencode, Pi and Hermes get a plugin file). But it is **not tied to them** — any program that can send one HTTP request can report its state.

> The design docs in [`docs/`](docs/) are written in Chinese. They are maintainer documentation: they record *why* each decision was made and which bugs were actually hit. This README and the app UI are available in English.

## States

| State | Looks like | Means |
|---|---|---|
| `idle` | green, steady | Idle, nothing to do |
| `thinking` | green, fast blink | Reasoning |
| `working` | green, slow blink | Running a tool / task |
| `done` | green, steady + a pulse | Finished (disappears after 3 min by default; configurable) |
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

**A reporter can also declare what kind of source it is** (`--source agent|system|custom`; otherwise it is inferred from the name): the Connect page uses it to split everything that is not a known agent into **System** and **Custom**, so a script watching the machine does not get mixed up with your own small tools. `signalhub hook` reports `agent`; commands wrapped by `signalhub run`, and a hand-written `signalhub working`, land in `custom`; `--source system` puts you in the system group (the built-in Meeting and Backup collectors report as it). It only affects how the Connect page groups things — aggregation, grouping, TTL and notifications are unaffected, so getting it wrong, or leaving it out, changes nothing about the lamp.

**The machine's own state can be a signal source too.** The Connect page's **System sources** section holds two built-in collectors, **both off by default**: **In a meeting** (the microphone or camera is in use by any app → a "Meeting" sign appears, and it reports "Meeting ended" afterwards) and **Time Machine backups** (`tmutil status` → a "Backup" sign, reporting "Backup finished" once done). They read system state every 5 seconds — **only whether a device is in use or a backup is running; they never capture or store any audio, video or file contents**, and reading this state does not trigger microphone or camera permission prompts. If a collector thread errors it writes a single log line and lets its sign expire via the 15-second TTL, so no false state is left behind. Both exist on macOS only; on other platforms the section simply does not list them.

If you would rather not type `group` every time, Settings → Connect has a **Group by working directory** switch: turn it on and reports that carry no `group` are grouped by their `cwd` instead — one directory per sign, one project per row. A report that does carry a `group` still wins, so the two can be mixed. The group name is that path; rename it under Appearance → Per-group display if it is too long. It is **off by default**, so upgrading never makes anyone's lamp sprout extra signs.

**Each group's label and position are yours to set** (Settings → Appearance → Per-group display): a group name is an identifier the reporter chose (`build-1`), and you can rename it to something readable, or **pin** it so it always sits first on the lamp. Unpinned groups still sort by urgency, so a red one can never be pushed below a green one. Both are display-only: the group names in the API, and hide/mute matching, are unaffected.

Three channels that do not rely on colour:

- **Shape**: by default `permission` and `error` are drawn as rounded squares, everything else as circles. Red/green is the hardest pair for colour blindness, and those are exactly the two states that most need your attention.
- **Thin ring**: a `stale` dot carries a thin ring that shortens as its grace period runs out, so you can see at a glance how long it has been quiet.
- **Brightness**: "quiet mode" is on by default — when idle, quiet or offline the whole sign fades to almost transparent, and only becomes fully visible when something is actually happening or needs you. Hover restores it. Can be turned off.

**Appearance is yours** (Settings → Appearance): four presets (Single light / Compact / Standard / Bold) set the whole look in one click, or tune each item — shape (expanded signs vs. a single light), size, dot shape, palette, background opacity, blink speed, and which parts of the sign to show (group name / state / duration / message).

**Every state can be styled individually** (Settings → Effects): colour, shape and rhythm can each be overridden per state; anything you leave alone follows the global defaults on the Appearance page. Four more presets (Follow global / Distinguish by shape / Rhythm by state / Blink only when needed) set the whole table at once.

**A system notification when something needs you**: fired when the state newly becomes "needs permission / needs attention / error", with the project name and the agent's own explanation. It only fires on the *transition*, not on every change — a single agent turn pushes state dozens of times, and a notification each time would just be noise. The state also has to **persist** (5 seconds by default, configurable), which filters out the `permission` prompts that come and go within a second. You can add a sound (works on macOS and Windows), **mute a group** (affects notifications and sound only — the lamp still shows it), and set a **quiet-hours window** (hourly granularity, handles crossing midnight; nothing fires or makes a sound during it, and nothing is delivered late — waking up to a pile of notifications from the night is exactly what quiet hours exist to prevent). There are also three **presets** (off by default, one switch each): push again through your phone-push hook when an approval has been stuck for 60 seconds; hold notifications while an agent is thinking or working; and turn the meeting sign blue and go silent while a call is on (with the "In a meeting" collector). They only decide *when to interrupt you* — there is no expression engine. Notifications also carry two buttons: **Open project folder** and **Mute this group**, so you do not have to go find the project yourself.

**Or forward it somewhere else**: configure any number of outbound hooks in the Forwarding tab — on a state change, POST to a URL (ntfy, Slack, your own endpoint) or run a local command, each filtered to the states you care about. Common services have presets; for your own endpoint pick **Custom body** and write your own JSON with `{state}` / `{group}` / `{message}` placeholders. They run asynchronously with a timeout, so a dead target cannot affect anything locally. Format, placeholders and security notes: [docs/api.md §4.9](docs/api.md).

**And there is a light beyond the screen**: at the bottom of the Forwarding tab, enter a WLED address and the state colour is POSTed straight to it (`POST /json/state`) — a lamp next to your desk follows the state, so you can tell what is going on without looking at the screen. Only the colour is sent: no group names, no session names, no message text. Saving lights it up once with the current state — that is the answer to "does it work?" — and the result lands in the same log as the outbound hooks (`ok 200 success` means it does). Nothing is configured by default.

**Watch it from your phone**: turn on Settings → Connect → Read-only web panel and you get a link (token included, ready to copy) that shows the lamp in any phone or tablet browser — the state, what each group is doing, how long it has been that way. Read-only, nothing to install. It is **off by default**, and it shares the reporting service's listening address: only with "Allow LAN access" on can devices on the same WiFi see it (the settings page then warns you not to pass the link around, and not to leave it on over public WiFi).

**The inbox** (click the lamp, `⌃⌥I`, or the first tray item) lists every session under "Needs you → Running → Just finished → Idle". Pending approvals can be allowed or denied right there; every row can jump to its terminal tab (Terminal / iTerm2 / tmux — the first jump asks for Automation permission to control the terminal), mute the group, or clear the session. When nothing needs you it stays a quiet empty panel. **Double-click the lamp** to toggle between the stack of signs and a single light; the choice is remembered. **Right-click a sign** to hide that group, mute it, clear its state, change the shape, or open settings. The lamp remembers where you dragged it. The settings panel header shows today's cumulative time per state, and the Status tab has a 30-minute timeline where each segment's width *is* its duration.

**Five global shortcuts** (listed in Settings → System, along with any that could not be registered): `⌃⌥I` opens the inbox, `⌃⌥L` shows / hides the lamp, `⌃⌥M` mutes / unmutes "the worst group", `⌃⌥0` clears the worst group, `⌃⌥S` toggles click-through. On Windows / Linux these are `Ctrl+Alt+I / L / M / 0 / S`. "The worst group" is the most urgent one — the same rule that puts a sign at the top of the lamp (pinning only changes the display order). Muting only affects notifications and sound; the lamp still shows that group, so the shortcut posts a notice saying what it did. They are deliberately **not configurable** — the real failure is another app already owning a combination, and making them configurable would not fix that — so any that could not be registered is flagged instead of becoming a key that does nothing.

**You are a signal source too**: the tray menu's "Manual state" submenu has "Focus for 25 minutes" (a countdown appears on the lamp, ticking down once a minute, then notifying and turning `done`), "In a meeting", "Away" (two grey signs — half-transparent in quiet mode, which reads as "nothing is happening here" from across the room, with the line on the sign saying which one it is) and "Clear manual state". They are **just an ordinary group** (named "我" / "Me" to match the UI language), so stats, history, the read-only panel, outbound hooks and physical lights all pick them up — there is no second state machine. Meeting and Away occupy one sign and **do not touch notification settings**: use "Pause / resume monitoring" in the tray for "do not disturb me". This is also the cheapest proof that SignalHub is useful with no agents at all.

**Approve from the lamp** (Settings → Connect → Approve from the lamp, **off by default**): once on, every time Claude Code wants to ask you to approve a tool call, that confirmation box moves onto the lamp — a bubble appears, you click Allow or Deny, or you ignore it (after 45 seconds the question goes back to the terminal). **This is the only feature that changes how the agent behaves**, so it only acts at the moment the agent was going to ask anyway (`PermissionRequest`) and does not intercept other tool calls; it **never says "allow" on your behalf** (timeout, superseded, switch off, LAN access on — all of them say nothing and the question returns to the terminal); and **if it cannot be shown in full it cannot be allowed** — a command over 240 characters counts as truncated, and that one can only be denied or approved back in the terminal (a command you cannot read in full should not be one click away from running). The full command appears only in this local window; what gets reported is still the short phrase ("run npm"), and neither notifications nor outbound hooks carry it. The endpoint requires a token (the entry written into `~/.claude/settings.json` carries it), and uninstalling removes that token-bearing entry too. From the command line: `signalhub install hooks claude-code --approve` (then turn on the switch on the Connect page — that is the real gate).

**Claude Code with the HTTP install mode is the only supported agent today.** `install_approve`, the approve URL and the endpoint's path check all accept `claude-code` only, and the switch appears on that card alone. For every other agent — **WorkBuddy included** — a permission request still turns the lamp red and still fires a notification, but the confirmation stays in the agent's own window; go there to allow or deny. The desktop WorkBuddy build does not fire the `PermissionRequest` hook (it never appears in the local hook log; the red state comes from `Notification` with `notification_type: "permission_prompt"`), and its hooks reference does not document whether an HTTP hook can return a decision. We did not simply widen the allowlist: a switch that does nothing when you flip it is worse than no switch.

**Every state has a silence timeout** (`working` 600s, `attention` / `error` 300s, `thinking` / `permission` / `idle` 600s, `done` 180s; all configurable under Behavior → Silence timeouts or Look → Per-state overrides). What happens after it depends on whether that reporter **knows the agent's process**:

- **It does** (Claude Code's command hooks, the opencode / Pi / Hermes plugins, `signalhub run`; Claude Code's optional HTTP hook mode carries no PID): if the process is still alive the session turns **idle** (the window is open, there just were no events to report); if the process is gone the session is removed immediately, without waiting for the timeout. **"Needs permission / attention / error" are not re-judged this way** — a live process does not prove the request was answered, so they still go stale.
- **It does not** (HTTP reporters, hand-written commands, the other hooks): the session turns `stale` (grey, steady) and is deleted 30 minutes later.

That way "the agent crashed / stopped talking" and "the agent is idle and waiting" look different, and you never get a lamp that keeps blinking for an agent that is long gone. **Hooks installed by an older version must be re-installed from the Connect page** to pick this up; until then they keep the old silence behaviour.

**The lamp does not immediately snap to "No agents reporting" when work finishes.** A `done` sign is deleted outright on expiry (it does not go `stale`), so at that moment the lamp would have nothing left to say but "No agents reporting" — exactly when you are asking yourself whether that last task actually finished. The lamp remembers the last completion: for ten minutes the empty state reads "**Just finished · 2 min ago**", and only after that does it fall back to "No agents reporting". That memory is display-only; it changes no state decision.

**See it before you install anything**: Settings → Demo plays a 30-second walkthrough — idle → thinking → working → waiting for approval → approved → done → an empty lamp that still remembers it just finished. It renders the **real lamp component** against scripted data: no real state is touched, no notification is fired, nothing is written to history, and no agent is actually started. Pause, step and replay are all there. **On a first run with no groups at all**, the sign itself offers a small "See demo" button that jumps straight to that page; it disappears for good once a real agent reports in, or once you dismiss it. Opening `?window=demo` in a browser shows the same walkthrough as a standalone page (for README gifs and for people still deciding whether to install) — it is not a fourth Tauri window.

**Settings are easier to get around**: the left rail has a search box matching tab names and Chinese/English aliases ("timeout", "ttl", "demo"), long pages get a sticky row of section links at the top, and the last tab you were on is remembered. Focus rings, an active-tab indicator and a 140ms page transition are in place; the animation turns itself off when the OS asks for reduced motion.

**Find out whether notifications work with one click**: Settings → Alerts has a "Send a test notification" button. It goes through **the same outlet as a real alert** (including the sound, if enabled) and touches no state and writes nothing to stats. It reports the current notification permission afterwards — without permission the OS drops a notification silently, with no popup, no sound and no error, so "I clicked and nothing happened" cannot tell a missing permission from Focus mode or a broken build. If you see nothing, check SignalHub under Notifications in System Settings and whether Focus is on in Control Center.

**One pre-release check instead of five pages**: "Run check" at the top of Settings → System walks through notification permission, the notification / sound / delay / quiet-hours / muted-group switches, whether installed hook commands still resolve, whether an install is stale after an upgrade, **how long ago an agent last reported**, whether "approve from lamp" matches a Claude Code HTTP install, custom preset parse errors, shortcut registration, the hook log and the active session count. Each line is flagged and the whole thing can be copied as a report. It reads data that already exists on the machine and **changes no settings** — far more useful in an issue than a screenshot.

**Come back and know what happened today**: Settings → History opens with a "Today at a glance" card — total recorded time, the state that took the most, time actually running, time waiting on you, approval counts and the longest wait, and how today compares with the daily average of previous days. Copy the whole thing as Markdown. It only adds up existing today / 7-day / approval data: **there is no event log, so sessions that appeared and vanished are not visible here**; idle and offline time is included, and approvals come from today's entries within the most recent 100 records. Under-claiming beats making things up.

## Installing

Grab a build from [Releases](https://github.com/iFishin/SignalHub/releases):

- **macOS** — the `.dmg`; drag SignalHub into Applications. Pick `aarch64` on Apple Silicon, `x64` on Intel.
- **Windows** — the `-setup.exe` (NSIS installer) or the `_x64_portable.zip` (unzip and run, no install).
- **Linux (beta)** — the `.AppImage` or the `.deb`. Transparency and always-on-top may not work under Wayland; if so, start it with `GDK_BACKEND=x11`. Without FUSE, use `--appimage-extract-and-run`.

**The builds are unsigned**, so the first launch is blocked once. On macOS, run this in a terminal:

```bash
xattr -dr com.apple.quarantine /Applications/SignalHub.app
```

That clears the quarantine flag recursively for the whole bundle — both the app and the `signalhub` command bundled inside it. (Right-clicking the icon → Open → Open also works, but may not cover that inner command.) On Windows, click "More info" → "Run anyway" in SmartScreen.

The `signalhub` command **ships inside the app** — no separate install. The one-click setup under Settings → Connect uses it. To use it in a terminal, put it on your PATH:

```bash
ln -sfn /Applications/SignalHub.app/Contents/MacOS/signalhub /usr/local/bin/signalhub
```

**Updating:** macOS has in-app updates since 0.2.4 (Settings → System shows whether there is a newer version; nothing installs until you click it). Linux and Windows are manual downloads for now — the unpacking library Windows needs was pulled upstream, and the Linux path has not been verified on real hardware (the reasoning lives in [.github/release-body.md](.github/release-body.md)).

Pushing a tag starting with `v` builds every platform's packages automatically (two macOS builds, Windows NSIS plus the portable zip, Linux deb + AppImage) — see [.github/workflows/release.yml](.github/workflows/release.yml).

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
signalhub install hooks claude-code      # also: csc / codex / cursor / gemini / goose / workbuddy / opencode / pi / hermes
signalhub install hooks claude-code --dry-run   # see what it would change first
```

It writes hooks into the agent's config file (`~/.claude/settings.json` for Claude Code). It touches **only the `hooks` key**, keeps everything else byte for byte, and backs the file up to `*.signalhub-backup` first. Running it twice does not duplicate entries; if the command changed it updates in place; and if the file is not valid JSON it aborts without writing anything. Undo with `signalhub uninstall hooks claude-code`.

**Goose, opencode, Pi and Hermes are the exception**: they use a plugin mechanism, so SignalHub installs a directory or file of its own (Goose: `~/.agents/plugins/signalhub/`, Pi: `~/.pi/agent/extensions/signalhub.ts`) and never touches your shared config — uninstalling removes the whole thing. The others insert an entry into a shared config file.

Restart the agent afterwards. Then: prompt → fast green, tool call → slow green, permission → red, answer → a pulse and back to steady. Per-agent differences (which ones have no "needs permission" event, whose payload field names differ) are in [docs/api.md §4](docs/api.md).

**An agent that is not on the list does not have to wait for a release.** Most of the long tail are Claude Code clones — same event names, same fields, same entry shape, only the config path and the name differ. For those, write an entry in `~/.signalhub/agents.json`: say which family it mirrors (`family`), where its config lives (`config`) and what it is called (`id` / `name`). It then shows up in the Connect page next to the ten built-ins, and install / uninstall / detection all work the same way — the CLI accepts it too (`signalhub install hooks my-agent`). If the event names genuinely differ, that still needs code; `signalhub run` is the stopgap there. Field table and example: [docs/api.md §4.18](docs/api.md).

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

Done: the floating lamp, state aggregation and TTL, the HTTP reporting API, the CLI (including `watch`), the settings panel, automatic hook installation (Claude Code / Codex / Cursor / Gemini CLI / Goose / opencode / Pi / Hermes), the connect self-check (the Connect page checks whether the command it installed still runs — that kind of failure is silent), the hook call log (the tail is right there on the System page), `signalhub run` (wraps any command and reports for it), the "what it's doing" line on the sign (generated from tool events, so you do not configure a sentence per agent), CLI text that follows the UI language (`--help`, errors and command output all have Chinese and English), system notifications with sound and action buttons, window position memory, the expanded/collapsed shapes, per-group right-click menus, today's duration stats, a 30-minute timeline and a 7-day history page, outbound hooks, per-state colour / shape / rhythm, per-group mute, quiet hours, a persistence threshold before alerting, repeat reminders while something stays stuck (again at 5 / 15 / 30 minutes, saying how long it has been waiting), a tray icon that follows the state, single-instance, launch at login, global shortcuts (open the inbox, show/hide the lamp, mute the worst group, clear the worst group, click-through — one that could not be registered is flagged in settings), an inbox with one row per session (needs you / running / just finished / idle, with allow / deny / jump / mute / clear), and jumping to the exact terminal tab (Terminal / iTerm2 / tmux; macOS only, with app activation and project-folder fallbacks), manual states (focus 25 minutes / in a meeting / away — you are a signal source too), grouping (an explicit `group` or `--group`, or an automatic fallback per working directory or per session — one sign per session), per-day stats persisted to disk (kept for 30 days), configuration export / import, an opt-in LAN access switch (off by default), click-through mode, source kinds on the Connect page (AI agents / system / custom, declared with `--source`), a read-only web panel (open a link on your phone to watch the lamp; off by default), physical lights (WLED — the state colour is POSTed to the light, colour only, nothing configured by default), approve from the lamp (when Claude Code asks you to approve a tool call, the confirmation box moves onto the lamp — off by default, and a command you cannot read in full cannot be allowed), and presets (push a stuck approval to your phone after 60 seconds / hold notifications while an agent is working / blue lamp and silence during a meeting — off by default, presets only, no expression engine), a demo mode (Settings → Demo plays a 30-second scripted walkthrough on the real lamp without touching real state; a first-run lamp offers a "See demo" button; `?window=demo` is a standalone browser page), settings tab search / section links / last-tab memory, keyboard focus rings and page transitions, a one-click test notification (Alerts, uses the real alert outlet and reports permission), a system check (System, a dozen checks with a copyable report), and a daily digest card (History, pure-function summary of today / 7-day / approval data, copyable).

To do: code signing and notarization (the steps have a place reserved in [.github/workflows/release.yml](.github/workflows/release.yml) — fill them in once you have certificates).

**Auto-update, today:** macOS has it (since 0.2.4, manual trigger — see the Install section above); Linux and Windows do not — the unpacking library Windows needs was pulled upstream, and the Linux path has not been verified on real hardware. The platform gate and the reasoning live in the module header of [src-tauri/src/update.rs](src-tauri/src/update.rs).

## License

MIT — see [LICENSE](LICENSE).
