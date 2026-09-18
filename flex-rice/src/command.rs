use std::io;
use std::process::{Child, Command, ExitStatus, Output};

use crate::spawn;

/// Abstract process execution to allow for pure in-memory testing.
#[allow(clippy::missing_errors_doc)]
pub trait CommandRunner {
    fn output(&self, cmd: &mut Command) -> io::Result<Output>;
    fn status(&self, cmd: &mut Command) -> io::Result<ExitStatus>;
    fn spawn(&self, cmd: &mut Command) -> io::Result<Child>;
}

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
