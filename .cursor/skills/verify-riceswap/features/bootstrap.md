# Bootstrap (first run)

**What it is:** the tool discovers its environment and lays down the shared layers so any later operation has something to work with.

## Sub-features
- `detect`: probe every external tool (pacman, yay/paru, hyprctl, grim, pkexec, floating terminal, git, pi) and attach the report to the envelope.
- `init`: create the data dir (`profiles/`, `wallpapers/` with the 4 bundled defaults, `hardware.conf`), lift the hardware blocks out of the live `~/.config/hypr/hyprland.conf` into `~/.config/hypr/riceswap/hardware.conf` (appending a `source =` line to the conf), copy the packaged GUI into `~/.config/quickshell/riceswap`, write `state.json`.
- `list`: enumerate profiles and the active one.

## How to get to it (user POV)
Fresh machine, right after install.sh — the panel cannot open until at least `init` has run.

## Driving it with the CLI
```bash
FH=$(mktemp -d)
# seat env scrubbed — see SKILL.md "Isolation model": files are isolated by
# HOME, the COMPOSITOR is only isolated by removing these.
HOME=$FH env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY cargo run --quiet -- detect
HOME=$FH env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY cargo run --quiet -- init
HOME=$FH env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY cargo run --quiet -- list
```

## Observable end state that proves it
- `detect`: envelope `data.tools` contains one status entry per probed tool — the roster is nine: pacman, yay, paru, hyprctl, grim, pkexec, riceswap-float, git, pi. A tool missing from PATH yields `available:false, exit_code:null` with `error` naming the cause (there is NO exit code for a tool that never ran — an older claim, corrected); every unusable tool also appends a `"<tool> is not usable: ..."` warning, so a headless `detect` is `ok:true` **with warnings**, not bare.
- `init`: envelope `data.created` lists everything made — `profiles/`, `wallpapers/` + the 4 `default-*.png`, `hardware.conf`, the `~/.config/hypr/riceswap/hardware.conf` symlink, the copied GUI tree under `~/.config/quickshell/riceswap/`. A second `init` returns `created: []` (that empty list IS the idempotency proof).
- `list` on a fresh store: `{"profiles":[],"active_profile":null}`.

## Gotchas
- `detect` is READ-ONLY in intent but NOT silent on disk: every invocation (this one included) creates `$HOME/.local/share/riceswap/state.json` — `operations::run` begins/finishes an operation journal around dispatch — and probing `yay`/`paru` lets those helpers write their own `~/.config/yay` and `~/.cache/yay`. Prove it right: the store dir is *expected* after; nothing else outside `$FH` may change (leak-check the REAL `~/.local/share/riceswap` with `find -newer`).
- init is idempotent: a second init must not clobber user files — drive twice, compare `data.created` ([] the second time) and the files' mtimes.
- state.json's operation entry gained a `source` field (null unless a resumable install holds a claim); anything pinning the journal shape must include it.
- The store has grown: `sources/` (acquisition cache) and `recipes/` (user-tier) appear after install/research runs, not after init — a "store contains exactly X" assertion must account for the operation that made them.
