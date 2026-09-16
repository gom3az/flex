//! Shared runner for the nine flex entry points.
//!
//! The eight per-provider binaries (`flex-power`, `flex-launch`, …) and the
//! `flex` compat dispatcher are all thin clap shells over this module, so
//! the contracts live here exactly once:
//!
//! - [`fail`] is the single owner of the `flex: error:` prefix plus
//!   `exit(1)` (B-022/B-027: nothing below this module adds its own `flex:`
//!   prefix to an error value).
//! - [`build_menu`] owns per-provider menu construction (including the
//!   `clip`/`wallpaper` empty-store early exits to 130).
//! - [`popup_guard`] owns the outside-a-popup re-exec into
//!   [`popup::toggle_with`], mirroring the wrappers' `POPUP_KITTY` guard.
//! - [`run_select`] owns the select loop: [`run::run_capture`], then the
//!   `ACTION:` emit plus exit mapping (chosen/delete/toggle/target print
//!   one line and exit `0`; `Quit{code}` exits with `code` and no output).
//! - [`canonical_tail`] owns the canonical flag order the dispatcher uses
//!   to re-exec provider binaries.
//!
//! [`popup::toggle_with`]: crate::popup::toggle_with
//! [`run::run_capture`]: flex_core::run::run_capture

use anyhow::{Context as _, Result};
use clap::Args;
use flex_core::backend::{EXIT_CANCELLED, EXIT_ERROR, EXIT_OK};
use flex_core::{CharSet, CharSetName, Menu, Outcome, Peaks, Theme, ThemeName};

use crate::{menu, popup, providers};
use providers::{center, clip, launch, power, proc, shot, theme_, wallpaper, wifi};

/// The nine menu providers, one per `flex-<name>` binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Shutdown/reboot/logout menu.
    Power,
    /// Application launcher.
    Launch,
    /// Screenshot flow.
    Shot,
    /// Theme switcher.
    Theme,
    /// Clipboard history.
    Clip,
    /// Control center (volume/brightness/network).
    Center,
    /// Wallpaper picker (image previews).
    Wallpaper,
    /// Wi-Fi picker.
    Wifi,
    /// Native process manager (kill menu).
    Proc,
}

impl Provider {
    /// Every provider, in binary-name order.
    pub const ALL: [Self; 9] = [
        Self::Power,
        Self::Launch,
        Self::Shot,
        Self::Theme,
        Self::Clip,
        Self::Center,
        Self::Wallpaper,
        Self::Wifi,
        Self::Proc,
    ];

    /// Parse a provider subcommand/binary name (`None` for anything else).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "power" => Some(Self::Power),
            "launch" => Some(Self::Launch),
            "shot" => Some(Self::Shot),
            "theme" => Some(Self::Theme),
            "clip" => Some(Self::Clip),
            "center" => Some(Self::Center),
            "wallpaper" => Some(Self::Wallpaper),
            "wifi" => Some(Self::Wifi),
            "proc" => Some(Self::Proc),
            _ => None,
        }
    }

    /// Subcommand name (also the `ACTION:` provider field).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Power => power::PROVIDER,
            Self::Launch => launch::PROVIDER,
            Self::Shot => shot::PROVIDER,
            Self::Theme => theme_::PROVIDER,
            Self::Clip => clip::PROVIDER,
            Self::Center => center::PROVIDER,
            Self::Wallpaper => wallpaper::PROVIDER,
            Self::Wifi => wifi::PROVIDER,
            Self::Proc => proc::PROVIDER,
        }
    }

    /// Binary name (`flex-<name>`), resolved next to the running dispatcher.
    #[must_use]
    pub fn bin_name(self) -> String {
        format!("flex-{}", self.name())
    }

    /// Popup variant for this provider (`menu` for power/shot/theme/wifi,
    /// `menu-wide` for launch/clip/center/wallpaper/proc — see
    /// [`popup::MENU_VARIANT`] / [`popup::WIDE_VARIANT`]).
    #[must_use]
    pub fn variant(self) -> &'static str {
        match self {
            Self::Power | Self::Shot | Self::Theme | Self::Wifi => popup::MENU_VARIANT,
            Self::Launch | Self::Clip | Self::Center | Self::Wallpaper | Self::Proc => {
                popup::WIDE_VARIANT
            }
        }
    }

    /// Window class for this provider's popup variant.
    #[must_use]
    pub fn variant_class(self) -> &'static str {
        match self {
            Self::Power | Self::Shot | Self::Theme | Self::Wifi => popup::MENU_CLASS,
            Self::Launch | Self::Clip | Self::Center | Self::Wallpaper | Self::Proc => {
                popup::WIDE_CLASS
            }
        }
    }
}

/// Hidden ranking-engine flag values (`--filter-mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
enum FilterModeArg {
    /// Tiered fuzzy (default).
    #[default]
    Spec,
    /// Subsequence predicate, provider order preserved.
    Legacy,
}

impl FilterModeArg {
    /// Engine value behind the flag.
    #[must_use]
    fn to_engine(self) -> flex_core::filter::FilterMode {
        match self {
            Self::Spec => flex_core::filter::FilterMode::Spec,
            Self::Legacy => flex_core::filter::FilterMode::Legacy,
        }
    }
}

/// Parse an upstream character-set name.
fn parse_char_set(name: &str) -> Result<CharSetName, String> {
    CharSetName::parse(name).ok_or_else(|| {
        format!("'{name}' is not a built-in character set (default, compat, extracompat)")
    })
}

/// Parse an upstream theme name.
fn parse_theme(name: &str) -> Result<ThemeName, String> {
    ThemeName::parse(name)
        .ok_or_else(|| format!("'{name}' is not a built-in theme (default, nocolor, plain)"))
}

/// Parse an upstream peaks mode.
fn parse_peaks(name: &str) -> Result<Peaks, String> {
    match name {
        "off" => Ok(Peaks::Off),
        "mono" => Ok(Peaks::Mono),
        "auto" => Ok(Peaks::Auto),
        other => Err(format!("'{other}' is not a peaks mode (off, mono, auto)")),
    }
}

/// Canonical CLI spelling of a peaks mode.
#[must_use]
pub fn peaks_as_str(peaks: Peaks) -> &'static str {
    match peaks {
        Peaks::Off => "off",
        Peaks::Mono => "mono",
        Peaks::Auto => "auto",
    }
}

/// Canonical CLI spelling of a filter mode.
#[must_use]
pub fn filter_mode_as_str(mode: flex_core::filter::FilterMode) -> &'static str {
    match mode {
        flex_core::filter::FilterMode::Spec => "spec",
        flex_core::filter::FilterMode::Legacy => "legacy",
    }
}

/// Global presentation flags shared by every entry point (upstream
/// `-s/-t/-p/--filter-mode`).
#[derive(Debug, Args)]
pub struct GlobalStyle {
    /// Ranking engine override (R1 escape hatch; `legacy` preserves
    /// provider order instead of score-reordering).
    #[arg(long, hide = true, default_value = "spec", global = true)]
    filter_mode: FilterModeArg,

    /// Character set (upstream `-s/--char-set`): default, compat, extracompat.
    #[arg(
        short = 's',
        long,
        default_value = "default",
        value_parser = parse_char_set,
        global = true
    )]
    char_set: CharSetName,

    /// Theme (upstream `-t/--theme`): default, nocolor, plain.
    #[arg(
        short = 't',
        long,
        default_value = "default",
        value_parser = parse_theme,
        global = true
    )]
    theme: ThemeName,

    /// Peak meters (upstream `-p/--peaks`): off, mono, auto.
    #[arg(
        short = 'p',
        long,
        default_value = "auto",
        value_parser = parse_peaks,
        global = true
    )]
    peaks: Peaks,
}

impl GlobalStyle {
    /// Engine-ready presentation flags behind the CLI globals.
    #[must_use]
    pub fn options(self) -> StyleOptions {
        StyleOptions {
            filter_mode: self.filter_mode.to_engine(),
            char_set: self.char_set,
            theme: self.theme,
            peaks: self.peaks,
        }
    }
}

/// Presentation flags shared by every provider (upstream `-s/-t/-p/--filter-mode`).
#[derive(Debug, Clone, Copy)]
pub struct StyleOptions {
    /// Ranking engine for the filtered view.
    pub filter_mode: flex_core::filter::FilterMode,
    /// Character set for box drawing.
    pub char_set: CharSetName,
    /// Color theme.
    pub theme: ThemeName,
    /// Peak-meter mode.
    pub peaks: Peaks,
}

impl StyleOptions {
    /// Apply the flags to a menu.
    #[must_use]
    pub fn apply(self, mut menu: Menu) -> Menu {
        menu.app.filter_mode = self.filter_mode;
        menu.char_set = CharSet::get(self.char_set);
        menu.theme = Theme::get(self.theme);
        menu.peaks = self.peaks;
        menu
    }
}

/// Build the interactive menu for `provider`.
///
/// This is the single home of per-provider menu construction (moved out of
/// the old `main.rs` `run()` match so the dispatcher and the eight binaries
/// cannot diverge). Empty `clip`/`wallpaper` stores keep their historical
/// behavior: a `flex: <provider>`: diagnostic and exit 130, no menu.
///
/// # Errors
///
/// Currently infallible (`Ok` always); the `Result` keeps the shared
/// call-site shape for providers whose construction may fail later.
pub fn build_menu(provider: Provider, style: StyleOptions) -> Result<Menu> {
    match provider {
        Provider::Power => {
            let tab = power::power_tab();
            Ok(style.apply(menu(power::PROVIDER, vec![tab])))
        }
        Provider::Launch => {
            let tab = launch::launch_tab();
            Ok(style.apply(menu(launch::PROVIDER, vec![tab])))
        }
        Provider::Shot => {
            let tab = shot::shot_tab();
            Ok(style.apply(menu(shot::PROVIDER, vec![tab])))
        }
        Provider::Theme => {
            let tab = theme_::theme_tab();
            Ok(style.apply(menu(theme_::PROVIDER, vec![tab])))
        }
        Provider::Clip => {
            let tab = clip::clip_tab();
            if tab.rows.is_empty() {
                flex_core::diag::warn("flex: clip: no history yet");
                std::process::exit(EXIT_CANCELLED);
            }
            Ok(style.apply(menu(clip::PROVIDER, vec![tab])))
        }
        Provider::Center => Ok(style.apply(center::center_menu())),
        Provider::Wallpaper => {
            let tab = wallpaper::wallpaper_tab();
            if tab.rows.is_empty() {
                flex_core::diag::warn("flex: wallpaper: no wallpapers found");
                std::process::exit(EXIT_CANCELLED);
            }
            let mut built = style.apply(menu(wallpaper::PROVIDER, vec![tab]));
            // Image previews need a kitty-compatible terminal; everywhere
            // else the picker is a plain list (no pane reserved).
            built.preview = flex_core::preview::enabled();
            Ok(built)
        }
        Provider::Wifi => Ok(style.apply(wifi::menu())),
        Provider::Proc => Ok(style.apply(menu(proc::PROVIDER, vec![proc::proc_tab()]))),
    }
}

/// This process's argv for a popup re-exec: the current executable plus the
/// actual arguments (mirrors the wrappers' `popup.sh <variant> "$0" "$@"`).
///
/// # Errors
///
/// When the current executable path cannot be read.
pub fn self_argv() -> Result<Vec<String>> {
    let exe = std::env::current_exe().context("cannot read the current executable path")?;
    let mut argv = vec![exe.display().to_string()];
    argv.extend(std::env::args().skip(1));
    Ok(argv)
}

/// Popup guard: inside a popup (`POPUP_KITTY == "1"`) this is a no-op; outside
/// one it toggles this provider's popup variant with this process's argv and
/// exits `0` — the `POPUP_KITTY` re-exec the wrappers used to own.
///
/// # Errors
///
/// When the current argv cannot be read or the toggle (probe/spawn) fails.
/// The error carries no `flex:` prefix; [`fail`] adds it.
pub fn popup_guard(provider: Provider) -> Result<()> {
    if popup::in_popup() {
        return Ok(());
    }
    popup::toggle_with(provider.variant_class(), &self_argv()?)?;
    std::process::exit(EXIT_OK);
}

/// Run the select loop for `provider` and emit the result.
///
/// [`flex_core::run::run_capture`] runs the interactive loop in-process;
/// terminal outcomes map exactly like the old `run::run`: chosen, deleted,
/// toggled and target rows print one `ACTION:` line and exit `0`;
/// `Quit{code}` exits with `code` and prints nothing; `Cancelled` exits
/// 130. Only TTY/emit failures return; everything else diverges.
///
/// Both the default select+execute path (whose execute phase is still a
/// stub — see the `TODO(executor)` markers at the call sites) and
/// `--print-action` share this function, so the row→action mapping under
/// probe is the mapping the menu really runs.
///
/// # Errors
///
/// When the terminal cannot initialize, frames cannot draw, events cannot
/// be polled/read, or the `ACTION:` line cannot be written. The error
/// carries no `flex:` prefix; [`fail`] adds it.
pub fn run_select(provider: Provider, style: StyleOptions) -> Result<()> {
    let built = build_menu(provider, style)?;
    emit_outcome(flex_core::run::run_capture(built)?)
}

/// Emit one terminal outcome and exit (the diverging half of [`run_select`]).
///
/// # Errors
///
/// When the `ACTION:` line cannot be written. Every success path diverges
/// via `process::exit`, so `Ok` is unreachable in practice but keeps the
/// signature honest for the emit-failure case.
fn emit_outcome(outcome: Outcome) -> Result<()> {
    match outcome {
        Outcome::Chosen {
            provider,
            action_id,
            label,
        } => {
            flex_core::backend::emit_action(&provider, &action_id, &label)?;
            std::process::exit(EXIT_OK);
        }
        Outcome::Delete {
            provider,
            action_id,
            label,
        } => {
            flex_core::backend::emit_delete(&provider, &action_id, &label)?;
            std::process::exit(EXIT_OK);
        }
        Outcome::Toggle {
            provider,
            action_id,
            label,
        } => {
            flex_core::backend::emit_toggle(&provider, &action_id, &label)?;
            std::process::exit(EXIT_OK);
        }
        Outcome::Target {
            provider,
            row,
            target,
            title,
        } => {
            flex_core::backend::emit_target(&provider, &row, &target, &title)?;
            std::process::exit(EXIT_OK);
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}

/// Canonical flag tail for re-execing a provider binary: `-s/-t/-p` plus
/// `--filter-mode` in fixed order, with `--print-action` last when set.
///
/// Pure (no I/O, no spawn), so the dispatcher's flag-ordering is unit
/// testable without opening a TUI.
#[must_use]
pub fn canonical_tail(style: StyleOptions, print_action: bool) -> Vec<String> {
    let mut tail = vec![
        "-s".to_string(),
        style.char_set.as_str().to_string(),
        "-t".to_string(),
        style.theme.as_str().to_string(),
        "-p".to_string(),
        peaks_as_str(style.peaks).to_string(),
        "--filter-mode".to_string(),
        filter_mode_as_str(style.filter_mode).to_string(),
    ];
    if print_action {
        tail.push("--print-action".to_string());
    }
    tail
}

/// Sole owner of the `flex: error:` prefix: print it once and exit 1.
///
/// Every entry point's `main` funnels fallible work through this, so error
/// values below the runner must never carry their own `flex:` prefix
/// (B-022/B-027) — otherwise the line would read
/// `flex: error: flex: …`. Exit code is [`EXIT_ERROR`].
pub fn fail(err: &anyhow::Error) -> ! {
    flex_core::diag::warn(&format!("flex: error: {err:#}"));
    std::process::exit(EXIT_ERROR);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> StyleOptions {
        StyleOptions {
            filter_mode: flex_core::filter::FilterMode::Spec,
            char_set: CharSetName::Default,
            theme: ThemeName::Default,
            peaks: Peaks::Auto,
        }
    }

    #[test]
    fn providers_parse_by_subcommand_name() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.name()), Some(provider));
        }
        assert_eq!(Provider::parse("bogus"), None);
        assert_eq!(Provider::parse(""), None);
        assert_eq!(Provider::parse("Power"), None, "match is lowercase-only");
    }

    #[test]
    fn providers_map_to_the_wrappers_popup_table() {
        for provider in Provider::ALL {
            let (variant, class) = match provider.name() {
                "power" | "shot" | "theme" | "wifi" => ("menu", popup::MENU_CLASS),
                "launch" | "clip" | "center" | "wallpaper" | "proc" => {
                    ("menu-wide", popup::WIDE_CLASS)
                }
                other => panic!("unexpected provider {other}"),
            };
            assert_eq!(provider.variant(), variant);
            assert_eq!(provider.variant_class(), class);
            assert_eq!(popup::class_for(provider.variant()), Some(class));
        }
    }

    #[test]
    fn bin_names_are_the_flex_dash_spelling() {
        assert_eq!(Provider::Power.bin_name(), "flex-power");
        assert_eq!(Provider::Wallpaper.bin_name(), "flex-wallpaper");
    }

    #[test]
    fn canonical_tail_is_ordered_and_complete() {
        assert_eq!(
            canonical_tail(style(), false),
            vec![
                "-s",
                "default",
                "-t",
                "default",
                "-p",
                "auto",
                "--filter-mode",
                "spec",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn canonical_tail_appends_print_action_last() {
        let tail = canonical_tail(style(), true);
        assert_eq!(
            tail.last().map(String::as_str),
            Some("--print-action"),
            "probe flag trails the style flags"
        );
    }

    #[test]
    fn build_menu_tags_every_provider() {
        // Non-empty providers build a menu tagged with their own name; the
        // two empty-store providers (`clip`, `wallpaper`) exit 130 instead
        // of returning, so they are covered by the integration prefix test.
        for provider in [
            Provider::Power,
            Provider::Launch,
            Provider::Shot,
            Provider::Theme,
            Provider::Center,
            Provider::Wifi,
            Provider::Proc,
        ] {
            let built = build_menu(provider, style()).expect("menu builds");
            assert_eq!(built.provider, provider.name());
        }
    }
}
