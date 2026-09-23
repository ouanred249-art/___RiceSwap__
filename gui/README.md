# RiceSwap GUI (Quickshell)

The panel users live in: one right-edge slide-in `PanelWindow` (~420px,
layer Top, `exclusionMode: Ignore`) with stacked view navigation, wired to
the real `init` / `list` / `info` / `delete` operations and the watched
`state.json`. Ticket #17; snapshot flow (#18), switch flow (#19), and
packaging (#20) land later.

## Where these files go

This directory mirrors the packaged Quickshell config directory exactly:

| In this repo      | Packaged at                        | After first-run `init`                    |
| ----------------- | ---------------------------------- | ----------------------------------------- |
| `gui/*`           | `/etc/xdg/quickshell/riceswap/*`   | copied to `~/.config/quickshell/riceswap/` |

Packaging (installing into `/etc/xdg/...`) is ticket #20's job; `init`
already copies that packaged directory into the user's config so users can
customize their own copy.

## Running it

Development (symlink so hot-reload picks up edits):

```sh
ln -s /path/to/dots-chnger/gui ~/.config/quickshell/riceswap
qs -c riceswap
```

Requirements: the `riceswap` binary on `PATH` (the panel spawns one
process per operation), `grim` (screenshots), and Hyprland.

## Autostart (Hyprland)

```ini
exec-once = qs -c riceswap
```

## Keybind (Super+R)

```ini
bind = $mainMod, R, global, quickshell:riceswap-toggle
```

Fallback / scripting path (no bind needed):

```sh
qs ipc call riceswap toggle
```

Escape closes a pushed view, or the panel itself at the root view.

## Theming

The panel repaints into the active rice's colors: with a profile active,
it reads pywal's palette at `~/.cache/wal/colors.json`
(`{special:{background,foreground}, colors:{color0..15}}`, watched live
via `FileView` + `watchChanges`). With no active profile — or no parsable
palette — it falls back to bundled neutral-dark constants in `Theme.qml`.

Judgment calls (also reported on the ticket):

- **Palette source: pywal, not matugen.** matugen has no canonical
  stable-path colors file (its output is template-defined), so it cannot
  be a contract the panel watches. pywal's `colors.json` location is
  documented and stable. A matugen-based rice that also writes pywal's
  file themes the panel too.
- **The neutral-dark fallback values are placeholders.** The spec calls
  the fallback palette "an asset to source during implementation".
- **Refresh screenshot spawns `grim` directly from QML.** The ten-op
  surface is frozen and has no capture operation, so the card menu runs
  `grim <profileDir>/<screenshot>` itself (the exact command `snapshot`
  uses) and cache-busts the `Image` URL with a revision counter. No
  backend delta, no 11th operation.

## Layout

- `shell.qml` — entry point: ShellRoot, panel window, toggle
  (`GlobalShortcut` + `IpcHandler`), view stack (push/pop), Escape.
- `Backend.qml` — watched `state.json` + one `Process` per operation
  (NDJSON progress lines → final `{ok, warnings, data}` envelope), plus
  the direct `grim` spawn.
- `Theme.qml` — palette gate (active profile ∧ parseable palette) and
  the neutral-dark fallback.
- `views/PanelView.qml` — base for stacked views: shell/theme context,
  slide/fade transition, inert-when-inactive.
- `views/OnboardingView.qml` — what setup does + Set Up running the real
  `init` with live progress; lands on Profiles when `state.json` flips.
- `views/ProfilesView.qml` — card list from real `list`, ⋯ menu overlay,
  in-panel delete confirmation running real `delete`, empty state.
- `views/ProfileCard.qml` — 16:9 thumbnail (grayscale → colour on hover,
  full colour when active), rice_info line, package chip, Active badge
  driven by `state.json`.
- `views/InfoView.qml` — read-only manifest from real `info`.
- `views/ComingSoonView.qml` — navigation stub for the switch (#19) and
  snapshot (#18) flows.
