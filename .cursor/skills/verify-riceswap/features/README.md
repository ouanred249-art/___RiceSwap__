# RiceSwap feature map

User-facing features of the `riceswap` CLI as the Quickshell panel invokes it. One file per feature; each answers: what it is, how a user reaches it, how to drive it in a scratch HOME, and what observable end state proves it.

| Feature | File | Reaches via |
|---|---|---|
| First-run bootstrap | `bootstrap.md` | `riceswap init`, `riceswap detect` |
| Snapshot the current rice | `snapshot.md` | `riceswap snapshot <name>` |
| Plan & switch profiles | `switch.md` | `riceswap plan <t>`, `riceswap switch <t>` (+ the verification tier and rollback after step 10) |
| Install a foreign rice | `install.md` | `riceswap install <source> [--shell <name>]` (+ the research tier's `pi` consultation) |
| Wallpapers in, wallpapers survive | `wallpapers.md` | `riceswap wallpaper-import <file>`, shared layer |

**Mapped-but-thin surfaces** (CLI ops with no feature file yet — their behavior is described in `src/cli.rs` USAGE and each module's doc; a maintenance run should graduate them if the panel starts exposing them): `info <name>`, `delete <name> [--force]`, `diff <a> <b>`.

**Beyond the CLI:** the full end-to-end acceptance ritual (throwaway user, spare VT, donor-graft fixture, sabotages, idempotent re-run) is `accept/run.sh` with its README in the same repo directory. It is the destination's definition of done and the ONLY place synthetic input may drive a real desktop; `accept/test-golden.sh` is its desktop-free half, CI-runnable.

Panel-only flows (the Super+R window, themes) have no CLI entry point of their own — they drive the ops above; prove them by proving the op, and say so in the report.

Verification of anything that would touch a real desktop is ONLY allowed inside the scratch-HOME + seat-scrub model of the parent SKILL.md (`env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY ...`) — HOME isolation does not isolate the compositor, and `ydotool` injects into whatever session holds the focused seat regardless of HOME.
