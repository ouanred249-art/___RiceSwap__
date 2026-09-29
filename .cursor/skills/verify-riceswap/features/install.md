# Install a foreign rice

**What it is:** the destination feature — one command takes a stranger's dotfiles (git URL or local dir) to a switched, reconciled desktop.

## Sub-features
- `install <git-url>` / `install <local-dir>`, `--shell <name>` when the tree holds several quickshell shells.
- Identity refusals: not-a-quickshell-rice, ambiguous without --shell, profile exists (complete), source is neither dir nor URL.
- Resume: an install that died mid-way is re-made by plain re-invocation (journal claim, written only after the exists-check — a refusal must leave `state.json` untouched).
- Cache: full clone keyed `sources/<slug>/<sha>`; second run reuses; no re-clone.
- The donor's dead dispatcher names are reconciled before the first switch reads them; the user's local dir is read in place, never copied into the cache and never mutated.
- Research tier (#38): unknown shell with no recipe triggers a stubbed/`pi` consultation; write-back only to `recipes/<shell>.toml`, never clobbers.

## How to get to it (user POV)
Terminal (future: panel): "install this rice" → `riceswap install https://…/dots`.

## Driving it with the CLI (scratch HOME)
Stage a donor-graft rice OUTSIDE $HOME (it is a source, not a desktop):
```bash
FH=$(mktemp -d); TREE=$(mktemp -d)
mkdir -p $TREE/.config/quickshell/demo $TREE/.config/hypr
printf 'import QtQuick\nShellRoot {}\n' > $TREE/.config/quickshell/demo/shell.qml
printf 'monitor=,preferred,auto,1\n' > $TREE/.config/hypr/hyprland.conf
printf '$lock_cmd = hyprctl dispatch %s\n' "'hl.dsp.global(\"quickshell:lock\")'" > $TREE/.config/hypr/hypridle.conf
HOME=$FH ./target/debug/riceswap init
HOME=$FH ./target/debug/riceswap install "$TREE"
```

## Observable end state that proves it
- envelope ok:true; `profiles/demo/profile.toml` has `source_url = "file://$TREE"`, a `[shell]` table, mirrored files.
- reconcile facts in envelope: `quickshell:lock` appears as a dead name / proposal (demo registers nothing — conservative engine must NOT rewrite blindly; assert the proposal, not a move).
- the source tree is byte-unchanged (hash `TREE` before/after — "read where it stands" contract).
- second `install $TREE` → refuses `profile-exists`, and the refusal does NOT leave a claim that a third run would resume (drive three times: ok, refuse, refuse).
- ambiguity: add a second quickshell dir to a fresh tree → refusal names both candidates; `--shell` picks.
- cache (URL path): only meaningful with a scripted `git`; in a bare scratch HOME without git → reason_code `git-missing` and zero dirs created under `sources/`.

## Gotchas
- `install` triggers `switch`; in a scratch HOME the compositor steps degrade — the evidence is the PRE-flip facts (profile materialized, reconciled, `current` flipped), exactly as in `switch.md`.
- NEVER point `install` at the real HOME store; the journal claim writes into `$HOME/.local/share/riceswap`.
