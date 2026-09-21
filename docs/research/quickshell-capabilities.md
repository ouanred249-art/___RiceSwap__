# Quickshell capabilities and constraints — findings for RiceSwap

> Wayfinder ticket: [Survey Quick Shell capabilities and constraints](https://github.com/ouanred249-art/dots-swetcher/issues/3)
> All claims verified against quickshell.org docs (v0.3.1) and production shell repos.

Quickshell is a Qt6/QML-based desktop shell toolkit. Source: <https://github.com/outfoxxed/quickshell>. Docs: <https://quickshell.org/docs/> — the site versions docs (v0.1.0 through v0.3.1); pin `https://quickshell.org/docs/v0.3.1/...` for current types.

## 1. Slide-in overlay panel via Hyprland keybind — yes, first-class

- **`PanelWindow`** (<https://quickshell.org/docs/types/Quickshell/PanelWindow/>): decoration-less window anchored to screen edges (`anchors { left/top/right/bottom }`), `margins`, `exclusiveZone`, `exclusionMode: ExclusionMode.Ignore` (reserves no space, pushes no tiled windows), `aboveWindows`, `focusable`. On Wayland maps to wlr-layer-shell: `WlrLayershell.layer` (Top/Overlay), `.namespace`, `.keyboardFocus` (None/OnDemand/Exclusive).
- Two keybind mechanisms, both production-proven:
  - **`GlobalShortcut`** (`Quickshell.Hyprland`, <https://quickshell.org/docs/types/Quickshell.Hyprland/GlobalShortcut/>): uses Hyprland's `hyprland_global_shortcuts_v1` protocol, no portal needed. Declare `GlobalShortcut { name: "toggle"; onPressed: { open = !open } }`; bind with `bind = $mod, K, global, quickshell:toggle` (appid defaults to `quickshell`).
  - **`IpcHandler`** + `bind = $mod, K, exec, qs ipc call panel toggle`. end-4's dots use both.
- Animation then drives x/y/margins/opacity on the panel content. Exact reference implementations: end-4's sidebar (`exclusiveZone: 0`, anchored right, toggled visibility) and full-screen overlay (`dots/.config/quickshell/ii/modules/ii/overlay/Overlay.qml`).

## 2. Talking to the Rust CLI backend — multiple documented routes (`Quickshell.Io`)

- **`IpcHandler`** (<https://quickshell.org/docs/types/Quickshell.Io/IpcHandler/>): built-in CLI IPC. `IpcHandler { target: "rect"; function setColor(color: color): void {...} }` called via `qs ipc call rect setColor orange`. Typed args/returns (string/int/bool/real/color, max 10 args); `qs ipc show` lists targets; `qs ipc prop get` reads properties. `IpcSignal` emits shell→external events.
- **`Process`** (<https://quickshell.org/docs/types/Quickshell.Io/Process/>): spawn the Rust CLI. `command: ["riceswap", "list", "--json"]`, `stdout` with `SplitParser`/`StdioCollector`, `stdinEnabled`, `exited(exitCode)` signal, `environment` override, `startDetached()`. Natural fit for "CLI returns JSON, QML parses with `JSON.parse`".
- **`Socket` / `SocketServer`** (<https://quickshell.org/docs/types/Quickshell.Io/Socket/>): Unix domain sockets for a persistent Rust daemon streaming state.
- **`FileView`** (<https://quickshell.org/docs/types/Quickshell.Io/FileView/>): read/write small files with `watchChanges` + `onFileChanged: reload()`, `atomicWrites`, and a `JsonAdapter` for direct JSON mapping. Ideal for a Rust-written state file the shell reactively watches.
- No standalone D-Bus module; D-Bus access is via `Quickshell.Services.*` (Mpris, Notifications, SystemTray, …). For a custom protocol: sockets or `Process`.

## 3. Animation primitives — full QtQuick, plus a Quickshell extra

Standard QtQuick: `PropertyAnimation`, `NumberAnimation`, `ColorAnimation`, `Behavior on <prop>`, `states`/`transitions`, `SpringAnimation`, `SmoothedAnimation`, `SequentialAnimation`/`ParallelAnimation`, `Animation { easing { type: Easing.OutCubic } }`, opacity/scale/x/y tweening (<https://doc.qt.io/qt-6/qtquick-qmlmodule.html>).

Quickshell adds **`EasingCurve`** (<https://quickshell.org/docs/types/Quickshell/EasingCurve/>): a reusable non-Item easing object with `interpolate(x, a, b)` and `valueAt(x)` for hand-driven animation.

Production patterns: caelestia's `components/Anim.qml` (NumberAnimation subclass with duration/easing presets); end-4 slides content x/offset inside a fixed PanelWindow while toggling visibility.

## 4. Existing configs to crib from — yes, two major ones

- **end-4/dots-hyprland** (16.2k stars): QML at `dots/.config/quickshell/ii/` — full-screen `Overlay.qml` with `HyprlandFocusGrab` + click-through `mask: Region`, slide-in sidebars, overview, wallpaper picker. Closest reference for a profile-card panel.
- **caelestia-dots/shell** (12.5k stars): cleanest architecture — `StyledWindow.qml` wraps PanelWindow, `CustomShortcut.qml` wraps GlobalShortcut, `Anim.qml` animation system, `CachingImage.qml`/`FadeImage.qml`. Its dashboard ("nexus") is a profile-card-like modal.
- Official examples: <https://quickshell.org/docs/examples>. Hyprland wiki covers Quickshell bars: <https://wiki.hyprland.org/Useful-Utilities/Status-Bars/>.

## 5. Deployment under Hyprland

- **Arch: `quickshell` is in the official repos** (`pacman -S quickshell`); git builds via AUR `quickshell-git`. Also packaged for Fedora, Debian, openSUSE, Ubuntu, Gentoo, Guix, Nix.
- Config locations (<https://quickshell.org/docs/v0.3.1/guide/distribution/>): `~/.config/quickshell/<name>/`; package-distributed: `/etc/xdg/quickshell/<name>`. Launch: `qs -c <name>`.
- Autostart: `exec-once = qs -c <name>`. Hot-reload built in (QML reloads on save; `Reloadable`/`PersistentProperties` for state across reloads).

## 6. Image display — yes, standard QtQuick

`Image { source: "file:///path/to/shot.png" }` works as normal (end-4's `WallpaperSelector.qml`/`Background.qml`, caelestia's `CachingImage.qml`/`FadeImage.qml` — async loading, `fillMode: Image.PreserveAspectCrop`). Plus `IconImage` (`Quickshell.Widgets`) for themed icons and **`ScreencopyView`** (`Quickshell.Wayland`) for live screen capture as an alternative to static screenshots.

## Bottom line for RiceSwap

All six GUI requirements check out. Recommended architecture, matching what caelestia/end-4 already prove:

1. One daemon: `exec-once = qs -c riceswap`
2. `PanelWindow` (layer Top, `exclusionMode: Ignore`, transparent) with a Loader-gated profile-card UI
3. `GlobalShortcut { name: "toggle" }` bound as `bind = $mod, R, global, quickshell:toggle` — or `IpcHandler` + `exec = qs ipc call riceswap toggle`
4. QtQuick `NumberAnimation`/`Behavior` with `Easing` curves for the slide
5. Rust ↔ shell bridge: `Process` for one-shot queries returning JSON, plus `FileView` with `watchChanges` on a state file (or `Socket` for a streaming daemon)
