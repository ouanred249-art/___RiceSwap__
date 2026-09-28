# Reconciliation fixtures (issue #35)

Ground truth, copied verbatim from the machine's profile store. Nothing here is
edited: these are the bytes the manual adaptation worked on, and the engine's
tests assert against them.

## `donor/` — the donor-graft case

A caelestia profile *before* the hand-fix, which is the state the ticket is
about: the configs still dispatch the ii (donor) vocabulary under the
`quickshell` appid while the shell that activates with them is caelestia.

* `.config/hypr/**` — `~/.local/share/riceswap/profiles/caelestia/backups/keybinds-2026-09-26/`
  (`keybinds.lua`, `custom-keybinds.lua`, `general.lua`, `hypridle.conf`), the
  originals the hand-fix replaced. Note the names the hand-fix *invented*:
  `quickshell:searchToggleRelease` → `caelestia:launcher` with a `release = true`
  bind option, `quickshell:regionScreenshot` → `caelestia:screenshotClip`. The
  engine deliberately does neither: `searchToggleRelease` and `regionScreenshot`
  are in no caelestia registry, so both stay as they are and both surface as
  proposals. The one move the hand-fix made that *is* provable —
  `quickshell:lock` → `caelestia:lock` in `hypridle.conf` — is the one the
  engine makes.
* `.config/quickshell/caelestia/**` — every file in the real caelestia QML tree
  that declares a shortcut (`components/misc/CustomShortcut.qml` carries the
  `appid: "caelestia"` the other seven inherit, and the 22 registered names live
  in those seven).

## `ii/` — the guard case

ii's own profile tree: the regression guard. ii registers its names under the
default `quickshell` appid, so `quickshell:*` inside the ii profile is its own
live vocabulary, and a correct engine writes nothing here at all.

* `.config/hypr/**` — `~/.local/share/riceswap/profiles/ii/.config/hypr`, whole,
  including the helper scripts its binds `exec` (the engine scans those and
  reports what it finds; it never edits them).
* `.config/quickshell/ii/**` — every file in the real ii QML tree that contains
  a `GlobalShortcut` block (27 files). The rest of the ii tree — views, assets,
  translations — declares no shortcut, so it is not part of what the engine
  derives.
* `.config/quickshell/riceswap/` is deliberately absent: the RiceSwap panel is a
  *second* shell in its own tree, and `quickshell:riceswap-toggle` surviving
  reconciliation is what the engine's built-in foreign set exists for.
