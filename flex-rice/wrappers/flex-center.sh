#!/usr/bin/env bash
# flex-center.sh — wrapper for `flex center` (M5 control-center cutover).
# Reads the single ACTION: line AFTER flex exits (zsh-safe `IFS= read -r`),
# then performs the side effect exactly like control-center.sh's
# flex_on_activate/on_toggle (launch exec, wifi connect/disconnect with
# inline password prompt, bt connect/disconnect, power ops, volume mute,
# theme activate). Danger rows (Reboot/Power Off) are confirmed UI-side by
# the shared double-Enter flow before SELECT ever arrives here.
# Test overrides: NMCLI, BLUETOOTHCTL, WPCTL, THEME_SWITCHER (commands),
# FLEX_CENTER_PASSWORD (skips the /dev/tty password prompt).
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

nmcli_cmd="${NMCLI:-nmcli}"
bt_cmd="${BLUETOOTHCTL:-bluetoothctl}"
wpctl_cmd="${WPCTL:-wpctl}"
theme_switcher="${THEME_SWITCHER:-$HOME/.config/scripts/theme-switcher.sh}"

out="$(flex center "$@")" # cancel (130) passes through via set -e

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: center "*) kind="select" ;;
    "ACTION:TOGGLE center "*) kind="toggle" ;;
    "ACTION:DELETE center "*) kind="delete" ;;
    *) echo "flex-center: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* center }"
id="${rest%% *}"
label_esc="${rest#* }"

# Center rows are never data: DELETE (armed via the Settings tab's
# deletable flag, which exists only to enable `m` → TOGGLE) is a no-op.
if [[ "$kind" == "delete" ]]; then exit 0; fi

[[ "$id" != */* && "$id" != *$'\n'* && -n "$id" ]] || { echo "flex-center: bad id: $id" >&2; exit 1; }

# Unescape the ACTION: label (`\\` → `\`, `\n` → newline). Patterns are
# deliberately UNQUOTED: inside double quotes `\\n` would collapse to a
# quoted `n` and match every "n". Placeholder mirrors control-center.sh.
center_unescape() {
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

do_launch() { # $1 = desktop id — launch_app_row semantics (app-cache.sh)
    local desk_id="$1" desk=""
    [[ "$desk_id" != */* && -n "$desk_id" ]] || { echo "flex-center: bad desktop id" >&2; exit 1; }
    local dir
    for dir in "$HOME/.local/share/applications" "/usr/share/applications"; do
        if [[ -f "$dir/$desk_id" ]]; then desk="$dir/$desk_id"; break; fi
    done
    [[ -n "$desk" ]] || { echo "flex-center: $desk_id not found" >&2; exit 1; }
    local exec="" term="false" key value
    while IFS='=' read -r key value; do
        case "$key" in
            Exec) [[ -z "$exec" ]] && exec="$value" ;;
            Terminal) term="$value" ;;
        esac
    done < <(grep -E '^(Exec|Terminal)=' "$desk")
    exec="$(printf '%s' "$exec" | sed -E 's/ %[A-Za-z]//g; s/^ *//; s/ *$//')"
    [[ -n "$exec" ]] || { echo "flex-center: empty Exec in $desk_id" >&2; exit 1; }
    if [[ "$term" == "true" ]]; then
        # shellcheck disable=SC2086
        eval 'setsid -f kitty -e '"$exec"' </dev/null >/dev/null 2>&1'
    else
        # shellcheck disable=SC2086
        eval 'setsid -f '"$exec"' </dev/null >/dev/null 2>&1'
    fi
}

do_mute() { # volume mute toggle (SELECT and TOGGLE on `vol`)
    "$wpctl_cmd" set-mute @DEFAULT_AUDIO_SINK@ toggle 2>/dev/null || true
}

do_bt_toggle() { # $1 = MAC — connect/disconnect on fresh `info` state
    local mac="$1"
    [[ -n "$mac" ]] || { echo "flex-center: empty MAC" >&2; exit 1; }
    if "$bt_cmd" info "$mac" 2>/dev/null | grep -q 'Connected: yes'; then
        "$bt_cmd" disconnect "$mac" 2>/dev/null || true
    else
        "$bt_cmd" connect "$mac" 2>/dev/null || true
    fi
}

do_wifi() { # $1 = SSID — fresh-state port of the bash `wifi)` arm
    local ssid="$1" wif
    wif="$("$nmcli_cmd" -t -f DEVICE,TYPE device 2>/dev/null | awk -F: '$2=="wifi"{print $1; exit}')" || true
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
    done < <("$nmcli_cmd" -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname "$wif" 2>/dev/null || true)
    if [[ "$row_status" == "Connected" ]]; then
        "$nmcli_cmd" device disconnect "$wif" 2>/dev/null || true
        notify-send -a "Control Center" "Disconnected" "$ssid" 2>/dev/null || true
    elif [[ -z "$row_sec" || "$row_sec" == "--" ]]; then
        # Unknown (stale label) also lands here: treated as open, like an
        # empty `$sec` in bash.
        if "$nmcli_cmd" device wifi connect "$ssid" ifname "$wif" 2>/dev/null; then
            notify-send -a "Control Center" "Connected" "$ssid" 2>/dev/null || true
        else
            true
        fi
    else
        local pw="${FLEX_CENTER_PASSWORD:-}"
        if [[ -z "$pw" ]]; then
            pw=""
            IFS= read -rsp "Password for $ssid: " pw </dev/tty 2>/dev/null || pw=""
            echo >&2
        fi
        [[ -z "$pw" ]] && exit 0
        if "$nmcli_cmd" device wifi connect "$ssid" password "$pw" ifname "$wif" 2>/dev/null; then
            notify-send -a "Control Center" "Connected" "$ssid" 2>/dev/null || true
        else
            notify-send -a "Control Center" "Failed" "$ssid" 2>/dev/null || true
        fi
    fi
}

do_power() { # $1 = pwkind — bash power arms verbatim
    case "$1" in
        pwlock) hyprlock ;;
        pwsuspend) systemctl suspend ;;
        pwreboot) systemctl reboot ;;
        pwoff) systemctl poweroff ;;
        pwlogout) pkill -SIGTERM Hyprland ;;
        *) echo "flex-center: unknown power action: $1" >&2; exit 1 ;;
    esac
}

label="$(center_unescape "$label_esc")"

case "$kind" in
    select)
        case "$id" in
            launch:*)
                # The id is a space-free row hash, not a file name (B-021):
                # resolve it to the desktop-id before touching the disk.
                desk_id="$(flex launch --resolve "${id#launch:}")" || {
                    echo "flex-center: unknown launch id: ${id#launch:}" >&2
                    exit 1
                }
                do_launch "$desk_id"
                ;;
            wifi) do_wifi "$label" ;;
            bt:*) do_bt_toggle "${id#bt:}" ;;
            pwlock|pwsuspend|pwreboot|pwoff|pwlogout) do_power "$id" ;;
            vol) do_mute ;; # no bash gauge-activate; Enter mutes (documented)
            bright) exit 0 ;; # no bash gauge-activate; ±5% adjust is a v1 gap
            theme)
                name="${label#Theme: }"
                [[ -n "$name" ]] || { echo "flex-center: empty theme name" >&2; exit 1; }
                exec "$theme_switcher" activate "$name"
                ;;
            noop) exit 0 ;;
            *) echo "flex-center: unknown action: $id" >&2; exit 1 ;;
        esac
        ;;
    toggle)
        case "$id" in
            vol) do_mute ;;
            bt:*) do_bt_toggle "${id#bt:}" ;;
            *) exit 0 ;; # brightness/theme TOGGLEs: no bash equivalent
        esac
        ;;
esac
