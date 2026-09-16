#!/usr/bin/env bash
# flex-wifi.sh — wrapper for `flex wifi` (M8 network-dialog cutover).
# Reads the single ACTION: line AFTER flex exits (zsh-safe `IFS= read -r`),
# then performs the side effect: radio on/off, disconnect, or connect (open
# networks connect directly; secured ones prompt on the tty). Row ids are
# `off|on|disconnect|wifi|noop`; a network's SSID travels in the escaped
# label (SSIDs may contain spaces, so they cannot be the id token) — the
# same contract as flex-center.sh's `wifi` arm.
# Test overrides: NMCLI, NOTIFY_SEND (commands), FLEX_WIFI_PASSWORD (skips
# the /dev/tty password prompt).
set -euo pipefail

# `flex` lives in ~/.local/bin (symlink to the release build), which
# Hyprland bind execs don't have on PATH. Only prepend when unresolved so
# deliberately-stubbed PATHs (tests) keep priority.
command -v flex >/dev/null 2>&1 || export PATH="$HOME/.local/bin:$PATH"

# Re-launch inside a floating popup when invoked from Waybar/bind (same
# pattern as kill-menu.sh): the TUI needs a real tty, which plain exec
# doesn't give. `menu` = the compact 640x420 dialog this replaces.
if [[ "${POPUP_KITTY:-}" != 1 ]]; then
    exec "$HOME/.config/scripts/popup.sh" menu "$0" "$@"
fi

nmcli_cmd="${NMCLI:-nmcli}"
notify_cmd="${NOTIFY_SEND:-notify-send}"

out="$(flex wifi "$@")" # cancel (130) passes through via set -e

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: wifi "*) ;;
    *) echo "flex-wifi: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* wifi }"
id="${rest%% *}"
label_esc="${rest#* }"
[[ "$id" != */* && "$id" != *$'\n'* && -n "$id" ]] || { echo "flex-wifi: bad id: $id" >&2; exit 1; }

# Unescape the ACTION: label (`\\` → `\`, `\n` → newline). Patterns are
# deliberately UNQUOTED: inside double quotes `\\n` would collapse to a
# quoted `n` and match every "n". Placeholder mirrors flex-center.sh.
wifi_unescape() {
    local text="$1" ph=$'\x01'
    text=${text//\\\\/$ph}
    text=${text//\\n/$'\n'}
    text=${text//$ph/\\}
    printf '%s' "$text"
}

# Undo nmcli -t escaping (`\\` → `\`, `\:` → `:`) for SSID comparison.
nmcli_unescape() {
    local text="$1" ph=$'\x01'
    text=${text//\\\\/$ph}
    text=${text//\\:/:}
    text=${text//$ph/\\}
    printf '%s' "$text"
}

notify() { "$notify_cmd" -a "Wi-Fi" "$@" 2>/dev/null || true; }

# Post-TUI status line. stderr is the popup's terminal (kitty hands the child
# the pty as stdio), and flex has already left the alternate screen, so this
# is what the user sees while a connect runs.
status() { printf '%s\n' "$*" >&2; }

wifi_iface() { # the first `*:wifi` device, or empty
    "$nmcli_cmd" -t -f DEVICE,TYPE device 2>/dev/null | awk -F: '$2=="wifi"{print $1; exit}' || true
}

# Whether a saved Wi-Fi profile exists for $1. NetworkManager names a profile
# after the SSID it was created for, so the name is the SSID; TYPE is never
# colon-free-name confusion: split against the LAST colon, because a name with
# a colon arrives escaped (`My\:Net`).
is_saved() {
    local want="$1" line name_esc kind
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        name_esc="${line%:*}"; kind="${line##*:}"
        [[ "$kind" == "802-11-wireless" ]] || continue
        [[ "$(nmcli_unescape "$name_esc")" == "$want" ]] && return 0
    done < <("$nmcli_cmd" -t -f NAME,TYPE connection show 2>/dev/null || true)
    return 1
}

# Connect using the stored profile / no credentials (open network).
connect_without_password() {
    "$nmcli_cmd" device wifi connect "$1" ifname "$2" 2>/dev/null
}

connect_with_password() {
    "$nmcli_cmd" device wifi connect "$1" password "$2" ifname "$3" 2>/dev/null
}

do_connect() { # $1 = SSID — fresh-state connect/disconnect for one network
    local ssid="$1" wif
    wif="$(wifi_iface)"
    [[ -n "${wif:-}" ]] || exit 0 # interface vanished since the snapshot
    local line inuse rest sec signal ssid_esc candidate row_sec="" row_status=""
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        inuse="${line%%:*}"; rest="${line#*:}"
        sec="${rest##*:}"; rest="${rest%:*}"
        signal="${rest##*:}"; ssid_esc="${rest%:*}"
        candidate="$(nmcli_unescape "$ssid_esc")"
        if [[ "$candidate" == "$ssid" ]]; then
            row_sec="$sec"
            [[ "$inuse" == "*" ]] && row_status="Connected"
            break
        fi
    # `--rescan no` reads NetworkManager's cache: the picker just scanned (its
    # background rescan), and triggering another scan would add ~3 s between
    # Enter and the connect. IN-USE/security are live state, not scan results.
    done < <("$nmcli_cmd" -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname "$wif" --rescan no 2>/dev/null || true)
    if [[ "$row_status" == "Connected" ]]; then
        # Selecting the network you are on drops it (the rofi picker's
        # behavior, kept here so the row is never a dead end).
        "$nmcli_cmd" device disconnect "$wif" 2>/dev/null || true
        notify "Disconnected" "$ssid"
        return 0
    fi
    local pw="${FLEX_WIFI_PASSWORD:-}"
    if [[ -z "$row_sec" || "$row_sec" == "--" ]]; then
        # Unknown (stale label) also lands here: treated as open, like an
        # empty `$sec` in the bash picker.
        status "Connecting to $ssid…"
        if connect_without_password "$ssid" "$wif"; then
            notify "Connected" "$ssid"
        else
            notify "Failed" "$ssid"
        fi
        return 0
    fi
    # A saved profile means NetworkManager already holds the credentials:
    # asking for the password again is wrong, so connect from the profile
    # first and only ask when that fails (e.g. the network's key changed).
    if is_saved "$ssid"; then
        status "Connecting to $ssid…"
        if connect_without_password "$ssid" "$wif"; then
            notify "Connected" "$ssid"
            return 0
        fi
        status "Saved credentials for $ssid were rejected — enter the password."
    fi
    if [[ -z "$pw" ]]; then
        # The prompt is printed HERE, not via `read -p`: `read -p` writes to
        # stderr, and an early cut sent that stderr to /dev/null, so the popup
        # sat blank and looked hung while it waited for a password.
        printf 'Password for %s: ' "$ssid" >&2
        pw=""
        IFS= read -rs pw </dev/tty 2>/dev/null || IFS= read -rs pw || pw=""
        printf '\n' >&2
    fi
    if [[ -z "$pw" ]]; then
        # Never fail silently: an empty answer (or the blind Enter that a
        # missing prompt invites) must say so instead of closing the popup
        # with nothing done.
        printf 'No password entered — not connecting to %s.\n' "$ssid" >&2
        exit 0
    fi
    status "Connecting to $ssid…"
    if connect_with_password "$ssid" "$pw" "$wif"; then
        notify "Connected" "$ssid"
    else
        notify "Failed" "$ssid"
    fi
}

label="$(wifi_unescape "$label_esc")"

case "$id" in
    on) "$nmcli_cmd" radio wifi on 2>/dev/null || true ;;
    off) "$nmcli_cmd" radio wifi off 2>/dev/null || true ;;
    disconnect)
        wif="$(wifi_iface)"
        [[ -n "${wif:-}" ]] || exit 0
        "$nmcli_cmd" device disconnect "$wif" 2>/dev/null || true
        notify "Disconnected" "${label#Disconnect from }"
        ;;
    wifi) do_connect "$label" ;;
    noop) exit 0 ;;
    *) echo "flex-wifi: unknown action: $id" >&2; exit 1 ;;
esac
