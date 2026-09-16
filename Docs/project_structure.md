# flex — Project Structure

Conventions for this repo — the flex TUI menu workspace. Read before
running commands, creating files/folders, structural changes, or adding
dependencies.

## Where the two halves live

| Half | Carried by | Publishable |
|---|---|---|
| **`flex-core`** — the engine: menu rendering, fuzzy filtering, key handling, the design system, kitty-graphics previews | this repo, `flex-core/` | Yes |
| **`flex-rice`** — this rice's nine providers, their executors, the `flex` dispatcher, the `flex-<provider>` binaries and the `flex-record` helper | this repo, `flex-rice/` | No (`publish = false`) |

Dependencies run one way (`flex-rice` → `flex-core`, a **path** dependency —
no tags, no `[patch]` overrides). `flex-core` must never gain a
machine-specific path back into `flex-rice`. The engine's only reach into a
consumer's providers is the `Menu::on_tick` / `TickHook` seam: the engine
owns the tick, the caller supplies the refresh.
`flex-rice::tick_hook` refreshes `center` gauges, picks up a finished `wifi`
scan and re-sweeps the `proc` process list, and `flex-rice::menu(provider,
tabs)` installs it — **use `menu()` instead of `Menu::new` inside `flex-rice`**,
or those providers silently stop refreshing.

## Crate layout

```text
flex/                        # cargo workspace root (two members)
  Cargo.toml                 # members, shared deps, lints, release profile
  Cargo.lock                 # COMMITTED (Q5) — single lockfile for both members
  rustfmt.toml               # mirror wiremix, max_width=100
  LICENSE-MIT
  LICENSE-APACHE
  CHANGELOG.md
  README.md                  # entry points, dispatcher, probe, env seams

  flex-core/                 # the reusable engine — no machine-specific paths
    Cargo.toml               # publishable; repository points at this repo
    src/                     # lib/render/filter/keys/theme/backend/run/…
    tests/                   # compliance, dropdown, keys, dwidth, fuzzy_corpus
    benches/rerank.rs        # criterion rerank regression

  flex-rice/                 # this rice's glue — machine-specific, never published
    Cargo.toml               # publish = false; [[bin]] flex + provider binaries + flex-record
    src/
      lib.rs                 # pub mod exec/providers/runner/popup/terminal/spawn; menu()/tick_hook()
      main.rs                # `flex` dispatcher: popup toggle, provider/verb re-exec
      runner.rs              # shared flow: popup_guard → build_menu → run_capture → exec
      popup.rs               # popup classes, in-popup detection, toggle helper
      terminal.rs            # $TERMINAL detection + popup spawn argv
      spawn.rs               # ETXTBSY-retrying process helpers (RetryExec)
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
        proc.rs              # /proc process list for the native kill menu
      exec/                  # one executor per provider (the ported side effects)
        mod.rs power.rs launch.rs shot.rs theme.rs
        clip.rs center.rs wallpaper.rs wifi.rs proc.rs record.rs
      bin/                   # thin entry points over runner
        flex-power.rs flex-launch.rs flex-shot.rs flex-theme.rs
        flex-clip.rs flex-center.rs flex-wallpaper.rs flex-wifi.rs
        flex-proc.rs flex-record.rs
    tests/
      golden.rs              # TestBackend goldens (empty tab bar M0; +danger/gauge/trunc M3)
      entrypoints.rs         # --help/--version contract for all binaries
      prefix.rs              # single `flex: error:` prefix across the provider binaries
      center.rs              # per-tab fixtures, gauge tick, TARGET/Action reporting
      clip.rs                # history rows, resolve round-trip, real-binary E2E
      clip_perf.rs           # (M1 spike, gated M5) 10k-row perf budget
      power.rs               # DRY_RUN gate + stubbed-PATH executor dispatch
      shot.rs                # stubbed capture-pipeline executor dispatch
      theme.rs               # theme rows + switcher executor dispatch
      wallpaper.rs           # (M7) scan parity, pane geometry, executor dispatch
      wifi.rs                # (M8) radio/scan rows, live seams, executor dispatch
      fixtures/              # captured subprocess stdout per provider (M2)
        center/
        launch/
        wifi/                # radio + nmcli snapshots for the network dialog (M8)
```

## Conventions

1. **The engine never executes side effects.** `flex-core` only produces an
   `Outcome` (selected `Row` + `action_id`). Process spawn/exec, clipboard
   writes, shutdown, etc. live exclusively in `flex-rice/src/exec/*.rs`, which
   the provider binaries call in-process.
2. **Exit codes are the contract.** The provider binaries select a row and
   execute its effect in Rust; all diagnostics go to stderr via
   `eprintln!`/`anyhow`. Exit codes: `0` = action executed, `130` = cancel,
   `1` = error (the contract lives in `flex-core/src/backend.rs`).
   `--print-action` is the only remaining producer of an `ACTION:` line: it
   prints the selected line and exits without executing.
   **Empty providers have one policy (B-026):** a chooser that found nothing
   shows a single `noop` placeholder row (`providers::empty_row`, shared id
   `providers::NOOP_ID`) and keeps the TUI, so the menu is never blank and
   `Enter` on it is a no-op (the executor short-circuits the `noop` id); a
   provider that cannot offer any action at all (`clip` without history,
   `wallpaper` without images) diagnoses on stderr and exits `130` before
   initialising the terminal.
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
   `action_id = hex(blake/simple-hash(content))` — stable across runs so the
   store round-trips. `wallpaper` reuses it over the absolute
   path, since paths contain spaces. `launch` and `theme` follow the same
   rule over the desktop-id / theme name (`launch::entry_id`,
   `theme_::entry_id`): a `.desktop` file or theme directory may be
   named `My App.desktop`/`My Theme`, and the `ACTION:` id token is
   whitespace-delimited, so the raw name can never be the id (B-021). A
   provider that puts a free-text value in the id must therefore hash it and
   ship a resolver — the executors call the library resolver functions
   (`clip::resolve`, `wallpaper::resolve`, `launch::resolve_id`,
   `theme_::resolve_name`) in-process; `wifi` is the one exception, and it
   keeps the SSID in the escaped *label* instead.
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
(479 tests). Keep the dependency direction (`flex-rice` → `flex-core`) and
never add a machine-specific path to `flex-core` — that is what keeps it
publishable.

## Commands (run from the repo root unless noted)

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` — the whole workspace
- `cargo build --release` → `target/release/{flex,flex-power,…}` (the eleven binaries)
- `cargo tree -i crossterm` (single-major check)
- `cargo bench -p flex-core --bench rerank`
- Negative-dependency gate: `! rg -l '"serde"|"toml"' flex-core/src flex-rice/src`

## Dotfiles integration

The live machine consumes this repo, not the other way round. The checkout
lives at `~/projects/flex`; dotfiles references it through a stable
`~/.local/bin` symlink farm so the next move touches symlinks, not configs:

- `~/.local/bin/flex` → `<checkout>/target/release/flex` (the dispatcher).
- `~/.local/bin/flex-<provider>` → `<checkout>/target/release/flex-<provider>`
  (one per provider, plus `flex-record`; every Hyprland bind, Waybar on-click
  and delegating script references the farm, never the checkout path).
  `setup.sh` creates the eleven links and `setup.sh --check` asserts they
  resolve.
