# flex — Implementation Plan (M0–M6)

Source of truth for staged delivery of the `flex` reusable Rust TUI menu library.
Companion specs: `Docs/project_structure.md`, `Docs/UI_UX_doc.md`, `Docs/Bug_tracking.md`.

## Frozen decisions (do not revisit without a new review)

- `ratatui =0.29.0` (NOT 0.30). Pinned with `=` in `Cargo.toml`, `crossterm` feature only.
  Single `crossterm` major in the tree — verified via `cargo tree` (see M0 gate).
- Hand-rolled fuzzy filter (~150 lines, `flex-core/src/filter.rs`), tiers:
  `prefix 100 > prefix-word 80 > consecutive-run>=3 60 > word-boundary 40 > scattered 10`,
  with gap/offset penalties; ordered-subsequence is REQUIRED for a match.
  Signature kept `nucleo`-swappable (rank fn behind a trait / free-fn seam).
- `unicode-width 0.2` non-CJK display-width handling (`flex-core/src/width.rs`).
- `clap 4` derive + `anyhow` for CLI/errors.
- NO `serde`/`toml` in v1. CI grep check rejects them:
  `! rg -l '"serde"|"toml"|serde::|toml::' flex-core/src flex-rice/src flex-core/tests flex-rice/tests`.
- Q1: bare-digit switches tab iff filter empty, else `Alt-digit`.
- Q2: clipboard `action_id` = content-hash hex (`RowId`, see `project_structure.md`).
- Q3: danger-arm timing — 50 ms confirm delay + 5 s arm expiry
  (`keys::ARM_CONFIRM_DELAY` / `keys::ARM_EXPIRE`; hold/repeat swallowed,
  single-`Enter` never confirms; release-gated by `flex-rice/tests/power.rs`).
- Q4: release profile `panic="abort" strip=true lto=true`.
- Q5: commit `Cargo.lock`; gitignore + stow-ignore `target/`.
- Q6: `q` quits only in NORMAL mode + empty filter (else it edits the filter).
- Q7: 1 s gauge tick; offline state renders dim `— offline`.
- Q8: no `serde`/`toml` in v1 (std-only parsing; CI grep gate rejects them).
- Library never executes side effects; the provider binaries execute the chosen action
  in-process (`exec/*.rs`), and `--print-action` is the only `ACTION:` producer;
  diagnostics go to stderr. Exit codes: `0` action chosen, `130` cancelled, `1` error.

## Tech stack (pinned, with docs)

| Crate | Version | Use | Docs |
|---|---|---|---|
| `ratatui` | `=0.29.0` + `crossterm` | Alt-screen TUI, `TestBackend` golden tests | https://docs.rs/ratatui/0.29.0 |
| `crossterm` | single major via `cargo tree` | `/dev/tty` backend event/terminal control | https://docs.rs/crossterm |
| `clap` | `4` derive | nine entry points: the `flex` dispatcher + eight `flex-<provider>` binaries | https://docs.rs/clap/4 |
| `anyhow` | `1` | Error context in binary/providers | https://docs.rs/anyhow |
| `unicode-width` | `0.2` | `flex-core/src/width.rs` display-width truncation | https://docs.rs/unicode-width/0.2 |
| `criterion` (dev) | `0.5` | `flex-core/benches/rerank.rs` | https://docs.rs/criterion |
| `pretty_assertions` (dev) | `1` | Readable golden diffs | https://docs.rs/pretty_assertions |

MSRV: Rust 1.96 stable. `rustfmt.toml` mirrors wiremix (`max_width=100`).
Lints: `[lints] unsafe deny; clippy all+pedantic deny` (see M0).

## Risk-adjusted ordering

Risk register drove this order — highest-unknown work is pulled earliest:

1. **Clipboard perf spike → pulled into M1** (not M4/M5). Rationale: clipboard
   history can be 10k+ rows; naive re-rank per keystroke or full redraw kills the
   60 fps / <16 ms frame budget. M1 prototypes the corpus + `clip_perf` test +
   `flex-core/benches/rerank.rs` skeleton so M2 providers inherit a proven budget.
2. **Danger-timing + `FLEX_TEST` seed → M1.** Danger rows (`shutdown`, `reboot`,
   `format`) need triple-coding + confirm timing defined before any provider
   ships; `FLEX_TEST` deterministic seed is needed for golden/key tests from day
   one. Both land with the core harness, not later.
3. **Launcher cutover before power cutover (M4).** Rationale: launcher is
   high-frequency + low-blast-radius (worst case: wrong app starts); power menu
   is low-frequency + high-blast-radius (wrong action shuts down the machine).
   Validate the executor + `ACTION:` protocol on the safe surface first.

## Stage overview + estimates

| Stage | Goal | Estimate |
|---|---|---|
| M0 scaffold | Crate, lints, backend shim, golden harness, gates green | 0.5 d |
| M1 core | filter/width/render/keys/theme/backend + spikes | 2–3 d |
| M2 providers | power/launch/clip/center stdout-once parsing | 2 d |
| M3 widgets | gauge tick, offline, shot/theme, center grid | 1–2 d |
| M4 cutover | per-provider executors (`exec/*.rs`), launcher-first cutover table | 1 d |
| M5 perf + hardening | benches, fuzzy corpus, dwidth/clip_perf gates | 1 d |
| M6 release | version, changelog, release profile verify | 0.5 d |

---

## M0 — Scaffold (THIS PHASE)

- [ ] `Cargo.toml`: pinned `ratatui =0.29.0` (`crossterm` feature), `clap` derive,
      `anyhow`, `unicode-width`; dev `criterion` + `pretty_assertions`;
      `[lints] unsafe deny, clippy all+pedantic deny`;
      `[profile.release] lto=true strip=true panic="abort"`.
- [ ] `rustfmt.toml` (`max_width=100`, wiremix mirror), `LICENSE-MIT`, `LICENSE-APACHE`,
      `CHANGELOG.md` skeleton, `README.md` (`ACTION:` protocol + wrapper recipes
      placeholder + cutover table).
- [ ] `flex-core/src/lib.rs`: pub `Row`/`Tab`/`Mode`/`Outcome`/`Theme`/`Provider`/`App` types.
- [ ] `flex-rice/src/main.rs`: `clap` subcommands `power|launch|shot|theme|clip|center`, stubs
      returning exits (no TUI yet).
- [ ] `flex-core/src/backend.rs`: `/dev/tty` alt-screen init/restore via `ratatui::init`/`restore`;
      `ACTION:` single-line stdout writer; exit codes `0/130/1`.
- [ ] Stub modules `filter`/`width`/`render`/`keys`/`providers`/`theme` with `TODO(M1)` markers.
- [ ] `flex-rice/tests/golden.rs`: one passing `TestBackend` test (empty tab bar @80x24).
- [ ] `.gitignore` + `.stow-local-ignore` entries for `target/`.
- [ ] Gates: `cargo fmt --check` && `cargo clippy --all-targets -- -D warnings` &&
      `cargo test` all green; `cargo tree` shows a single `crossterm` major.

## M1 — Core (filter/width/render/keys/theme/backend)

- [x] `flex-core/src/filter.rs`: hand-rolled fuzzy (~150 lines); tiers
      `100/80/60/40/10` + gap/offset penalties; ordered-subsequence required;
      `nucleo`-swappable rank seam. Unit tests for each tier + penalty ordering.
- [x] `flex-core/src/width.rs`: `unicode-width 0.2` truncation/padding; `…` ellipsis;
      CJK out-of-scope (documented); `flex-core/tests/dwidth.rs` fixtures.
- [x] `flex-core/src/theme.rs`: hardcoded `Theme` tokens per `UI_UX_doc.md`
      (`bg #1e1e2e`, `fg #cdd6f4`, `dim #6c7086`, `accent #89b4fa`,
      `selected_bg #313244`, `danger #f38ba8`, `gauge_fill #a6e3a1`).
- [x] `flex-core/src/render.rs`: row column math (`█` col 0 accent outside inversion, label col 2,
      `avail = W-2-1-M-1`, truncation, right dim meta); bare-rows vs standard mode;
      danger triple-code (color + `!!` + text label); gauge + `— offline`.
- [x] `flex-core/src/keys.rs`: key priority chain + Q1 (bare-digit iff filter empty else `Alt-digit`)
      + Q6 (`q` quits only NORMAL+empty filter); `flex-core/tests/keys.rs` state-machine tests.
- [x] `flex-core/src/backend.rs`: real `/dev/tty` open, event poll, 1 s gauge tick (Q7),
      resize handling; `FLEX_TEST` deterministic seed support.
- [x] **Spike (M1): clipboard perf** — 10k-row synthetic corpus, `cargo test clip_perf`
      asserts re-rank + render < frame budget; `flex-core/benches/rerank.rs` skeleton.
- [x] **Danger-timing spec lock** — confirm-hold/step for danger rows defined +
      covered in `flex-core/tests/keys.rs` before any provider uses it.
- [x] `flex-core/tests/fuzzy_corpus.rs`: tier/penalty regression corpus checked in.

## M2 — Providers (parse subprocess stdout once)

- [ ] `flex-rice/src/providers.rs` + `src/providers/{power,launch,clip,center}.rs`:
      each provider spawns at most one subprocess, parses stdout once into `Vec<Row>`,
      then owns filtering/rendering. Library never executes the chosen action.
- [x] `power` (M6 cutover DONE 2026-09-15): static 5-row table
      (`src/providers/power.rs`) with `power-menu.sh` row-set parity
      (order/labels/metas exact: Lock/`hyprlock`, Suspend/`systemctl suspend`,
      Reboot/`systemctl reboot`, Power Off/`systemctl poweroff`,
      Logout/`pkill -SIGTERM Hyprland`; ids are the bash `flex_on_activate`
      case arms `lock|suspend|reboot|poweroff|logout`); `Reboot`/`Power Off`
      danger rows confirmed by the shared `keys` double-Enter flow (no
      duplicated logic); event loop via `run::run` with `--filter-mode`
      support; `main.rs` dispatches `power` (no stubs remain).
      the power executor (SELECT→bash-verbatim power command AFTER
      the TUI exits; `DRY_RUN=1` echoes `would run: …` instead of executing
      — merge gated on dry-run verification given the blast radius).
      Keybinds repointed: `SUPER+M` → the power binary (its old
      `hyprshutdown || hyprctl dispatch exit` is now the Logout row),
      waybar `custom/power` on-click → the power binary; `SHIFT+Esc`
      stays on `kill-menu.sh` (htop-based, out of scope).
      `power-menu.sh` deleted post-parity, then the bash `flex-tui` engine deleted
      (last sourcer gone; `git grep flex-tui` zero functional hits).
      Tests: `flex-rice/tests/power.rs` (18: exact row fixtures, danger key-seq
      replays incl. the single-`Enter`-never-confirms release-gate property,
      49 ms hold swallow, 5 s expiry→re-arm, 80x24 default + armed-danger
      goldens, `FLEX_TEST` determinism, all-5-ids dry-run gate + stubbed-PATH
      live dispatch, malformed/unknown rejection) + 2 provider unit tests.
- [x] `launch` (M2 cutover DONE 2026-09-15): `.desktop` scan
      (`src/providers/launch.rs`) with `app-cache.sh` row-set parity
      (90/90 names, zero drift vs `~/.cache/app-launcher.list`), `Terminal`
      meta, `%X`-preserving `Exec`; row id = space-free hash of the
      desktop-id (`launch::entry_id`) with the hidden `flex launch --resolve`
      lookup the executor uses before launching (B-021 — a `.desktop` file
      may be named `My App.desktop`); event loop (`flex-core/src/run.rs`: poll 1 s,
      `handle_key`, render-per-frame, `tick`, `ACTION:`/`ACTION:DELETE`/quit
      exits, `FLEX_TEST` seeded step); hidden `--filter-mode=spec|legacy`
      escape hatch;       the launch executor (`setsid`/`%X`-strip/
      `kitty -e` semantics matching `launch_app_row`); keybinds repointed,
      `app-launcher.sh` deleted, `app-cache.sh` KEPT (row-set reference;
      its only sourcer `control-center.sh` was deleted in the M5 cutover).
      Tests: `flex-rice/tests/launch.rs` (fixtures, `fir→Enter` replay, Esc chain,
      legacy order, 80x24 golden, seed determinism) + `parity_against_app_cache`
      (ignored, live-cache probe).
- [x] `clip` (M4 cutover DONE 2026-09-15): byte-safe ingest
      (`src/providers/clip.rs`) mirroring `cliphist.sh pick()` cleaning
      (drop `NUL`/`0x1F`/`ESC`, `\t`→space, lossy UTF-8, drop empties —
      Rust argv/pipe only, globs always literal); pins-first (file order,
      deterministic) then history newest-first (`tac`), first-wins dedup;
      `action_id` = content-hash hex (Q2, `RowId`), label = 120-char
      preview, meta = `📌 Pinned`; standard spec rows, `deletable=true`,
      NAVIGATE `m` → `KeyOutcome::Toggle` (`ACTION:TOGGLE`, wrapper flips
      the pin); `CLIPHIST_FILE`/`CLIPHIST_PINS` env overrides (test seam);
      hidden `flex clip --resolve <hash>` for wrapper hash→content lookup;
      event loop via `run::run` with `--filter-mode` support; empty store →
      stderr + exit 130. the clip executor (SELECT→decode+`wl-copy`
      post-TUI with pty redirect; DELETE→`grep -aFxv` both files;
      TOGGLE→pin/unpin) with `sel`/`pin`/`unpin` bash semantics; keybind
      `SUPER+SHIFT+V` repointed, `cliphist.sh pick()` a delegating stub
      (`add`/`pin`/`unpin` intact, verified live). Tests: `flex-rice/tests/clip.rs`
      (22: binary corpus, globs, 120-char preview, pins-first, hashes,
      widths, `m`/Delete flows, filter→Enter replay, 80x24 golden,
      determinism, stubbed-pipeline wrapper tests, live-store parity probe)
      + `flex-rice/tests/clip_perf.rs` cold-ingest gate (see M5).
- [x] `center` (M5 cutover DONE 2026-09-15): 5-tab `Menu` builder
      (`src/providers/center.rs`) with control-center.sh row-set parity
      (tab order, labels/metas, `flex_bar`/gauge formulas machine-checked
      against bash); Launchers reuse the `.desktop` scan via
      `launch::scan_dirs`/`launch::rows` (kind-prefixed ids, standard rows);
      `nmcli`/`bluetoothctl` snapshot parses (tolerant; offline → dim
      `— offline` rows, Q7); power rows delegate danger-arm/confirm to the
      shared `keys` flow (no duplicated logic); Settings gauge rows +
      theme rows (shared `theme_` scan); 1 s tick rewrites gauge
      labels/metas in place only (60-tick soak test: no scroll/filter
      reset, R7); `Row::offline` per-row dim flag (`lib.rs`+`render.rs`,
      additive); the center executor (`SELECT`→exec/connect,
      `TOGGLE`→mute/bt, danger-confirmed `SELECT`→power ops,
      `NMCLI`/`BLUETOOTHCTL`/`WPCTL`/`THEME_SWITCHER`/`FLEX_CENTER_PASSWORD`
      overrides); keybind `SUPER+X` repointed, `control-center.sh` deleted,
      `app-cache.sh` KEPT intentionally (orphaned reference for the row
      set), the bash `flex-tui` engine kept (power-menu/kill-menu/
      wallpaper-picker still need `popup.sh`). Tests: `flex-rice/tests/center.rs` (27:
      fixtures per tab, 125x30 golden + gauge states, key-seq replays,
      stubbed-pipeline wrapper tests) + 7 provider unit tests;
      `tests/fixtures/center/` reference snapshots.
- [ ] Provider golden tests with `fixtures/*.txt` stdout captures.
      (Launch uses `tests/fixtures/launch/*.desktop` + `flex-rice/tests/launch.rs` instead —
      `.desktop` fixtures, not stdout captures.)

## M3 — Widgets + remaining subcommands

- [ ] Gauge widget: 1 s tick rerender, `gauge_fill #a6e3a1`; offline = dim `— offline` (Q7).
- [x] `shot` subcommand (screenshot rows) + `theme` subcommand (theme-switcher
      rows) — cutover DONE 2026-09-15. `shot` (`src/providers/shot.rs`): static
      7-row table in bash `CAP_ROWS` order with exact labels (leading spaces
      + `🖥` preserved) and `PNG`/`MP4` metas; ids are the bash `case` arms
      (`area-shot` … `full-rec-audio`); standard spec rows, non-deletable.
      `theme` (`src/providers/theme_.rs`, std-only JSON scan, no serde):
      sorted `available/` dirs, label = theme name, id = space-free hash of
      it (`theme_::entry_id`; see B-021 — a directory may be named
      `My Theme`), meta = wallpaper
      basename (`(no metadata)`/`unknown` fallbacks) + `  Active` suffix for
      the current theme (bash meta+status join); non-deletable. `main.rs`
      dispatches both through `run::run` with `--filter-mode` support.
      Executors: the shot executor (capture pipeline verbatim from the
      deleted bash, runs AFTER the TUI exits; `SCREENSHOT_DIR` /
      `RECORDING_START` test overrides) + the theme executor
      (`exec theme-switcher.sh activate`; `THEME_SWITCHER` override).
      Keybinds repointed (`SUPER+s` → the shot binary, `SUPER+T` →
      the theme binary); `screenshot.sh` deleted; `theme-switcher.sh pick()`
      is now a delegating stub (`list/current/activate/delete/rofi`
      intact, verified live). Tests: `flex-rice/tests/shot.rs` (10) + `flex-rice/tests/theme.rs`
      (12) — exact row fixtures, golden default view @80x24, key-seq
      replays (navigate/filter → `Enter` → correct `ACTION:`), `Esc`/`Delete`
      chains, `FLEX_TEST` determinism, stubbed-pipeline wrapper tests.
- [x] `center` grid layout @1000x600 (M5 cutover DONE 2026-09-15):
      standard spec rows on all 5 tabs (Launchers keeps its `Terminal`
      meta, not bare-rows); wide-viewport golden @125x30 in
      `flex-rice/tests/center.rs` (5-tab bar + gauge states 0/50/100/muted/offline).
- [ ] `flex-rice/tests/golden.rs` additions: danger row, gauge online/offline, truncation @80x24.

## M4 — Cutover (launcher BEFORE power)

- [x] Per-provider executors (`flex-rice/src/exec/*.rs`): own the side effects
      the six original wrappers introduced, now called in-process by the
      `flex-<provider>` binaries. The retired wrappers parsed the single
      `ACTION:` line; the executors run the same effects directly.
- [x] `clip` cutover DONE (M4, ahead of M4 schedule): the clip executor
      + `pick()` delegating stub + `README.md` cutover table ✅.
- [x] `center` cutover DONE (M5): the center executor
      + `README.md` cutover table ✅ + `control-center.sh` deleted
      (keybind `SUPER+X` repointed). Remaining provider (`power`) still
      landed here in M4, `power` LAST.
- [x] Cutover table in `README.md` (script → provider → status):
      `launch` ✅ (M2), `shot`/`theme` ✅ (M3), `clip` ✅ (M4),
      `center` ✅ (M5), `power` ✅ (M6, LAST).
- [x] Cutover order: `launch` first (safe, high-frequency), then `clip`/`center`/`shot`/`theme`,
      `power` LAST (high-blast-radius). `launch` ✅ (M2), `shot`/`theme` ✅ (M3),
      `clip` ✅ (M4), `center` ✅ (M5), `power` ✅ (M6).
      Post-cutover cleanup (M6): `power-menu.sh` deleted after executor parity
      (dry-run + stubbed dispatch green); the bash `flex-tui` engine deleted after
      its last sourcer went away (zero-hit `git grep` verified worktree-wide).
      Intentionally KEPT (out of scope, documented): `popup.sh` (still exec'd
      by `kill-menu.sh` and the delegating stubs), `app-cache.sh` (orphaned
      row-set reference since M2/M5, kept deliberately),
      `kill-menu.sh` (htop-based, not a flex surface) on `SHIFT+Esc`.
      Later: `wallpaper-picker.sh` became a delegating stub and
      `picker-chrome.sh` was deleted in the M7 wallpaper cutover (below);
      `rofi/scripts/wifi.sh` became a delegating stub in the M8 Wi-Fi
      cutover (below).
      **Subsequently retired (2026-09-16):** all eight wrappers were removed in
      the per-provider-binary migration; their effects live in `exec/*.rs` and
      the `ACTION:` wire protocol left the call path.
- [x] the bash `flex-tui` dispatcher updated; stow packaging verified (`target/` ignored).
      (M6: bash dispatcher deleted — superseded by the `flex` dispatcher and the
      eight `flex-<provider>` binaries; `stow -n flex` dry-run links everything
      except `target/`.)

## M7 — Wallpaper cutover (previews kept)

The wallpaper picker was the last fzf surface (`SUPER+W`) and the only picker
whose value is the image preview, so the cutover had to carry the preview
across rather than drop it.

- [x] `src/providers/wallpaper.rs` DONE: two bash roots
      (`~/Pictures/Wallpapers`, `~/Pictures/Screenshots`; `WALLPAPER_DIRS`
      override), `find -maxdepth 2 -type f -iname` parity (four suffixes,
      symlinks skipped, `sort -u` order), label = basename, meta = source
      directory + `  Active` for the wallpaper in use (`set-wallpaper.sh`'s
      cache file, `hyprpaper.conf` fallback), id = FNV-1a hex of the path,
      hidden `flex wallpaper --resolve <id>` (clip's Q2 pattern, so the
      space-delimited `ACTION:` line stays parseable), `Row::preview_image`
      carrying the path to the pane.
- [x] `flex-core/src/preview.rs` DONE (kitty graphics): `preview::pane` geometry (45 %
      of the frame, right-aligned, one gutter column, `MIN_LIST_WIDTH` 24,
      dropped under 40 columns), `place_escape` (`a=T,f=100,t=f,i=<fixed>,
      c/r, C=1, q=2` with the base64 path as payload), `delete_escape`,
      `park_escape`, and a derived-PNG cache for non-PNG sources
      (`magick`/`convert`, `FLEX_PREVIEW_CONVERT`); `FLEX_PREVIEW=0|1` and
      `FLEX_PREVIEW_CACHE` are the other seams. Verified against kitty 0.47:
      `f` is mandatory (kitty does **not** sniff file-transmission formats),
      `C=1` needs kitty ≥ 0.20, `q=2` keeps APC replies out of the event loop,
      alt-screen entry/exit clears images for free.
- [x] Render/loop wiring DONE: `Menu::preview` + `render::preview_area` +
      `render::cursor_position` (renderer stays ANSI-free; `run` opens a
      second `/dev/tty` handle and paints after the frame flush, then
      re-parks the caret so it never sits in the pane).
- [x] the wallpaper executor DONE (hex-id validation → `--resolve` →
      `[[ -f ]]` → `set-wallpaper.sh`, `SET_WALLPAPER` override; the old
      picker is a delegating stub, `picker-chrome.sh` deleted).
- [x] `SUPER+W` repointed to the wallpaper binary; `flex-rice/tests/wallpaper.rs` (21) +
      preview/wallpaper unit tests (20) + the entrypoints contract.

## M8 — Wi-Fi dialog cutover (network popup)

Waybar's `network` module was the last non-flex popup: its `on-click` ran
`rofi/scripts/wifi.sh`, which had been rewritten to `exec nmtui` — a command
this host does not have (`rpm -q NetworkManager-tui` → not installed, no
sudo), so the "dialog" only printed an install hint (B-015). The cutover
makes the dialog a flex provider instead of a package dependency.

- [x] `src/providers/wifi.rs` DONE: radio state from `nmcli radio wifi`
      (`enabled` → `Turn Wi-Fi Off`; anything else → the single
      `Turn Wi-Fi On` row, since a down radio cannot scan), `Disconnect from
      {ssid}` when a scan reports `IN-USE` `*`, then one row per network (id
      `wifi`, SSID in the escaped label — space-safe, the `center` contract).
      Metas come from `center::wifi_meta_body` (the bash-exact body without
      the `select` renderer's `◇ ` prefix, which is meaningless in standard
      mode), so the row set cannot drift between the two surfaces.
      Degradation reuses the `center` rules verbatim: failed `nmcli`/no
      device → one dim `— offline` row, empty scan → `(No Wi-Fi networks)`,
      and the shared `center::snapshot` seam helper.
- [x] Standard rows (`bare_rows = false`): unlike the `center` `Networks`
      tab, the signal/security meta **is** the surface, so it must render.
      Filterable (rofi `-dmenu` parity), never deletable.
- [x] the wifi executor DONE (`menu` popup variant): radio on/off,
      `device disconnect`, open-network connect, `/dev/tty` password prompt
      for secured ones (`FLEX_WIFI_PASSWORD` seam), selecting the connected
      network drops it (rofi parity). `NMCLI`/`NOTIFY_SEND` overrides for
      tests.
- [x] Waybar `network.on-click` → the wifi binary;
      `rofi/scripts/wifi.sh` reduced to a delegating stub (external callers
      keep working) — `wifi.rasi`/`wifi-prompt.rasi` are now unused assets.
- [x] `flex-rice/tests/wifi.rs` (32) + the entrypoints contract: row-set fixtures,
      offline/empty/radio-off degradation, live `$WIFI_*` seam path,
      cached-first opening (placeholder + scan swap, focus identity across a
      reorder, filter survival, idle tick is a no-op), key-seq replays, 80x24
      golden, stubbed executor dispatch (incl. bad/unknown ids refused and
      cancel passing 130 through untouched).
- [x] Latency: a triggered `nmcli` scan blocks ~3 s, which put the popup on a
      blank terminal until it returned. `wifi::menu` now builds the first frame
      from the **cached** scan (`--rescan no`, 10 ms) and runs the real scan on
      a worker thread; `Menu::tick` → `wifi::refresh_scan` swaps its rows in,
      keeping the cursor on the same SSID. Cold cache → dim `Scanning…` row
      instead of a false `(No Wi-Fi networks)`; the executor's connect probe
      reads the cache too, so Enter does not wait for a second scan. Measured
      live in a pty with a stubbed slow scan: first frame 22 ms, fresh rows
      3.0 s (scan + one tick).
- [x] Metas are laid out in fixed sub-columns (`wifi::net_meta`): the renderer
      right-aligns one string per row, so equal widths are what line the fields
      up — signal padded to `NNN%`, fixed-width bar, security padded to the
      widest class in the list, reserved `Connected` column. Lock glyphs are
      Nerd Font private-use icons (single cell, the deleted rofi picker's),
      not colour emoji. Executor feedback fixed with it: the password prompt is
      printed explicitly (it used to go to `/dev/null` via `read -p`, leaving
      the popup blank while it waited), an empty answer says so instead of
      exiting silently, and a `Connecting to <ssid>…` line covers the connect.
- [x] Saved networks connect from their stored profile: the wifi executor's
      `is_saved()` matches an `802-11-wireless` profile in `nmcli … connection
      show` (name = SSID, split on the last colon so `My\:Net` works) and only
      asks for a password when no profile exists or the stored credentials are
      rejected — saying so first. The picker shows the state in the meta's own
      column (`Connected`/`Saved`/blank, `wifi::state_marker`), so the prompt is
      never a surprise; `wifi::Snapshot` carries the four reads (radio,
      devices, scan, profiles).
- [x] Live verification (real `nmcli`, `gom3a-5g`): the wrapper runs in a pty,
      renders the radio/disconnect/network rows with signal+security metas in
      36 ms, `Esc` exits 130, and `nmcli radio wifi` stays `enabled` (no side
      effects on cancel).

## M5 — Perf + hardening

- [ ] `flex-core/benches/rerank.rs` (criterion): 10k-row rerank regression; budget documented.
- [x] `flex-rice/tests/clip_perf.rs`: automated perf gate (fails on budget breach) —
      M4-enforced (was M1 spike): cold 3200-row file ingest → first frame
      @80x24 ≤ 0.5 s in debug AND release (release measured 6.4 ms).
- [ ] `flex-core/tests/dwidth.rs` + `flex-core/tests/fuzzy_corpus.rs` expanded; `cargo test` full matrix green.
- [ ] CI grep check: reject `serde`/`toml` (`rg` gate in CI + documented here).
- [ ] Clippy pedantic + fmt clean; `cargo tree` single-crossterm re-verified.

## M6 — Release

- [x] `CHANGELOG.md` filled; version bump (`1.0.0`); `README.md` cutover table all ✅.
      Freeze decisions recorded above (Q1 digit rule, Q2 hash, Q3 5s+50ms,
      Q4 abort+strip, Q6 q-quit, Q7 1 s tick, Q8 no serde).
- [x] Release profile verify: `cargo build --release` + `strip`/`lto`/`panic=abort`
      effective (binary size + `panic=abort` smoke check).
      M6 result: `cargo test --release` 184/184 green (1 ignored live probe);
      `target/release/flex` = 984K (`strip=true lto=true panic="abort"` in
      `Cargo.toml`; `panic=abort` smoke = release suite passing under the
      abort profile with no unwinding paths exercised).
- [x] `Cargo.lock` committed; tag; post-cutover cleanup (remove superseded rofi scripts
      only after soak + explicit approval).
      M6 result: `Cargo.lock` present (commit on maintainer approval — no
      commits/tags made by this session); v1 tag proposed `flex-v1.0.0`
      (NOT created — no tags without request); cleanup done per M4 note above.
      **2026-09-15 follow-up:** the `rofi` stow package itself is now gone —
      `stow -D rofi` + `rm -rf rofi/`, the `rofi` arm of `theme-switcher.sh`
      removed, and the `$HOME/.config/rofi/colors.rasi` symlink entries dropped
      from `generate-theme.sh`/`theme-switcher.sh` so no theme switch can
      recreate the directory. RASI generation was dropped with it
      (`generate_rasi()`, `generate-static-theme.sh`'s heredoc, the saved-theme
      file list, and `theme-switcher.sh`'s theme-validity check), and every
      saved theme's `colors.rasi`/`theme.rasi` was deleted.
- [x] M6 hardening extras: `flex-core/tests/keys.rs` seed table 12 → 29 cases
      (`TODO(M6)` closed; shifted runes, `Left`/`Right` wrap matrix,
      out-of-range digits, gaugeless `F1`, mark persistence, `Ctrl-w` shapes,
      empty-view keys + empty-app/empty-tab targeted tests); render-path
      audit (code audit, no `tput`/`stty`/subprocess in
      `render`/`filter`/`width`/`keys`/`theme` — sole `Command::new` is the
      center snapshot at provider load, once up front per convention; power
      rows are fully static); `DRY_RUN=1` dry-run mode added to
      the power executor and gated (all 5 ids dry-run to exact bash commands,
      stub log proves nothing executes); full hygiene
      (`fmt --check` + `clippy --all-targets -D warnings` + debug AND
      release suites green); `shellcheck` re-checked 2026-09-15 — still
      absent (B-004 stayed open at that time; `bash -n` + `zsh -n` +
      stubbed-pipeline tests covered the then-live power wrapper).

---

## Test matrix

| Suite | File | What it gates |
|---|---|---|
| Golden (TestBackend) | `flex-rice/tests/golden.rs` | Empty tab bar, danger, gauge on/offline, truncation @80x24 |
| Entry-point contract | `flex-rice/tests/entrypoints.rs` | `--help`/`--version` on all nine binaries |
| Error prefix | `flex-rice/tests/prefix.rs` | Exactly one `flex: error:` across the provider binaries |
| Key state machine | `flex-core/tests/keys.rs` | Q1 digit/Alt-digit, Q6 q-quit, danger confirm timing (29-case `FLEX_TEST` seed table + empty-app safeties) |
| Power cutover | `flex-rice/tests/power.rs` | Bash-exact rows, single-Enter-never-confirms gate, hold/expiry replays, armed-danger golden, executor dry-run + dispatch |
| Fuzzy corpus | `flex-core/tests/fuzzy_corpus.rs` | Tier ordering 100/80/60/40/10, penalties, subsequence-required |
| Display width | `flex-core/tests/dwidth.rs` | Truncation/pad, `…`, non-CJK widths |
| Clip perf | `flex-rice/tests/clip_perf.rs` | 10k rerank+render budget |
| Wallpaper cutover | `flex-rice/tests/wallpaper.rs` | Bash-exact scan (`-maxdepth 2 -iname` + `sort -u`), hash id ↔ `--resolve` round trip, pane geometry/goldens, key-seq replays, stubbed executor dispatch (bad/unknown ids refused) |
| Wi-Fi dialog cutover | `flex-rice/tests/wifi.rs` | Radio/scan row set (incl. offline, empty scan, radio off), live `$WIFI_*` seam path, cached-first open + background rescan swap (placeholder, focus identity across a reorder, filter survival, idle tick no-op), filter/navigate/Esc replays, 80x24 standard-mode golden, stubbed `nmcli` dispatch (open/secure/connected-toggle/noop/cancel + bad/unknown ids refused) |
| Bench | `flex-core/benches/rerank.rs` | Criterion regression signal |
| Lints/fmt/tree | CI gates | `fmt --check`, `clippy -D warnings`, single crossterm, no serde/toml grep |

Viewport matrix for all golden tests: `640x420` + `1000x600` @ `font_size 10`
(see `UI_UX_doc.md`); terminal grid reference `80x24` for `TestBackend` goldens.
