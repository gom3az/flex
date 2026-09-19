//! Terminal-agnostic popup spawning.
//!
//! [`detect`] reads the `TERMINAL` environment variable and [`spawn_argv`]
//! builds the full spawn argv for a popup window of a given class. Only
//! `kitty` is implemented today ([`TerminalKind::Kitty`]) — the enum is the
//! extension point, so adding a terminal is additive:
//!
//! 1. Add a variant to [`TerminalKind`] (document its popup CLI there).
//! 2. Add its basename aliases to [`parse_kind`] (`wezterm` also ships as
//!    `wezterm-gui`, for example — match every spelling users may export).
//! 3. Add a match arm to [`spawn_argv`] with that terminal's class plus
//!    marker env. The `POPUP_KITTY=1` marker must survive the port so
//!    [`in_popup`] keeps working — rename it only together with the shell
//!    wrappers that test it.
//! 4. Extend the unit tests below (argv shape, basename table, fallback).
//!
//! [`in_popup`]: crate::popup::in_popup
//!
//! Known future degradation: `foot` has no per-instance font-size override
//! on its CLI (`kitty -o font_size=10` has no `foot` equivalent), so a
//! future `foot` arm would inherit the ambient font size until `foot` gains
//! such an override. That arm must still pass the `POPUP_KITTY=1` marker.

/// The terminal emulator used to host popups.
///
/// Only `kitty` is implemented today; the missing variants are deliberate —
/// see the module docs for how to add one (`foot`, `wezterm` and `ghostty`
/// are the expected next candidates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    /// `kitty`, matched by basename (`kitty`, `/usr/bin/kitty`, …).
    ///
    /// Popup template: `kitty --class <class> -o font_size=10 -e env
    /// POPUP_KITTY=1 <cmd…>` — the `font_size=10` override is the mixer
    /// parity from `popup.sh` (denser rows, main config untouched).
    Kitty,
}

const KITTY_BASENAME: &str = "kitty";

/// Basename of a `TERMINAL` value (`/usr/bin/kitty` → `kitty`).
///
/// Plain names pass through unchanged; empty input stays empty, so it falls
/// through to the unknown-terminal fallback in [`detect`].
fn basename(command: &str) -> &str {
    match command.rsplit('/').next() {
        Some(base) => base,
        None => command,
    }
}

/// Match a `TERMINAL` value against the known-terminal table.
///
/// The match is on the basename, so `/usr/bin/kitty` resolves exactly like
/// `kitty`. Returns `None` for empty or unknown values — callers fall back
/// to [`TerminalKind::Kitty`] with a warning (see [`detect`]).
#[must_use]
pub fn parse_kind(raw: &str) -> Option<TerminalKind> {
    if basename(raw) == KITTY_BASENAME {
        Some(TerminalKind::Kitty)
    } else {
        None
    }
}

/// Detect the terminal from the `TERMINAL` environment variable.
///
/// Basename-matched through [`parse_kind`], so both `kitty` and
/// `/usr/bin/kitty` resolve. An unknown, empty or unset value prints exactly
/// one warning line to stderr and falls back to [`TerminalKind::Kitty`]: an
/// ambient variable must never dead-key a keybind, so this never exits.
#[must_use]
pub fn detect() -> TerminalKind {
    let raw = std::env::var("TERMINAL").unwrap_or_default();
    if let Some(kind) = parse_kind(&raw) {
        kind
    } else {
        let shown: &str = raw.lines().next().unwrap_or_default();
        flex_core::diag::warn(&format!(
            "flex: warning: unknown TERMINAL {shown:?}, falling back to kitty"
        ));
        TerminalKind::Kitty
    }
}

/// Build the full spawn argv for a popup of `class` running `cmd`.
///
/// For [`TerminalKind::Kitty`] this is `kitty --class <class> -o
/// font_size=10 -e env POPUP_KITTY=1 <cmd…>` — the argv form of the
/// `popup.sh` spawn line, so the marker env (`POPUP_KITTY=1`) and the
/// font-size override carry over unchanged. An empty `cmd` yields a bare
/// terminal of that class (the user's shell, still marked as a popup).
#[must_use]
pub fn spawn_argv(kind: TerminalKind, class: &str, cmd: &[String]) -> Vec<String> {
    match kind {
        TerminalKind::Kitty => {
            let mut argv = Vec::with_capacity(8 + cmd.len());
            argv.push("kitty".to_string());
            argv.push("--class".to_string());
            argv.push(class.to_string());
            argv.push("-o".to_string());
            argv.push("font_size=10".to_string());
            argv.push("-e".to_string());
            argv.push("env".to_string());
            argv.push("POPUP_KITTY=1".to_string());
            argv.extend(cmd.iter().cloned());
            argv
        }
    }
}

/// Build the argv that runs a user command inside a terminal of `kind`.
///
/// For [`TerminalKind::Kitty`] this is `kitty -e <cmd…>`. This is the
/// "run this app in a terminal" form used for `Terminal=true` desktop
/// entries ([`crate::exec::launch`]) — **not** a
/// popup. Unlike [`spawn_argv`] it carries no `--class`, no
/// `-o font_size=10` override and no `POPUP_KITTY=1` marker: the launched
/// application must not be mistaken for a flex popup by
/// [`in_popup`](crate::popup::in_popup).
#[must_use]
pub fn exec_argv(kind: TerminalKind, cmd: &[String]) -> Vec<String> {
    match kind {
        TerminalKind::Kitty => {
            let mut argv = Vec::with_capacity(2 + cmd.len());
            argv.push("kitty".to_string());
            argv.push("-e".to_string());
            argv.extend(cmd.iter().cloned());
            argv
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(words: &[&str]) -> Vec<String> {
        words.iter().map(ToString::to_string).collect()
    }

    fn restore_terminal_env(saved: Option<String>) {
        match saved {
            Some(value) => std::env::set_var("TERMINAL", value),
            None => std::env::remove_var("TERMINAL"),
        }
    }

    #[test]
    fn kitty_argv_matches_popup_sh_spawn_line() {
        assert_eq!(
            spawn_argv(TerminalKind::Kitty, "flex-menu", &cmd(&["flex", "power"])),
            cmd(&[
                "kitty",
                "--class",
                "flex-menu",
                "-o",
                "font_size=10",
                "-e",
                "env",
                "POPUP_KITTY=1",
                "flex",
                "power",
            ]),
        );
    }

    #[test]
    fn kitty_exec_argv_is_the_bare_run_a_command_form() {
        assert_eq!(
            exec_argv(TerminalKind::Kitty, &cmd(&["htop"])),
            cmd(&["kitty", "-e", "htop"]),
        );
        assert_eq!(
            exec_argv(TerminalKind::Kitty, &cmd(&["myapp", "--open", "file"])),
            cmd(&["kitty", "-e", "myapp", "--open", "file"]),
        );
    }

    #[test]
    fn exec_argv_has_no_popup_class_font_or_marker() {
        // A launched app must not inherit the popup template: no `--class`,
        // no `-o font_size=10`, no `POPUP_KITTY=1`.
        let argv = exec_argv(TerminalKind::Kitty, &cmd(&["htop"]));
        assert!(!argv.iter().any(|arg| arg == "--class"));
        assert!(!argv.iter().any(|arg| arg == "font_size=10"));
        assert!(!argv.iter().any(|arg| arg == "POPUP_KITTY=1"));
    }

    #[test]
    fn kitty_argv_with_empty_cmd_is_a_bare_marked_terminal() {
        assert_eq!(
            spawn_argv(TerminalKind::Kitty, "flex-menu-wide", &[]),
            cmd(&[
                "kitty",
                "--class",
                "flex-menu-wide",
                "-o",
                "font_size=10",
                "-e",
                "env",
                "POPUP_KITTY=1",
            ]),
        );
    }

    #[test]
    fn parse_kind_matches_basename_table() {
        assert_eq!(parse_kind("kitty"), Some(TerminalKind::Kitty));
        assert_eq!(parse_kind("/usr/bin/kitty"), Some(TerminalKind::Kitty));
        assert_eq!(parse_kind("bin/kitty"), Some(TerminalKind::Kitty));
    }

    #[test]
    fn parse_kind_rejects_empty_and_unknown() {
        assert_eq!(parse_kind(""), None);
        assert_eq!(parse_kind("foot"), None);
        assert_eq!(parse_kind("wezterm"), None);
        assert_eq!(parse_kind("ghostty"), None);
        assert_eq!(parse_kind("KITTY"), None, "match is case-sensitive");
        assert_eq!(parse_kind("kitty-foo"), None, "no prefix matching");
        assert_eq!(parse_kind("/usr/bin/foot"), None);
    }

    #[test]
    fn detect_accepts_kitty_spellings() {
        let saved = std::env::var("TERMINAL").ok();
        for value in ["kitty", "/usr/bin/kitty"] {
            std::env::set_var("TERMINAL", value);
            assert_eq!(detect(), TerminalKind::Kitty, "TERMINAL={value}");
        }
        restore_terminal_env(saved);
    }

    #[test]
    fn detect_falls_back_to_kitty_for_empty_unset_and_unknown() {
        // Every assertion below expects the same fallback, so concurrent
        // tests mutating `TERMINAL` cannot flake these: any value maps to
        // `Kitty` today (the warning goes to stderr by construction).
        let saved = std::env::var("TERMINAL").ok();
        std::env::remove_var("TERMINAL");
        assert_eq!(detect(), TerminalKind::Kitty, "unset falls back");
        std::env::set_var("TERMINAL", "");
        assert_eq!(detect(), TerminalKind::Kitty, "empty falls back");
        std::env::set_var("TERMINAL", "wezterm");
        assert_eq!(detect(), TerminalKind::Kitty, "unknown falls back");
        restore_terminal_env(saved);
    }
}
