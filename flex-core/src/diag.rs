//! Best-effort diagnostics: writes that must never panic.
//!
//! `eprintln!`/`println!` panic when the write fails — a hung-up popup pty
//! gives `EIO`, a closed pipe `EPIPE`. The release profile is
//! `panic = "abort"`, so such a failure aborts the process instead of
//! reporting. A diagnostic is never worth aborting over, so these helpers
//! write and discard the error.
//!
//! Use these (not `eprintln!`/`println!`) on any runtime path where the
//! stream may already be gone. Rendering and the `ACTION:` emit do not go
//! through here: rendering writes to `/dev/tty`, and
//! [`emit_action`](crate::backend::emit_action) returns a `Result` the
//! caller can handle.

use std::io::Write as _;

/// Write one line to stderr, ignoring write failures.
pub fn warn(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Write `message` to stderr with no trailing newline, then flush, ignoring
/// write failures (the password prompts).
pub fn warn_inline(message: &str) {
    let mut err = std::io::stderr();
    let _ = write!(err, "{message}");
    let _ = err.flush();
}

/// Write one line to stdout, ignoring write failures.
pub fn note(message: &str) {
    let _ = writeln!(std::io::stdout(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_callable_without_panicking() {
        warn("test warning");
        warn("");
        warn_inline("test inline");
        note("test note");
    }
}
