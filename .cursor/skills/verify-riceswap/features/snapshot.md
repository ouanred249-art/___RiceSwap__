# Snapshot the current rice

**What it is:** capture a running desktop (configs, packages, wallpapers, screenshot) into an isolated profile.

## Sub-features
- `snapshot <name>`: auto-detect config dirs under `$HOME`, scan packages/services, mirror files into `profiles/<name>/`, screenshot via grim.
- `--force` overwrite; `--only <paths>` narrows after the scan.
- Active-profile protection: snapshotting over the *active* name forks instead of overwriting.

## How to get to it (user POV)
Panel → "Capture this rice" after tuning the desktop.

## Driving it with the CLI (scratch HOME, no compositor)
The real machine's session can't be copied here, so stage a fake rice in the scratch HOME first:
```bash
FH=$(mktemp -d)
mkdir -p $FH/.config/hypr $FH/.config/quickshell/demo
printf 'monitor=,preferred,auto,1\n' > $FH/.config/hypr/hyprland.conf
printf 'import QtQuick\nShellRoot {}\n' > $FH/.config/quickshell/demo/shell.qml
HOME=$FH ./target/debug/riceswap init
HOME=$FH ./target/debug/riceswap snapshot demo
HOME=$FH ./target/debug/riceswap info demo
```

## Observable end state that proves it
- envelope ok; `profiles/demo/profile.toml` exists with `[[files]]` entries covering `.config/hypr` and the quickshell dir; the files themselves are mirrored (byte-compare hyprland.conf); `services` picked up from `exec-once` lines if present.
- screenshot: in a scratch HOME grim is absent — the manifest records no screenshot and the envelope carries that as a warning, not a failure.
- second `snapshot demo` without `--force` → refused; with `--force` → replaced; active-profile protection: after a successful `switch demo` (even a partial one), `snapshot demo` forks instead of clobbering.

## Gotchas
- The scan is `$HOME`-rooted: any path outside the scratch HOME that appears in the profile is a leak — grep the manifest.
- `--only` narrowing happens after the same scan, so `detect` output and snapshot selection must agree on what existed.
