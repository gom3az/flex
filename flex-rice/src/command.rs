use std::io;
use std::process::{Child, Command, ExitStatus, Output};

use crate::spawn;

/// Abstract process execution to allow for pure in-memory testing.
#[allow(clippy::missing_errors_doc)]
pub trait CommandRunner {
    /// Execute and return the output.
    fn output(&self, cmd: &mut Command) -> io::Result<Output>;
    /// Execute and return the status.
    fn status(&self, cmd: &mut Command) -> io::Result<ExitStatus>;
    /// Spawn the process.
    fn spawn(&self, cmd: &mut Command) -> io::Result<Child>;
}

/// The production runner that implements transient-ETXTBSY retry logic.
pub struct OsCommandRunner;

impl CommandRunner for OsCommandRunner {
    fn output(&self, cmd: &mut Command) -> io::Result<Output> {
        tracing::debug!("executing output: {:?}", cmd);
        spawn::output(cmd)
    }

    fn status(&self, cmd: &mut Command) -> io::Result<ExitStatus> {
        tracing::debug!("executing status: {:?}", cmd);
        spawn::status(cmd)
    }

    fn spawn(&self, cmd: &mut Command) -> io::Result<Child> {
        tracing::debug!("spawning: {:?}", cmd);
        spawn::spawn(cmd)
    }
}
