#!/usr/bin/env bash
# flex-power.sh — wrapper for `flex power` (M6 power cutover).
# Reads the single ACTION: line AFTER flex exits (zsh-safe `IFS= read -r`),
# then runs the power command exactly like the deleted
# `waybar/.config/waybar/power-menu.sh` flex_on_activate arms (hyprlock,
# systemctl suspend/reboot/poweroff, pkill Hyprland). Danger rows (Reboot /
# Power Off) are confirmed UI-side by the shared double-Enter flow before
# SELECT ever arrives here — this script never re-prompts.
# Test/verify override: DRY_RUN=1 echoes the would-be command
# (`would run: …`) instead of executing it. Given the blast radius
# (wrong action shuts down the machine), the merge is gated on dry-run
# verification: every action id must dry-run to its exact bash command
# (see tests/power.rs).
set -euo pipefail

# `flex` lives in ~/.local/bin (symlink to the release build), which
# Hyprland bind execs don't have on PATH. Only prepend when unresolved so
# deliberately-stubbed PATHs (tests) keep priority.
command -v flex >/dev/null 2>&1 || export PATH="$HOME/.local/bin:$PATH"

# Re-launch inside a floating popup when invoked from a bind (same pattern
# as kill-menu.sh): the TUI needs a real tty, which plain exec doesn't give.
if [[ "${POPUP_KITTY:-}" != 1 ]]; then
    exec "$HOME/.config/scripts/popup.sh" menu "$0" "$@"
fi

dry_run="${DRY_RUN:-0}"

out="$(flex power "$@")" # cancel (130) passes through via set -e

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: power "* | "ACTION:DELETE power "*) ;;
    *) echo "flex-power: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* power }"
id="${rest%% *}"
[[ "$id" != */* && "$id" != *$'\n'* && -n "$id" ]] || { echo "flex-power: bad id: $id" >&2; exit 1; }

# Blast-radius gate: DRY_RUN=1 prints instead of executing.
run() {
    if [[ "$dry_run" == "1" ]]; then
        printf 'would run:'
        printf ' %s' "$@"
        printf '\n'
    else
        "$@"
    fi
}

case "$id" in
    lock) run hyprlock ;;
    suspend) run systemctl suspend ;;
    reboot) run systemctl reboot ;;
    poweroff) run systemctl poweroff ;;
    logout) run pkill -SIGTERM Hyprland ;;
    *) echo "flex-power: unknown action: $id" >&2; exit 1 ;;
esac
