//! `flex` binary: compat dispatcher over the eight `flex-<provider>` binaries.
//!
//! `flex <provider> [flags]` re-execs the matching `flex-<provider>` binary
//! with flags reconstructed in canonical order (so `flex -t nocolor launch`
//! ≡ `flex launch -t nocolor`); `flex popup <menu|menu-wide> <cmd…>`
//! toggles the popup for dotfiles `kill-menu.sh`. Hidden `--resolve`
//! lookups are handled inline, exactly as before. All providers run the
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
        /// Resolve a row-hash id to its desktop-id (`firefox.desktop`).
        /// Hidden id lookup: the retired launch/center wrappers used it to
        /// turn a space-free hash back into the `.desktop` file (B-021).
        #[arg(long, hide = true)]
        resolve: Option<String>,
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
        /// Resolve a row-hash id to its theme (directory) name. Hidden id
        /// lookup: the retired theme wrapper used it before handing the
        /// name to `theme-switcher.sh`.
        #[arg(long, hide = true)]
        resolve: Option<String>,
    },
    /// Clipboard history.
    Clip {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
        /// Resolve a content-hash id to its stored (`<NEWLINE>`-encoded)
        /// line. Hidden id lookup: the retired clip wrapper used it to turn
        /// the hash back into content.
        #[arg(long, hide = true)]
        resolve: Option<String>,
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
        /// Resolve a path-hash id to its absolute wallpaper path. Hidden id
        /// lookup: the retired wallpaper wrapper used it to turn the id back
        /// into a path.
        #[arg(long, hide = true)]
        resolve: Option<String>,
    },
    /// Wi-Fi picker (connect/disconnect, radio on/off).
    Wifi {
        /// Print the selected `ACTION:` line without executing it.
        #[arg(long)]
        print_action: bool,
    },
}

fn main() {
    // `flex: error:` is added once, in the runner, and nowhere else: errors
    // bubbling up must carry no `flex:` prefix of their own (B-022).
    if let Err(err) = run() {
        runner::fail(&err);
    }
}

/// Parse args and dispatch: `popup` toggles, `--resolve` answers inline,
/// anything else re-execs the provider binary with canonical flags.
///
/// # Errors
///
/// Returns an error when the popup toggle fails, a `--resolve` id is
/// unknown, the sibling provider binary cannot be located, or its spawn
/// fails. Messages carry no `flex:` prefix; `main` adds it via the runner.
fn run() -> Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    // Hidden id lookups (`flex <provider> --resolve <id>`) print the
    // provider identity behind a row id and exit; every other invocation
    // re-execs the provider binary (or toggles a popup).
    if resolve_lookup(&cli.command)?.is_some() {
        return Ok(());
    }
    match &cli.command {
        Command::Popup { variant, cmd } => run_popup(variant, cmd),
        Command::Power { print_action } => reexec(Provider::Power, style, *print_action),
        Command::Launch { print_action, .. } => reexec(Provider::Launch, style, *print_action),
        Command::Shot { print_action } => reexec(Provider::Shot, style, *print_action),
        Command::Theme { print_action, .. } => reexec(Provider::Theme, style, *print_action),
        Command::Clip { print_action, .. } => reexec(Provider::Clip, style, *print_action),
        Command::Center { print_action } => reexec(Provider::Center, style, *print_action),
        Command::Wallpaper { print_action, .. } => {
            reexec(Provider::Wallpaper, style, *print_action)
        }
        Command::Wifi { print_action } => reexec(Provider::Wifi, style, *print_action),
    }
}

/// Hidden id lookups: `flex <provider> --resolve <id>` prints the
/// provider identity the id stands for (desktop-id, theme name, wallpaper
/// path, clipboard line) and returns `Ok(Some(()))`.
///
/// `Ok(None)` means the command is a normal provider invocation. The message
/// for an unknown id is built here, in one place, and carries no `flex:`
/// prefix of its own — `main` adds that exactly once via the runner (B-022).
fn resolve_lookup(command: &Command) -> Result<Option<()>> {
    use flex_rice::providers;
    let (provider, id, resolved) = match command {
        Command::Clip {
            resolve: Some(hash),
            ..
        } => ("clip", hash, providers::clip::resolve(hash)),
        Command::Wallpaper {
            resolve: Some(id), ..
        } => (
            "wallpaper",
            id,
            providers::wallpaper::resolve(id).map(|path| path.display().to_string()),
        ),
        Command::Launch {
            resolve: Some(hash),
            ..
        } => ("launch", hash, providers::launch::resolve_id(hash)),
        Command::Theme {
            resolve: Some(hash),
            ..
        } => ("theme", hash, providers::theme_::resolve_name(hash)),
        _ => return Ok(None),
    };
    let Some(value) = resolved else {
        anyhow::bail!("{provider}: unknown id '{id}'");
    };
    flex_core::diag::note(&value);
    Ok(Some(()))
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

/// Re-exec the provider binary with canonical flags and exit with its code.
///
/// # Errors
///
/// When the sibling binary cannot be located or spawned. A running child
/// that exits (any code) never returns an error: its code becomes ours.
fn reexec(provider: Provider, style: StyleOptions, print_action: bool) -> Result<()> {
    let argv = reexec_argv(provider, style, print_action);
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
            Command::Launch { print_action, .. } => (Provider::Launch, style, print_action),
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
        let Command::Launch { print_action, .. } = cli.command else {
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
