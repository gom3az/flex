#!/usr/bin/env bash
# flex-wallpaper.sh — wrapper for `flex wallpaper` (wallpaper cutover).
# Reads the single ACTION: line AFTER flex exits (zsh-safe `IFS= read -r`),
# resolves the row's path-hash id back to the image with the hidden
# `flex wallpaper --resolve` lookup (ids stay space-free on the ACTION:
# line, exactly like clip hashes), then hands the path to set-wallpaper.sh,
# which repoints hyprpaper and regenerates the theme.
# Test override: SET_WALLPAPER.
set -euo pipefail

# `flex` lives in ~/.local/bin (symlink to the release build), which
# Hyprland bind execs don't have on PATH. Only prepend when unresolved so
# deliberately-stubbed PATHs (tests) keep priority.
command -v flex >/dev/null 2>&1 || export PATH="$HOME/.local/bin:$PATH"

# Re-launch inside a floating popup when invoked from a bind (same pattern
# as kill-menu.sh): the TUI needs a real tty, which plain exec doesn't give.
# `menu-wide` matches the picker it replaced (wallpaper-picker.sh).
if [[ "${POPUP_KITTY:-}" != 1 ]]; then
    exec "$HOME/.config/scripts/popup.sh" menu-wide "$0" "$@"
fi

out="$(flex wallpaper "$@")" # cancel (130) passes through via set -e

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: wallpaper "*) ;;
    *) echo "flex-wallpaper: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* wallpaper }"
id="${rest%% *}"
# Ids are FNV-1a hex of the path (16 chars); reject anything else before
# spawning the resolve lookup.
[[ "$id" =~ ^[0-9a-f]{16}$ ]] || { echo "flex-wallpaper: bad id: $id" >&2; exit 1; }

path="$(flex wallpaper --resolve "$id")" ||
    { echo "flex-wallpaper: unknown id: $id" >&2; exit 1; }
[[ -f "$path" ]] || { echo "flex-wallpaper: not a file: $path" >&2; exit 1; }

exec "${SET_WALLPAPER:-$HOME/.config/scripts/set-wallpaper.sh}" "$path"
