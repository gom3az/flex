#!/usr/bin/env bash
# flex-shot.sh — wrapper for `flex shot` (M3 shot cutover).
# Reads the single ACTION: line AFTER flex exits, then runs the capture.
# slurp/grim need a clean screen, so the popup must close before capture runs.
# The capture is detached via setsid so killing the popup doesn't kill it.
set -euo pipefail

command -v flex >/dev/null 2>&1 || export PATH="$HOME/.local/bin:$PATH"

if [[ "${POPUP_KITTY:-}" != 1 ]]; then
    exec "$HOME/.config/scripts/popup.sh" menu "$0" "$@"
fi

out="$(flex shot "$@")"

IFS= read -r line <<< "$out"
case "$line" in
    "ACTION: shot "* | "ACTION:DELETE shot "*) ;;
    *) echo "flex-shot: unexpected output: $line" >&2; exit 1 ;;
esac
rest="${line#* shot }"
id="${rest%% *}"
[[ "$id" != */* && "$id" != *$'\n'* && -n "$id" ]] || { echo "flex-shot: bad id: $id" >&2; exit 1; }

save_dir="${SCREENSHOT_DIR:-$HOME/Pictures/Screenshots}"
mkdir -p "$save_dir"
rec_start="${RECORDING_START:-$HOME/.config/waybar/recording-start.sh}"
timestamp=$(date +%Y-%m-%d_%H-%M-%S)

# Write capture script to a temp file — setsid bash can't define functions.
cap_script=$(mktemp /tmp/flex-shot-cap.XXXXXX.sh)
trap "rm -f '$cap_script'" EXIT

cat > "$cap_script" <<CAPSCRIPT
#!/usr/bin/env bash
case "\$1" in
    "area-shot")
        set +e
        geometry=\$(slurp) && grim -g "\$geometry" "\$2"
        wl-copy < "\$2" >/dev/null 2>&1
        notify-send "Screenshot saved" "\$2"
        ;;
    "full-shot")
        grim "\$2"
        wl-copy < "\$2" >/dev/null 2>&1
        notify-send "Screenshot saved" "\$2"
        ;;
    "win-shot")
        grim -g "\$(hyprctl -j activewindow | jq -r '\"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"')" "\$2"
        wl-copy < "\$2" >/dev/null 2>&1
        notify-send "Screenshot saved" "\$2"
        ;;
    "area-rec")
        set +e
        geometry=\$(slurp) && "\$4" -g "\$geometry" "\$2"
        ;;
    "area-rec-audio")
        set +e
        geometry=\$(slurp) && "\$4" -a -g "\$geometry" "\$2"
        ;;
    "full-rec")
        "\$4" "\$2"
        ;;
    "full-rec-audio")
        "\$4" -a "\$2"
        ;;
    *) echo "flex-shot: unknown id: \$1" >&2; exit 1 ;;
esac
CAPSCRIPT

chmod +x "$cap_script"

case "$id" in
    "area-shot")
        filepath="${save_dir}/Screenshot-${timestamp}.png"
        ;;
    "full-shot"|"win-shot")
        filepath="${save_dir}/Screenshot-${timestamp}.png"
        ;;
    "area-rec"|"area-rec-audio"|"full-rec"|"full-rec-audio")
        filepath="${save_dir}/Recording-${timestamp}.mp4"
        ;;
esac

# Detach capture — runs in its own session, survives popup death.
setsid --fork bash "$cap_script" "$id" "$filepath" "$rec_start" &
cap_pid=$!

# Close popup window so slurp/grim get a clean screen.
popup_pid=$(ps -eo pid,args | grep 'kitty --class kitty-menu' | grep -v grep | awk '{print $1}' | head -1)
[[ -n "$popup_pid" ]] && kill "$popup_pid" 2>/dev/null || true

# Wait for popup window to disappear.
for _ in {1..20}; do
    hyprctl clients -j 2>/dev/null | python3 -c "
import sys, json
cls = json.load(sys.stdin)
for c in cls:
    if 'kitty-menu' in str(c.get('class','')):
        sys.exit(0)
sys.exit(1)
" 2>/dev/null && sleep 0.05 || break
done

wait "$cap_pid" || true
