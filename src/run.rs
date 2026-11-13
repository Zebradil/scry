//! The run (spec 4, 6.3): evaluate the selected configurations, build them, fetch each one in
//! its own task, and render the report. The base worktree, the out-links and the seam's SSH
//! masters are released when the run ends, also on an error or Ctrl-C.

use std::collections::HashSet;
use std::ffi::OsString;
use std::future::Future;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::cli::{Cli, ColmenaArgs, Command, CommonArgs, FlakeArgs, Format};
use crate::exec::process::Process;
use crate::exec::{Exec, ExecError};
use crate::exit::{Outcome, UsageError, ignore_broken_pipe};
use crate::fetch::{self, BuildStep, Job, Toplevel, Wanted};
use crate::report::{self, ClosureKind, Configuration, Mode, Report, Side, Source, Status};
use crate::revision::{self, Cleanup, Revision};
use crate::source::select::{self, Candidate};
use crate::source::{Evaluated, Selected, colmena, flake, paths};
use crate::transport::machine::Machine;
use crate::{build_step, fixture, render, shared, tui};

/// The binary's entry point: parses the command line (clap exits 2 on a usage error), runs the
/// subcommand, prints a fatal error as `scry: <message>` (spec 9) and maps the outcome to the
/// exit code (spec 4.7).
pub fn main() -> ExitCode {
    crate::completions::complete_from_env();
    let cli = Cli::parse();
    let diff_style = cli.command.common().is_some_and(|common| common.exit_code);
    let outcome = dispatch(&cli.command).unwrap_or_else(|error| {
        // Not `eprintln!`, which panics when stderr is a terminal that hung up.
        let _ = writeln!(std::io::stderr(), "scry: {error:#}");
        Outcome::of_error(&error)
    });
    ExitCode::from(outcome.exit_code(diff_style))
}

fn dispatch(command: &Command) -> Result<Outcome> {
    if let Command::Completions { shell } = command {
        let mut stdout = std::io::stdout().lock();
        let written = crate::completions::registration(*shell, &mut stdout);
        ignore_broken_pipe(written).context("writing the completion script")?;
        return Ok(Outcome::NoChanges);
    }
    if fixture::demo::enabled() {
        eprintln!(
            "scry: {} is set: a synthetic fleet, no machine is contacted",
            fixture::demo::ENV
        );
    }
    let runtime = tokio::runtime::Runtime::new().context("starting the async runtime")?;
    let outcome = runtime.block_on(async {
        let exec = exec_from_env()?;
        let env = Env::from_process()?;
        // Not locked: the viewer writes to stdout from its own thread.
        let mut stdout = std::io::BufWriter::new(std::io::stdout());
        run(
            exec,
            command,
            &env,
            &mut stdout,
            &mut std::io::stderr(),
            signal(),
        )
        .await
    });
    // A viewer thread stuck on a hung-up terminal must not keep the process alive.
    runtime.shutdown_background();
    outcome
}

/// The command seam: the fixture's fake when `SCRY_FIXTURE` is set (spec 11), else real
/// processes.
pub fn exec_from_env() -> Result<Arc<dyn Exec>> {
    if fixture::demo::enabled() {
        return Ok(Arc::new(fixture::demo::fake()));
    }
    Ok(Arc::new(Process::from_env()?))
}

/// `SIGINT`, `SIGTERM` or `SIGHUP`, whichever comes first. Children run in their own process
/// groups (ADR 0012), so the terminal's Ctrl-C reaches only scry, which drops its work, closes
/// the viewer and cleans up for each of them alike (ADR 0051).
async fn signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let wait = |kind| async move {
        match signal(kind) {
            Ok(mut signal) => drop(signal.recv().await),
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        () = wait(SignalKind::interrupt()) => {}
        () = wait(SignalKind::terminate()) => {}
        () = wait(SignalKind::hangup()) => {}
    }
}

/// The viewer: [`tui::run`], or a stand-in in tests.
pub type Viewer = fn(
    Report,
    Vec<String>,
    mpsc::UnboundedReceiver<Configuration>,
    tui::Options,
    &AtomicBool,
) -> std::io::Result<tui::Finished>;

/// What the run reads from its surroundings, apart from the command seam.
#[derive(Debug, Clone)]
pub struct Env {
    /// colmena searches for the hive from here; a relative flake reference starts here.
    pub cwd: PathBuf,
    /// `$USER`, for the flake default selection and local targets (spec 4.8).
    pub user: Option<String>,
    /// Whether `scutil --get LocalHostName` names this machine too (spec 4.2).
    pub macos: bool,
    pub no_color: Option<OsString>,
    pub stdout_is_terminal: bool,
    /// The run's temporary directory goes here: out-links and the base worktree (spec 4.4, 4.5).
    pub temp_dir: PathBuf,
    /// Opened for `--format auto` when stdout is a terminal.
    pub viewer: Viewer,
}

impl Env {
    pub fn from_process() -> Result<Self> {
        Ok(Self {
            cwd: std::env::current_dir().context("reading the current directory")?,
            user: std::env::var("USER").ok().filter(|user| !user.is_empty()),
            macos: cfg!(target_os = "macos"),
            no_color: std::env::var_os("NO_COLOR"),
            stdout_is_terminal: std::io::stdout().is_terminal(),
            temp_dir: std::env::temp_dir(),
            viewer: tui::run,
        })
    }
}

/// Runs one comparing subcommand and writes its report to `out`, or shows it in the viewer.
/// Warnings and the build step's output go to `err`, before the viewer opens. The run ends
/// early when `interrupt` completes before the viewer opens or, without the viewer, before the
/// report is complete: its commands are killed by drop and the run fails as with unfinished
/// configurations (spec 4.7, 6.3). However it ends, the base worktree, the temporary directory
/// and the seam's connections are released.
pub async fn run(
    exec: Arc<dyn Exec>,
    command: &Command,
    env: &Env,
    out: &mut impl Write,
    err: &mut impl Write,
    interrupt: impl Future<Output = ()>,
) -> Result<Outcome> {
    let temp = tempfile::Builder::new()
        .prefix("scry-")
        .tempdir_in(&env.temp_dir)
        .context("creating the run's temporary directory")?;
    let cleanup = Cleanup::default();
    let result = work(
        &exec,
        command,
        env,
        temp.path(),
        &cleanup,
        out,
        err,
        interrupt,
    )
    .await;
    cleanup.remove(&*exec).await;
    exec.shutdown().await;
    drop(temp);
    result
}

/// A selected configuration after evaluation and build: fetch it, or it is already final.
#[derive(Debug)]
enum Plan {
    Fetch(Job),
    Done(Configuration),
}

/// How long the run waits for the viewer to restore the terminal once told to quit.
const VIEWER_GRACE: Duration = Duration::from_secs(1);

/// `--format auto` on a terminal opens the viewer (spec 4.6).
fn viewer(format: Format, stdout_is_terminal: bool) -> bool {
    format == Format::Auto && stdout_is_terminal
}

#[allow(clippy::too_many_arguments)]
async fn work(
    exec: &Arc<dyn Exec>,
    command: &Command,
    env: &Env,
    temp: &Path,
    cleanup: &Cleanup,
    out: &mut impl Write,
    err: &mut impl Write,
    interrupt: impl Future<Output = ()>,
) -> Result<Outcome> {
    tokio::pin!(interrupt);
    let prepare = async {
        Ok(match command {
            Command::Colmena(args) => {
                let (mode, plans) = colmena(exec, args, env, temp, cleanup, err).await?;
                (&args.common, mode, plans)
            }
            Command::Flake(args) => {
                let (mode, plans) = flake(exec, args, env, temp, cleanup, err).await?;
                (&args.common, mode, plans)
            }
            Command::Paths(args) => (
                &args.common,
                Mode::Paths,
                vec![Plan::Fetch(paths::job(args))],
            ),
            Command::Completions { .. } => {
                bail!("`scry completions` prints a script, not a report")
            }
        })
    };
    let (common, mode, plans) = tokio::select! {
        prepared = prepare => prepared?,
        () = &mut interrupt => bail!("interrupted"),
    };
    let mut report = Report {
        schema_version: report::SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        mode,
        configurations: Vec::new(),
        shared: Vec::new(),
    };
    let mut names: Vec<String> = plans
        .iter()
        .map(|plan| match plan {
            Plan::Fetch(job) => job.name.clone(),
            Plan::Done(configuration) => configuration.name.clone(),
        })
        .collect();
    names.sort();
    let (sender, events) = mpsc::unbounded_channel();
    let fetching = fetch_all(exec, plans, common, sender);
    let color = render::color_enabled(
        common.color,
        env.no_color.as_deref(),
        env.stdout_is_terminal,
    );

    if viewer(common.format, env.stdout_is_terminal) {
        let options = tui::Options {
            expand_churn: common.expand_churn,
            color,
        };
        let header = report.clone();
        let quit = Arc::new(AtomicBool::new(false));
        let shown = Arc::clone(&quit);
        let open = env.viewer;
        // A thread of its own, so the tasks fetch while the viewer blocks.
        let mut viewer =
            tokio::task::spawn_blocking(move || open(header, names, events, options, &shown));
        tokio::pin!(fetching);
        let mut fetched = false;
        // In raw mode Ctrl-C is a key that quits the viewer; a signal, or a failure here,
        // tells it to quit, and the run waits for the terminal to be restored before it ends.
        let failure = loop {
            tokio::select! {
                finished = &mut viewer => {
                    let finished = finished
                        .map_err(|error| anyhow!("the viewer failed: {error}"))?
                        .context("running the viewer")?;
                    // Returning drops `fetching`, which kills what still runs.
                    report.configurations = finished.configurations;
                    return Ok(Outcome::of_report(&report, !finished.all_finished));
                }
                fetched_all = &mut fetching, if !fetched => match fetched_all {
                    Ok(_) => fetched = true,
                    Err(error) => break error,
                },
                () = &mut interrupt => break anyhow!("interrupted"),
            }
        };
        quit.store(true, Ordering::Relaxed);
        // A viewer blocked reading a terminal that hung up never sees the flag (crossterm's
        // poll blocks in `read` on such a tty), so the run cleans up without it; `dispatch`
        // does not wait for its thread either.
        let _ = tokio::time::timeout(VIEWER_GRACE, viewer).await;
        return Err(failure);
    }

    drop((names, events));
    report.configurations = tokio::select! {
        configurations = fetching => configurations?,
        () = &mut interrupt => bail!("interrupted"),
    };
    report.configurations.sort_by(|a, b| a.name.cmp(&b.name));
    report.shared = shared::shared(&report.configurations);
    let written = match common.format {
        Format::Json => render::json::write(&report, out),
        Format::Text | Format::Auto => {
            let options = render::text::Options {
                color,
                expand_churn: common.expand_churn,
            };
            render::text::write(&report, options, out)
        }
    };
    ignore_broken_pipe(written.and_then(|()| out.flush())).context("writing the report")?;
    Ok(Outcome::of_report(&report, false))
}

/// Fetches every configuration in its own task, at most `--jobs` at once, each sent on
/// `events` as soon as it is final. In plan order.
async fn fetch_all(
    exec: &Arc<dyn Exec>,
    plans: Vec<Plan>,
    common: &CommonArgs,
    events: mpsc::UnboundedSender<Configuration>,
) -> Result<Vec<Configuration>> {
    let tasks = plans.into_iter().map(|plan| {
        let (exec, events) = (Arc::clone(exec), events.clone());
        async move {
            let configuration = match plan {
                Plan::Fetch(job) => fetch::configuration(&*exec, &job).await,
                Plan::Done(configuration) => configuration,
            };
            // Text and JSON do not listen.
            let _ = events.send(configuration.clone());
            configuration
        }
    });
    each(&Arc::new(Semaphore::new(common.jobs.get())), tasks).await
}

/// Runs each future in its own task, at most as many at once as `limit` has permits. The
/// results come in input order. A task that panicked fails the whole, so the run still
/// cleans up. Dropping this future aborts the tasks, which kills their commands.
async fn each<T, F>(limit: &Arc<Semaphore>, futures: impl IntoIterator<Item = F>) -> Result<Vec<T>>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let mut tasks = JoinSet::new();
    for (i, future) in futures.into_iter().enumerate() {
        let limit = Arc::clone(limit);
        tasks.spawn(async move {
            let _permit = limit.acquire_owned().await.expect("never closed");
            (i, future.await)
        });
    }
    let mut results: Vec<Option<T>> = std::iter::repeat_with(|| None).take(tasks.len()).collect();
    while let Some(joined) = tasks.join_next().await {
        let (i, result) = joined.map_err(|error| anyhow!("{error}"))?;
        results[i] = Some(result);
    }
    Ok(results
        .into_iter()
        .map(|r| r.expect("every task joined"))
        .collect())
}

/// `scry colmena` up to the plans: version check, revision, step 1, selection, step 2 for head
/// and base, build step.
async fn colmena(
    exec: &Arc<dyn Exec>,
    args: &ColmenaArgs,
    env: &Env,
    temp: &Path,
    cleanup: &Cleanup,
    err: &mut impl Write,
) -> Result<(Mode, Vec<Plan>)> {
    colmena::check(&**exec).await?;
    colmena::check_lock(&**exec, &env.cwd).await?;
    let revision = match &args.source.base {
        Some(rev) => {
            let worktree = temp.join("worktree");
            let open = revision::open(&**exec, &env.cwd, HIVES, rev, &worktree, cleanup);
            Some(open.await?)
        }
        None => None,
    };
    let _ = writeln!(err, "scry: evaluating the hive");
    let nodes = colmena::nodes(&**exec, &env.cwd)
        .await
        .context("evaluating the hive")?;
    if nodes.is_empty() {
        bail!("the hive has no nodes");
    }
    let candidates: Vec<Candidate<'_>> = nodes
        .iter()
        .map(|n| Candidate {
            name: &n.name,
            tags: &n.tags,
        })
        .collect();
    let chosen = match &args.source.on {
        Some(patterns) => select::select(&candidates, patterns, true)?,
        None => (0..nodes.len()).collect(),
    };
    let selected: Vec<Selected> = chosen
        .iter()
        .map(|&i| match revision {
            Some(_) => Selected::local(&nodes[i].name, ClosureKind::Nixos),
            None => colmena::selected(&nodes[i]),
        })
        .collect();

    progress_evaluating(err, selected.len(), "node", revision.is_some());
    let evals = Arc::new(Semaphore::new(args.common.eval_jobs.get()));
    let head = selected.iter().map(|s| {
        let (exec, dir, name, error) = (
            Arc::clone(exec),
            env.cwd.clone(),
            s.name.clone(),
            s.error.clone(),
        );
        async move {
            if let Some(error) = error {
                bail!(error);
            }
            colmena::evaluate(&*exec, &dir, &name).await
        }
    });
    let base = match &revision {
        None => None,
        Some(revision) => {
            let present = async {
                if !revision.has_source(&**exec, HIVES).await? {
                    return Ok(HashSet::new());
                }
                let nodes = colmena::nodes(&**exec, &revision.dir).await?;
                anyhow::Ok(nodes.into_iter().map(|n| n.name).collect())
            };
            let present: Arc<Result<HashSet<String>, String>> =
                Arc::new(present.await.map_err(|error| format!("{error:#}")));
            Some(selected.iter().map(move |s| {
                let (exec, dir, name, present) = (
                    Arc::clone(exec),
                    revision.dir.clone(),
                    s.name.clone(),
                    Arc::clone(&present),
                );
                async move {
                    match &*present {
                        Err(error) => Err(anyhow!("{error}")),
                        Ok(names) if !names.contains(&name) => Ok(None),
                        Ok(_) => colmena::evaluate(&*exec, &dir, &name).await.map(Some),
                    }
                }
            }))
        }
    };
    let (head, base) = evaluate(&evals, head, base).await?;

    if !args.source.no_build {
        match &revision {
            None => {
                let names: Vec<String> = selected
                    .iter()
                    .zip(&head)
                    .filter(|(_, head)| head.is_ok())
                    .map(|(s, _)| s.name.clone())
                    .collect();
                let built = build_step::colmena(&**exec, &env.cwd, &names).await;
                not_built(err, built);
            }
            Some(_) => build_revision(&**exec, &head, base.as_deref(), temp, err).await,
        }
    }
    Ok(plans(
        Source::Colmena,
        revision,
        selected,
        head,
        base,
        &args.common,
        args.source.no_build,
    ))
}

/// `scry flake` up to the plans: `REF#name`, revision, listing, selection, one evaluation per
/// configuration for head and base, build step.
async fn flake(
    exec: &Arc<dyn Exec>,
    args: &FlakeArgs,
    env: &Env,
    temp: &Path,
    cleanup: &Cleanup,
    err: &mut impl Write,
) -> Result<(Mode, Vec<Plan>)> {
    let (reference, name) = flake::split(&args.reference)?;
    if name.is_some() && args.source.on.is_some() {
        return Err(UsageError(format!(
            "{}: give either REF#name or --on, not both",
            args.reference
        ))
        .into());
    }
    let revision = match &args.source.base {
        None => None,
        // Nix reads a reference as a path only when it starts with `.` or `/`.
        Some(_) if !reference.starts_with(['.', '/']) => {
            return Err(UsageError(format!(
                "--base needs a flake in a local git checkout, such as `.`; {reference} is not a path"
            ))
            .into());
        }
        Some(rev) => {
            let source = env.cwd.join(reference);
            let worktree = temp.join("worktree");
            let open = revision::open(&**exec, &source, FLAKES, rev, &worktree, cleanup);
            Some(open.await?)
        }
    };
    let _ = writeln!(err, "scry: listing the configurations of {reference}");
    let entries = flake::list(&**exec, reference).await?;
    if entries.is_empty() {
        bail!("{reference} has no nixosConfigurations, darwinConfigurations or homeConfigurations");
    }
    let hosts = match revision {
        None => flake::hosts(&**exec, env.macos).await?,
        Some(_) => Vec::new(),
    };
    let here = flake::Here {
        hosts: &hosts,
        user: env.user.as_deref(),
    };
    let chosen = if let Some(name) = name {
        vec![flake::by_name(&entries, name)?]
    } else if let Some(patterns) = &args.source.on {
        let candidates: Vec<Candidate<'_>> = entries
            .iter()
            .map(|e| Candidate {
                name: &e.name,
                tags: &[],
            })
            .collect();
        select::select(&candidates, patterns, false)?
    } else if revision.is_some() {
        (0..entries.len()).collect()
    } else {
        let chosen = here.select(&entries);
        if chosen.is_empty() {
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            let user = env.user.as_deref().unwrap_or("unset");
            return Err(UsageError(format!(
                "no configuration of {reference} belongs to this machine (host {}, user {user}); \
                 select from {} with --on",
                hosts.join(" or "),
                names.join(", ")
            ))
            .into());
        }
        chosen
    };
    for given in &args.targets {
        if !chosen.iter().any(|&i| entries[i].name == given.name) {
            let _ = writeln!(
                err,
                "scry: warning: --target {}={}: {} is not selected",
                given.name, given.target, given.name
            );
        }
    }
    let selected: Vec<Selected> = chosen
        .iter()
        .map(|&i| match revision {
            Some(_) => Selected::local(&entries[i].name, entries[i].set.kind()),
            None => here.selected(&entries[i], &args.targets),
        })
        .collect();
    let entry = |i: usize| entries[chosen[i]].clone();

    progress_evaluating(err, selected.len(), "configuration", revision.is_some());
    let evals = Arc::new(Semaphore::new(args.common.eval_jobs.get()));
    let head = selected.iter().enumerate().map(|(i, s)| {
        let (exec, flake, entry, error) = (
            Arc::clone(exec),
            reference.to_owned(),
            entry(i),
            s.error.clone(),
        );
        async move {
            if let Some(error) = error {
                bail!(error);
            }
            flake::evaluate(&*exec, &flake, &entry).await
        }
    });
    let base = match &revision {
        None => None,
        Some(revision) => {
            let at = revision.dir.to_string_lossy().into_owned();
            let present = async {
                if !revision.has_source(&**exec, FLAKES).await? {
                    return Ok(HashSet::new());
                }
                anyhow::Ok(flake::list(&**exec, &at).await?.into_iter().collect())
            };
            let present: Arc<Result<HashSet<flake::Entry>, String>> =
                Arc::new(present.await.map_err(|error| format!("{error:#}")));
            Some((0..selected.len()).map(move |i| {
                let (exec, at, entry, present) =
                    (Arc::clone(exec), at.clone(), entry(i), Arc::clone(&present));
                async move {
                    match &*present {
                        Err(error) => Err(anyhow!("{error}")),
                        Ok(entries) if !entries.contains(&entry) => Ok(None),
                        Ok(_) => flake::evaluate(&*exec, &at, &entry).await.map(Some),
                    }
                }
            }))
        }
    };
    let (head, base) = evaluate(&evals, head, base).await?;

    if !args.source.no_build {
        match &revision {
            None => {
                let built = build_step::nix(&**exec, &drvs(&head), &temp.join("new")).await;
                not_built(err, built);
            }
            Some(_) => build_revision(&**exec, &head, base.as_deref(), temp, err).await,
        }
    }
    Ok(plans(
        Source::Flake,
        revision,
        selected,
        head,
        base,
        &args.common,
        args.source.no_build,
    ))
}

/// The files colmena and Nix look for, in their order.
const HIVES: &[&str] = &["flake.nix", "hive.nix"];
const FLAKES: &[&str] = &["flake.nix"];

type Head = Vec<Result<Evaluated>>;
type Base = Vec<Result<Option<Evaluated>>>;

/// Evaluates head and, in revision mode, base, together under `--eval-jobs`.
async fn evaluate<H, B>(
    limit: &Arc<Semaphore>,
    head: impl IntoIterator<Item = H>,
    base: Option<impl IntoIterator<Item = B>>,
) -> Result<(Head, Option<Base>)>
where
    H: Future<Output = Result<Evaluated>> + Send + 'static,
    B: Future<Output = Result<Option<Evaluated>>> + Send + 'static,
{
    let base = async {
        match base {
            Some(base) => each(limit, base).await.map(Some),
            None => Ok(None),
        }
    };
    let (head, base) = tokio::join!(each(limit, head), base);
    Ok((head?, base?))
}

fn drvs<'a>(evaluated: impl IntoIterator<Item = &'a Result<Evaluated>>) -> Vec<String> {
    evaluated
        .into_iter()
        .filter_map(|e| e.as_ref().ok())
        .map(|e| e.drv.clone())
        .collect()
}

/// Revision mode: one `nix build` per revision, everything on this machine (spec 4.4).
async fn build_revision(
    exec: &dyn Exec,
    head: &Head,
    base: Option<&[Result<Option<Evaluated>>]>,
    temp: &Path,
    err: &mut impl Write,
) {
    not_built(
        err,
        build_step::nix(exec, &drvs(head), &temp.join("new")).await,
    );
    let base: Vec<Result<Evaluated>> = base
        .unwrap_or_default()
        .iter()
        .filter_map(|b| match b {
            Ok(Some(evaluated)) => Some(Ok(evaluated.clone())),
            _ => None,
        })
        .collect();
    not_built(
        err,
        build_step::nix(exec, &drvs(&base), &temp.join("old")).await,
    );
}

/// A build that could not start gets a line of its own; the run goes on (spec 4.4).
/// Evaluation captures its output and can take seconds per configuration, so the run says what
/// it waits on (spec 4.4).
fn progress_evaluating(err: &mut impl Write, count: usize, noun: &str, revision: bool) {
    let plural = if count == 1 { "" } else { "s" };
    let sides = if revision { " at head and base" } else { "" };
    let _ = writeln!(err, "scry: evaluating {count} {noun}{plural}{sides}");
}

fn not_built(err: &mut impl Write, built: Result<(), ExecError>) {
    if let Err(error) = built {
        let _ = writeln!(err, "scry: the build step did not run: {error}");
    }
}

/// Each selected configuration as a job, or as a final configuration when its evaluation
/// failed or it is absent at base.
fn plans(
    source: Source,
    revision: Option<Revision>,
    selected: Vec<Selected>,
    head: Head,
    base: Option<Base>,
    common: &CommonArgs,
    no_build: bool,
) -> (Mode, Vec<Plan>) {
    let build = Some(if no_build {
        BuildStep::Skipped
    } else {
        BuildStep::Ran
    });
    let built = |machine: &Machine, evaluated: &Evaluated| Wanted {
        machine: machine.clone(),
        toplevel: Toplevel::Path {
            path: evaluated.out.clone(),
            build,
        },
    };
    let short = revision
        .as_ref()
        .map(|r| r.base.rev.chars().take(7).collect::<String>());
    let mut base = base.map(Vec::into_iter);
    let plans = selected
        .into_iter()
        .zip(head)
        .map(|(s, head)| {
            let base = base
                .as_mut()
                .map(|b| b.next().expect("one per configuration"));
            let done = |status: Status, new: Option<Side>| {
                Plan::Done(Configuration {
                    name: s.name.clone(),
                    source,
                    kind: ClosureKind::Unknown,
                    target: match &s.old {
                        Machine::Remote(remote) => Some(remote.target.to_string()),
                        Machine::Local => None,
                    },
                    status,
                    old: None,
                    new,
                })
            };
            let failed = |error: anyhow::Error| {
                done(
                    Status::Error {
                        error: format!("{error:#}"),
                    },
                    None,
                )
            };
            let head = match head {
                Ok(head) => head,
                Err(error) => return failed(error),
            };
            let old = match base {
                None => Wanted {
                    machine: s.old.clone(),
                    toplevel: Toplevel::Current(s.kind),
                },
                Some(Err(error)) => {
                    let short = short.as_deref().unwrap_or_default();
                    return failed(error.context(format!("at base {short}")));
                }
                Some(Ok(None)) => {
                    let new = Side {
                        path: head.out.clone(),
                        system_version: None,
                        location: s.new.location(),
                    };
                    return done(Status::AbsentAtBase, Some(new));
                }
                Some(Ok(Some(base))) => built(&Machine::Local, &base),
            };
            Plan::Fetch(Job {
                name: s.name.clone(),
                source,
                old,
                new: built(&s.new, &head),
                max_file_size: common.max_file_size,
                timeout: Duration::from_secs(common.timeout),
            })
        })
        .collect();
    let mode = match revision {
        Some(revision) => Mode::Revision {
            base: revision.base,
            head: revision.head,
        },
        None => Mode::Deployed,
    };
    (mode, plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_only_for_auto_on_a_terminal() {
        assert!(viewer(Format::Auto, true));
        assert!(!viewer(Format::Auto, false));
        assert!(!viewer(Format::Text, true));
        assert!(!viewer(Format::Json, true));
    }
}
