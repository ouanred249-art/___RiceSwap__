# Bootstrap (first run)

**What it is:** the tool discovers its environment and lays down the shared layers so any later operation has something to work with.

## Sub-features
- `detect`: probe every external tool (pacman, yay/paru, hyprctl, grim, pkexec, floating terminal, git, pi) and attach the report to the envelope.
- `init`: create the data dir (`profiles/`, `wallpapers/` with the 4 bundled defaults, `hardware.conf`), write `state.json`.
- `list`: enumerate profiles and the active one.

## How to get to it (user POV)
Fresh machine, right after install.sh — the panel cannot open until at least `init` has run.

## Driving it with the CLI
```bash
FH=$(mktemp -d)
HOME=$FH cargo run --quiet -- detect      # read-only; last line is the envelope
HOME=$FH cargo run --quiet -- init
HOME=$FH cargo run --quiet -- list
```

## Observable end state that proves it
- `detect`: envelope `data.tools` contains a status entry per probed tool, exit codes present even for failing tools (in a headless box hyprctl fails — the envelope must still be ok:true).
- `init`: `$FH/.local/share/riceswap/` contains `state.json`, `wallpapers/` (bundled images present), `hardware.conf`; `list` then returns `ok:true` with `active_profile: null` and `profiles: []`.

## Gotchas
- `detect` must never mutate `$HOME` (only reads). Prove it: store dir absent before, absent after.
- init is idempotent: a second init on an initialized store must not clobber user files — drive it twice, compare.
