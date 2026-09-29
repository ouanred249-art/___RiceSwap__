# RiceSwap feature map

User-facing features of the `riceswap` CLI as the Quickshell panel invokes it. One file per feature; each answers: what it is, how a user reaches it, how to drive it in a scratch HOME, and what observable end state proves it.

| Feature | File | Reaches via |
|---|---|---|
| First-run bootstrap | `bootstrap.md` | `riceswap init`, `riceswap detect` |
| Snapshot the current rice | `snapshot.md` | `riceswap snapshot <name>` |
| Plan & switch profiles | `switch.md` | `riceswap plan <t>`, `riceswap switch <t>` |
| Install a foreign rice | `install.md` | `riceswap install <source> [--shell <name>]` |
| Wallpapers in, wallpapers survive | `wallpapers.md` | `riceswap wallpaper-import <file>`, shared layer |

Panel-only flows (the Super+R window, themes) have no CLI entry point of their own — they drive the ops above; prove them by proving the op, and say so in the report.

Verification of anything that would touch a real desktop is ONLY allowed inside the scratch-HOME model of the parent SKILL.md.
