# Wallpapers in, wallpapers survive

**What it is:** a shared wallpaper layer every profile reads, so switching rice never breaks the desktop's art, and imports own the file they take.

## Sub-features
- bundled defaults land on `init` (4 images).
- `wallpaper-import <file>`: validates the image signature, MOVES the file into the layer.
- profiles reference the layer; `install` copies a rice's bundled `Wallpapers` dir in (never moves out of a source the tool doesn't own); the caelestia recipe's `[[dirs]]` guarantees `~/Wallpapers` exists.

## How to get to it (user POV)
Panel: pick a wallpaper / a rice ships its own set; after switching, the art is still there.

## Driving it with the CLI (scratch HOME)
```bash
FH=$(mktemp -d); IMG=$(mktemp)
printf '\x89PNG\r\n\x1a\n' > "$IMG"        # minimal PNG signature (stub image)
# NOTE: a signature-only file is enough for the validator if the tool checks
# magic bytes only — first run should confirm whether it rejects it (it may
# require decodable dimensions; if so, use one of the repo's assets/ images).
cp assets/wallpapers/default-dusk.png "$IMG"
HOME=$FH ./target/debug/riceswap init
HOME=$FH ./target/debug/riceswap wallpaper-import "$IMG"
```

## Observable end state that proves it
- the file GONE from its original path (import MOVES; contrast with install's COPY — prove by `test -e "$IMG"` failing after import),
- present under `$FH/.local/share/riceswap/wallpapers/`,
- a non-image (`printf 'not an image' > bad.txt`) → refused, file untouched,
- bundled defaults: after `init`, `wallpapers/` holds the 4 images; switching profiles never deletes them.

## Gotchas
- the shared layer is HOME-rooted like everything else — the scratch HOME isolates it; a stray import into the real `~/.local/share/riceswap/wallpapers` is a leak, caught by `find ~/.local/share/riceswap/wallpapers -newer` on the REAL store after any verification run (it must be empty).
