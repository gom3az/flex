# flex — Optimization Plan (OPT-1 … OPT-11)

Source of truth for staged performance / size / efficiency work.
Companion specs: `Docs/Implementation.md`, `Docs/project_structure.md`.
Frozen decisions from `Implementation.md` still hold (ratatui `=0.29.0`, no serde/toml in v1, `lto/strip/panic=abort`, clippy all+pedantic deny, `cargo fmt --check` gate).

How to use: work top-down by phase. Each item has file:line evidence, concrete tasks, acceptance gate, and verification command. Check boxes as you land them.

Global gates (every OPT):
- `cargo test --locked` green
- `cargo clippy --locked --all-targets -- -D warnings` green
- `cargo fmt --all --check` green

---

## Phase A — Hot path: filter + visible rows (highest payoff, lowest risk)

### OPT-1 — Fuzzy `score()` per-row fold + double alloc
Evidence: `flex-core/src/filter.rs:123-124` (`folded_needle`/`folded_hay` inside `score()`), `filter.rs:232-234` (`lowercase_chars` = `String` + `Vec<char>`), `filter.rs:241-243` (`is_subsequence` duplicates), called via `filter.rs:194-218` → `lib.rs:745-746`. Inner scan `filter.rs:147-154`, second scan `prefix_word_start:269-273`. Corpus: `flex-core/benches/rerank.rs:15` (3200 rows).

- [x] Fold `needle` once per `rank_all` / `rank_all_with`; pass `&[char]` into ranker
- [x] Score haystack streaming (`chars().flat_map(to_lower)`) without materializing `Vec<char>`; reuse `positions` buffer across rows
- [x] Keep tier contract intact: `prefix 100 > prefix-word 80 > run>=3 60 > boundary 40 > scattered 10`, subsequence REQUIRED, gap/offset penalties unchanged
- [x] Add per-query benches (`"a"` worst-case, `"zzz-no-match"`, mixed) + `throughput`; keep `rerank_3200_mixed`

Gate: bench time/keystroke down on 3200-row corpus; no golden/rank test changes.
Verify: `cargo bench -p flex-core --bench rerank`, `cargo test -p flex-core`

### OPT-2 — `visible_rows()` clone + 4–5× recompute per frame
Evidence: `flex-core/src/lib.rs:731-749` (`filter.clone()`, `RowId clone` for key, `Vec clone` on hit + on store), call sites `render.rs:463,877,1101`, `lib.rs:757-759,764,776-779`.

- [x] Return shared hits (`Rc<[usize]>` / `Arc`) instead of cloning `Vec<usize>` on hit and store
- [x] Probe cache with borrowed key (`&str` compare, no `String`/`RowId` clone)
- [x] Compute once per frame in `render()` and thread through `draw_list` / `draw_dropdown` / `draw_filter` / `visible_len` / focus helpers
- [x] Document cache-invalidation rule (pointer+len+first-id currently misses on realloc — keep behavior, do not silently broaden)

Gate: ~5× fewer `filter String` + `Vec<usize>` allocs/frame under perf test; identical visible output.
Verify: `cargo test -p flex-core`, manual frame alloc count via `dhat`/counter test if available

---

## Phase B — Render + text measurement

### OPT-3 — Width/sanitize recomputed per cell per frame
Evidence: `flex-core/src/width.rs:95-100` (`measure` allocates `tokens` + `clusters`), `width.rs:140-169` (`sanitize` → `String`, `truncate_exact` re-measures). Callers `render.rs:337,606,679,690,726,807,818,910,936,1013,1128,1195,1247`.

Landed: `CachedText` (`flex-core/src/lib.rs:71-115`) memoizes sanitized text + `Measured` per field with content validation; single-pass `sanitize_measured` (`width.rs:260`); draw paths use `truncate_measured_cow` (`render.rs:1326,1408`); help text measured once (`help_measured:1246`, `help_content_width:197`); `FrameCtx` threads the visible view through draw helpers (`render.rs:336-347`).

- [x] Precompute `Measured` + sanitized text once at `Row`/`Tab` build time
- [x] Use `truncate_measured` (`width.rs:179-205`) in all draw paths; no `measure`/`sanitize` inside `draw_*`
- [x] Cache per-tab `content_width` (help), per-header target width

Gate: per-frame cost becomes O(visible chars) not O(total chars); zero `sanitize` allocs in `draw_list` flame.
Verify: `cargo test -p flex-core`, add `str_width/sanitize/draw_list` microbench

### OPT-4 — Full redraw + per-node layout + draw allocs
Evidence: `flex-core/src/render.rs:303-316` (`fill_bg` full-screen memset via API), per-node `Layout::default().split()` at `render.rs:475-482,563-569,733-745,768-789,831-838`, per-row `repeat()` allocs at `render.rs:853-856`, plus `format!/clone` at `render.rs:352,357,430,435,713-716,726,844,938,1095,1125,1127,1188-1193,996,1207`.

Partial: full-screen `fill_bg` dropped (no `fill_bg` in `render.rs`; background relies on `Buffer` diff — see `render.rs:336-347`); `FrameCtx` computes the visible view + node metrics + marker widths once per frame (`render.rs:310,342-347`); `help_measured` cached (`render.rs:1246`). Remaining: per-node `Layout::default().split()` still present (`render.rs:570,669,698,828,861,953,959,986,1010`, `meter.rs:164,225`); meter/volume bar `format!` per cell not yet interned.

- [x] Drop `fill_bg` full clear; rely on ratatui `Buffer` diff (clear chrome only)
- [x] Drop `fill_bg` full clear; rely on ratatui `Buffer` diff (clear chrome only)
- [x] Hoist `Layout` constraints to closed-form rect calculations; construct solver once per list, not per node/detail/volume bar
- [x] Pre-build volume/meter bar strings or render via borrowed spans; intern `FILTER_PROMPT`, `%`, `muted`; reuse thread-local format buffer instead of `format!`/`to_string` per cell
- [x] Same for `preview.rs:118-123,129,396` (`format!` per place/delete/park)

Gate: 60fps + meter tick with no full-screen API memset; identical goldens.
Verify: `cargo test -p flex-core`, `cargo bench -p flex-core`

### OPT-5 — Preview per-frame I/O + allocs (wallpaper only)
Evidence: `flex-core/src/preview.rs:592-597` (`transmit_file` → `placement(rect, png_size(&file))` every `sync`), `preview.rs:405-407,650-654` (2× `open/read` per frame on PNG fast path + `PathBuf clone`), `preview.rs:476-485` (`cache_path` metadata+`format!`+join), `preview.rs:302` (`Vec` per col in `fitted_box`).

Landed: `TransmitMemo` + `last_rect` (`preview.rs:565-582`) skip `open`/`read`/`metadata` on repeat frames; `png_size` runs once after the unchanged check (`preview.rs:684-700`); `cache_dir` borrowed not cloned and converter resolved once (`transmit_file:760-776`, `converter:779-797`); `fitted_box` reuses thread-local `FIT_BUF` (`preview.rs:340-380`).

- [x] Memoize `(src → file, file → size/placed)` across frames; move `png_size` after `unchanged(pane,src)` check
- [x] Reuse candidate/fit buffer across frames; no `cache_dir.clone()` per `sync`

Gate: zero syscalls + zero Vec allocs per frame when pane+src unchanged.
Verify: `cargo test -p flex-core`, wallpaper manual test at 80×24

---

## Phase C — Polling / spawns / /proc (idle-cost killers)

### OPT-6 — 1s tick fans out to all providers unconditionally
Evidence: `flex-core/src/backend.rs:190-191` (`poll_timeout 1s`), `flex-rice/src/providers.rs:48-67` (`tick_hook` → center+wifi+proc+net+bt+notify, only center gated at `center.rs:756-765` B-025). `bt.rs:546-587` (3+N spawns/s), `net.rs:189-221`, `notify.rs:847-860` (disk read + 2 spawns + rebuild/s).

Landed: `tick_hook` (`providers.rs:58-100`) gates proc/net/bt/notify on `active_tab_is` + `throttle_due` (proc 3s, net 3s, bt 5s, notify 2s); center gauges and wifi scan stay on the 1s/event-driven path.

- [x] Gate each refresh on active tab (copy `refresh_gauges` pattern)
- [x] Throttle proc/net/bt/notify to 2–5s or on-demand via `wake_ui`; back `bt show/devices` with `BgTask` like wifi scan
- [x] Keep 1s tick for gauges/clock only

Gate: idle on unrelated tab = ~0 spawns/s (verify with `strace -f -e execve` or spawn counter test).
Verify: `cargo test -p flex-rice`, `cargo clippy --locked --all-targets`

### OPT-7 — Full `/proc`+`/sys` sweep + disk write every tick
Evidence: `providers/proc.rs:382-427,481-567` (`read_dir(/proc)` + 4–5 files × ~300 pids ≈ 1200 syscalls/s; per-compare `to_lowercase:232-236,414-418`), `exec/net.rs:312-477,514-560` (6× `/proc/net/*` + per-pid fd walk + `write_proc_stat_cache` every tick at `:455`), text cache parse at `exec/net.rs:97-135`.

Landed: tick throttled + active-tab gated via OPT-6 (`providers.rs:65-99`); `pid_net_cache` with staggered per-pid TTL rescans fds only for new/stale pids (`exec/net.rs:69-102,449-452`, pruned per tick at `:501-503`); proc-stat cache written only when the talker set is non-empty (`exec/net.rs:493-500`).

- [x] Throttle to 2–3s; skip when proc/net tab not active
- [x] Cache `inet_inode → pid` across ticks; rescan fds only for new pids
- [x] Write proc-stat cache only when rates requested (Waybar `sample_json` already caches at `exec/net.rs:563-576`)

Gate: idle syscalls/s down >5×; no SSD churn per second.
Verify: `cargo test -p flex-rice`, tick with `/proc` stub corpus

### OPT-8 — Parser string-alloc storm
Evidence: `providers/center.rs:225-231` (3× `replace` per SSID; `to_string` per field at `240-268`), `providers/wifi.rs:201-242,161-184`, `exec/notify.rs:131-249` (4× `split_whitespace` + `to_lowercase` + `format!` per row per tick via `providers/notify.rs:80,223`), `exec/notify.rs:796-809` + `providers/notify.rs:183-186,342-346` (rel-time `String` per row per tick), `providers/proc.rs:321-322`, `providers/net.rs:190,166`.

Landed: `entity_cache` keyed by `(notification id, timestamp)` (`providers/notify.rs:76-101`); `rel_time_cache` keyed by `(timestamp, age_minutes)` with exact under-2-minute path (`providers/notify.rs:103-132`); byte-level `is_system_app_name` via `eq_ignore_ascii_case` without alloc (`providers/notify.rs:134-144`); shared `tools::decode_stdout` moves valid UTF-8 instead of lossy-copying (`tools.rs:108-126`, used by center/wifi/bt).

- [x] Cache `extract_entities` per `(notification.id, timestamp)`; byte-level `starts_with` before `to_lowercase`; reuse meta buffers
- [x] Recompute relative time only on minute rollover
- [x] Hoist `format!("{} {}", comm, pid)` / SSID unescape out of per-tick loop

Gate: 50–300 rows × N ticks no longer produces 10k+ temp `String`s/s idle.
Verify: `cargo test -p flex-rice`

### OPT-9 — Full `Vec<Row>` rebuild + clone per tick
Evidence: `providers/proc.rs:431-446`, `providers/net.rs:194-202`, `providers/bt.rs:560-574`, `providers/notify.rs:853-858` (`tab.rows = fresh`), clones at `notify.rs:180,189-207`, `net.rs:166,190`, `proc.rs:262,283,296`, `bt.rs:551`; `wifi.rs:480-514` double scan to restore focus.

Landed: `sync_rows_in_place` + `restore_focus` (`providers.rs:144-210`); wired into proc (`proc.rs:438-454`), net bandwidth/interfaces/speedtest (`net.rs:183-226`), bt devices/adapters (`bt.rs:602-615`), notify feed (`notify.rs:924-928`); wifi `refresh_scan` restores focus through the filtered view in a single pass (`wifi.rs:497-531`).

- [x] In-place update (rewrite `label/meta/volume` only, keep `Row` ids/targets) following `center::refresh_gauges`
- [x] `Arc<str>` / `Arc<Target>` for stable labels; `std::mem::replace` + diff instead of wholesale assign
- [x] Single focus-restore pass

Gate: no filter recompute per second; no focus/scroll jitter; fewer `Row`/`String` reallocs.
Verify: `cargo test -p flex-rice`

---

## Phase D — Dependency/feature trim and tool resolution

### OPT-11 — Dependency/feature + tool-resolution trim
Evidence: `flex-rice/Cargo.toml:83-91` (`ratatui` full, `crossterm` full, `tokio rt+rt-multi-thread+macros+process+sync`, `zbus async-io+blocking-api`, `tracing-subscriber env-filter` → regex, `clap derive` per bin). Only `center_menu:721-730` + `speedtest::trigger_background` need async; mixer/record/shot/power are sync. Tool resolution: `providers/center.rs:123-139` (UTF-8 lossy copy ×7+3), `exec/bt.rs:166-170`, `exec/wifi.rs:442-447,452-546`, `exec/mixer.rs:25-30` (`PATH.split(':')` + `stat` per entry per spawn, `Vec<String>` argv per call).

Partial: tool-resolution half landed — `tools.rs` parses ambient `PATH` once (`ambient_dirs`), memoizes the four hot tools per-`OnceLock` (`resolve_ambient`), keeps stub-`PATH` uncached (`resolve_tool`), and `decode_stdout`/`decode_bytes` move/borrow valid UTF-8 (`tools.rs:38-126`, used by center/wifi/bt). Remaining: dependency/feature trim and no-TUI crate split.

- [ ] `tokio/rt` only where possible (drop `rt-multi-thread,process` except users), `ratatui/default-features=false`, `tracing-subscriber` fmt-only (drop `env-filter`/regex), split `record/mixer` to no-TUI crate without `ratatui/zbus`
- [x] `OnceLock<Vec<PathBuf>>` for `PATH` dirs + per-tool `OnceLock<PathBuf>` (`nmcli/bluetoothctl/wpctl/brightnessctl`); `&'static [&'static str]` argv consts; `String::from_utf8` with lossy fallback only on error

Gate: lower link floor + no per-tick `stat` storm; thread-pool not spawned for sync bins.
Verify: `cargo build --release`, `cargo test --locked`, `cargo clippy --locked --all-targets`

---

## Ordering + rollout

1. Phase A (OPT-1, OPT-2) — isolated to `flex-core`, bench-gated, no provider changes.
2. Phase B (OPT-3, OPT-4, OPT-5) — render path, golden-gated.
3. Phase C (OPT-6 … OPT-9) — behavior-sensitive (timing/focus); land one provider per commit with tick tests.
4. Phase D (OPT-11) — packaging; features and dependency trim.

Suggested commits: one OPT per commit (`perf(core): …`, `perf(rice): …`, `perf(pkg): …`), each with before/after bench or spawn-count note in the message body.
