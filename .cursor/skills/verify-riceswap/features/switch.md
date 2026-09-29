# Plan & switch profiles

**What it is:** the round trip between two rices — diff-first (`plan`), then the full flip (`switch`): packages, services, shell, config symlinks, reload — **followed by a verification tier that decides whether the desktop actually came up** (not a numbered step; the ten steps are byte-pinned in `tests/contracts.rs`).

## Sub-features
- `plan <target>`: what would change; touches no profile and no live path.
- `switch <target>`: steps 1–10 — preflight/diff, install missing packages (batched prompts), remove orphans (never system/session), stop old services/shell, unlink (preserving real files into the leaving profile's `backups/`), link new, flip `current`, reconcile + start services + shell, reload Hyprland.
- Then **verification** (`src/verify.rs`, frozen vocabulary): Tier C always — `shell-alive`, `invariant-live` (live `hyprctl globalshortcuts`), declared `ipc-liveness`; Tier F — recipe `[[probe]]` drives when `grim`+`ydotool` exist and a shell was verified. Verdicts `verified-full | verified-core | fail`. An unavailable probe is a skip (`ok:true`, reason in evidence) — never a false fail; a degraded pass is `verified-core` with a warning, never phantom rows.
- On `fail`: `phase:"verify"`, frozen `reason_code` (`shell-not-alive`, `invariant-dead-names`, `recipe-drift`, `probe-failed:<id>`), and **rollback**: `current` flips back FIRST, old services/shell restart, links restore; packages the switch moved are enumerated in `data.next` and deliberately NOT reverted. `rollback-failed` names the manual recovery.

## How to get to it (user POV)
Panel → pick a rice → confirm the diff → one click.

## Driving it with the CLI (scratch HOME)
Stage two profiles (snapshot + install, or two hand-made manifests), **scrub the seat env** (SKILL.md — otherwise step 8 reloads your LIVE desktop):
```bash
FH=$(mktemp -d); S="env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS"
HOME=$FH $S ./target/debug/riceswap plan demo
HOME=$FH $S ./target/debug/riceswap switch demo
```

## Observable end state that proves it
- `plan` touches nothing: hash the whole scratch tree **EXCLUDING `state.json`** and compare — every invocation rewrites the journal (`run()` begin/finish), so the old "state.json byte-unchanged" proof fails by design. Also assert afterwards: `operation: null`, `active_profile` unchanged.
- `switch` with the seat scrubbed: the pre-compositor phases complete (backups of real link targets, unlink/link, `current` flip, leaving profile's `backups/` populated); `hyprctl` reload degrades with a warning; verdict `verified-core` — with `shell-alive` ok ("names no shell") and `invariant-live` as a skip. `ok:true` — this degradation IS the tested path.
- Envelope facts: `data.completed_steps == 10`; `report.reconcile` carries the full Report (~19 keys: `registered`, `dead_names`, `proposals`, `resolutions` — the applied moves live under `resolutions`, not "applied" — `layers`, `env`, `dirs_created`, `files_written`, `backups`, `foreign_entries`, `appid_drift`, `skipped`, `warnings`); `report.verification {verdict, checks[]}` rides on pass AND failure alike, and is the ONE place the verdict lives.
- To see the failure path honestly in a sandbox: give the profile a `[shell]` with `start = "false"`. Expect `ok:false`, `reason_code:"shell-not-alive"`, `completed_steps:10`, `current` back on the old profile, `data.next` describing the switch-back. A `[shell]` that starts and survives its 700 ms grace but can't be probed lands `verified-core` with a Tier-F warning instead.
- Reconcile facts must be asserted by SHAPE even when the repair set is empty.

## Gotchas
- NEVER drive `switch` against the real HOME — the user switches themselves (standing rule).
- NEVER drive with inherited compositor env (see above): HOME isolation does not isolate the SEAT.
- Package prompts route through pkexec/sudo fallbacks; with `DBUS_SESSION_BUS_ADDRESS` scrubbed, pkexec refuses — removal failures continue as warnings, required-install failures stay fatal (the #15/#16 contract).
- An installed profile with `[[probe]]` entries on a machine that has grim+ydotool runs Tier F FOR REAL against whatever display it can reach — the probes carry ydotool input to the focused session regardless of HOME. In scratch drives that means: don't stage probe-declaring recipes unless the seat is scrubbed AND you accept the tier degrading (it will — no live shell).
