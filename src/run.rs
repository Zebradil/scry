use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::exit::Outcome;

/// The binary's entry point: parses the command line (clap exits 2 on a usage error), runs the
/// subcommand, prints a fatal error as `scry: <message>` (spec 9) and maps the outcome to the
/// exit code (spec 4.7).
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let diff_style = cli.command.common().is_some_and(|common| common.exit_code);
    let outcome = dispatch(cli).unwrap_or_else(|error| {
        eprintln!("scry: {error:#}");
        Outcome::of_error(&error)
    });
    ExitCode::from(outcome.exit_code(diff_style))
}

fn dispatch(cli: Cli) -> anyhow::Result<Outcome> {
    match cli.command {
        Command::Colmena(_)
        | Command::Flake(_)
        | Command::Paths(_)
        | Command::Completions { .. } => {
            anyhow::bail!("not implemented yet")
        }
    }
}
