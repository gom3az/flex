# flex — Project Structure

Conventions for this repo — the flex TUI menu workspace. Read before
running commands, creating files/folders, structural changes, or adding
dependencies.

## Where the two halves live

| Half | Carried by | Publishable |
|---|---|---|
| **`flex-core`** — the engine: menu rendering, fuzzy filtering, key handling, the design system, kitty-graphics previews | this repo, `flex-core/` | Yes |
| **`flex-rice`** — this rice's eight providers, the `flex` binary and the shell wrappers | this repo, `flex-rice/` | No (`publish = false`) |

Dependencies run one way (`flex-rice` → `flex-core`, a **path** dependency —
no tags, no `[patch]` overrides). `flex-core` must never gain a
machine-specific path back into `flex-rice`. The engine's only reach into a
consumer's providers is the `Menu::on_tick` / `TickHook` seam: the engine
owns the tick, the caller supplies the refresh.
`flex-rice::tick_hook` refreshes `center` gauges and picks up a finished `wifi`
scan, and `flex-rice::menu(provider, tabs)` installs it — **use `menu()`
instead of `Menu::new` inside `flex-rice`**, or those two providers silently stop
refreshing.

## Crate layout

```text
flex/                        # cargo workspace root (two members)
  Cargo.toml                 # members, shared deps, lints, release profile
  Cargo.lock                 # COMMITTED (Q5) — single lockfile for both members
  rustfmt.toml               # mirror wiremix, max_width=100
  LICENSE-MIT
  LICENSE-APACHE
  CHANGELOG.md
  README.md                  # ACTION: protocol, wrapper recipes, cutover table

  flex-core/                 # the reusable engine — no machine-specific paths
    Cargo.toml               # publishable; repository points at this repo
    src/                     # lib/render/filter/keys/theme/backend/run/…
    tests/                   # compliance, dropdown, keys, dwidth, fuzzy_corpus
    benches/rerank.rs        # criterion rerank regression

  flex-rice/                 # this rice's glue — machine-specific, never published
    Cargo.toml               # publish = false; [[bin]] name = "flex"
    src/
      lib.rs                 # pub mod providers + re-exported menu()/tick_hook()
      main.rs                # clap power|launch|shot|theme|clip|center|wallpaper|wifi; prints ACTION:
      providers.rs           # module list + the per-tick refresh dispatcher
      providers/
        power.rs
        launch.rs
        clip.rs
        center.rs
        shot.rs
        theme_.rs            # `theme` is a crate-adjacent ident; file uses trailing underscore
        wallpaper.rs         # image scan + kitty-graphics preview rows (M7)
        wifi.rs              # radio/scan rows for the network dialog (M8)
    tests/
      golden.rs              # TestBackend goldens (empty tab bar M0; +danger/gauge/trunc M3)
      center.rs              # per-tab fixtures, gauge tick, TARGET/Action reporting
      clip.rs                # history rows, resolve round-trip, real-binary E2E
      clip_perf.rs           # (M1 spike, gated M5) 10k-row perf budget
      power.rs               # DRY_RUN gate + stubbed-PATH dispatch
      shot.rs                # stubbed pipeline dispatch
      theme.rs               # theme rows + switcher dispatch
      wallpaper.rs           # (M7) scan parity, pane geometry, wrapper dispatch
      wifi.rs                # (M8) radio/scan rows, live seams, wrapper dispatch
      wrappers.rs            # bind-path contract: executable + popup-wrapped
      fixtures/              # captured subprocess stdout per provider (M2)
        center/
        launch/
        wifi/                # radio + nmcli snapshots for the network dialog (M8)
    wrappers/
      flex-power.sh flex-launch.sh flex-clip.sh flex-center.sh
      flex-shot.sh flex-theme.sh flex-wallpaper.sh flex-wifi.sh
```

### Why `flex-rice/wrappers/` never moves

The wrapper directory sits exactly where the Hyprland binds, Waybar
on-clicks and delegating scripts expect it (via the `~/.local/bin`
symlink farm — see "Dotfiles integration" below). Moving it breaks every
config reference at once. That is not hypothetical: `04e2599` in the old
dotfiles history moved the wrappers one level and took out all seven
keybinds.

## Conventions

1. **The engine never executes side effects.** `flex-core` only produces an
   `Outcome` (selected `Row` + `action_id`). Process spawn/exec, clipboard
   writes, shutdown, etc. live exclusively in `wrappers/*.sh` (M4), which parse
   the single `ACTION:` stdout line.
2. **Binary prints exactly one `ACTION:` line** to stdout on success
   (`ACTION: <provider> <action_id> <escaped-label>`). All diagnostics go to
   stderr via `eprintln!`/`anyhow`. Exit codes: `0` = action, `130` = cancel,
   `1` = error (the contract lives in `flex-core/src/backend.rs`).
   **Empty providers have one policy (B-026):** a chooser that found nothing
   shows a single `noop` placeholder row (`providers::empty_row`, shared id
   `providers::NOOP_ID`) and keeps the TUI, so the menu is never blank and
   `Enter` on it is a no-op in every wrapper; a provider that cannot offer any
   action at all (`clip` without history, `wallpaper` without images)
   diagnoses on stderr and exits `130` before initialising the terminal.
3. **Providers parse subprocess stdout once.** At most one child spawn per
   invocation; read stdout to `Vec<Row>` up front; filter/render in-process after
   that. No re-spawning per keystroke. Two documented exceptions, both driven by
   the `TickHook` seam: `center`'s 1 s gauge tick re-reads `wpctl`/`brightnessctl`
   in place (Q7), and `wifi` opens from the cached scan and runs **one**
   background scan whose rows replace the list on a later tick (a triggered
   `nmcli` scan blocks ~3 s, which would leave the popup blank until it returned).
3b. **Image previews are the one out-of-band surface** (`flex-core/src/preview.rs`).
   The `wallpaper` provider's pane is painted after each frame with kitty
   graphics protocol escapes written to a second `/dev/tty` handle (never
   through the ratatui buffer, so `render` stays ANSI-free and goldens stay
   pixel-exact), and non-PNG sources are converted once into
   `$XDG_CACHE_HOME/flex/previews` by ImageMagick (cached by path+mtime+size).
   Both are display-only: no provider row, id, label, filter or `ACTION:` line
   depends on them, and every failure (no kitty, no converter, unwritable
   cache) degrades to a blank pane plus one stderr line.
4. **`RowId` hash hex for clipboard (Q2).** `clip` rows use
   `action_id = hex(blake/simple-hash(content))` — stable across runs so wrappers
   can round-trip history entries. `wallpaper` reuses it over the absolute
   path and adds a hidden `--resolve` lookup, since paths contain spaces.
   `launch` and `theme` follow the same rule over the desktop-id / theme
   name (`launch::entry_id`, `theme_::entry_id`) with `flex launch --resolve`
   and `flex theme --resolve`: a `.desktop` file or theme directory may be
   named `My App.desktop`/`My Theme`, and the `ACTION:` id token is
   whitespace-delimited, so the raw name can never be the id (B-021). A
   provider that puts a free-text value in the id must therefore hash it and
   ship a resolver — `wifi` is the one exception, and it keeps the SSID in
   the escaped *label* instead.
   (Hash fn: std-only in v1, no extra deps.)
5. **`target/` gitignored.** Root `.gitignore` carries `/target`; never commit
   build artifacts. `Cargo.lock` IS committed (Q5) — the single lockfile for
   both members.
6. **No `serde`/`toml`.** CI grep gate rejects them:
   `! rg -l '"serde"|"toml"|serde::|toml::' flex-core/src flex-rice/src flex-core/tests flex-rice/tests`.
   Config is CLI flags + hardcoded `Theme` only.
7. **Deterministic tests via `FLEX_TEST`.** When `FLEX_TEST=1`, RNG/time seeds are
   fixed (M1) so golden/key tests are reproducible.
8. **Lints deny by default.** `[workspace.lints]` in the root `Cargo.toml`
   (`unsafe_code` deny; clippy all+pedantic deny) with `[lints] workspace = true`
   in `flex-rice`; `flex-core` carries the identical denies inline.
   `cargo clippy --all-targets -- -D warnings` must pass.
9. **Formatting:** `cargo fmt --all --check` green; `rustfmt.toml max_width=100`.
10. **Single `crossterm` major.** `ratatui 0.29` pulls `crossterm`; no other dep may
    pull a second major. Verify with `cargo tree -i crossterm` / `cargo tree | rg crossterm`.

## Working on the engine

There is no dance: the engine lives at `flex-core/` in this repo. Change it
and its consumers in one commit; one `cargo test` covers both sides
(345 tests). Keep the dependency direction (`flex-rice` → `flex-core`) and
never add a machine-specific path to `flex-core` — that is what keeps it
publishable.

## Commands (run from the repo root unless noted)

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` — the whole workspace (345 tests: 236 rice + 109 engine)
- `cargo build --release` → `target/release/flex` (what the wrappers exec)
- `cargo tree -i crossterm` (single-major check)
- `cargo bench -p flex-core --bench rerank`
- Negative-dependency gate: `! rg -l '"serde"|"toml"' flex-core/src flex-rice/src`

## Dotfiles integration

The live machine consumes this repo, not the other way round. The checkout
lives at `~/projects/flex`; dotfiles references it through a stable
`~/.local/bin` symlink farm so the next move touches symlinks, not configs:

- `~/.local/bin/flex` → `<checkout>/target/release/flex` (the binary;
  wrappers resolve it by name, falling back to `~/.local/bin` on PATH).
- `~/.local/bin/flex-<provider>.sh` → `<checkout>/flex-rice/wrappers/…`
  (one per provider; every Hyprland bind, Waybar on-click and delegating
  script references the farm, never the checkout path).
