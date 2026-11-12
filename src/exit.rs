use std::io;

use crate::report::Report;

/// A usage error: bad flags, an unknown selection pattern, `--base` outside a git checkout.
/// Exits 2 with or without `--exit-code`. Raise it as `anyhow::Error`; [`Outcome::of_error`]
/// finds it under any added context.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

/// How a run ended, as far as the exit code is concerned (spec 4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every configuration compared, none changed.
    NoChanges,
    /// Every configuration compared, at least one changed.
    Changes,
    /// A configuration failed, or a fatal error ended the run.
    Failed,
    Usage,
}

impl Outcome {
    /// `unfinished`: some configuration had not finished when the run ended (the user quit the
    /// TUI, or Ctrl-C); it counts as failed.
    pub fn of_report(report: &Report, unfinished: bool) -> Self {
        let statuses = || report.configurations.iter().map(|c| &c.status);
        if unfinished || statuses().any(|s| s.failed()) {
            Self::Failed
        } else if statuses().any(|s| s.has_changes()) {
            Self::Changes
        } else {
            Self::NoChanges
        }
    }

    /// A run that ended in an error instead of a report.
    pub fn of_error(error: &anyhow::Error) -> Self {
        if error.is::<UsageError>() {
            Self::Usage
        } else {
            Self::Failed
        }
    }

    /// `diff_style`: `--exit-code` was given.
    pub fn exit_code(self, diff_style: bool) -> u8 {
        match (self, diff_style) {
            (Self::NoChanges, _) | (Self::Changes, false) => 0,
            (Self::Changes, true) | (Self::Failed, false) => 1,
            (Self::Failed, true) | (Self::Usage, _) => 2,
        }
    }
}

/// Treats a broken pipe as success. Rust ignores SIGPIPE, so writing to stdout after the reader
/// left (`scry … | head`) returns `BrokenPipe`; the run then ends quietly with the exit code it
/// would have had. Wrap every write to stdout in this.
pub fn ignore_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{ClosureKind, Configuration, Mode, SCHEMA_VERSION, Source, Status};

    fn report(statuses: Vec<Status>) -> Report {
        Report {
            schema_version: SCHEMA_VERSION,
            tool_version: "1.0.0".into(),
            mode: Mode::Deployed,
            configurations: statuses
                .into_iter()
                .enumerate()
                .map(|(i, status)| Configuration {
                    name: format!("web-{i}"),
                    source: Source::Colmena,
                    kind: ClosureKind::Unknown,
                    target: None,
                    status,
                    old: None,
                    new: None,
                })
                .collect(),
            shared: vec![],
        }
    }

    fn error() -> Status {
        Status::Error {
            error: "ssh root@web-b: connect to host web-b port 22: Connection refused".into(),
        }
    }

    /// Each row of the table in spec 4.7: (outcome, default code, `--exit-code` code).
    #[test]
    fn exit_code_table() {
        let rows = [
            (
                Outcome::of_report(&report(vec![Status::NoChanges]), false),
                0,
                0,
            ),
            (
                Outcome::of_report(
                    &report(vec![Status::NoChanges, Status::AbsentAtBase]),
                    false,
                ),
                0,
                1,
            ),
            (
                Outcome::of_report(&report(vec![Status::AbsentAtBase, error()]), false),
                1,
                2,
            ),
            (
                Outcome::of_error(&anyhow::Error::new(UsageError("no match: web".into()))),
                2,
                2,
            ),
        ];
        for (outcome, default, diff_style) in rows {
            assert_eq!(outcome.exit_code(false), default, "{outcome:?}");
            assert_eq!(outcome.exit_code(true), diff_style, "{outcome:?}");
        }
    }

    #[test]
    fn outcome_classification() {
        let timeout = Status::Timeout {
            error: "timeout after 60 s".into(),
        };
        assert_eq!(
            Outcome::of_report(&report(vec![]), false),
            Outcome::NoChanges
        );
        assert_eq!(
            Outcome::of_report(&report(vec![timeout]), false),
            Outcome::Failed
        );
        assert_eq!(
            Outcome::of_report(&report(vec![Status::NoChanges]), true),
            Outcome::Failed
        );
        let usage = anyhow::Error::new(UsageError("no match: web".into())).context("selection");
        assert_eq!(Outcome::of_error(&usage), Outcome::Usage);
        let fatal = anyhow::anyhow!("colmena not found on PATH");
        assert_eq!(Outcome::of_error(&fatal), Outcome::Failed);
    }

    #[test]
    fn broken_pipe_is_quiet_other_errors_are_not() {
        let broken = io::Error::from(io::ErrorKind::BrokenPipe);
        assert!(ignore_broken_pipe(Err(broken)).is_ok());
        let other = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(ignore_broken_pipe(Err(other)).is_err());
    }
}
