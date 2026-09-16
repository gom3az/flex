#!/usr/bin/env bash
# flex-clip.sh — wrapper for `flex clip` (M4 clip cutover).
# Reads the single ACTION: line AFTER flex exits (zsh-safe `IFS= read -r`),
# resolves the content-hash id back to the stored line via
# `flex clip --resolve`, then performs the side effect. Selection executes
# after the loop: wl-copy forks a daemon to serve the clipboard, so its
# stdout/stderr must not point at the popup pty (or kitty keeps the window
# open after this script exits) — same redirect as cliphist.sh.
# Test overrides: CLIPHIST_FILE, CLIPHIST_PINS (honored by bash and flex).
set -euo pipefail

# `flex` lives in ~/.local/bin (symlink to the release build), which
# Hyprland bind execs don't have on PATH. Only prepend when unresolved so
# deliberately-stubbed PATHs (tests) keep priority.
command -v flex >/dev/null 2>&1 || export PATH="$HOME/.local/bin:$PATH"

# Re-launch inside a floating popup when invoked from a bind (same pattern
# as kill-menu.sh): the TUI needs a real tty, which plain exec doesn't give.
if [[ "${POPUP_KITTY:-}" != 1 ]]; then
    exec "$HOME/.config/scripts/popup.sh" menu-wide "$0" "$@"
fi

HISTFILE="${CLIPHIST_FILE:-$HOME/.cache/cliphist}"
PINFILE="${CLIPHIST_PINS:-$HOME/.cache/cliphist.pins}"
PLACEHOLDER="<NEWLINE>"

out="$(flex clip "$@")" # cancel (130) passes through via set -e

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: clip "*) kind="select" ;;
    "ACTION:DELETE clip "*) kind="delete" ;;
    "ACTION:TOGGLE clip "*) kind="toggle" ;;
    *) echo "flex-clip: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* clip }"
id="${rest%% *}"
[[ "$id" =~ ^[0-9a-f]+$ ]] || { echo "flex-clip: bad id: $id" >&2; exit 1; }

raw="$(flex clip --resolve "$id")" || { echo "flex-clip: unknown id: $id" >&2; exit 1; }
[[ -n "$raw" ]] || { echo "flex-clip: unknown id: $id" >&2; exit 1; }
preview="$(printf '%s' "$raw" | head -c 50)"

case "$kind" in
    "select")
        # Mirror cliphist.sh flex_on_activate: decode <NEWLINE> back to
        # newlines and copy. printf (not echo) so entries starting with
        # -n/-e still copy literally; the trailing newline matches the
        # old `echo "$raw"` pipeline.
        printf '%s\n' "$raw" | sed "s/$PLACEHOLDER/\n/g" | wl-copy >/dev/null 2>&1
        notify-send -a "Cliphist" "Copied to clipboard" "$preview"
        ;;
    "delete")
        # Mirror cliphist.sh _clip_delete_row: LC_ALL=C + -a so history
        # holding invalid bytes never collapses to "binary file matches"
        # (which would wipe the file on mv — verified 2026-09-15).
        if [[ -f "$HISTFILE" ]]; then
            LC_ALL=C grep -aFxv -- "$raw" "$HISTFILE" > "${HISTFILE}.tmp" || true
            mv "${HISTFILE}.tmp" "$HISTFILE"
        fi
        if [[ -f "$PINFILE" ]]; then
            LC_ALL=C grep -aFxv -- "$raw" "$PINFILE" > "${PINFILE}.tmp" || true
            mv "${PINFILE}.tmp" "$PINFILE"
        fi
        notify-send -a "Cliphist" "Deleted" "$preview"
        ;;
    "toggle")
        # Mirror cliphist.sh flex_on_toggle: m pins/unpins the entry.
        mkdir -p "$(dirname "$PINFILE")"
        [[ ! -f "$PINFILE" ]] && touch "$PINFILE"
        if LC_ALL=C grep -aqFx -- "$raw" "$PINFILE" 2>/dev/null; then
            LC_ALL=C grep -aFxv -- "$raw" "$PINFILE" > "${PINFILE}.tmp" || true
            mv "${PINFILE}.tmp" "$PINFILE"
            notify-send -a "Cliphist" "Unpinned" "$preview"
        else
            printf '%s\n' "$raw" >> "$PINFILE"
            notify-send -a "Cliphist" "Pinned to history" "$preview"
        fi
        ;;
esac
