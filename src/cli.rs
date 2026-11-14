use std::num::NonZeroUsize;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::transport::machine::Target;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("SCRY_REV"), ")");

/// A non-empty value logs the full argv and stderr of every external command to stderr
/// (spec 9). Read by the real seam.
pub const DEBUG_ENV: &str = "SCRY_DEBUG";

const ENVIRONMENT_HELP: &str = "\
Environment:
  NO_COLOR     A non-empty value disables colour under --color auto
  NIX_SSHOPTS  Options for every ssh invocation, split shell-style as Nix does (quotes group
               words); they win over scry's own
  SCRY_DEBUG   A non-empty value logs the full argv and stderr of every external command to stderr";

/// Shows what a change to a Nix fleet does to each machine: packages, config files and boot.
#[derive(Debug, Parser)]
#[command(name = "scry", version = VERSION, after_help = ENVIRONMENT_HELP)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Compare the nodes of the colmena hive in the current directory
    #[command(after_help = ENVIRONMENT_HELP)]
    Colmena(ColmenaArgs),
    /// Compare the NixOS, nix-darwin and home-manager configurations of a flake
    #[command(after_help = ENVIRONMENT_HELP)]
    Flake(FlakeArgs),
    /// Compare two toplevels on one machine
    #[command(after_help = ENVIRONMENT_HELP)]
    Paths(PathsArgs),
    /// Print the shell completion registration script
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

impl Command {
    /// The flags every comparing subcommand takes; `None` for `completions`.
    pub fn common(&self) -> Option<&CommonArgs> {
        match self {
            Self::Colmena(args) => Some(&args.common),
            Self::Flake(args) => Some(&args.common),
            Self::Paths(args) => Some(&args.common),
            Self::Completions { .. } => None,
        }
    }
}

#[derive(Debug, Args)]
pub struct ColmenaArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Debug, Args)]
pub struct FlakeArgs {
    /// The flake; `REF#name` selects exactly one configuration
    #[arg(value_name = "REF", default_value = ".")]
    pub reference: String,
    /// Reach configuration NAME at TARGET ([user@]host) instead of the host its name gives
    #[arg(long = "target", value_name = "NAME=TARGET")]
    pub targets: Vec<TargetOverride>,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Debug, Args)]
pub struct PathsArgs {
    /// The machine both toplevels live on, [user@]host; this machine when absent
    #[arg(long, value_name = "TARGET")]
    pub target: Option<Target>,
    /// Old toplevel, such as /run/booted-system; symlinks resolve on that machine
    #[arg(value_name = "OLD")]
    pub old: String,
    /// New toplevel, such as /run/current-system
    #[arg(value_name = "NEW")]
    pub new: String,
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Selection, revision and build flags of `colmena` and `flake`.
#[derive(Debug, Args)]
pub struct SourceArgs {
    /// Comma-separated patterns matched against whole names: `*` and `?` globs, `@tag` for
    /// colmena tags. A pattern that matches nothing is an error
    #[arg(long, value_name = "SEL", value_delimiter = ',', action = clap::ArgAction::Set)]
    pub on: Option<Vec<String>>,
    /// Compare against the same configurations built from git revision REV instead of what
    /// the machines run; contacts no machine
    #[arg(long, value_name = "REV")]
    pub base: Option<String>,
    /// Skip the build step; toplevels must already exist
    #[arg(long)]
    pub no_build: bool,
}

#[derive(Debug, Args)]
pub struct CommonArgs {
    /// `auto`: TUI when stdout is a terminal, text otherwise
    #[arg(long, value_enum, default_value_t = Format::Auto)]
    pub format: Format,
    /// `auto`: colour when stdout is a terminal and NO_COLOR is unset or empty
    #[arg(long, value_enum, default_value_t = Color::Auto)]
    pub color: Color,
    /// Show hash-only and version-only file groups expanded
    #[arg(long)]
    pub expand_churn: bool,
    /// Exit like diff(1): 0 no changes, 1 changes, 2 trouble
    #[arg(long)]
    pub exit_code: bool,
    /// Configurations fetched at once
    #[arg(long, value_name = "N", default_value_t = DEFAULT_JOBS)]
    pub jobs: NonZeroUsize,
    /// Concurrent `nix eval` / `colmena eval` calls; one NixOS evaluation takes about one core
    /// and 1 GB. Defaults to the CPU count
    #[arg(long, value_name = "N", default_value_t = cpu_count(), hide_default_value = true)]
    pub eval_jobs: NonZeroUsize,
    /// Seconds per configuration for fetching; builds and evaluations have no timeout
    #[arg(long, value_name = "SECONDS", default_value_t = 60,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: u64,
    /// Files larger than this on either side are reported without content
    #[arg(long, value_name = "BYTES", default_value_t = 1_048_576)]
    pub max_file_size: u64,
}

const DEFAULT_JOBS: NonZeroUsize = NonZeroUsize::new(16).unwrap();

fn cpu_count() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Auto,
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// `--target NAME=TARGET` of `scry flake`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOverride {
    pub name: String,
    pub target: Target,
}

impl FromStr for TargetOverride {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, target) = s
            .split_once('=')
            .ok_or_else(|| format!("`{s}` is not NAME=TARGET"))?;
        if name.is_empty() {
            return Err(format!("`{s}` has no configuration name before `=`"));
        }
        Ok(Self {
            name: name.to_owned(),
            target: target.parse()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("scry").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_match_spec() {
        let cli = parse(&["colmena"]).unwrap();
        let common = cli.command.common().unwrap();
        assert_eq!(common.format, Format::Auto);
        assert_eq!(common.color, Color::Auto);
        assert!(!common.expand_churn && !common.exit_code);
        assert_eq!(common.jobs.get(), 16);
        assert_eq!(common.eval_jobs, cpu_count());
        assert_eq!(common.timeout, 60);
        assert_eq!(common.max_file_size, 1_048_576);

        let Command::Flake(flake) = parse(&["flake"]).unwrap().command else {
            panic!("not flake");
        };
        assert_eq!(flake.reference, ".");
        assert!(flake.targets.is_empty());
        assert_eq!(flake.source.on, None);
    }

    #[test]
    fn selection_splits_on_commas() {
        let Command::Colmena(args) = parse(&["colmena", "--on", "web-*,@db"]).unwrap().command
        else {
            panic!("not colmena");
        };
        assert_eq!(args.source.on.unwrap(), ["web-*", "@db"]);
    }

    #[test]
    fn targets_parse_and_repeat() {
        let Command::Flake(flake) = parse(&[
            "flake",
            "--target",
            "web-a=root@10.0.0.1",
            "--target",
            "alice@laptop=laptop",
        ])
        .unwrap()
        .command
        else {
            panic!("not flake");
        };
        assert_eq!(flake.targets[0].name, "web-a");
        assert_eq!(flake.targets[0].target.as_str(), "root@10.0.0.1");
        assert_eq!(flake.targets[1].name, "alice@laptop");
    }

    #[test]
    fn usage_errors() {
        for args in [
            &[][..],
            &["flake", "--target", "web-a"],
            &["flake", "--target", "=web-a"],
            &["flake", "--target", "web-a=-oProxyCommand=x"],
            &["paths", "--target", "root@", "/a", "/b"],
            &["paths", "/a"],
            &["colmena", "--jobs", "0"],
            &["colmena", "--eval-jobs", "0"],
            &["colmena", "--timeout", "0"],
            &["colmena", "--format", "yaml"],
            &["colmena", "--on", "a", "--on", "b"],
            &["colmena", "--target", "web-a=web-a"],
            &["completions", "powershell"],
        ] {
            let err = parse(args).expect_err(&format!("{args:?}"));
            assert_eq!(err.exit_code(), 2, "{args:?}: {err}");
        }
    }

    #[test]
    fn help_and_version_are_not_errors() {
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        let version = parse(&["--version"]).unwrap_err();
        assert_eq!(version.kind(), ErrorKind::DisplayVersion);
        assert_eq!(version.exit_code(), 0);
    }
}
