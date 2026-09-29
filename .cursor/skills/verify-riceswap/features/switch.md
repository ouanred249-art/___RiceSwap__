# Plan & switch profiles

**What it is:** the round trip between two rices — diff-first (`plan`), then the full flip (`switch`): packages, services, shell, config symlinks, reload.

## Sub-features
- `plan <target>`: what would change; touches nothing.
- `switch <target>`: install missing packages (batched prompts), remove orphans (never system/session), stop old services/shell, unlink (preserving real files into the leaving profile's `backups/`), link new, flip `current`, start services + shell, reload Hyprland.
- The reconcile pass runs inside every switch (dead dispatcher names repaired, proposals reported) — see reconcile fixtures.

## How to get to it (user POV)
Panel → pick a rice → confirm the diff → one click.

## Driving it with the CLI (scratch HOME)
With two staged profiles (e.g. snapshot + install, or two hand-made manifests):
```bash
HOME=$FH ./target/debug/riceswap plan demo
HOME=$FH ./target/debug/riceswap switch demo   # expected to degrade without a compositor
```

## Observable end state that proves it
- `plan`: `state.json` and both profile dirs byte-unchanged after — prove by hash comparison, not by the command's name.
- `switch` in a scratch HOME: the phases BEFORE the compositor (backup of real link targets, unlink/link, `current` symlink flip, manifest of the leaving profile gains backups) complete; reload/shell-start degrade visibly. `current` → target profile, and the previously-active profile's real files were preserved under `backups/`, not deleted.
- Reconcile facts in the envelope (`report.reconcile`): registered names, applied moves, proposals — assert shape even when the repair set is empty.

## Gotchas
- NEVER drive `switch` against the real HOME — the standing rule from the repo's CLAUDE.md: the user switches themselves.
- Package prompts route through pkexec/sudo fallbacks; in a scratch HOME they fail per design — the switch must continue as a warning for removals, fatal for required installs (the #15/#16 contract).
