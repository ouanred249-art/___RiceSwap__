# Codebook recipe schema — draft (caelestia + ii extraction)

> Wayfinder ticket: [Research #24](https://github.com/ouanred249-art/___RiceSwap/issues/24) — map [#22](https://github.com/ouanred249-art/___RiceSwap/issues/22).
> Feeds: engine design #27, `[adapt]` schema design #28.
> Method: diffs of hand-fixed caelestia configs vs their backups, QML shortcut registration greps, `hyprctl globalshortcuts` live read, `caelestia` python package + CLI help, both profiles' hypr configs. READ ONLY; nothing executed that mutates a live session.

## 0. Evidence base (all absolute paths)

- Ground truth diff: `~/.local/share/riceswap/profiles/caelestia/backups/keybinds-2026-09-26/` (ORIGINALS: `keybinds.lua`, `custom-keybinds.lua`, `general.lua`, `hypridle.conf`) vs rewritten `~/.local/share/riceswap/profiles/caelestia/.config/hypr/hyprland/keybinds.lua`, `custom/keybinds.lua`, `custom/env.lua`, `hypridle.conf`.
- Donor reference rice (ii): `~/.local/share/riceswap/profiles/ii/.config/hypr/hyprland/keybinds.lua` (identical to the caelestia backup — the backup *is* ii's file), `~/.local/share/riceswap/profiles/ii/.config/quickshell/ii/` (71 `GlobalShortcut` blocks, no explicit `appid` → defaults to `quickshell`).
- Target registrations: `~/.local/share/riceswap/profiles/caelestia/.config/quickshell/caelestia/components/misc/CustomShortcut.qml` (`GlobalShortcut { appid: "caelestia" }`), 22 `CustomShortcut` uses across `modules/Shortcuts.qml`, `modules/areapicker/AreaPicker.qml`, `modules/lock/Lock.qml`, `services/{Brightness,Hypr,Notifs,Players}.qml`.
- Live confirmation: `hyprctl globalshortcuts` currently prints exactly `caelestia:<name>` ×22 (plus `quickshell:riceswap-toggle` from the side shell `qs -c riceswap`, which BOTH profiles launch: `.config/hypr/hyprland/execs.lua:7`).
- Path/env constants: `/usr/lib/python3.14/site-packages/caelestia/utils/paths.py` (`wallpapers_dir = Path(os.getenv("CAELESTIA_WALLPAPERS_DIR", pictures_dir / "Wallpapers"))`, `wallpapers_dir:36`, line 36/42/45), `.../caelestia/utils/Paths.qml:22` (`wallsdir: Quickshell.env("CAELESTIA_WALLPAPERS_DIR") || absolutePath(GlobalConfig.paths.wallpaperDir)`).
- CLI surface: `caelestia --help` → subcommands shell, toggle, scheme, screenshot, record, clipboard, emoji, wallpaper, resizer, install, update. `caelestia shell -h` → `-d/--daemon -r/--restart -k/--kill -s -l`. `caelestia scheme get -h` → `-n -f -m -v`. Versions: caelestia-shell 2.5.0, caelestia-cli 1.1.3, Quickshell 0.2.1-git.

## 1. Fact-class inventory

Every fact the manual caelestia adaptation consumed, as classes FC-1..FC-9.

### FC-1 Dispatcher appid / namespace
The `global` dispatcher token is `<appid>:<name>`. caelestia registers under appid `caelestia` (`CustomShortcut.qml:6`); ii omits `appid` → Quickshell default `quickshell`. The rewrite `quickshell:*` → `caelestia:*` is the mechanical half of the adaptation; layer namespaces also differ (`caelestia-${name}` per `StyledWindow.qml:10` vs `quickshell:*` per ii's `rules.lua:132-150`) but keybinds don't touch those.
**Status: fully runtime-derivable** — `hyprctl globalshortcuts` returns the prefixed names of every live shell.

### FC-2 Registered-shortcut inventory (target side)
22 names confirmed live and statically: launcher, launcherInterrupt, nexus, showall, dashboard, session, sidebar, utilities, lock, unlock, screenshot, screenshotClip, screenshotFreeze, screenshotFreezeClip, brightnessUp, brightnessDown, mediaToggle, mediaNext, mediaPrev, mediaStop, clearNotifs, refreshDevices.
**Status: fully runtime-derivable** (live probe = ground truth; QML grep = pre-boot derivation). Descriptions come free with `hyprctl globalshortcuts`.

### FC-3 Intent mapping (donor name → target capability)
The semantic join — the heart of the codebook. Observed rewrite record (backup line → current line):

| donor bind (quickshell:*) | intent | caelestia resolution | evidence |
|---|---|---|---|
| searchToggleRelease | launcher | `shortcut caelestia:launcher` + `release = true` (caelestia has no separate release-variant; the `release` bind option replaces the donor's two-name trick) | keybinds.lua:11-12 → rewritten 12-13 |
| overviewWorkspacesToggle | overview/all-panels | `caelestia:showall` | :22 |
| sidebarLeftToggle (+B,+O) / sidebarRightToggle | sidebar / utilities | `caelestia:sidebar` / `caelestia:utilities` (donor's left/right split collapses to one sidebar; right sidebar re-pointed to utilities) | :25-29 |
| sessionToggle | session menu | `caelestia:session` | :35 |
| regionScreenshot | snip→clipboard | `caelestia:screenshotClip` (also custom/keybinds.lua:4 CTRL+K) | :68 |
| brightness via `qs ipc call brightness increment` exec | brightness | `caelestia:brightnessUp/Down` shortcuts | :40-43 |
| lock (hypridle) | lock | `caelestia:lock` | hypridle.conf:1 |
| cheatsheetToggle, oskToggle, workspaceNumber, barToggle, overviewClipboardToggle, overviewEmojiToggle, mediaControlsToggle, overlayToggle, panelFamilyCycle, wallpaperSelectorToggle, regionOcr, screenTranslate, regionRecord, sidebarLeftToggleDetach, regionSearch | see FC-4 | — | — |

**Status: irreducible recipe content.** Name similarity (regionScreenshot vs screenshotClip) is not safely automatable; the mapping must be authored once and cached (write-back, #22).

### FC-4 Fallback when no counterpart exists
Three distinct resolutions were chosen by hand — this is a per-rice policy list, not one rule:
- **drop**: `workspaceNumber` (SUPER_L tap-to-put-workspace), `barToggle`, `cheatsheetToggle`, `oskToggle`, `overlayToggle`, `mediaControlsToggle`, `sidebarLeftToggleDetach`, `panelFamilyCycle`, `wallpaperSelectorToggle` (the UI selectors) — deleted outright.
- **exec-fallback to donor CLI**: `overviewClipboardToggle` → `pkill fuzzel || caelestia clipboard`; `overviewEmojiToggle` → `pkill fuzzel || caelestia emoji -p`; `regionRecord` → `caelestia record -r`; `wallpaperSelectorRandom` → `caelestia wallpaper -r`; `toggleLightDark` → `caelestia scheme get | grep -q "Mode: dark" && caelestia scheme set -m light || caelestia scheme set -m dark`.
- **exec-fallback to external tools** (donor scripts survive when the target has no equivalent): `regionSearch` → `pidof slurp || .../snip_to_search.sh`; `regionOcr` → the grim+slurp+tesseract pipeline. Note the adaptation *removed the `qsIsAlive ||` guard prefix* from these — the guard is a donor-shell IPC fact (`qs -c $qsConfig ipc call TEST_ALIVE`), meaningless under caelestia.
- **kept as-is** (shell-agnostic): all window/workspace/media/virtual-machine/session binds below the shell block — the adapter must only rewrite lines that dispatch `global <other-ns>:*`.
- **cross-shell side channel**: `quickshell:riceswap-toggle` (SUPER+R, from `~/.local/share/riceswap/profiles/ii/.config/quickshell/riceswap/shell.qml:83`) is launched by BOTH profiles (`execs.lua:7`) and must be left untouched — the engine must track that several live appids can coexist, and only the *primary* shell's namespace is being reconciled.

**Status: irreducible recipe content** (which fallback, and for which intent), **derivable trigger** (FC-2 inventory tells you there is no counterpart).

### FC-5 Path / env facts + launcher-vs-CLI divergence
- Env var name: `CAELESTIA_WALLPAPERS_DIR` (both `paths.py:36` and `Paths.qml:22` read it first).
- Divergence: launcher/UI resolves `env || GlobalConfig.paths.wallpaperDir` (QML-config default); CLI resolves `env || $XDG_PICTURES_DIR/Wallpapers` (python default). Without the env var set, the two can disagree; setting it forces both to one dir. `custom/env.lua:5`: `hl.env("CAELESTIA_WALLPAPERS_DIR", HOME .. "/Wallpapers")` — the *value* encodes this machine's collection location (`~/Wallpapers`, neither default).
- Sibling vars exist (`CAELESTIA_SCREENSHOTS_DIR`, `CAELESTIA_RECORDINGS_DIR`, `CAELESTIA_LIB_DIR` — `paths.py:42,45`, `Paths.qml:23-24`) but adaptation used only the wallpapers one.
**Status: var names derivable** (grep `Quickshell.env(`/`getenv(` in target sources); **values irreducible** (user preference); the divergence fact itself is research-once, cacheable in the recipe.

### FC-6 Shell lifecycle commands
Donor (ii): `qs -c ii` (execs), restart via `killall ydotool qs quickshell; qs -c $qsConfig &`. Target: `caelestia shell -d` / `-k` / `-r`; current configs use `exec-once`-style `qs -c caelestia` (`hyprland/execs.lua:6` still works because caelestia ships a quickshell config named `caelestia`) BUT keybind reload uses `caelestia shell -k ; caelestia shell -d` (SHIFT+SUPER+ALT+Slash) and restart `killall ydotool qs quickshell; caelestia shell -d & disown`. `qsConfig` env var is set to `caelestia` in `hyprland/variables.lua:5` (donor: `ii`) — every `$qsConfig`-interpolated IPC string (`qs -c $qsConfig ipc call …`) is donor-only plumbing (FC-4 note).
**Status: derivable at install** (`caelestia shell -h`, `qs --help`) but recipe pins it for determinism.

### FC-7 Lock dispatch form (host quirk)
This Hyprland build **evaluates `dispatch` arguments as Lua**, so `hyprctl dispatch global caelestia:lock` is wrong and `hyprctl dispatch 'hl.dsp.global("caelestia:lock")'` (the Lua-call string form) is what the working config uses (`hypridle.conf:1`, current vs backup diff). Consequence for hypridle: `lock_cmd` uses the Lua form; `after_sleep_cmd` became `caelestia shell -d; hyprctl dispatch '<lua lock form>'` (upstream's "wake the detached shell, then lock" — backup used donor `quickshell:lockFocus`, which caelestia does not register).
**Status: probeable** on the live host (dispatch round-trip test), **pinned per-recipe/host**; the hypridle integration template itself is irreducible composite content.

### FC-8 Scheme/color plumbing
`caelestia scheme get|set -m` (mode dark/light) replaces donor `toggleLightDark`; scheme files at `~/.local/state/caelestia/scheme.json` (`paths.py:32`). Used inside an exec-fallback string (FC-4).
**Status: derivable at install** (CLI help), part of the recipe's capability table.

### FC-9 Registered-name semantics that don't fit the table
`launcherInterrupt` exists for the key-repeat-cancel pattern the donor encoded with `searchToggleRelease`/`…Interrupt` naming; `lock`/`unlock` pair; `screenshotFreeze*` variants. A capability vocabulary must be rich enough that these map cleanly (e.g. `launcher.toggle`, `launcher.interrupt`, `lock.session`).

## 2. Proposed schema (TOML)

Design principle from FC-1..FC-8: a recipe describes **one rice**, not one migration. The engine (#27) reconciles *pairwise* by joining donor-capability → target-capability. So the recipe is a capability table keyed by a stable intent vocabulary; `shortcut` / `exec` / absent is the resolution trilemma from FC-4.

```toml
# adapt-recipe.toml — one per rice; lives inside the profile (profile.toml extension or sibling)
schema_version = 1
rice = "caelestia"

[shell]
quickshell_config = "caelestia"        # derivable; qs -c <name>
appid = "caelestia"                    # derivable: prefix in `hyprctl globalshortcuts`
launch  = "qs -c caelestia"
restart = "killall ydotool qs quickshell; caelestia shell -d & disown"
kill    = "caelestia shell -k"

[quirks]
dispatch_arg_is_lua = true             # probeable; pins the hl.dsp.global("…") string form
ipc_guard = 'qs -c {config} ipc call TEST_ALIVE || '   # donor-side template; empty for non-qs IPC

[env]
CAELESTIA_WALLPAPERS_DIR = "~/Wallpapers"   # value irreducible; name derivable

[[capability]]                         # the codebook; ids are the join key for #27
id = "launcher.toggle"
shortcut = "launcher"                  # un-prefixed; engine prefixes with shell.appid
bind_options = { release = true }      # rewrite directive for donor two-name patterns
[[capability]]
id = "screenshot.region.clipboard"
shortcut = "screenshotClip"
[[capability]]
id = "clipboard.history"
exec = "pkill fuzzel || caelestia clipboard"          # exec-fallback form
[[capability]]
id = "screenshot.region.translate"                     # absent entirely → "drop"
# (no entry, or: resolution = "drop")
[[capability]]
id = "session.lock"
exec = '''hyprctl dispatch 'hl.dsp.global("caelestia:lock")' & pidof qs quickshell hyprlock || hyprlock'''

[hypridle]                             # integration template consuming capabilities
lock_cmd = "{capability:session.lock.exec} & pidof qs quickshell hyprlock || hyprlock"
before_sleep_cmd = "loginctl lock-session"
after_sleep_cmd = "caelestia shell -d >/dev/null 2>&1; hyprctl dispatch 'hl.dsp.global(\"caelestia:lock\")'"

[foreign_namespaces]                   # FC-4: coexisting appids never to rewrite
"quickshell:riceswap-toggle" = "keep"  # side shell launched from execs.lua
```

Migration algorithm (for #27): parse donor binds → for each `global <ns>:<name>`, look up donor recipe capability by name → look up target recipe capability by `id` → emit shortcut (with `shell.appid` prefix + `bind_options`), `exec`, or drop; strip `ipc_guard` prefixes of the leaving shell; leave non-`global` binds untouched.

## 3. Filled example — ii (donor, reference rice)

```toml
schema_version = 1
rice = "ii"

[shell]
quickshell_config = "ii"
appid = "quickshell"                   # default; no appid: in its 71 GlobalShortcut blocks
launch  = "qs -c ii"
restart = "killall ydotool qs quickshell; qs -c ii &"

[quirks]
ipc_guard = 'qs -c ii ipc call TEST_ALIVE || '   # donor uses qs IPC + fuzzel fallback chains

[[capability]] id = "launcher.toggle"            shortcut = "searchToggleRelease"
[[capability]] id = "launcher.toggle.release"    shortcut = "searchToggleRelease"       # separate name, no bind option
[[capability]] id = "launcher.interrupt"         shortcut = "searchToggleReleaseInterrupt"
[[capability]] id = "overview.workspaces"        shortcut = "overviewWorkspacesToggle"
[[capability]] id = "clipboard.history"          shortcut = "overviewClipboardToggle"
[[capability]] id = "emoji.picker"               shortcut = "overviewEmojiToggle"
[[capability]] id = "sidebar.left"               shortcut = "sidebarLeftToggle"
[[capability]] id = "sidebar.right"              shortcut = "sidebarRightToggle"
[[capability]] id = "cheatsheet.toggle"          shortcut = "cheatsheetToggle"          # caelestia: none
[[capability]] id = "keyboard.onscreen"          shortcut = "oskToggle"                 # caelestia: none
[[capability]] id = "media.controls"             shortcut = "mediaControlsToggle"
[[capability]] id = "overlay.widgets"            shortcut = "overlayToggle"
[[capability]] id = "session.lock"               shortcut = "lock"
[[capability]] id = "session.lock.focus"         shortcut = "lockFocus"                 # hypridle after_sleep; caelestia: none
[[capability]] id = "screenshot.region.clipboard" shortcut = "regionScreenshot"
[[capability]] id = "screenshot.region.search"   shortcut = "regionSearch"
[[capability]] id = "screenshot.region.ocr"      shortcut = "regionOcr"
[[capability]] id = "screenshot.screen.translate" shortcut = "screenTranslate"          # caelestia: none
[[capability]] id = "record.region"              shortcut = "regionRecord"
[[capability]] id = "record.region.sound"        shortcut = "regionRecordWithSound"
[[capability]] id = "wallpaper.selector"         shortcut = "wallpaperSelectorToggle"
[[capability]] id = "wallpaper.random"           shortcut = "wallpaperSelectorRandom"
[[capability]] id = "theme.mode.toggle"          shortcut = "toggleLightDark"
# … plus barToggle, panelFamilyCycle, workspaceNumber, sessionToggle (same table shape)
```

ii needs no `[env]` (its wallpaper path is donor-internal to the QML config; the divergence bug only bites caelestia), and no `[hypridle]` overrides beyond stock — the current ii `hypridle.conf` still carries the donor `quickshell:lock` form, correct for ii.

## 4. Derivable vs irreducible — the #27/#28 split

| Fact | Engine-derivable? | How / why not |
|---|---|---|
| target appid prefix (FC-1) | **yes, at switch time** | `hyprctl globalshortcuts` name prefixes; multiple appids visible at once |
| registered shortcut inventory + descriptions (FC-2) | **yes, at switch time** | same live probe; static grep of `GlobalShortcut`/`CustomShortcut` QML pre-boot |
| donor bind names in use | **yes** | parse profile `*.lua` for `hl.dsp.global(` |
| layer namespaces | **yes** | QML `WlrLayershell.namespace`; not needed for keybinds |
| env var NAMES (FC-5) | **yes, at install** | grep `Quickshell.env(` + `os.getenv(` in target sources |
| lifecycle commands (FC-6) | **yes, at install** | `caelestia shell -h` / `qs --help` CLI introspection; recipe pins anyway |
| scheme CLI verbs (FC-8) | **yes, at install** | `caelestia scheme -h` |
| `dispatch_arg_is_lua` (FC-7) | **probeable, host-level** | live dispatch round-trip; belongs to host facts, not rice facts, but recipe may pin |
| which intents have NO counterpart (FC-4 trigger) | **yes** | inventory join: donor name has no target match |
| **intent vocabulary + donor↔target name mapping (FC-3)** | **NO** | semantic (regionScreenshot→screenshotClip); fuzzy name matching unsafe |
| **resolution per unmatched intent: drop / shell-CLI exec / external tool exec (FC-4)** | **NO** | taste + CLI knowledge (`caelestia clipboard` vs fuzzel chain vs slurp+tesseract script) |
| **env var VALUES, e.g. wallpaper dir (FC-5)** | **NO** | user/machine preference |
| **launcher-vs-CLI path divergence fact (FC-5)** | research-once | derivable by reading both code paths, expensive; cache in recipe (write-back) |
| **hypridle integration template (FC-7)** | **NO** | composite of quirk form + wake command + fallback chain |
| foreign namespaces to keep (FC-4, `riceswap-toggle`) | partly | engine sees live registrations, but *why* one is foreign (side shell) is recipe/host knowledge |

**Bottom line for #27/#28:** the engine can always answer "what exists" (namespaces, registrations, CLI shapes — probe it, never trust stale copies). The recipe must always answer "what means what and what to do about it" (capability vocabulary, intent mapping, fallback policy, env values, integration templates). Write-back (#22) then caches derived facts alongside authored ones so re-adoption is O(parse), and the engine re-validates the derivable subset at switch time and *fails loudly* if a cached derivable fact no longer matches live — that validation gap is the schema's `[quirks]`-vs-probe boundary.
