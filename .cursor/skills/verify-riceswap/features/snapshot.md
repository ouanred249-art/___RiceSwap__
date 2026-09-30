# Snapshot the current rice

**What it is:** capture a running desktop (configs, packages, services, assets, screenshot) into an isolated profile.

## Sub-features
- `snapshot <name>`: re-runs the `detect` scan over `$HOME` (config dirs from the allowlist + hyprland reference chain, `exec-once` services, pacman-resolved package split, asset dirs), mirrors the chosen `$HOME`-relative paths into `profiles/<name>/` dereferencing symlinks, appends the shared-hardware `source =` line to a captured `hyprland.conf`, takes a grim screenshot (failure = warning, never error), writes `profile.toml`.
- `--force` overwrites a NON-active existing profile; `--only a,b,c` (comma-separated, one value) narrows after the scan.
- Active-profile protection: snapshotting over the **active** name REFUSES — even with `--force`. "Forking" means snapshotting under a NEW name while some profile is active (`forked: true` in the envelope). (Older claim inverted: it never forks over the same name.)

## How to get to it (user POV)
Panel → "Capture this rice" after tuning the desktop.

## Driving it with the CLI (scratch HOME, no compositor)
Stage a fake rice; note `init` itself creates `~/.config/quickshell/riceswap`, so a scratch snapshot legitimately captures THREE paths, not two:
```bash
FH=$(mktemp -d); S="env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY"
mkdir -p $FH/.config/hypr $FH/.config/quickshell/demo
printf 'monitor=,preferred,auto,1\nexec-once = waybar\n' > $FH/.config/hypr/hyprland.conf
printf 'import QtQuick\nShellRoot {}\n' > $FH/.config/quickshell/demo/shell.qml
HOME=$FH $S ./target/debug/riceswap init
HOME=$FH $S ./target/debug/riceswap snapshot demo
HOME=$FH $S ./target/debug/riceswap snapshot demo            # refused: exists
HOME=$FH $S ./target/debug/riceswap snapshot demo --force    # replaced
ln -sfn $FH/.local/share/riceswap/profiles/demo $FH/.local/share/riceswap/current
HOME=$FH $S ./target/debug/riceswap snapshot demo            # refused: ACTIVE name
HOME=$FH $S ./target/debug/riceswap snapshot forked          # ok, forked:true
HOME=$FH $S ./target/debug/riceswap snapshot narrowed --only .config/hypr
```

## Observable end state that proves it
- envelope ok; `checked_paths` lists `.config/hypr` plus EACH quickshell subdir (`shell_candidates` splits `.config/quickshell` by shell name) — the manifest's `[[files]]` agrees.
- the mirrored `hyprland.conf` equals the original PLUS the injected `source = ~/.config/hypr/riceswap/hardware.conf` line — byte-compare the file, expect exactly that one-line difference (claiming byte-identity is a stale proof: it fails).
- manifest carries: `[[services]]` from `exec-once` lines, `[packages]` official/aur split, `[rice_info]` (auto-detected bar/terminal/colors), and — written empty by snapshot, filled only by `install` — `[profile].source_url`/`source_commit` and the `[shell]` table.
- screenshot: env-dependent, do NOT assume absence — on a host with grim and a reachable compositor, a scratch snapshot records `screenshot = "screenshot.png"` with zero warnings. The invariant that holds everywhere: grim failing is a warning + empty field, never a failure.

## Gotchas
- The scan is `$HOME`-rooted: no captured PATH may point outside `$FH`. The package split shells the real `pacman -Qo`, so the machine's OWN packages legitimately appear — that is a package list, not a path leak; grep the manifest's paths, not its packages.
- `--only` takes ONE comma-separated value; `--only a b` parses as a stray positional.
- Never point it at the real HOME for verification: it writes a profile into the live store.
