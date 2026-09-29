# Wallpapers in, wallpapers survive

**What it is:** a shared wallpaper layer every profile reads, so switching rice never breaks the desktop's art, and imports own the file they take.

## Sub-features
- bundled defaults land on `init` — exactly 4 images (`default-dawn/day/dusk/night.png`; prefers a packaged `/usr/share/riceswap/wallpapers` dir, else the 4 embedded in the binary; never overwrites existing files).
- `wallpaper-import <file>`: gates on **extension AND magic bytes** (PNG/JPEG/GIF/BMP/WEBP/AVIF/TIFF/JXL — decodable dimensions are NOT required), then **MOVES** the file into the layer (rename, copy+remove across devices), never clobbering: a name clash gets a numbered destination; a byte-identical re-import is a no-op overwrite.
- profiles reference the layer; `install` **copies** a rice's `Wallpapers/` dir in (never moves out of a source the tool doesn't own).
- the caelestia recipe's `[env] CAELESTIA_WALLPAPERS_DIR = "~/Wallpapers"` and `[[dirs]]` guarantee are applied **inside the reconcile pass**, which SKIPS entirely when a profile mirrors no `.config/hypr` — a quickshell-only fixture gets the images and NO `~/Wallpapers`, no env block. This gate is invisible from the envelope's wallpaper facts; assert it by observing.

## How to get to it (user POV)
Panel: pick a wallpaper / a rice ships its own set; after switching, the art is still there.

## Driving it with the CLI (scratch HOME)
```bash
FH=$(mktemp -d); SRC=$(mktemp -d)
S="env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS"
cp assets/wallpapers/default-dusk.png "$SRC/dusk.png"   # a REAL image file: the
                                                        # extension gate refuses
                                                        # extensionless mktemp paths
HOME=$FH $S ./target/debug/riceswap init
HOME=$FH $S ./target/debug/riceswap wallpaper-import "$SRC/dusk.png"
printf 'not an image' > "$SRC/bad.png"
HOME=$FH $S ./target/debug/riceswap wallpaper-import "$SRC/bad.png"
# dirs/env guarantee needs a hypr-carrying rice:
RICE=$SRC/myrice; mkdir -p $RICE/.config/quickshell/caelestia $RICE/.config/hypr $RICE/Wallpapers
printf 'import QtQuick\nQuickShell {}\n' > $RICE/.config/quickshell/caelestia/shell.qml
printf 'monitor=,preferred,auto,1\n' > $RICE/.config/hypr/hyprland.conf
cp assets/wallpapers/default-dawn.png $RICE/Wallpapers/
HOME=$FH $S ./target/debug/riceswap install "$RICE" --shell caelestia
```

## Observable end state that proves it
- the file GONE from `$SRC` (import MOVES — `test -e` fails after), present under `$FH/.local/share/riceswap/wallpapers/`.
- `bad.png` → `ok:false` naming "not a recognized image format"; the file STAYS in `$SRC`; not in the layer.
- after the install: envelope `data.wallpapers` lists the copies; `$RICE/Wallpapers/` intact (COPY, hash before/after); `$FH/Wallpapers/` exists; the profile's mirrored `.config/hypr/custom/env.lua` carries the `# >>> riceswap:adapt >>>` block writing `hl.env("CAELESTIA_WALLPAPERS_DIR", os.getenv("HOME") .. "/Wallpapers")` — `os.getenv` at runtime, deliberately not a baked path.
- bundled 4 after `init`; switching profiles never deletes the layer.

## Gotchas
- Every `~`/`$HOME` expansion is rooted at the PASSED `$HOME` (verified: no `getpwuid`/real-uid lookup anywhere in `src/`) — the scratch HOME isolates it fully.
- Leak-check remains the standing proof: `find ~/.local/share/riceswap/wallpapers -newer <marker>` on the REAL store after any verification run must be empty.
- The fake `caelestia` install will fail *verification* on a machine whose real shell binary exists but whose stub tree can't survive it (`shell-not-alive`, rolled back) — the wallpaper facts above land BEFORE that verdict; read `data.wallpapers`/`data.reconcile`, not `ok`.
