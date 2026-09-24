<p align="center">
  <img src="assets/wallpapers/default-dusk.png" width="340" alt="RiceSwap wallpaper — dusk">
</p>

<h1 align="center">RiceSwap</h1>

<p align="center">
  <img src="https://img.shields.io/badge/rice-swap-Hyprland-F4538A?style=for-the-badge&labelColor=2E294E">
  <img src="https://img.shields.io/badge/backend-Rust-E9D48D?style=for-the-badge&labelColor=2E294E">
  <img src="https://img.shields.io/badge/gui-Quickshell-569CD6?style=for-the-badge&labelColor=2E294E">
  <img src="https://img.shields.io/badge/license-MIT-74A9C6?style=for-the-badge&labelColor=2E294E">
</p>

<p align="center">
  A profile-based rice switcher for <b>Hyprland</b>. Store entirely different
  desktops — bar, terminal, packages, services, wallpapers — in isolated
  profiles and flip between them with one button. No terminal required.
</p>

<p align="center">
  <b>
    <a href="#install">Install</a> ·
    <a href="#quick-start">Quick start</a> ·
    <a href="#how-it-works">How it works</a> ·
    <a href="#the-operations">The operations</a> ·
    <a href="#develop-from-source">Develop</a>
  </b>
</p>

---

## ✨ What it does

| | |
|---|---|
| 📸 **Snapshot** your current rice | Auto-detects config dirs, packages, fonts & wallpapers, captures a screenshot, and freezes it all into an isolated profile |
| 🔄 **Switch** to another rice in one click | Sees exactly what will change (diff-first), then removes old packages, installs new ones, re-links configs, stops old services, starts new ones, and reloads Hyprland |
| 🧊 **Keep hardware safe** | Monitors, input, and GPU settings are lifted into a shared layer once — switching rices never re-detects your display |
| 🖼️ **Wallpapers that survive** | A shared wallpaper layer with 4 bundled defaults; wallpapers you download are imported into it, so any profile looks put-together |
| 🧯 **Recover with one click** | No auto-rollback magic: if a switch fails, the panel tells you exactly which profile to switch back to and does it for you |
| 🎨 **Fits your desktop** | The panel themes itself from your active rice's palette (pywal), so it never looks bolted on |

The whole experience lives in a Quickshell panel — **Super+R** — and needs zero
terminal usage after install.

---

## 🏞️ Bundled wallpapers

Every rice gets these four, so a profile is never left with a broken desktop:

| 🌅 Dawn | ☀️ Day | 🌆 Dusk | 🌙 Night |
|:---:|:---:|:---:|:---:|
| ![dawn](assets/wallpapers/default-dawn.png) | ![day](assets/wallpapers/default-day.png) | ![dusk](assets/wallpapers/default-dusk.png) | ![night](assets/wallpapers/default-night.png) |

---

## 📦 Install

**No AUR, no yay** — one script, straight from the repo:

```sh
git clone https://github.com/ouanred249-art/___RiceSwap__
cd ___RiceSwap__
./install.sh        # builds the backend, installs binary + GUI + wallpapers,
                    # and adds the two keybind lines to your Hyprland config
```

`./install.sh` works from any checkout, or from anywhere (it clones main into
a temp dir if you're not in one). Options: `--prefix <dir>` (default
`/usr/local`), `--no-sudo` (installs into `~/local`), `--hyprland <path>`
(point at your config; classic `hyprland.conf` files get the lines written
into a managed block, Lua configs get the lines printed for you).

### From the AUR (alternative)

RiceSwap is distributed through the AUR (Arch-first, x86_64):

| Package | What it is |
|---|---|
| `riceswap-bin` | The quickest path: installs the prebuilt tarball from GitHub releases |
| `riceswap` | Builds from source |
| `riceswap-git` | Tracks the latest commit |

```sh
# with your AUR helper of choice (yay or paru)
yay -S riceswap-bin
# or
paru -S riceswap
```

The package installs four things and **nothing else**:

| Artifact | Goes to |
|---|---|
| the `riceswap` binary | `/usr/bin` |
| the Quickshell GUI | `/etc/xdg/quickshell/riceswap/` (copied to your `~/.config` on first run) |
| the 4 bundled wallpapers | `/usr/share/riceswap/wallpapers/` |
| the keybind snippet | `/usr/share/riceswap/riceswap-keybinds.conf` — a data file, **never edited into your Hyprland config** |

> **No `$HOME` writes, no `hyprland.conf` edits.** Package updates never touch
> your data; your profiles live in `~/.local/share/riceswap/`.

### The two keybinds

From the shipped `/usr/share/riceswap/riceswap-keybinds.conf`, add to your
Hyprland keybinds (or your dotfiles tool of choice):

```ini
# Autostart the panel
exec-once = qs -c riceswap

# Super+R toggles it
bind = $mainMod, R, global, quickshell:riceswap-toggle
```

Prefer no keybind? `qs ipc -c riceswap call riceswap toggle` does the same thing.

---

## 🚀 Quick start

Press <kbd>Super</kbd>+<kbd>R</kbd> — that's it. On first launch the panel
shows **onboarding**, which runs `init` for you: it creates the shared data
layers, lifts your hardware config out, installs the bundled wallpapers, and
copies the GUI into your user config.

Then, from the panel:

1. **Snapshot** your current desktop — name it, confirm the pre-checked chips
   of detected configs/packages/fonts, and watch the screenshot appear.
2. **Switch** to any profile — review the diff (packages in/out, services
   stop/start, blocked paths), confirm, and follow the live step checklist.
3. **Snapshot-first** — if a switch is blocked by a real (non-symlink) file
   at one of its targets, the panel chains you into a snapshot of your current
   desktop first (adopting that file into a profile) so the switch never
   clobbers it.
4. **Cancel or recover** — cancel stops at the next safe step boundary; a
   failed switch ends on a recovery screen with one-click restore.

<p align="center">
  <sub>
    ✓ verify &nbsp; · &nbsp; ● computing plan &nbsp; · &nbsp; ○ activating &nbsp; · &nbsp;
    ○ stop old services &nbsp; · &nbsp; ○ link configs &nbsp; · &nbsp; ○ apply packages &nbsp; · &nbsp;
    ○ reload Hyprland &nbsp; · &nbsp; ○ start new services
  </sub>
</p>

---

## ⚙️ How it works

**Isolated profiles, activated by symlinks.** Each rice is a directory under
`~/.local/share/riceswap/profiles/<name>/` mirroring your `$HOME` layout, with
a versioned `profile.toml` manifest (packages split official/AUR, services with
explicit start/stop, files, and an open `rice_info` table the panel renders
as-is). Activation is a `current` symlink pointing at the active profile.

**The backend is ten NDJSON operations** — one process per call, spawned by
the panel, streaming progress lines and closing with a single
`{ok, warnings, data}` envelope. State the panel watches comes from
`state.json`, so a switch that outlives the panel still shows its progress the
moment you reopen it.

**Packages & privileges, done natively.** Official packages go through
`pkexec pacman` (the system password dialog). AUR packages run your existing
helper (`yay`/`paru`) in a small floating terminal so its prompt always works.

---

## 🔧 The operations

| Operation | Does |
|---|---|
| `init` | Idempotent first-run bootstrap: shared layers, hardware extraction, bundled wallpapers |
| `detect` | Snapshot pre-flight: candidate config dirs, packages, assets |
| `snapshot <name>` | Writes a profile from confirmed selections (`--force` to overwrite) |
| `plan <target>` | Read-only switch diff: package in/out, service stop/start, blocked paths |
| `switch <target>` | Runs the approved plan, streams steps, honours cancel (`--aur-helper <helper>` pins the helper) |
| `list` | All profiles with manifest data |
| `info <name>` | One profile's full manifest |
| `delete <name>` | Removes a profile; refuses the active one unless `--force` |
| `diff <a> <b>` | Package/config delta between two profiles |
| `wallpaper-import <path>` | Moves an image into the shared wallpapers layer |

In v1 these are not a user-facing CLI — they're the seam the GUI drives and
every test runs through.

---

## 🛠️ Develop from source

```sh
# backend
cargo build --release
cargo test                     # 11 suites: contract + behavior through the operation seam
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# run the panel (dev symlink so hot-reload works)
ln -s /path/to/riceswap/gui ~/.config/quickshell/riceswap
qs -c riceswap
```

Needs `quickshell`, `hyprland`, `grim`, `pacman` on PATH, with the `riceswap`
binary available for the panel to spawn. GUI details live in [gui/README.md](gui/README.md).

---

<p align="center">
  <sub>
    Arch-first · x86_64 · Rust backend + Quickshell GUI · MIT
  </sub>
</p>
