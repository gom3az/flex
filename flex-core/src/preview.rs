//! Graphics preview pane (wallpaper cutover).
//!
//! The wallpaper picker shows the focused image next to the list. The pane is
//! reserved by [`render`](crate::render) (pure geometry, golden-testable) and
//! painted out-of-band here: after every frame the event loop hands the
//! focused row's image to [`Preview::sync`], which emits protocol escapes to
//! the terminal through the [`ImageBackend`] seam (today only the kitty
//! graphics protocol, [`KittyBackend`], is implemented).
//!
//! Protocol notes (kitty `docs/graphics-protocol.rst`, kitty 0.47 verified):
//!
//! - `a=T` transmits **and** displays; re-transmitting a used id replaces the
//!   image and drops its previous placements, so a single fixed [`IMAGE_ID`]
//!   is enough for the one preview slot.
//! - `t=f` sends the file *path* as the (base64) payload — the terminal reads
//!   the file itself, so flex never encodes pixel data and needs no image
//!   crate. `f=100` (PNG) is mandatory: kitty does **not** sniff the format of
//!   file transmissions (it defaults to raw RGBA), which is why non-PNG
//!   sources are converted once into a cached PNG (see [`Preview`]).
//! - `c`/`r` (columns/rows) scale the image into the pane, letterboxed so the
//!   aspect ratio survives; `C=1` keeps the cursor put (kitty ≥ 0.20);
//!   `q=2` suppresses the reply, so no APC responses leak into the event loop.
//! - Text erasure never affects placed images, and entering/leaving the
//!   alternate screen clears them, so the TUI's own redraws cannot smear the
//!   pane.
//!
//! Side-effect scope: this is the only module that (a) writes bytes straight
//! to the terminal rather than into the ratatui buffer and (b) may spawn a
//! process *after* provider load ([`Preview::transmit_file`], at most one
//! converter run per unseen image, result cached on disk). Both are confined
//! to the preview slot: no provider row, id, or filter ever depends on them,
//! and everything degrades to "no image" when the pieces are missing.

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use crate::content_hash_hex;

/// Image id for the single preview slot (any non-zero value).
pub const IMAGE_ID: u32 = 0x00f1_5e01;

/// Pane width as a percentage of the frame width.
pub const PANE_WIDTH_PERCENT: u16 = 45;
/// Smallest pane we are willing to reserve (cells).
pub const PANE_MIN_WIDTH: u16 = 16;
/// List width kept for the rows when the pane is reserved (cells).
pub const MIN_LIST_WIDTH: u16 = 24;
/// Smallest list height that can still show a pane.
pub const PANE_MIN_HEIGHT: u16 = 3;
/// Longest base64 payload kitty accepts in a single escape code.
pub const MAX_PAYLOAD_CHARS: usize = 4096;
/// Longest file path we will transmit (base64 is 4 chars per 3 bytes).
const MAX_PATH_BYTES: usize = MAX_PAYLOAD_CHARS / 4 * 3 - 3;

/// Thumbnail box for converted sources (large enough for a 10 pt preview
/// pane on a `HiDPI` screen, small enough to convert in a few milliseconds).
pub const THUMB_BOX: &str = "800x600>";

/// `FLEX_PREVIEW` selects the pane backend.
///
/// Accepted values: `auto` (unset default; reserve a pane when the terminal
/// supports graphics), `kitty` (force the pane on), `off` (force it off), and
/// the legacy `1`/`0` (force on/off). `sixel` is diagnosed as not implemented
/// and behaves as `off`; any other value falls back to `auto`.
pub const PREVIEW_ENV: &str = "FLEX_PREVIEW";
/// `FLEX_PREVIEW_CACHE` overrides the derived-PNG cache directory.
pub const CACHE_ENV: &str = "FLEX_PREVIEW_CACHE";
/// `FLEX_PREVIEW_CONVERT` forces the converter command for non-PNG sources.
pub const CONVERTER_ENV: &str = "FLEX_PREVIEW_CONVERT";

/// Converter candidates, in preference order (`ImageMagick` 7, then 6).
const CONVERTERS: [&str; 2] = ["magick", "convert"];

/// PNG file signature (`%PNG\r\n\x1a\n`).
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Base64 alphabet (RFC 4648, standard).
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The terminal-image protocol seam.
///
/// Only the kitty graphics protocol ([`KittyBackend`]) is implemented today;
/// keeping escape generation behind this trait lets another protocol slot in
/// without changing [`Preview`] or its callers.
pub trait ImageBackend {
    /// Escape sequence placing `file` at `placement` under `image_id`.
    ///
    /// `None` when the placement is empty or the path is too long for one
    /// escape code; the caller then leaves the pane blank.
    fn place(&self, image_id: u32, file: &Path, placement: Placement) -> Option<String>;

    /// Escape sequence deleting `image_id` and its transmitted data.
    fn delete(&self, image_id: u32) -> String;
}

/// The kitty graphics protocol backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct KittyBackend;

impl ImageBackend for KittyBackend {
    fn place(&self, image_id: u32, file: &Path, placement: Placement) -> Option<String> {
        if placement.cols == 0 || placement.rows == 0 {
            return None;
        }
        let bytes = file.as_os_str().as_bytes();
        if bytes.len() > MAX_PATH_BYTES {
            return None;
        }
        let payload = base64(bytes);
        let (row, col) = (placement.y.saturating_add(1), placement.x.saturating_add(1));
        // Pixel offsets are only meaningful when non-zero, and must stay below
        // one cell (protocol rule).
        let offsets = match (placement.x_px, placement.y_px) {
            (0, 0) => String::new(),
            (x, 0) => format!(",X={x}"),
            (0, y) => format!(",Y={y}"),
            (x, y) => format!(",X={x},Y={y}"),
        };
        Some(format!(
            "\x1b[{row};{col}H\x1b_Ga=T,f=100,t=f,i={image_id}{offsets},c={},r={},C=1,q=2;{payload}\x1b\\",
            placement.cols, placement.rows
        ))
    }

    fn delete(&self, image_id: u32) -> String {
        format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
    }
}

/// The active terminal-image backend.
#[must_use]
pub fn backend() -> &'static dyn ImageBackend {
    &KittyBackend
}

/// Preview pane modes resolved from [`PREVIEW_ENV`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    /// Reserve a pane only when the terminal supports graphics.
    Auto,
    /// Always reserve a pane (the kitty protocol is assumed).
    Kitty,
    /// Never reserve a pane.
    Off,
}

/// Resolve a [`PREVIEW_ENV`] value into a mode and an optional diagnostic.
///
/// Mapping is case-sensitive and exact:
///
/// - unset or `auto` -> [`PreviewMode::Auto`], no diagnostic
/// - `kitty` or the legacy `1` -> [`PreviewMode::Kitty`], no diagnostic
/// - `off`, the legacy `0`, or `""` -> [`PreviewMode::Off`], no diagnostic
/// - `sixel` -> [`PreviewMode::Off`] with a "not implemented" diagnostic
/// - anything else -> [`PreviewMode::Auto`], no diagnostic (an unknown value
///   must never disable the pane or error)
#[must_use]
pub fn parse_preview(raw: Option<&str>) -> (PreviewMode, Option<String>) {
    match raw {
        Some("kitty" | "1") => (PreviewMode::Kitty, None),
        Some("off" | "0" | "") => (PreviewMode::Off, None),
        Some("sixel") => (
            PreviewMode::Off,
            Some("flex: preview: sixel backend not implemented".to_string()),
        ),
        _ => (PreviewMode::Auto, None),
    }
}

/// Whether the wallpaper picker should reserve a preview pane.
///
/// Reads [`PREVIEW_ENV`] once (see [`parse_preview`]), emits any diagnostic
/// exactly once, then [`PreviewMode::Auto`] defers to terminal detection while
/// [`PreviewMode::Kitty`]/[`PreviewMode::Off`] force the pane on/off.
#[must_use]
pub fn enabled() -> bool {
    let raw = std::env::var(PREVIEW_ENV).ok();
    let (mode, diagnostic) = parse_preview(raw.as_deref());
    if let Some(message) = diagnostic {
        crate::diag::warn(&message);
    }
    enabled_for(mode, terminal_supports_graphics())
}

/// Decide the pane for a resolved `mode` and terminal `graphics` support.
fn enabled_for(mode: PreviewMode, graphics: bool) -> bool {
    match mode {
        PreviewMode::Off => false,
        PreviewMode::Kitty => true,
        PreviewMode::Auto => graphics,
    }
}

/// Detect a kitty graphics capable terminal from the environment.
fn terminal_supports_graphics() -> bool {
    if std::env::var("KITTY_WINDOW_ID").is_ok_and(|value| !value.is_empty()) {
        return true;
    }
    if let Ok(program) = std::env::var("TERM_PROGRAM") {
        if matches!(program.as_str(), "kitty" | "ghostty" | "WezTerm") {
            return true;
        }
    }
    std::env::var("TERM").is_ok_and(|term| term.contains("kitty") || term.contains("ghostty"))
}

/// Where and how big the image is drawn inside the pane.
///
/// kitty stretches whatever it is given to fill the `c`×`r` cell box exactly
/// (measured: a 4:1 image in a 71×69-cell pane came out 568×1096 px, aspect
/// 0.52), so the box itself has to carry the image's aspect ratio — the same
/// arithmetic `icat` does. `x_px`/`y_px` are the sub-cell remainders that
/// centre the box (`X`/`Y` must stay smaller than one cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Placement origin column (0-based cells).
    pub x: u16,
    /// Placement origin row (0-based cells).
    pub y: u16,
    /// Box width in cells.
    pub cols: u16,
    /// Box height in cells.
    pub rows: u16,
    /// Sub-cell horizontal offset in pixels.
    pub x_px: u16,
    /// Sub-cell vertical offset in pixels.
    pub y_px: u16,
}

/// Fallback cell size (pixels) when the terminal does not report one.
///
/// Every kitty-family terminal answers `TIOCGWINSZ` with pixel dimensions, so
/// this only covers exotic cases; the aspect math then uses typical cell
/// proportions instead of failing.
pub const FALLBACK_CELL: (f32, f32) = (8.0, 16.0);

/// Pane rectangle reserved on the right of the list area, if any.
///
/// `list_h` is the list area height (the pane shares it); the pane keeps one
/// gutter column before it, [`MIN_LIST_WIDTH`] cells for the rows, at least
/// [`PANE_MIN_WIDTH`] and [`PANE_MIN_HEIGHT`] cells for itself, and is simply
/// dropped on frames too small for that.
#[must_use]
pub fn pane(area: Rect, list_h: u16, wanted: bool) -> Option<Rect> {
    if !wanted || list_h < PANE_MIN_HEIGHT {
        return None;
    }
    let width = usize::from(area.width);
    let min_list = usize::from(MIN_LIST_WIDTH);
    let min_pane = usize::from(PANE_MIN_WIDTH);
    if width < min_list + min_pane + 1 {
        return None;
    }
    let max_pane = width - min_list - 1;
    let percent = width * usize::from(PANE_WIDTH_PERCENT) / 100;
    let pane_w = u16::try_from(percent.clamp(min_pane, max_pane)).ok()?;
    Some(Rect {
        x: area.x + area.width - pane_w,
        y: area.y,
        width: pane_w,
        height: list_h,
    })
}

/// Terminal cell size in pixels.
///
/// `None` when the terminal reports no pixel geometry; callers fall back to
/// [`FALLBACK_CELL`].
#[must_use]
pub fn cell_size() -> Option<(f32, f32)> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        f32::from(size.width) / f32::from(size.columns),
        f32::from(size.height) / f32::from(size.rows),
    ))
}

/// Largest whole-cell box inside `pane` whose pixel aspect matches `image`.
///
/// Cells are rarely square (8×19 px on the reference session), so a box that
/// simply spans the pane distorts every image: kitty stretches whatever it is
/// given to fill the `c`×`r` box exactly (measured: a 4:1 image in a 71×69-cell
/// pane came out 568×1096 px). This walks the whole-cell boxes that fit the
/// pane, keeps the best achievable aspect error, then returns the **largest**
/// box still within [`ASPECT_TOLERANCE`] of it — a few percent of aspect
/// deviation is invisible, a third of the preview area is not.
#[must_use]
pub fn fitted_box(pane: Rect, image: (u32, u32), cell: (f32, f32)) -> (u16, u16) {
    if pane.width == 0 || pane.height == 0 || image.0 == 0 || image.1 == 0 {
        return (pane.width, pane.height);
    }
    let (cell_w, cell_h) = (cell.0.max(1.0), cell.1.max(1.0));
    // Preview images are never near 2^24 px; the cast is exact in practice.
    #[allow(clippy::cast_precision_loss)]
    let aspect = image.0 as f32 / image.1 as f32;
    let mut candidates: Vec<(u16, u16, i64)> = Vec::with_capacity(usize::from(pane.width));
    let mut best_error = f32::INFINITY;
    for cols in 1..=pane.width {
        let rows = (f32::from(cols) * cell_w / aspect / cell_h).round();
        if rows < 1.0 || rows > f32::from(pane.height) {
            continue;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rows = rows as u16;
        let box_aspect = (f32::from(cols) * cell_w) / (f32::from(rows) * cell_h);
        // Relative error: log-ratio, so 2:1 vs 1:2 is as wrong as 1:1 vs 0.5:1.
        let error = (box_aspect / aspect).ln().abs();
        best_error = best_error.min(error);
        candidates.push((cols, rows, scale(error)));
    }
    // `scale` is larger-is-better, so the tolerance band is `>= cutoff`.
    let cutoff = scale(best_error + ASPECT_TOLERANCE);
    candidates
        .iter()
        .filter(|(_, _, error)| *error >= cutoff)
        .max_by_key(|(cols, rows, error)| (u32::from(*cols) * u32::from(*rows), *error))
        .map_or((pane.width, pane.height), |(cols, rows, _)| (*cols, *rows))
}

/// Aspect error we accept while trading up for a bigger preview (log-ratio).
pub const ASPECT_TOLERANCE: f32 = 0.03;

/// Fixed-point aspect error where **larger is better** (for `max_by_key`).
#[allow(clippy::cast_possible_truncation)]
fn scale(error: f32) -> i64 {
    i64::MAX - (error * 1_000_000.0) as i64
}

/// Placement for `image` inside `pane`, centred.
///
/// `image` is the transmitted PNG's pixel size, `cell` the terminal's cell
/// size ([`cell_size`], [`FALLBACK_CELL`] when unknown); without an image size
/// the pane box is used as-is.
#[must_use]
pub fn placement(pane: Rect, image: Option<(u32, u32)>, cell: (f32, f32)) -> Placement {
    let (cell_w, cell_h) = (cell.0.max(1.0), cell.1.max(1.0));
    let (cols, rows) = match image {
        Some(size) => fitted_box(pane, size, cell),
        None => (pane.width, pane.height),
    };
    let pane_w = f32::from(pane.width) * cell_w;
    let pane_h = f32::from(pane.height) * cell_h;
    let off_x = ((pane_w - f32::from(cols) * cell_w) / 2.0).max(0.0);
    let off_y = ((pane_h - f32::from(rows) * cell_h) / 2.0).max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (shift_cols, shift_rows) = ((off_x / cell_w) as u16, (off_y / cell_h) as u16);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x_px, y_px) = (
        (off_x - f32::from(shift_cols) * cell_w) as u16,
        (off_y - f32::from(shift_rows) * cell_h) as u16,
    );
    // Stay inside the pane even if rounding pushed the box outward.
    let x = pane
        .x
        .saturating_add(shift_cols)
        .min(pane.x + pane.width.saturating_sub(cols));
    let y = pane
        .y
        .saturating_add(shift_rows)
        .min(pane.y + pane.height.saturating_sub(rows));
    Placement {
        x,
        y,
        cols,
        rows,
        x_px,
        y_px,
    }
}

/// Escape sequence placing `file` at `placement` (`a=T,f=100,t=f,…,C=1,q=2`).
///
/// Delegates to [`ImageBackend::place`] on the active [`backend`].
#[must_use]
pub fn place_escape(image_id: u32, file: &Path, placement: Placement) -> Option<String> {
    backend().place(image_id, file, placement)
}

/// Escape sequence deleting the preview image and its data (`a=d,d=I`).
///
/// Delegates to [`ImageBackend::delete`] on the active [`backend`].
#[must_use]
pub fn delete_escape(image_id: u32) -> String {
    backend().delete(image_id)
}

/// Escape sequence parking the cursor at a 0-based cell (1-based CUP).
#[must_use]
pub fn park_escape(pos: (u16, u16)) -> String {
    format!(
        "\x1b[{};{}H",
        pos.1.saturating_add(1),
        pos.0.saturating_add(1)
    )
}

/// Whether `path` starts with the PNG signature.
#[must_use]
pub fn is_png(path: &Path) -> bool {
    png_size(path).is_some()
}

/// Pixel size of a PNG (`IHDR`), read from the first 24 bytes.
///
/// Every file we transmit is a PNG — the source itself or our converted
/// thumbnail — so the image dimensions never need an image crate. `None` for
/// anything that is not a readable PNG header.
#[must_use]
pub fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read as _;
    let mut header = [0_u8; 24];
    let mut file = std::fs::File::open(path).ok()?;
    file.read_exact(&mut header).ok()?;
    if header[..PNG_MAGIC.len()] != PNG_MAGIC {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// Standard base64 with padding.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(BASE64[(triple >> 18) as usize & 0x3f]));
        out.push(char::from(BASE64[(triple >> 12) as usize & 0x3f]));
        out.push(if chunk.len() > 1 {
            char::from(BASE64[(triple >> 6) as usize & 0x3f])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(BASE64[triple as usize & 0x3f])
        } else {
            '='
        });
    }
    out
}

/// Derived-PNG cache directory (`$XDG_CACHE_HOME/flex/previews` by default).
#[must_use]
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var(CACHE_ENV) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("flex/previews");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/flex/previews")
}

/// Cache path for `src`: keyed by path, mtime and size, so an edited file
/// regenerates while repeat visits reuse the PNG.
#[must_use]
pub fn cache_path(dir: &Path, src: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let key = content_hash_hex(&format!("{}\u{0}{mtime}\u{0}{}", src.display(), meta.len()));
    Some(dir.join(format!("{key}.png")))
}

/// The preview slot: what is currently on screen and how to derive it.
///
/// Reuse one instance per event loop; it memoizes the converter lookup and
/// the (source, transmitted file, pane) triple so an unchanged focus never
/// re-emits escapes.
#[derive(Debug, Default)]
pub struct Preview {
    /// Cache directory for derived PNGs; `None` disables conversion.
    cache_dir: Option<PathBuf>,
    /// Resolved converter command, once probed.
    converter: Option<String>,
    /// Whether [`Preview::converter`] already ran.
    converter_probed: bool,
    /// What the terminal currently shows.
    shown: Option<Shown>,
    /// A "no preview possible" diagnostic was already printed.
    warned: bool,
    /// Terminal cell size in pixels, once measured.
    cell: Option<(f32, f32)>,
}

/// One placed image (source, transmitted file, placement on screen).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    source: PathBuf,
    file: PathBuf,
    placed: Placement,
}

impl Preview {
    /// Build a preview using the environment's cache directory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_dir: Some(cache_dir()),
            ..Self::default()
        }
    }

    /// Build a preview writing derived PNGs to `dir` (tests).
    #[must_use]
    pub fn with_cache_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: Some(dir.into()),
            ..Self::default()
        }
    }

    /// Force the converter command instead of probing `magick`/`convert`.
    #[must_use]
    pub fn with_converter(mut self, command: impl Into<String>) -> Self {
        self.converter = Some(command.into());
        self.converter_probed = true;
        self
    }

    /// Whether an image is currently placed.
    #[must_use]
    pub fn is_showing(&self) -> bool {
        self.shown.is_some()
    }

    /// Report (once per process, on stderr) that an image cannot be shown.
    ///
    /// The pane silently stays blank by design, but a permanently empty pane
    /// should not be a mystery: the usual causes are an unwritable cache
    /// directory, a missing converter, or `FLEX_PREVIEW_CONVERT` pointing at
    /// something that fails.
    fn warn_once(&mut self, src: &Path) {
        if self.warned {
            return;
        }
        self.warned = true;
        if std::env::var("FLEX_DIAG").is_ok() {
            crate::diag::warn(&format!(
                "flex: preview: cannot show {} (no preview cache at {} or no working image converter; \
                 see {CONVERTER_ENV}/{CACHE_ENV})",
                src.display(),
                self.cache_dir
                    .as_deref()
                    .map_or_else(|| "-".to_string(), |dir| dir.display().to_string()),
            ));
        }
    }

    /// Paint (or clear) the pane for `source` and re-park the cursor.
    ///
    /// Called once per frame after `Terminal::draw`; a no-op when the focused
    /// image and pane are unchanged. Write failures are reported but never
    /// fatal to the TUI (the caller ignores them).
    ///
    /// # Errors
    ///
    /// Returns the terminal write error, if any.
    pub fn sync(
        &mut self,
        out: &mut impl Write,
        pane: Option<Rect>,
        source: Option<&Path>,
        park: Option<(u16, u16)>,
    ) -> io::Result<()> {
        let Some((rect, src)) = pane.zip(source) else {
            return self.hide(out);
        };
        let Some(file) = self.transmit_file(src) else {
            self.warn_once(src);
            return self.hide(out);
        };
        let cell = self.cell_size();
        let placed = placement(rect, png_size(&file), cell);
        let unchanged = self.shown.as_ref().is_some_and(|shown| {
            shown.source == src && shown.file == file && shown.placed == placed
        });
        if unchanged {
            return Ok(());
        }
        let Some(escape) = backend().place(IMAGE_ID, &file, placed) else {
            return self.hide(out);
        };
        out.write_all(escape.as_bytes())?;
        if let Some(pos) = park {
            out.write_all(park_escape(pos).as_bytes())?;
        }
        out.flush()?;
        self.shown = Some(Shown {
            source: src.to_path_buf(),
            file,
            placed,
        });
        Ok(())
    }

    /// Cell size in pixels, re-read when the terminal grid changes.
    ///
    /// Cached after the first successful read (the value only changes on a
    /// font-size change, which also re-runs the whole layout); a terminal that
    /// reports no pixel geometry falls back to [`FALLBACK_CELL`].
    fn cell_size(&mut self) -> (f32, f32) {
        if let Some(cell) = self.cell {
            return cell;
        }
        let cell = cell_size().unwrap_or(FALLBACK_CELL);
        self.cell = Some(cell);
        cell
    }

    /// Drop the placed image (no-op when nothing is shown).
    fn hide(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.shown.take().is_none() {
            return Ok(());
        }
        out.write_all(backend().delete(IMAGE_ID).as_bytes())?;
        out.flush()
    }

    /// PNG file to transmit for `src`.
    ///
    /// PNG sources go through untouched (no process, no cache); anything else
    /// is converted once into [`Preview::cache_dir`] by
    /// [`Preview::converter`]. `None` means "no preview" (no converter, no
    /// cache directory, or a failed conversion) — never an error.
    #[must_use]
    pub fn transmit_file(&mut self, src: &Path) -> Option<PathBuf> {
        if is_png(src) {
            return Some(src.to_path_buf());
        }
        let dir = self.cache_dir.clone()?;
        let cache = cache_path(&dir, src)?;
        if cache.is_file() {
            return Some(cache);
        }
        let command = self.converter()?;
        if !convert(&command, src, &cache) {
            return None;
        }
        Some(cache)
    }

    /// Resolve (once) the converter command.
    fn converter(&mut self) -> Option<String> {
        if self.converter_probed {
            return self.converter.clone();
        }
        self.converter_probed = true;
        if let Ok(explicit) = std::env::var(CONVERTER_ENV) {
            self.converter = if explicit.is_empty() {
                None
            } else {
                Some(explicit)
            };
            return self.converter.clone();
        }
        self.converter = CONVERTERS
            .iter()
            .find(|command| available(command))
            .map(|command| (*command).to_string());
        self.converter.clone()
    }
}

/// Whether `command` runs (converter probe, once per process).
fn available(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Convert `src` into `cache` (atomically: write a sibling temp file, then
/// rename), downscaling into [`THUMB_BOX`].
fn convert(command: &str, src: &Path, cache: &Path) -> bool {
    let Some(dir) = cache.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let tmp = cache.with_extension("png.tmp");
    let _ = std::fs::remove_file(&tmp);
    // Argument order is ImageMagick's: `magick <in> <ops> png:<out>`; the
    // `png:` prefix pins the output format regardless of the temp suffix.
    let status = std::process::Command::new(command)
        .arg(src)
        .args(["-auto-orient", "-resize", THUMB_BOX, "-strip"])
        .arg(format!("png:{}", tmp.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !status.is_ok_and(|status| status.success()) {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    std::fs::rename(&tmp, cache).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flex-preview-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Minimal PNG header (signature + IHDR) for `width`x`height`.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    fn png_bytes() -> Vec<u8> {
        png_header(400, 100)
    }

    fn placement_of(x: u16, y: u16, cols: u16, rows: u16) -> Placement {
        Placement {
            x,
            y,
            cols,
            rows,
            x_px: 0,
            y_px: 0,
        }
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (raw, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
            (&[0xff, 0x00, 0x7f][..], "/wB/"),
        ] {
            assert_eq!(base64(raw), encoded, "encode {raw:?}");
        }
    }

    #[test]
    fn path_payload_is_the_base64_path() {
        let escape =
            place_escape(7, Path::new("/a/b.png"), placement_of(40, 1, 36, 20)).expect("escape");
        assert!(escape.starts_with("\x1b[2;41H"), "cursor first: {escape:?}");
        assert!(escape.contains("\x1b_Ga=T,f=100,t=f,i=7,c=36,r=20,C=1,q=2;"));
        assert!(escape.ends_with(&format!("{}\x1b\\", base64(b"/a/b.png"))));
    }

    #[test]
    fn sub_cell_offsets_are_emitted_only_when_non_zero() {
        let base = placement_of(40, 1, 36, 20);
        let centred = Placement {
            x_px: 3,
            y_px: 5,
            ..base
        };
        let escape = place_escape(7, Path::new("/a.png"), centred).expect("escape");
        assert!(
            escape.contains("i=7,X=3,Y=5,c=36,r=20"),
            "offsets come with the id: {escape:?}"
        );

        let only_x = Placement { x_px: 3, ..base };
        assert!(place_escape(7, Path::new("/a.png"), only_x)
            .expect("escape")
            .contains("i=7,X=3,c=36"));
        let only_y = Placement { y_px: 4, ..base };
        assert!(place_escape(7, Path::new("/a.png"), only_y)
            .expect("escape")
            .contains("i=7,Y=4,c=36"));
    }

    #[test]
    fn overlong_paths_and_empty_placements_have_no_escape() {
        let long = format!("/{}.png", "x".repeat(MAX_PATH_BYTES));
        assert!(place_escape(1, Path::new(&long), placement_of(0, 0, 10, 10)).is_none());
        assert!(place_escape(1, Path::new("/a.png"), placement_of(0, 0, 0, 10)).is_none());
        assert!(place_escape(1, Path::new("/a.png"), placement_of(0, 0, 10, 0)).is_none());
    }

    #[test]
    fn delete_and_park_escapes_are_exact() {
        assert_eq!(delete_escape(3), "\x1b_Ga=d,d=I,i=3,q=2\x1b\\");
        assert_eq!(park_escape((0, 0)), "\x1b[1;1H");
        assert_eq!(park_escape((3, 7)), "\x1b[8;4H");
    }

    #[test]
    fn parse_preview_covers_every_documented_value() {
        let cases: [(Option<&str>, PreviewMode, Option<&str>); 10] = [
            (None, PreviewMode::Auto, None),
            (Some("auto"), PreviewMode::Auto, None),
            (Some("kitty"), PreviewMode::Kitty, None),
            (Some("1"), PreviewMode::Kitty, None),
            (Some("off"), PreviewMode::Off, None),
            (Some("0"), PreviewMode::Off, None),
            (Some(""), PreviewMode::Off, None),
            (
                Some("sixel"),
                PreviewMode::Off,
                Some("flex: preview: sixel backend not implemented"),
            ),
            (Some("KITTY"), PreviewMode::Auto, None),
            (Some("garbage"), PreviewMode::Auto, None),
        ];
        for (raw, mode, diagnostic) in cases {
            assert_eq!(
                parse_preview(raw),
                (mode, diagnostic.map(str::to_string)),
                "value {raw:?}"
            );
        }
    }

    #[test]
    fn enabled_for_combines_mode_and_terminal_support() {
        assert!(!enabled_for(PreviewMode::Off, true));
        assert!(!enabled_for(PreviewMode::Off, false));
        assert!(enabled_for(PreviewMode::Kitty, false));
        assert!(enabled_for(PreviewMode::Kitty, true));
        assert!(enabled_for(PreviewMode::Auto, true));
        assert!(!enabled_for(PreviewMode::Auto, false));
    }

    #[test]
    fn kitty_backend_matches_the_free_escapes() {
        let file = Path::new("/a/b.png");
        let placed = placement_of(40, 1, 36, 20);
        assert_eq!(
            KittyBackend.place(7, file, placed),
            place_escape(7, file, placed)
        );
        assert_eq!(
            KittyBackend.place(7, file, placement_of(0, 0, 0, 10)),
            place_escape(7, file, placement_of(0, 0, 0, 10))
        );
        assert_eq!(KittyBackend.delete(3), delete_escape(3));
        assert_eq!(backend().delete(3), "\x1b_Ga=d,d=I,i=3,q=2\x1b\\");
    }

    /// The live popup grid: 158x72 cells of 8.0x19.2 px (kitty @ ls +
    /// hyprctl on the running session).
    const LIVE_CELL: (f32, f32) = (8.0, 19.2);

    #[test]
    fn fitted_box_carries_the_image_aspect_ratio() {
        let pane = Rect::new(0, 0, 71, 69);
        // Every shape keeps its aspect inside a couple of percent, and the box
        // stays big: the point of the fit is to trade a hair of accuracy for
        // preview area.
        for (image, label) in [
            ((2560_u32, 1440_u32), "16:9 landscape"),
            ((400, 100), "4:1 landscape"),
            ((1080, 1920), "9:16 portrait"),
            ((1440, 1440), "square"),
        ] {
            let (cols, rows) = fitted_box(pane, image, LIVE_CELL);
            let box_aspect = f32::from(cols) * LIVE_CELL.0 / (f32::from(rows) * LIVE_CELL.1);
            #[allow(clippy::cast_precision_loss)]
            let want = image.0 as f32 / image.1 as f32;
            let error = (box_aspect / want).ln().abs();
            assert!(
                error < 0.05,
                "{label}: box aspect {box_aspect:.3} vs {want:.3} (cols={cols}, rows={rows})"
            );
            assert!(cols >= 30, "{label}: preview keeps its area (cols={cols})");
        }

        // The shape that used to render as a 0.52-aspect smear.
        let (cols, rows) = fitted_box(pane, (400, 100), LIVE_CELL);
        let box_aspect = f32::from(cols) * LIVE_CELL.0 / (f32::from(rows) * LIVE_CELL.1);
        assert!(box_aspect > 3.5, "4:1 stays wide: {box_aspect:.2}");

        // Degenerate inputs never panic and stay inside the pane.
        assert_eq!(fitted_box(pane, (0, 0), LIVE_CELL), (71, 69));
        assert_eq!(fitted_box(Rect::new(0, 0, 0, 0), (4, 2), LIVE_CELL), (0, 0));
        let (cols, rows) = fitted_box(Rect::new(0, 0, 1, 1), (16, 9), LIVE_CELL);
        assert!(cols <= 1 && rows <= 1, "a 1x1 pane fits a 1x1 box");
    }

    #[test]
    fn placement_centres_the_box_inside_the_pane() {
        let pane = Rect::new(44, 0, 36, 21);
        let placed = placement(pane, Some((2560, 1440)), LIVE_CELL);
        assert!(placed.cols >= 30, "cols: {}", placed.cols);
        assert!(
            placed.y > pane.y,
            "a 16:9 image is centred in a tall pane: y={}",
            placed.y
        );
        assert!(
            placed.y + placed.rows <= pane.y + pane.height,
            "box stays inside the pane"
        );
        // Vertical slack is split evenly (within one cell of rounding).
        let pane_px = f32::from(pane.height) * LIVE_CELL.1;
        let box_px = f32::from(placed.rows) * LIVE_CELL.1;
        let top = f32::from(placed.y) * LIVE_CELL.1 + f32::from(placed.y_px);
        let bottom = pane_px - top - box_px;
        assert!(
            (top - bottom).abs() <= LIVE_CELL.1,
            "top {top:.0}px vs bottom {bottom:.0}px"
        );

        // Unknown image size: the pane box is used, still inside the frame.
        let fallback = placement(pane, None, LIVE_CELL);
        assert_eq!((fallback.cols, fallback.rows), (36, 21));
        assert_eq!((fallback.x, fallback.y), (44, 0));

        // A genuinely tall image in a wide pane is height-limited, so it is
        // centred horizontally instead (a 9:16 image still fills the width).
        let wide = Rect::new(0, 0, 71, 69);
        let banner = placement(wide, Some((100, 2000)), LIVE_CELL);
        assert!(banner.rows >= 65, "fills the height: {}", banner.rows);
        assert!(banner.x > wide.x, "centred horizontally: x={}", banner.x);
        assert!(banner.x + banner.cols <= wide.x + wide.width);
        assert!(
            (f32::from(banner.x) - f32::from(wide.width - banner.cols) / 2.0).abs() <= 1.0,
            "slack split evenly: x={} cols={}",
            banner.x,
            banner.cols
        );
    }

    #[test]
    fn png_size_reads_the_ihdr_and_rejects_other_files() {
        let dir = scratch("size");
        let png = dir.join("a.png");
        std::fs::write(&png, png_header(2560, 1440)).expect("write");
        assert_eq!(png_size(&png), Some((2560, 1440)));
        // Header-only magic (12 bytes) is not a usable PNG.
        let short = dir.join("short.png");
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(b"rest");
        std::fs::write(&short, bytes).expect("write");
        assert_eq!(png_size(&short), None);
        // Zero dimensions are rejected too.
        let zero = dir.join("zero.png");
        std::fs::write(&zero, png_header(0, 100)).expect("write");
        assert_eq!(png_size(&zero), None);
        assert_eq!(png_size(&dir.join("missing.png")), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn pane_geometry_keeps_the_list_usable() {
        let area = Rect::new(0, 0, 80, 24);
        let reserved = pane(area, 21, true).expect("pane reserved");
        assert_eq!(reserved.width, 36, "45% of 80");
        assert_eq!(reserved.x, 44, "right-aligned, one gutter column");
        assert_eq!(reserved.height, 21, "shares the list height");
        assert_eq!(
            reserved.x + reserved.width,
            area.width,
            "flush with the right edge"
        );

        // Narrow frames drop the pane instead of crushing the list.
        assert!(pane(Rect::new(0, 0, 40, 24), 21, true).is_none());
        assert!(pane(Rect::new(0, 0, 24, 24), 21, true).is_none());
        // Too short for an image.
        assert!(pane(Rect::new(0, 0, 80, 24), 2, true).is_none());
        // Not wanted (non-kitty terminal, non-wallpaper provider).
        assert!(pane(Rect::new(0, 0, 80, 24), 21, false).is_none());

        // A wide frame gives the pane its percentage, not the whole frame.
        let wide = pane(Rect::new(0, 0, 125, 30), 27, true).expect("pane");
        assert_eq!(wide.width, 56, "45% of 125");
        assert_eq!(wide.x, 69);
    }

    #[test]
    fn png_detection_uses_the_signature_not_the_name() {
        let dir = scratch("png");
        let real = dir.join("a.png");
        std::fs::write(&real, png_bytes()).expect("write png");
        assert!(is_png(&real));
        let disguised = dir.join("b.jpg");
        std::fs::write(&disguised, png_bytes()).expect("write png");
        assert!(is_png(&disguised), "signature wins over extension");
        let jpeg = dir.join("c.png");
        std::fs::write(&jpeg, [0xff, 0xd8, 0xff, 0xe0, 0x00]).expect("write jpeg");
        assert!(!is_png(&jpeg), "extension alone is not enough");
        assert!(!is_png(&dir.join("missing.png")));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn cache_key_follows_path_mtime_and_size() {
        let dir = scratch("cache");
        let src = dir.join("wall.jpg");
        std::fs::write(&src, b"one").expect("write");
        let first = cache_path(&dir, &src).expect("key");
        assert_eq!(first.extension().and_then(|e| e.to_str()), Some("png"));
        // Same file, same key.
        assert_eq!(cache_path(&dir, &src).as_ref(), Some(&first));
        // Different byte length busts the key.
        std::fs::write(&src, b"one two three").expect("rewrite");
        let second = cache_path(&dir, &src).expect("key");
        assert_ne!(second, first, "size change regenerates");
        // Missing files have no key at all.
        assert!(cache_path(&dir, &dir.join("nope.jpg")).is_none());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn png_sources_are_transmitted_without_a_converter() {
        let dir = scratch("direct");
        let src = dir.join("wall.png");
        std::fs::write(&src, png_bytes()).expect("write");
        let mut preview = Preview::with_cache_dir(dir.join("cache"));
        assert_eq!(preview.transmit_file(&src).as_deref(), Some(src.as_path()));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn non_png_sources_are_converted_once_and_cached() {
        let dir = scratch("convert");
        let src = dir.join("wall.jpg");
        std::fs::write(&src, [0xff, 0xd8, 0xff, 0xe0, 0x01, 0x02]).expect("write");
        let log = dir.join("calls.log");
        let stub = dir.join("stub-convert.sh");
        // The stub stands in for ImageMagick: it writes a real (header-only)
        // 800x600 PNG so the size/dimension path is exercised too.
        let produced = dir.join("stub-output.png");
        std::fs::write(&produced, png_header(800, 600)).expect("write header");
        std::fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho \"$@\" >> {log}\n\
                 for a in \"$@\"; do case \"$a\" in png:*) cp {produced} \"${{a#png:}}\";; esac; done\n",
                log = log.display(),
                produced = produced.display()
            ),
        )
        .expect("write stub");
        let mut perms = std::fs::metadata(&stub).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&stub, perms).expect("chmod");

        let cache = dir.join("cache");
        let mut preview =
            Preview::with_cache_dir(&cache).with_converter(stub.to_string_lossy().to_string());
        let got = preview.transmit_file(&src).expect("converted");
        assert_eq!(got.parent(), Some(cache.as_path()));
        assert!(is_png(&got), "converter output is a PNG");
        assert_eq!(png_size(&got), Some((800, 600)), "dimensions are readable");
        let first_log = std::fs::read_to_string(&log).expect("log");
        assert!(first_log.contains("-resize"), "downscaled: {first_log:?}");
        assert!(first_log.contains(THUMB_BOX));

        // Second visit reuses the cached PNG: no second converter run, even
        // through a fresh preview instance.
        let mut again =
            Preview::with_cache_dir(&cache).with_converter(stub.to_string_lossy().to_string());
        assert_eq!(again.transmit_file(&src).as_deref(), Some(got.as_path()));
        assert_eq!(
            std::fs::read_to_string(&log).expect("log"),
            first_log,
            "one conversion per unseen image"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_failing_converter_degrades_to_no_preview() {
        let dir = scratch("fail");
        let src = dir.join("wall.webp");
        std::fs::write(&src, [0x52, 0x49, 0x46, 0x46]).expect("write");
        let mut preview = Preview::with_cache_dir(dir.join("cache")).with_converter("false");
        assert!(preview.transmit_file(&src).is_none());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn sync_places_clears_and_skips_unchanged_frames() {
        let dir = scratch("sync");
        let png = dir.join("wall.png");
        std::fs::write(&png, png_bytes()).expect("write");
        let mut preview = Preview::with_cache_dir(dir.join("cache"));
        let pane = Rect::new(44, 0, 36, 21);
        let mut out: Vec<u8> = Vec::new();

        preview
            .sync(&mut out, Some(pane), Some(&png), Some((3, 22)))
            .expect("first frame");
        let first = String::from_utf8(out.clone()).expect("utf8");
        assert!(first.contains("a=T,f=100,t=f"), "{first:?}");
        assert!(first.ends_with("\x1b[23;4H"), "cursor re-parked: {first:?}");
        assert!(preview.is_showing());

        preview
            .sync(&mut out, Some(pane), Some(&png), Some((3, 22)))
            .expect("second frame");
        assert_eq!(out.len(), first.len(), "an unchanged frame emits nothing");

        // Focus loss clears the image.
        preview
            .sync(&mut out, Some(pane), None, Some((3, 22)))
            .expect("cleared");
        let cleared = String::from_utf8(out.clone()).expect("utf8");
        assert!(cleared.ends_with(&delete_escape(IMAGE_ID)), "{cleared:?}");
        assert!(!preview.is_showing());

        // Nothing shown: clearing again is a no-op.
        let before = out.len();
        preview.sync(&mut out, None, None, None).expect("no-op");
        assert_eq!(out.len(), before);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn moving_focus_or_resizing_re_transmits() {
        let dir = scratch("focus");
        let first = dir.join("one.png");
        let second = dir.join("two.png");
        std::fs::write(&first, png_bytes()).expect("write");
        std::fs::write(&second, png_bytes()).expect("write");
        let mut preview = Preview::with_cache_dir(dir.join("cache"));
        let pane = Rect::new(44, 0, 36, 21);
        let mut out: Vec<u8> = Vec::new();

        preview
            .sync(&mut out, Some(pane), Some(&first), None)
            .expect("first image");
        assert!(String::from_utf8_lossy(&out).contains(&base64(first.as_os_str().as_bytes())));

        // Focus moves to another wallpaper: the new path is transmitted and
        // replaces the previous placement (same image id).
        preview
            .sync(&mut out, Some(pane), Some(&second), None)
            .expect("second image");
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains(&base64(second.as_os_str().as_bytes())));
        assert_eq!(text.matches("a=T").count(), 2, "one placement per change");

        // The same image in a different pane (window resize) is re-placed so
        // it keeps fitting the narrower/wider slot.
        let resized = Rect::new(50, 0, 30, 18);
        preview
            .sync(&mut out, Some(resized), Some(&second), None)
            .expect("resized");
        assert_eq!(String::from_utf8_lossy(&out).matches("a=T").count(), 3);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
