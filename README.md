# flex — reusable Rust TUI menu library

`flex` powers the dotfiles popup menus (`power`, `launch`, `clip`, `center`,
`shot`, `theme`, `wallpaper`, `wifi`, `proc`). Every provider is a Rust binary
that renders its menu and executes the selected row **in-process** — the
retired shell wrappers and the `ACTION:` wire protocol are no longer on the
call path.

## Workspace layout

Both halves of flex live in this repo as workspace members:

| Crate | Where it lives | Publishable |
|---|---|---|
| `flex-core` | This repo, `flex-core/`: the engine — menu/list rendering, fuzzy filtering, key handling, the design system, kitty-graphics previews. No machine-specific paths. | Yes |
| `flex-rice` | This repo, `flex-rice/`: the nine providers, their executors (`exec/`), the `flex` dispatcher and the `flex-<provider>` binaries. Reads `~/.config/themes`, `hyprpaper.conf`, `~/.cache/cliphist`, `/proc` and ML4W's wallpaper cache. | No (`publish = false`) |

Dependencies run one way (`flex-rice` → `flex-core`). The engine's only former
reach into providers is now a seam: `Menu::on_tick` takes a `TickHook`, and
`flex-rice` supplies the one that refreshes `center` gauges and picks up a
finished `wifi` scan — build menus in this repo with `flex_rice::menu(…)`,
which installs it.

```sh
cargo test                       # the whole workspace
cargo build --release            # → target/release/{flex,flex-power,…}
cargo clippy --locked --all-targets -- -D warnings
```

To iterate on the engine locally, patch it in without committing the override —
see `Docs/project_structure.md` → "Working on the engine".

[gom3az/flex-core]: https://github.com/gom3az/flex (retired; the engine now lives in this repo at `flex-core/`)

## Entry points

Eleven binaries are built from `flex-rice`: the `flex` dispatcher, one binary
per provider, and the `flex-record` helper.

| Binary | Provider | What it does |
|---|---|---|
| `flex` | dispatcher | `flex popup …`, `flex <provider> [verb] [args…]` |
| `flex-power` | power | Shutdown/reboot/logout menu: `hyprlock`, `systemctl suspend\|reboot\|poweroff`, `pkill -SIGTERM Hyprland` |
| `flex-launch` | launch | Application launcher: scans `.desktop` entries and detaches the chosen app with `setsid -f` (`$TERMINAL -e` for `Terminal=true`) |
| `flex-shot` | shot | Screenshot/recording flow: `slurp`, `grim`, `wl-copy`, `notify-send`, or the `flex-record` helper (`RECORDING_START` overrides) |
| `flex-theme` | theme | Theme switcher: scans `~/.config/themes/available` and activates the selection in-process; `list`/`current`/`activate`/`delete` verbs (`$THEME_SWITCHER` overrides with `<switcher> activate <name>`) |
| `flex-clip` | clip | Clipboard history: `wl-copy` a selection, delete it, pin/unpin it; `add`/`pin`/`unpin`/`current` verbs |
| `flex-center` | center | Control center: volume/brightness/network/bluetooth/power/theme tabs |
| `flex-wallpaper` | wallpaper | Wallpaper picker with a kitty-graphics preview pane; sets the selection in-process (hyprpaper socket + `hyprpaper.conf`); `set <path>` verb (`$SET_WALLPAPER` overrides with `<setter> <path>`) |
| `flex-wifi` | wifi | Wi-Fi picker: radio on/off, disconnect, connect (saved profile or password prompt) |
| `flex-proc` | proc | Native process manager: filter `/proc`, Enter = SIGTERM, Delete = SIGKILL, `m` = stop/continue |
| `flex-record` | — | Recording helper: `[-a] [-g GEOM] FILE` (start), `status`, `stop` |

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

The executors resolve row ids in-process via the library resolver functions
(`flex_rice::providers::{clip,wallpaper,launch,theme_}`), so no lookup is a
separate CLI step.

## `--print-action` probe

`--print-action` on a provider binary prints the selected `ACTION:` line and
exits **without executing** — an end-to-end probe of the real binary's
row→action mapping with no pty. It is the only remaining producer of an
`ACTION:` line; nothing consumes one.

## Exit codes

- `0` — the action executed (or a probe printed its line)
- `130` — the user cancelled (`Esc`/`q`), or an empty `clip`/`wallpaper` store
- `1` — an error

All diagnostics go to **stderr**. `flex: error:` is printed exactly once, by
`flex_rice::runner::fail`; messages below it carry no `flex:` prefix of their
own (B-022/B-027).

## Zero bash, not zero exec

There is no shell in the flex call path: the binaries select a row and execute
it in Rust. Two deliberate **subprocess passthroughs** remain, forced by
`unsafe_code = "deny"` (no libc `setsid`/`termios`):

- `setsid -f` to detach a session (launching an app, spawning the `shot`
  capture worker);
- `stty -echo` around the secured-Wi-Fi password prompt.

These are direct `Command` spawns of the named tools, not shell invocations.

## Env seams

| Variable | Provider | Purpose |
|---|---|---|
| `NMCLI`, `BLUETOOTHCTL`, `WPCTL` | wifi, center | Tool overrides |
| `NOTIFY_SEND` | wifi, center | Notification tool override |
| `THEME_SWITCHER` | theme, center | Theme-activation override; unset/empty runs the in-process activator |
| `SET_WALLPAPER` | wallpaper | Wallpaper-setting override; unset/empty runs the in-process setter |
| `DRY_RUN` | power | When exactly `1`, print `would run: <cmd>` instead of executing |
| `FLEX_WIFI_PASSWORD`, `FLEX_CENTER_PASSWORD` | wifi, center | Skip the `/dev/tty` password prompt |
| `SCREENSHOT_DIR`, `RECORDING_START` | shot | Capture output dir / recording helper override (defaults to `flex-record`) |
| `CLIPHIST_FILE`, `CLIPHIST_PINS`, `CLIPHIST_CURRENT` | clip | History, pins and current-entry store overrides |
| `FLEX_PROC_KTHREADS` | proc | Show kernel threads (empty cmdline) in the process list |
| `FLEX_RECORD_INFO` | record | Recording registry path override (default `/tmp/recording.info`) |
| `TERMINAL` | popups | Terminal used to host a popup (see below) |

## Popups

Popups use the window classes `flex-menu` (compact variant: power/shot/theme/
wifi) and `flex-menu-wide` (wide variant: launch/clip/center/wallpaper/proc).
Toggle is keyed on the **variant**, not the provider, so opening `wifi` while
the `power` popup is up closes it instead of stacking. The hosting terminal
comes from `$TERMINAL`; an unknown or empty value warns once on stderr and
falls back to kitty, never exits `1` (`flex-rice/src/popup.rs`,
`flex-rice/src/terminal.rs`).

## `setup.sh`

`setup.sh` symlinks the eleven release binaries from `target/release/` into
`~/.local/bin`; `setup.sh --check` is the gate that all eleven resolve to
executables.

## Image previews (`wallpaper`)

`flex wallpaper` is the only provider with a preview pane. `render` reserves
the right 45 % of the list area (dropped under 40 columns); `run` paints the
focused row's `Row::preview_image` there with kitty graphics protocol escapes
after every frame (`flex-core/src/preview.rs`) — no fzf, no icat, no image crate. PNG
sources are transmitted straight from disk (`t=f`); other formats are
converted once into `$XDG_CACHE_HOME/flex/previews` with ImageMagick
(`FLEX_PREVIEW_CONVERT`) and the PNG is reused afterwards. kitty stretches an
image to fill whatever `c`×`r` box it is given, so flex computes an
aspect-correct box from the PNG's `IHDR` dimensions and the terminal's cell
size (`TIOCGWINSZ`, via crossterm) and centres the image in the pane. Without a
kitty-compatible terminal (`FLEX_PREVIEW=1` overrides the detection) or with a
failing converter, the pane simply stays blank and the list still works.
