//! The seam sits at "run this argv on that machine", not at "spawn this local process": the core
//! states what to run and where, the real implementation decides how (directly, or through
//! `ssh` with a shared connection), and the fake answers per machine without having to unpick
//! `ssh` command lines. See ADR 0005.

use std::future::Future;
use std::pin::Pin;

use crate::transport::machine::Machine;

/// Canned output for tests and the fixture run mode. M4.
pub mod fake;
/// The real implementation: local processes, and remote ones through `ssh`. M4.
pub mod process;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The command seam (spec 11). Every external command scry runs (`nix`, `colmena`, `git`,
/// `sh`, and `ssh` inside the real implementation) goes through it; nothing else spawns a
/// process.
///
/// Boxed futures keep the trait object-safe, so the run holds an `Arc<dyn Exec>` and the
/// fixture run mode swaps in the fake at runtime.
pub trait Exec: Send + Sync {
    /// Runs `command` on `machine` and waits for it to exit.
    ///
    /// On a remote machine the argv is single-quoted word by word into one `ssh` command line
    /// (spec 6.1); callers that need a script pass `sh -c <script>` and keep data on stdin.
    ///
    /// A non-zero exit is an `Ok` output: some callers expect one. `Err` is reserved for what
    /// the caller cannot recover from as a command result: the program is missing, `ssh`
    /// itself failed, or I/O with the child broke.
    ///
    /// Dropping the future kills the process. Timeouts (`tokio::time::timeout`) and Ctrl-C
    /// cancel commands by dropping them; there is no separate kill call.
    fn run<'a>(
        &'a self,
        machine: &'a Machine,
        command: Command,
    ) -> BoxFuture<'a, Result<Output, ExecError>>;

    /// Releases what the implementation opened during the run: `ssh -O exit` to each master it
    /// started, the socket directory (spec 6.1). Called once at the end, also after Ctrl-C.
    fn shutdown(&self) -> BoxFuture<'_, ()>;
}

/// One external command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    /// Written to the child's stdin, which is then closed. Path lists go here, never into
    /// `args` (spec 6.1, argv limits).
    pub stdin: Vec<u8>,
    pub stderr: Stderr,
}

impl Command {
    /// A command with empty stdin and captured stderr.
    pub fn new<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            stdin: Vec::new(),
            stderr: Stderr::Capture,
        }
    }
}

/// What happens to the child's stderr. Stdout is always captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stderr {
    /// Kept in [`Output::stderr`] for error messages and the debug log.
    Capture,
    /// Passed through to scry's stderr, for the build step: its progress stays visible while
    /// stdout stays clean for text and JSON (spec 4.4).
    Inherit,
}

/// A finished command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit code, `None` when a signal ended the process.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Empty under [`Stderr::Inherit`].
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Command failures in the wording of spec 9. `line` is the first line of stderr.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    /// The program is not installed there: a failed spawn locally, exit 127 remotely.
    #[error("`{program}` not found on {machine}")]
    NotFound { program: String, machine: Machine },
    /// `ssh` itself failed (exit 255): unreachable host, refused key, host key mismatch.
    #[error("ssh {target}: {line}")]
    Ssh { target: String, line: String },
    /// The command ran and failed. Built by callers that require success.
    #[error("`{program}` failed ({}): {line}", exit_text(*.code))]
    Failed {
        program: String,
        code: Option<i32>,
        line: String,
    },
    /// Spawning the child or talking to it failed.
    #[error("`{program}`: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

fn exit_text(code: Option<i32>) -> String {
    code.map_or_else(
        || "killed by a signal".to_owned(),
        |code| format!("exit {code}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_follow_spec_wording() {
        let not_found = ExecError::NotFound {
            program: "nix".into(),
            machine: Machine::Local,
        };
        assert_eq!(not_found.to_string(), "`nix` not found on this machine");
        let failed = ExecError::Failed {
            program: "git".into(),
            code: Some(128),
            line: "fatal: not a git repository".into(),
        };
        assert_eq!(
            failed.to_string(),
            "`git` failed (exit 128): fatal: not a git repository"
        );
    }
}
