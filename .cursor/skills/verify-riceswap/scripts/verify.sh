#!/usr/bin/env bash
# verify.sh — drive one mapped feature of the verify-riceswap skill in a
# scratch HOME, archive evidence, clean up. Evidence SURVIVES cleanup.
#
# Usage:  .cursor/skills/verify-riceswap/scripts/verify.sh <feature>
#         (feature: bootstrap | snapshot | switch | install | wallpapers)
#
# Prints the envelope of each step; exits nonzero on the first ok:false
# unless the feature's map says that outcome IS the proof.
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"

FEATURE="${1:?usage: verify.sh <feature>}"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE=".scratch/verify-riceswap/$TS/$FEATURE"
mkdir -p "$EVIDENCE"

BIN="${RICESWAP_BIN:-./target/debug/riceswap}"
[ -x "$BIN" ] || { echo "build first: cargo build (or set RICESWAP_BIN)"; exit 1; }

FH="$(mktemp -d)"
TREE="$(mktemp -d)"
STUBBIN="$(mktemp -d)"
trap 'rm -rf "$FH" "$TREE" "$STUBBIN"' EXIT

# Session isolation, not just file isolation: a drive that inherits the
# driver's HYPRLAND_INSTANCE_SIGNATURE/WAYLAND_DISPLAY reloads the USER'S
# LIVE desktop at step 8, and a fake profile shell attaches to the real
# compositor. Scrub the seat env on every call.
printf '#!/bin/sh\necho "harness stub: pi absent" >&2\nexit 127\n' > "$STUBBIN/pi"
chmod +x "$STUBBIN/pi"
# The stub must PRECEDE the real PATH, and stand in for pi itself: the
# research tier skips only when pi is unusable or a recipe exists — an
# unstubbed drive makes a real, networked, authenticated LLM call with the
# user's key (see features/install.md).
SCRUB_ENV=(env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY -u DISPLAY
           -u XDG_SEAT -u XDG_SESSION_TYPE -u DBUS_SESSION_BUS_ADDRESS)

run() { # <label> <args...> — HOME+seat-scoped call; envelope -> evidence/<label>.ndjson
  local label="$1"; shift
  echo "── $label: riceswap $*"
  local out
  out="$(HOME="$FH" PATH="$STUBBIN:$PATH" "${SCRUB_ENV[@]}" "$BIN" "$@" || true)"
  printf '%s\n' "$out" > "$EVIDENCE/$label.ndjson"
  printf '%s\n' "$out" | tail -1 | python3 -c 'import json,sys; d=json.load(sys.stdin); print("  ok:",d.get("ok")); print("  warnings:",d.get("warnings")); print("  keys:",list((d.get("data") or {}).keys())[:12])'
}

case "$FEATURE" in
  bootstrap)
    run detect detect
    run init init
    run list list
    ;;
  snapshot)
    mkdir -p "$FH/.config/hypr" "$FH/.config/quickshell/demo"
    printf 'monitor=,preferred,auto,1\n' > "$FH/.config/hypr/hyprland.conf"
    printf 'import QtQuick\nShellRoot {}\n' > "$FH/.config/quickshell/demo/shell.qml"
    run init init
    run snapshot-demo snapshot demo
    run info-demo info demo
    ;;
  switch)
    mkdir -p "$FH/.config/hypr" "$FH/.config/quickshell/demo"
    printf 'monitor=,preferred,auto,1\n' > "$FH/.config/hypr/hyprland.conf"
    run init init
    run snapshot-demo snapshot demo
    run plan plan demo
    run switch switch demo || true   # degrades without a compositor by design
    ;;
  install)
    mkdir -p "$TREE/.config/quickshell/demo" "$TREE/.config/hypr"
    printf 'import QtQuick\nShellRoot {}\n' > "$TREE/.config/quickshell/demo/shell.qml"
    printf 'monitor=,preferred,auto,1\n' > "$TREE/.config/hypr/hyprland.conf"
    printf '%s\n' '$lock_cmd = hyprctl dispatch '"'"'hl.dsp.global("quickshell:lock")'"'"' &' \
      > "$TREE/.config/hypr/hypridle.conf"
    run init init
    ( cd "$TREE" && find . -type f -exec sha256sum {} + ) | sort > "$EVIDENCE/tree-before.sha"
    run install "install" "$TREE"
    ( cd "$TREE" && find . -type f -exec sha256sum {} + ) | sort > "$EVIDENCE/tree-after.sha"
    diff "$EVIDENCE/tree-before.sha" "$EVIDENCE/tree-after.sha" \
      && echo "  source tree untouched: PROVEN"
    ;;
  wallpapers)
    run init init
    cp assets/wallpapers/default-dusk.png "$TREE/import-me.png"
    run import "wallpaper-import" "$TREE/import-me.png"
    test ! -e "$TREE/import-me.png" && echo "  import moved the file: PROVEN"
    printf 'not an image' > "$TREE/bad.txt"
    run import-bad "wallpaper-import" "$TREE/bad.txt" || true
    test -e "$FH/.local/share/riceswap/wallpapers/import-me.png" \
      && echo "  layer holds the import: PROVEN"
    ;;
  *)
    echo "unknown feature: $FEATURE (see features/README.md)"; exit 2;;
esac

echo
echo "evidence kept at $EVIDENCE (scratch HOME was cleaned up)"
