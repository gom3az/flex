//! `flex` binary: compat dispatcher over the eight `flex-<provider>` binaries.
//!
//! `flex <provider> [flags]` re-execs the matching `flex-<provider>` binary
//! with flags reconstructed in canonical order (so `flex -t nocolor launch`
//! ≡ `flex launch -t nocolor`); `flex popup <menu|menu-wide> <cmd…>`
//! toggles the popup for dotfiles `kill-menu.sh`. All providers run the
//! full TUI event loop and execute the selected row in-process
//! (`exec/*.rs`); the wrappers are retired.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use flex_core::backend::EXIT_ERROR;
use flex_rice::runner::{self, GlobalStyle, Provider, StyleOptions};

/// Compat dispatcher: parse a provider (or `popup`) and re-exec or toggle.
#[derive(Debug, Parser)]
#[command(name = "flex", version, about = "Reusable TUI menu library")]
struct Cli {
    /// Global presentation flags (accepted before or after the provider).
    #[command(flatten)]
    style: GlobalStyle,

    /// Which menu provider to show (or the `popup` toggle helper).
    #[command(subcommand)]
    command: Command,
}

/// Menu providers (each re-execs its `flex-<provider>` binary) plus the
/// `popup` toggle helper for dotfiles `kill-menu.sh`.
#[derive(Debug, Subcommand)]
enum Command {
    /// Toggle a popup variant running the given command.
    Popup {
        /// Popup variant: `menu` or `menu-wide` (class names accepted too).
        variant: String,
        /// Command to run inside the popup when opening.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Power menu (shutdown/reboot/…).
    Power {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Application launcher.
    Launch {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Screenshot flow.
    Shot {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Theme switcher.
    Theme {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
        /// Optional CLI verb (`list`/`current`/`activate`/`delete`).
        #[command(subcommand)]
        op: Option<ThemeVerb>,
    },
    /// Clipboard history.
    Clip {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
        /// Optional non-interactive store verb (`add`/`pin`/`unpin`/`current`/`watch`/`daemon`).
        #[command(subcommand)]
        op: Option<ClipVerb>,
    },
    /// Control center (volume/brightness/network).
    Center {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Wallpaper picker (image previews).
    Wallpaper {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
        /// Optional `set <path>` verb.
        #[command(subcommand)]
        op: Option<WallpaperVerb>,
    },
    /// Wi-Fi picker (connect/disconnect, radio on/off).
    Wifi {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Process manager (kill menu): filter and signal processes.
    Proc {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Network monitor and top bandwidth consumers.
    Net {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Native Bluetooth manager.
    Bt {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Notification Center drawer.
    Notify {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
    /// Power Profile switcher.
    Profile {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
}

/// Non-interactive `clip` verbs forwarded to `flex-clip` (mirrors the
/// `Op` enum in `bin/flex-clip.rs`).
#[derive(Debug, Subcommand)]
enum ClipVerb {
    /// Capture the current clipboard into the history store.
    Add,
    /// Pin the current clipboard entry, or `TEXT` when given.
    Pin { text: Option<String> },
    /// Unpin the current clipboard entry, or `TEXT` when given.
    Unpin { text: Option<String> },
    /// Print the current clipboard entry (decoded).
    Current,
    /// Watch Wayland selection updates and add them to history.
    #[command(alias = "daemon")]
    Watch,
}

/// Re-exec tokens for a `clip` verb (`add`/`pin TEXT`/`unpin TEXT`/`current`/`watch`/`daemon`).
fn clip_verb_tail(op: Option<&ClipVerb>) -> Vec<String> {
    match op {
        None => Vec::new(),
        Some(ClipVerb::Add) => vec![String::from("add")],
        Some(ClipVerb::Current) => vec![String::from("current")],
        Some(ClipVerb::Watch) => vec![String::from("watch")],
        Some(ClipVerb::Pin { text }) => verb_with_text("pin", text.as_deref()),
        Some(ClipVerb::Unpin { text }) => verb_with_text("unpin", text.as_deref()),
    }
}

/// `[verb]` or `[verb, text]` for the pin/unpin verbs.
fn verb_with_text(verb: &str, text: Option<&str>) -> Vec<String> {
    let mut tail = vec![verb.to_string()];
    if let Some(value) = text {
        tail.push(value.to_string());
    }
    tail
}

/// Non-interactive `theme` verbs forwarded to `flex-theme` (mirrors the `Op`
/// enum in `bin/flex-theme.rs`).
#[derive(Debug, Subcommand)]
enum ThemeVerb {
    /// List available themes.
    List,
    /// Print the active theme and wallpaper.
    Current,
    /// Activate a theme by name.
    Activate { name: String },
    /// Delete an available theme by name.
    Delete { name: String },
}

/// Re-exec tokens for a `theme` verb.
fn theme_verb_tail(op: Option<&ThemeVerb>) -> Vec<String> {
    match op {
        None => Vec::new(),
        Some(ThemeVerb::List) => vec![String::from("list")],
        Some(ThemeVerb::Current) => vec![String::from("current")],
        Some(ThemeVerb::Activate { name }) => verb_with_text("activate", Some(name)),
        Some(ThemeVerb::Delete { name }) => verb_with_text("delete", Some(name)),
    }
}

/// Non-interactive `wallpaper` verb forwarded to `flex-wallpaper` (mirrors
/// the `Op` enum in `bin/flex-wallpaper.rs`).
#[derive(Debug, Subcommand)]
enum WallpaperVerb {
    /// Set the wallpaper to a file.
    Set { path: String },
}

/// Re-exec tokens for a `wallpaper` verb.
fn wallpaper_verb_tail(op: Option<&WallpaperVerb>) -> Vec<String> {
    match op {
        None => Vec::new(),
        Some(WallpaperVerb::Set { path }) => verb_with_text("set", Some(path)),
    }
}

fn main() {
    runner::init_logging();
    // `flex: error:` is added once, in the runner, and nowhere else: errors
    // bubbling up must carry no `flex:` prefix of their own (B-022).
    if let Err(err) = run() {
        runner::fail(&err);
    }
}

/// Parse args and dispatch: `popup` toggles, anything else re-execs the
/// provider binary with canonical flags.
///
/// # Errors
///
/// Returns an error when the popup toggle fails, the sibling provider
/// binary cannot be located, or its spawn fails. Messages carry no `flex:`
/// prefix; `main` adds it via the runner.
fn run() -> Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    match &cli.command {
        Command::Popup { variant, cmd } => run_popup(variant, cmd),
        Command::Power { print_action } => reexec(Provider::Power, style, *print_action, &[]),
        Command::Launch { print_action } => reexec(Provider::Launch, style, *print_action, &[]),
        Command::Shot { print_action } => reexec(Provider::Shot, style, *print_action, &[]),
        Command::Theme { print_action, op } => reexec(
            Provider::Theme,
            style,
            *print_action,
            &theme_verb_tail(op.as_ref()),
        ),
        Command::Clip { print_action, op } => reexec(
            Provider::Clip,
            style,
            *print_action,
            &clip_verb_tail(op.as_ref()),
        ),
        Command::Center { print_action } => reexec(Provider::Center, style, *print_action, &[]),
        Command::Wallpaper { print_action, op } => reexec(
            Provider::Wallpaper,
            style,
            *print_action,
            &wallpaper_verb_tail(op.as_ref()),
        ),
        Command::Wifi { print_action } => reexec(Provider::Wifi, style, *print_action, &[]),
        Command::Proc { print_action } => reexec(Provider::Proc, style, *print_action, &[]),
        Command::Net { print_action } => {
            reexec(Provider::Net, style, *print_action, &[String::from("-m")])
        }
        Command::Bt { print_action } => reexec(Provider::Bt, style, *print_action, &[]),
        Command::Notify { print_action } => reexec(
            Provider::Notify,
            style,
            *print_action,
            &[String::from("-m")],
        ),
        Command::Profile { print_action } => reexec(Provider::Profile, style, *print_action, &[]),
    }
}

/// Toggle a popup variant running `cmd` (the `kill-menu.sh` helper).
///
/// # Errors
///
/// When `cmd` is empty (`popup.sh` exits 1 on empty args — parity is exact)
/// or the toggle itself fails. Messages carry no `flex:` prefix.
fn run_popup(variant: &str, cmd: &[String]) -> Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("popup: expected a variant and a command (try `flex popup menu <cmd…>`)");
    }
    flex_rice::popup::toggle_with(variant, cmd)
}

/// Full re-exec argv for a provider binary: its `flex-<name>` program plus
/// the canonical flag tail.
///
/// Pure (no I/O, no spawn): flag order is identical no matter where clap
/// accepted the globals, so this is unit-testable without opening a TUI.
#[must_use]
fn reexec_argv(provider: Provider, style: StyleOptions, print_action: bool) -> Vec<String> {
    let mut argv = vec![provider.bin_name()];
    argv.extend(runner::canonical_tail(style, print_action));
    argv
}

/// [`reexec_argv`] plus trailing verb tokens (e.g. `["pin", "text"]`), which
/// the child parses as its subcommand after the global flags.
#[must_use]
fn reexec_argv_with(
    provider: Provider,
    style: StyleOptions,
    print_action: bool,
    extra: &[String],
) -> Vec<String> {
    let mut argv = reexec_argv(provider, style, print_action);
    argv.extend(extra.iter().cloned());
    argv
}

/// Sibling `flex-<name>` binary next to the running dispatcher.
///
/// # Errors
///
/// When the current executable path cannot be read or has no parent.
fn sibling_binary(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot read the current executable path")?;
    let Some(dir) = exe.parent() else {
        anyhow::bail!("cannot locate {name} next to the flex binary");
    };
    Ok(dir.join(name))
}

/// Re-exec the provider binary with canonical flags (plus any verb tokens)
/// and exit with its code.
///
/// # Errors
///
/// When the sibling binary cannot be located or spawned. A running child
/// that exits (any code) never returns an error: its code becomes ours.
fn reexec(
    provider: Provider,
    style: StyleOptions,
    print_action: bool,
    extra: &[String],
) -> Result<()> {
    let argv = reexec_argv_with(provider, style, print_action, extra);
    let program = argv.first().map(String::as_str).unwrap_or_default();
    let bin = sibling_binary(program)?;
    let tail: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let status = std::process::Command::new(&bin)
        .args(&tail)
        .status()
        .with_context(|| format!("cannot spawn {}", bin.display()))?;
    std::process::exit(status.code().unwrap_or(EXIT_ERROR));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(argv: &[&str]) -> (Provider, StyleOptions, bool) {
        let cli = Cli::try_parse_from(argv).expect("test argv parses");
        let style = cli.style.options();
        match cli.command {
            Command::Launch { print_action } => (Provider::Launch, style, print_action),
            other => panic!("expected launch, got {other:?}"),
        }
    }

    #[test]
    fn global_flags_reconstruct_identically_before_or_after_the_provider() {
        let (provider, style, print_action) = parsed(&["flex", "-t", "nocolor", "launch"]);
        let leading = reexec_argv(provider, style, print_action);
        let (provider, style, print_action) = parsed(&["flex", "launch", "-t", "nocolor"]);
        let trailing = reexec_argv(provider, style, print_action);
        let (provider, style, print_action) = parsed(&["flex", "launch"]);
        let bare = reexec_argv(provider, style, print_action);
        assert_eq!(
            leading, trailing,
            "`flex -t nocolor launch` ≡ `flex launch -t nocolor`"
        );
        assert_eq!(
            leading,
            vec![
                "flex-launch",
                "-s",
                "default",
                "-t",
                "nocolor",
                "-p",
                "auto",
                "--filter-mode",
                "spec",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
            "canonical order is -s/-t/-p/--filter-mode"
        );
        assert_ne!(leading, bare, "a theme override changes the argv");
    }

    #[test]
    fn print_action_survives_the_reconstruction() {
        let cli = Cli::try_parse_from(["flex", "launch", "--print-action"]).expect("parses");
        let style = cli.style.options();
        let Command::Launch { print_action } = cli.command else {
            panic!("expected launch");
        };
        let argv = reexec_argv(Provider::Launch, style, print_action);
        assert_eq!(
            argv.last().map(String::as_str),
            Some("--print-action"),
            "probe flag trails the style flags"
        );
    }

    #[test]
    fn popup_rejects_an_empty_cmd_without_spawning() {
        assert!(
            run_popup("menu", &[]).is_err(),
            "empty cmd errors like popup.sh (exits 1 on empty args)"
        );
    }

    #[test]
    fn all_providers_resolve_to_sibling_binary_names() {
        for provider in Provider::ALL {
            let argv = reexec_argv(provider, default_style(), false);
            assert_eq!(
                argv.first().map(String::as_str),
                Some(provider.bin_name()).as_deref(),
                "program is the sibling binary name"
            );
        }
    }

    /// Test-only default style (mirrors the CLI defaults).
    fn default_style() -> StyleOptions {
        StyleOptions {
            filter_mode: flex_core::filter::FilterMode::Spec,
            char_set: flex_core::CharSetName::Default,
            theme: flex_core::ThemeName::Default,
            peaks: flex_core::Peaks::Auto,
        }
    }
}
