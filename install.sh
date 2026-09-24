#!/usr/bin/env bash
#
# install.sh — one-shot installer for RiceSwap, no AUR / no yay needed.
#
# Builds the Rust backend, places the binary and the Quickshell GUI, installs
# the bundled wallpapers, and shows the two Hyprland lines you add yourself
# (the script never edits your Hyprland config).
#
# Usage:
#   ./install.sh            full install (needs rustc+cargo, make, install)
#   ./install.sh --help
#
set -euo pipefail

BIN_NAME="riceswap"
PREFIX="/usr/local"
GIT_URL="https://github.com/ouanred249-art/___RiceSwap__"
HYPR_CONF=""
DEST_GUI_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/quickshell/riceswap"
DEST_XDG_GUI="/etc/xdg/quickshell/riceswap"
DEST_WALL_DIR="$PREFIX/share/riceswap/wallpapers"
DEST_KEYBINDS="$PREFIX/share/riceswap/riceswap-keybinds.conf"

usage() {
    cat <<'EOF'
RiceSwap installer (no AUR required)

Works from any checkout of the repo, or from anywhere else — with no local
checkout it clones main from GitHub into a temp dir first, so the whole
install is one command.

What it does:
  0. finds a local checkout, or clones main from GitHub
  1. cargo build --release          (backend binary)
  2. installs the binary           → /usr/local/bin/riceswap
  3. installs the Quickshell GUI   → ~/.config/quickshell/riceswap/
                                     (where `qs -c riceswap` looks) and, when
                                      running with sudo, also to /etc/xdg/quickshell
  4. installs bundled wallpapers   → /usr/local/share/riceswap/wallpapers
  5. installs the keybind snippet  → /usr/share/riceswap/riceswap-keybinds.conf
  6. adds the two keybind lines to your Hyprland config (--hyprland,
     default: first of ~/.config/hypr/hyprland.conf or the xdg dir),
     guarded by a "# RiceSwap" marker block — re-runnable, removes old block first

Options:
  --prefix <dir>      install root (default /usr/local)
  --no-sudo           install without sudo (writes to ~/local instead)
  --url <giturl>      git URL to clone when no local checkout is found
                      (default: https://github.com/ouanred249-art/___RiceSwap__)
  --hyprland <path>   Hyprland config to add the keybind lines to
                      (default: ~/.config/hypr/hyprland.conf, or nothing for Lua configs —
                       in that case the lines are printed for you to add by hand)
  -h, --help          show this help

Prerequisites: rustc + cargo, git, make, install. Quickshell + Hyprland at run time.
EOF
}

NO_SUDO=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) PREFIX="$2"; shift 2 ;;
        --no-sudo) NO_SUDO=1; PREFIX="$HOME/local"; shift ;;
        --url) GIT_URL="$2"; shift 2 ;;
        --hyprland) HYPR_CONF="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 1 ;;
    esac
done

# --- locate the source tree --------------------------------------------------
# Prefer a local checkout; otherwise clone into a temp dir that is cleaned up
# on exit.
CLONED_DIR=""
find_repo() {
    local top
    if top="$(git -C "$(pwd)" rev-parse --show-toplevel 2>/dev/null)" \
        && [[ -n "$top" && -f "$top/Cargo.toml" ]] \
        && grep -q '^name = "riceswap"' "$top/Cargo.toml"; then
        REPO_DIR="$top"
        return
    fi
    local tmp
    tmp="$(mktemp -d)"
    CLONED_DIR="$tmp"
    trap '[[ -n "$CLONED_DIR" ]] && rm -rf "$CLONED_DIR"' EXIT
    echo "==> 0/6 no local checkout found; cloning $GIT_URL"
    git clone --depth 1 "$GIT_URL" "$tmp/riceswap"
    REPO_DIR="$tmp/riceswap"
}
find_repo

SUDO=""
if [[ -w "$PREFIX" || $NO_SUDO -eq 1 ]]; then
    SUDO=""
else
    SUDO="sudo"
fi

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: '$1' is required but not found on PATH" >&2
        exit 1
    fi
}
require cargo
require make
require install
require find
require cp
require chmod
require awk

# GNU coreutils' `install` has no -r flag (that's BSD); install a whole tree
# as source-dir-contents → dest-dir-contents, then normalize file modes.
install_tree() {
    local src="$1" dest="$2" mode="$3"
    mkdir -p "$dest"
    cp -a "$src/." "$dest/"
    find "$dest" -type f -exec chmod "$mode" {} +
}

echo "==> 1/6 building backend (cargo build --release) in $REPO_DIR"
( cd "$REPO_DIR" && cargo build --release )

BIN_PATH="$REPO_DIR/target/release/$BIN_NAME"
[[ -x "$BIN_PATH" ]] || { echo "error: build did not produce $BIN_PATH" >&2; exit 1; }

echo "==> 2/6 installing binary → $PREFIX/bin/$BIN_NAME"
$SUDO install -Dm755 "$BIN_PATH" "$PREFIX/bin/$BIN_NAME"

echo "    installing floating-terminal wrapper → $PREFIX/bin/riceswap-float"
$SUDO install -Dm755 "$REPO_DIR/scripts/riceswap-float" "$PREFIX/bin/riceswap-float"

echo "==> 3/6 installing Quickshell GUI → $DEST_GUI_DIR (qs -c riceswap)"
if [[ -e "$DEST_GUI_DIR" && ! -L "$DEST_GUI_DIR" ]]; then
    echo "    note: $DEST_GUI_DIR already exists — leaving it in place (update manually if needed)"
else
    rm -rf "$DEST_GUI_DIR" 2>/dev/null || true
    install_tree "$REPO_DIR/gui" "$DEST_GUI_DIR" 644
fi
# System-wide fallback for multi-user setups: install to the xdg dir too, so
# `qs -c riceswap` resolves for users without a personal copy.
if [[ -n "$SUDO" ]]; then
    $SUDO bash -c "mkdir -p '$DEST_XDG_GUI'; cp -a '$REPO_DIR'/gui/. '$DEST_XDG_GUI'/; find '$DEST_XDG_GUI' -type f -exec chmod 644 {} +" 2>/dev/null || true
fi

echo "==> 4/6 installing bundled wallpapers → $DEST_WALL_DIR"
if [[ -n "$SUDO" ]]; then
    $SUDO bash -c "mkdir -p '$DEST_WALL_DIR'; cp -a '$REPO_DIR'/assets/wallpapers/. '$DEST_WALL_DIR'/; find '$DEST_WALL_DIR' -type f -exec chmod 644 {} +"
else
    install_tree "$REPO_DIR/assets/wallpapers" "$DEST_WALL_DIR" 644
fi

echo "==> 5/6 installing keybind snippet → $DEST_KEYBINDS"
$SUDO install -Dm644 "$REPO_DIR/share/riceswap-keybinds.conf" "$DEST_KEYBINDS"

# --- 6/6: Hyprland keybinds --------------------------------------------------
# For a classic hyprland.conf, rewrite the managed block idempotently.
# Lua-based configs (hypr/hyprland/*.lua) are left alone — the two lines are
# printed for manual addition instead.
MARKER="# >>> riceswap (managed by install.sh) >>>"
UNMARKER="# <<< riceswap <<<"
BLOCK_BODY='exec-once = qs -c riceswap'
BLOCK_LINE='bind = $mainMod, R, global, quickshell:riceswap-toggle'

if [[ -z "$HYPR_CONF" ]]; then
    for p in "$HOME/.config/hypr/hyprland.conf" "${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprland.conf"; do
        [[ -f "$p" ]] && HYPR_CONF="$p" && break
    done
fi

HYPR_DONE=0
if [[ -n "$HYPR_CONF" && -f "$HYPR_CONF" ]]; then
    echo "==> 6/6 updating keybinds in $HYPR_CONF"
    # Strip a previous managed block, then append a fresh one.
    awk -v m="$MARKER" -v u="$UNMARKER" '
        $0 == m { inblk=1; next }
        $0 == u { inblk=0; next }
        !inblk { print }
    ' "$HYPR_CONF" > "$HYPR_CONF.riceswap-tmp"
    {
        cat "$HYPR_CONF.riceswap-tmp"
        echo
        echo "$MARKER"
        echo "$BLOCK_BODY"
        echo "$BLOCK_LINE"
        echo "$UNMARKER"
    } > "$HYPR_CONF"
    rm -f "$HYPR_CONF.riceswap-tmp"
    HYPR_DONE=1
fi

echo
echo "============================================================"
if [[ $HYPR_DONE -eq 1 ]]; then
    echo "Installed — keybinds written to $HYPR_CONF."
else
    echo "Installed. Add these two lines to your Hyprland config"
    echo "(the snippet is also at $DEST_KEYBINDS):"
    echo
    echo "    $BLOCK_BODY"
    echo "    $BLOCK_LINE"
    echo
fi
echo "Then:  $PREFIX/bin/$BIN_NAME init     (bootstrap)  and   qs -c riceswap"
echo "Toggle the panel with Super+R, or:  qs ipc -c riceswap call riceswap toggle"
echo "============================================================"
