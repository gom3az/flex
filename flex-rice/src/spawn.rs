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
        tracing::debug!("RetryExec::status_retrying: {:?}", self);
        status(self)
    }

    fn output_retrying(&mut self) -> io::Result<Output> {
        tracing::debug!("RetryExec::output_retrying: {:?}", self);
        output(self)
    }

    fn spawn_retrying(&mut self) -> io::Result<Child> {
        tracing::debug!("RetryExec::spawn_retrying: {:?}", self);
        spawn(self)
    }
}

#[allow(dead_code)]
pub(crate) trait AsyncRetryExec {
    async fn status_retrying(&mut self) -> io::Result<ExitStatus>;
    async fn output_retrying(&mut self) -> io::Result<Output>;
}

impl AsyncRetryExec for tokio::process::Command {
    async fn status_retrying(&mut self) -> io::Result<ExitStatus> {
        tracing::debug!("AsyncRetryExec::status_retrying: {:?}", self);
        // Note: we just loop natively, since we don't block the thread.
        let mut last = None;
        for _ in 0..ATTEMPTS {
            match self.status().await {
                Ok(value) => return Ok(value),
                Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                    last = Some(err);
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("spawn: exec stayed busy")))
    }

    async fn output_retrying(&mut self) -> io::Result<Output> {
        tracing::debug!("AsyncRetryExec::output_retrying: {:?}", self);
        let mut last = None;
        for _ in 0..ATTEMPTS {
            match self.output().await {
                Ok(value) => return Ok(value),
                Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                    last = Some(err);
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("spawn: exec stayed busy")))
    }
}

/// Non-blocking background worker thread helper with a lock-free result mailbox.
///
/// Spawns `f` on a background worker thread and deposits the result into a shared mailbox.
/// The main thread can non-blockingly check or take the result (e.g. during `Menu::tick`).
#[derive(Debug)]
pub(crate) struct BgTask<T> {
    result: std::sync::Arc<std::sync::Mutex<Option<T>>>,
}

static WAKEUP_NOTIFIER: std::sync::Mutex<Option<std::sync::Arc<tokio::sync::Notify>>> =
    std::sync::Mutex::new(None);

/// Register the current interactive menu's wakeup notifier.
pub fn set_wakeup_notifier(notify: std::sync::Arc<tokio::sync::Notify>) {
    if let Ok(mut slot) = WAKEUP_NOTIFIER.lock() {
        *slot = Some(notify);
    }
}

/// Awaken the running menu's event loop to immediately tick and redraw.
pub fn wake_ui() {
    if let Ok(slot) = WAKEUP_NOTIFIER.lock() {
        if let Some(notify) = slot.as_ref() {
            notify.notify_one();
        }
    }
}

impl<T: Send + 'static> BgTask<T> {
    /// Create a completed `BgTask` holding `val` immediately.
    pub(crate) fn ready(val: T) -> Self {
        Self {
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(val))),
        }
    }

    /// Spawn a background worker thread executing `f`.
    pub(crate) fn spawn<F>(f: F) -> Self
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = std::sync::Arc::clone(&result);
        std::thread::spawn(move || {
            let val = f();
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(val);
            }
            wake_ui();
        });
        Self { result }
    }

    /// Non-blockingly take the completed result if available.
    pub(crate) fn take(&self) -> Option<T> {
        self.result.lock().ok()?.take()
    }

    /// Returns `true` if the background task has finished and populated the mailbox.
    #[allow(dead_code)]
    pub(crate) fn is_ready(&self) -> bool {
        self.result.lock().is_ok_and(|guard| guard.is_some())
    }
}

/// Helper to configure and run a command detached via `setsid -f` with nulled stdio.
///
/// Redirects stdin, stdout, and stderr to `/dev/null` and retries on transient `ETXTBSY`.
///
/// # Errors
///
/// Returns an error if `setsid` execution fails.
pub(crate) fn spawn_detached(
    setsid_bin: &std::path::Path,
    tool_bin: &std::path::Path,
    args: &[&str],
) -> io::Result<ExitStatus> {
    let mut cmd = Command::new(setsid_bin);
    cmd.arg("-f")
        .arg(tool_bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    status(&mut cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_task_spawns_and_yields_result() {
        let task = BgTask::spawn(|| 42);
        for _ in 0..100 {
            if let Some(val) = task.take() {
                assert_eq!(val, 42);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("BgTask timed out");
    }
}
