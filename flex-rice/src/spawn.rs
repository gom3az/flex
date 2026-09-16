//! Process spawning with a bounded transient-`ETXTBSY` retry.
//!
//! A tool path is momentarily "busy" (`ExecutableFileBusy`) while some
//! process holds it open for writing; a forked child can inherit the writer's
//! descriptor until its own `exec`. That is rare in production but inherent to
//! the stub-`PATH` tests, which write a tool script and exec it while sibling
//! tests fork. The condition clears in microseconds, so [`retrying`] sleeps
//! briefly and retries a bounded number of times; every other error
//! propagates unchanged.
//!
//! [`status`], [`output`] and [`spawn`] are the executor-facing wrappers; use
//! them instead of calling the `Command` method directly.

use std::io;
use std::process::{Child, Command, ExitStatus, Output};
use std::time::Duration;

/// Total attempts before giving up on a persistently busy executable.
const ATTEMPTS: u32 = 50;
/// Sleep between attempts (the inherited descriptor is released at the
/// sibling's next `exec`, so a couple of milliseconds is plenty).
const BACKOFF: Duration = Duration::from_millis(2);

/// Run `run`, retrying while it fails with a transient `ExecutableFileBusy`.
///
/// # Errors
///
/// Re-returns the first non-transient error immediately, and the last busy
/// error once the attempt budget is exhausted.
pub(crate) fn retrying<T>(mut run: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match run() {
            Ok(value) => return Ok(value),
            Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                last = Some(err);
                std::thread::sleep(BACKOFF);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("spawn: exec stayed busy")))
}

/// [`retrying`] around [`Command::status`].
///
/// # Errors
///
/// As [`Command::status`], plus a persistently busy executable.
pub(crate) fn status(cmd: &mut Command) -> io::Result<ExitStatus> {
    retrying(|| cmd.status())
}

/// [`retrying`] around [`Command::output`].
///
/// # Errors
///
/// As [`Command::output`], plus a persistently busy executable.
pub(crate) fn output(cmd: &mut Command) -> io::Result<Output> {
    retrying(|| cmd.output())
}

/// [`retrying`] around [`Command::spawn`].
///
/// # Errors
///
/// As [`Command::spawn`], plus a persistently busy executable.
pub(crate) fn spawn(cmd: &mut Command) -> io::Result<Child> {
    retrying(|| cmd.spawn())
}

/// Retry-on-`ETXTBSY` variants of the [`Command`] execution methods, for the
/// chained builder call sites in the executors.
///
/// Bring into scope with `use crate::spawn::RetryExec as _;` and call
/// `…status_retrying()` / `…output_retrying()` / `…spawn_retrying()` where the
/// executor would otherwise call `status()` / `output()` / `spawn()`.
pub(crate) trait RetryExec {
    /// [`Command::status`] with the shared transient-`ETXTBSY` retry.
    ///
    /// # Errors
    ///
    /// As [`Command::status`], plus a persistently busy executable.
    fn status_retrying(&mut self) -> io::Result<ExitStatus>;

    /// [`Command::output`] with the shared transient-`ETXTBSY` retry.
    ///
    /// # Errors
    ///
    /// As [`Command::output`], plus a persistently busy executable.
    fn output_retrying(&mut self) -> io::Result<Output>;

    /// [`Command::spawn`] with the shared transient-`ETXTBSY` retry.
    ///
    /// # Errors
    ///
    /// As [`Command::spawn`], plus a persistently busy executable.
    fn spawn_retrying(&mut self) -> io::Result<Child>;
}

impl RetryExec for Command {
    fn status_retrying(&mut self) -> io::Result<ExitStatus> {
        status(self)
    }

    fn output_retrying(&mut self) -> io::Result<Output> {
        output(self)
    }

    fn spawn_retrying(&mut self) -> io::Result<Child> {
        spawn(self)
    }
}
