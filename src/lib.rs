//! scry compares the old and new toplevel of every selected Nix configuration and reports what
//! changes in packages, files and boot. `docs/spec.md` sets the behaviour, `docs/architecture.md`
//! the structure. Every module is declared here once; a milestone fills the modules it owns and
//! declares any further submodules in its own files, never here.

// M0: scaffold.

/// Command-line surface: clap types for every subcommand and flag (spec 4, 4.6). M0.
pub mod cli;
/// Exit codes from a run's outcome, the usage error type, and quiet broken pipes on stdout
/// (spec 4.7). M0.
pub mod exit;
/// The report: serde types of the `--format json` output, the one input of every renderer
/// (spec 8.1, 8.2). M0.
pub mod report;

// M1: packages core. Pure.

/// Package comparison of two closures into `report::Packages` (spec 7.2). M1.
pub mod packages;
/// Byte sizes and deltas as text, `1.2 GiB`, `+41.0 MiB`, `0 B` (spec 7.2). M1.
pub mod size;
/// Store paths: name and hash split, the `parseDrvName` pname/version split and output
/// suffixes (`name`), `compareVersions` with the git-revision rule (`version`), both
/// `nix path-info --json` shapes (`pathinfo`) (spec 7.1, 7.2). M1.
pub mod store;

// M2: files and boot core. Pure.

/// Boot section: kernel, initrd, modules, kernel parameters, specialisations (spec 7.5). M2.
pub mod boot;
/// File changes: comparing two trees (`compare`), churn classification (`churn`), unified diffs
/// (`diff`), word highlight (`highlight`), store reference pairing (`refs`) (spec 7.3, 7.4). M2.
pub mod files;
/// Tree listings: parsing `find`/`ls -ld`/`cksum` output, the state of the level walk, followed
/// trees and exclusions per closure kind, the dereferenced tree of relative path to entry
/// (spec 5, 6.2). M2.
pub mod tree;

// M3: rendering.

/// Text and JSON output of a report, and colour choice (spec 8.3, 8.5). M3.
pub mod render;
/// Shared changes across configurations and the `(also in N)` lookup (spec 8.2). M3.
pub mod shared;

// M4: the command seam and transport.

/// The command seam: the trait every external command runs through, its real implementation
/// and the fake (spec 11). Trait and types M0, implementations M4.
pub mod exec;
/// Machines and how commands reach them: `ssh` invocation and connection sharing (`ssh`),
/// remote script quoting and `PATH` (`script`), this machine vs a target (`machine`)
/// (spec 6.1). `machine` M0, the rest M4.
pub mod transport;

// M5: fetching.

/// Per-side round trips through the seam: probe, level listing, content fetch, references and
/// small rebuilds (spec 6.2, 7.4). M5.
pub mod fetch;

// M6: sources and the run.

/// The build step: `colmena build` or `nix build`, out-links in the run's temporary directory
/// (spec 4.4). M6.
pub mod build_step;
/// Revision mode: the base worktree at `--base REV` and its cleanup (spec 4.5). M6.
pub mod revision;
/// The run: entry point, per-configuration tasks under `--jobs`, timeouts, Ctrl-C, cleanup,
/// the report and the output (spec 4.7, 6.3). Entry point M0, the rest M6.
#[cfg(not(target_arch = "wasm32"))]
pub mod run;
/// Where configurations come from: selection patterns (`select`), colmena hives, flakes and
/// hand-given paths (spec 4.1, 4.2). M6.
pub mod source;

// M7: TUI.

/// The interactive viewer on ratatui and crossterm, with live per-configuration status
/// (spec 8.4). M7.
pub mod tui;

// M8: fixtures and completions.

/// Dynamic shell completion for bash, zsh and fish (spec 10). M8.
#[cfg(not(target_arch = "wasm32"))]
pub mod completions;
/// Fixture run mode: a synthetic fleet served by the fake seam (spec 11). M8.
#[cfg(not(target_arch = "wasm32"))]
pub mod fixture;
