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

## `reference/` — what the hand-fix left (issue #36)

The caelestia profile's own configs **after** the manual adaptation, copied
verbatim from `~/.local/share/riceswap/profiles/caelestia/.config/hypr`. Nothing
here is ever edited or written: it is the other side of the convergence
comparison the built-in `caelestia` recipe exists to pass, so the recipe's
entries can be checked against a human's own work rather than against itself.

* `.config/hypr/hyprland/keybinds.lua`, `.config/hypr/custom/keybinds.lua`,
  `.config/hypr/hypridle.conf`, `.config/hypr/custom/env.lua` — the four files
  the hand-fix rewrote, and the only ones with a dispatcher name in them.
* `custom/general.lua` is deliberately *not* copied. The two 4-finger gestures
  in the donor's copy did dispatch `quickshell:overviewWorkspacesToggle`, but
  the file on the machine is a wholesale HyprGlass tuning rewrite that has
  nothing to do with dispatcher names, so comparing against it would prove
  nothing. The test says so where it matters.
* The two differences that are *not* dispatch rewrites — the human's comment
  blocks, and the `after_sleep_cmd` composite FC-7 records — are named in
  `src/reconcile.rs`'s convergence tests, each with the reason no `[[resolution]]`
  can express it.
