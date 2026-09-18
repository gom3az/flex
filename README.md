# flex — reusable Rust TUI menu library

`flex` powers the dotfiles popup menus (`power`, `launch`, `clip`, `center`,
`shot`, `theme`, `wallpaper`, `wifi`, `proc`, `mixer`, `net`, `bt`, `notify`).
Every provider is a Rust binary that renders its menu and executes the selected
row **in-process** — the retired shell wrappers and the `ACTION:` wire protocol
are no longer on the call path.

## Workspace layout

Both halves of flex live in this repo as workspace members:

| Crate | Where it lives | Publishable |
|---|---|---|
| `flex-core` | This repo, `flex-core/`: the engine — menu/list rendering, fuzzy filtering, key handling, the design system, kitty-graphics previews. No machine-specific paths. | Yes |
| `flex-rice` | This repo, `flex-rice/`: the providers, their executors (`exec/`), the `flex` dispatcher, the `flex-<provider>` binaries and the `flex-record` helper. Reads `~/.config/themes`, `hyprpaper.conf`, `~/.cache/cliphist`, `/proc`, PipeWire (`wpctl`), NetworkManager (`nmcli`), BlueZ (`bluetoothctl`), MPRIS players, and FreeDesktop D-Bus notifications. | No (`publish = false`) |

Dependencies run one way (`flex-rice` → `flex-core`). The engine's only former
reach into providers is now a seam: `Menu::on_tick` takes a `TickHook`, and
`flex-rice` supplies the one that refreshes `center` gauges, picks up a finished
`wifi` scan, polls `proc`/`net`/`bt`, and updates `notify` live items —
build menus in this repo with `flex_rice::menu(…)`, which installs it.

```sh
cargo test                       # the whole workspace
cargo build --release            # → target/release/{flex,flex-power,…}
cargo clippy --locked --all-targets -- -D warnings
```

To iterate on the engine locally, patch it in without committing the override —
see `Docs/project_structure.md` → "Working on the engine".

[gom3az/flex-core]: https://github.com/gom3az/flex (retired; the engine now lives in this repo at `flex-core/`)

## Entry points

Binaries built from `flex-rice`: the `flex` dispatcher, per-provider binaries,
and the `flex-record` helper.

| Binary | Provider | What it does |
|---|---|---|
| `flex` | dispatcher | `flex popup …`, `flex <provider> [verb] [args…]` |
| `flex-power` | power | Shutdown/reboot/logout menu: `hyprlock`, `systemctl suspend\|reboot\|poweroff`, `pkill -SIGTERM Hyprland` |
| `flex-profile` | profile | Power-profile menu |
| `flex-launch` | launch | Application launcher: scans `.desktop` entries and detaches the chosen app with `setsid -f` (`$TERMINAL -e` for `Terminal=true`) |
| `flex-shot` | shot | Screenshot/recording flow: `slurp`, `grim`, `wl-copy`, `notify-send`, or the `flex-record` helper (`RECORDING_START` overrides) |
| `flex-theme` | theme | Theme switcher: scans `~/.config/themes/available` and activates the selection in-process; `list`/`current`/`activate`/`delete` verbs (`$THEME_SWITCHER` overrides with `<switcher> activate <name>`) |
| `flex-clip` | clip | Clipboard history: restore a selection to clipboard with `wl-copy`, delete it, pin/unpin it; `add`/`pin`/`unpin`/`current`/`watch` (alias: `daemon`) verbs |
| `flex-center` | center | Control center: volume/brightness/network/bluetooth/power/theme tabs |
| `flex-wallpaper` | wallpaper | Wallpaper picker with a kitty-graphics preview pane; sets the selection in-process (hyprpaper socket + `hyprpaper.conf`); `set <path>` verb (`$SET_WALLPAPER` overrides with `<setter> <path>`) |
| `flex-wifi` | wifi | Wi-Fi picker: radio on/off, disconnect, connect (saved profile or password prompt) |
| `flex-proc` | proc | Native process manager: filter `/proc`, Enter = SIGTERM, Delete = SIGKILL, `m` = stop/continue |
| `flex-record` | — | Recording helper: `[-a] [-g GEOM] FILE` (start), `status`, `stop` |
| `flex-mixer` | mixer | Toggles a `wiremix` (PipeWire TUI) floating terminal window via `pgrep`/`pkill`; not a flex TUI menu |
| `flex-net` | net | Network interface and bandwidth telemetry monitor |
| `flex-bt` | bt | Bluetooth device manager, pairing, and battery status monitor |
| `flex-notify` | notify | Notification Center Drawer (`-m`), Waybar JSON polling (`--status`), CLI verbs (`send`/`clear-all`/`toggle-dnd`), and background D-Bus daemon (`daemon`) with audio cues |

Every provider binary runs the same shared flow:

```text
runner::popup_guard → runner::build_menu → flex_core::run::run_capture → exec::<provider>::execute
```

`runner` (`flex-rice/src/runner.rs`) owns the popup re-exec guard, menu
construction, the select loop and the exit mapping; each `exec::<provider>`
module (`flex-rice/src/exec/*.rs`) owns that provider's side effects. They run
**in-process** — nothing parses stdout and no shell is involved.

## The `flex` dispatcher

`flex` (`flex-rice/src/main.rs`) is a compat dispatcher over the provider
binaries:

- `flex popup <menu|menu-wide> <cmd…>` toggles (or spawns) the popup running
  `cmd` (the helper dotfiles scripts such as the mixer popup use).
- `flex <provider> [verb] [args…]` re-execs the sibling `flex-<provider>`
  binary, reconstructing the global flags (`-s/-t/-p/--filter-mode`) in
  canonical order, so `flex -t nocolor launch` ≡ `flex launch -t nocolor`.
  Verb-bearing providers accept the non-interactive verbs (`flex clip add`,
  `flex theme list`, `flex wallpaper set <path>`).
- `flex net` and `flex notify` inject `-m` into the re-exec argv
  automatically (monitor-mode / drawer-mode flag).

The executors resolve row ids in-process via the library resolver functions
(`flex_rice::providers::{clip,wallpaper,launch,theme_}`), so no lookup is a
separate CLI step.

## Env seams

| Variable | Provider | Purpose |
|---|---|---|
| `NMCLI` | wifi, center | `nmcli` tool override |
| `BLUETOOTHCTL`, `WPCTL` | center | Tool overrides |
| `NOTIFY_SEND` | wifi | Notification tool override (`center` hardcodes `notify-send`) |
| `THEME_SWITCHER` | theme, center | Theme-activation override; unset/empty runs the in-process activator |
| `SET_WALLPAPER` | wallpaper | Wallpaper-setting override; unset/empty runs the in-process setter |
| `DRY_RUN` | power, profile | When exactly `1`, print `would run: <cmd>` instead of executing |
| `FLEX_WIFI_PASSWORD`, `FLEX_CENTER_PASSWORD` | wifi, center | Skip the `/dev/tty` password prompt |
| `SCREENSHOT_DIR`, `RECORDING_START` | shot | Capture output dir / recording helper override (defaults to `flex-record`) |
| `CLIPHIST_FILE`, `CLIPHIST_PINS`, `CLIPHIST_CURRENT` | clip | History, pins and current-entry store overrides |
| `FLEX_PROC_KTHREADS` | proc | Show kernel threads (empty cmdline) in the process list |
| `FLEX_PROC_SORT` | proc | Sort column override (default: memory) |
| `FLEX_PROC_EXPAND` | proc | Show full cmdline args when `all` or `1` |
| `WALLPAPER_DIRS` | wallpaper | Colon-separated scan root override |
| `WALLPAPER_STATE` | wallpaper | Active-wallpaper state file override |
| `POWER_PROFILE_FILE` | profile | Profile state file override |
| `FLEX_PREVIEW` | wallpaper | Force-enable kitty graphics preview (`1`) |
| `FLEX_PREVIEW_CACHE` | wallpaper | Override derived-PNG cache directory |
| `FLEX_RECORD_INFO` | record | Recording registry path override (default `$XDG_RUNTIME_DIR/flex-record.info`) |
| `TERMINAL` | popups | Terminal used to host a popup (see below) |

## Popups

Popups use the window classes `flex-menu` (compact variant: power/shot/theme/
wifi/bt/profile), `flex-menu-wide` (wide variant: launch/clip/center/wallpaper/proc/net),
and `flex-notify-center` (right-side drawer).
Toggle is keyed on the **variant**, not the provider, so opening `wifi` while
the `power` popup is up closes it instead of stacking. The hosting terminal
comes from `$TERMINAL`; an unknown or empty value warns once on stderr and
falls back to kitty, never exits `1` (`flex-rice/src/popup.rs`,
`flex-rice/src/terminal.rs`).

`flex-mixer` does not use a popup variant — it toggles a `wiremix` floating
terminal window directly.

## `setup.sh`

`setup.sh` symlinks the release binaries from `target/release/` into
`~/.local/bin`; `setup.sh --check` is the gate that they resolve to
executables.

## Notification Daemon (`flex-notify`)

`flex-notify` includes a full `zbus` D-Bus notification server (`flex-notify --daemon`) handling `org.freedesktop.Notifications`. To run it as a systemd user daemon:

```bash
# 1. Systemd service (~/.config/systemd/user/flex-notify.service)
# 2. D-Bus activation (~/.local/share/dbus-1/services/org.freedesktop.Notifications.service)

systemctl --user daemon-reload
systemctl --user enable --now flex-notify.service
```
