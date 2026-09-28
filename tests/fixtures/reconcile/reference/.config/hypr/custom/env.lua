-- Caelestia's launcher and CLI look for wallpapers in
-- $CAELESTIA_WALLPAPERS_DIR first; without it they fall back to a default
-- that does not match this machine's collection.
hl.env("CAELESTIA_WALLPAPERS_DIR", os.getenv("HOME") .. "/Wallpapers")
