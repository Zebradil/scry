//! The build step (spec 4.4): one build per source and revision, its output passed through to
//! stderr so stdout stays clean. A failed build does not abort the run: each configuration
//! whose toplevel is missing afterwards fails on its own, with the wording `fetch::BuildStep`
//! gives it.

use std::path::Path;

use crate::exec::{Command, Exec, ExecError, Stderr};
use crate::source::colmena;
use crate::transport::machine::Machine;

/// colmena, deployed mode: `colmena build --on <names>` in the hive's directory. colmena
/// builds `buildOnTarget` nodes, or every node with `build_on_target`, on their targets. Its
/// local results get no out-link: colmena roots them only with `--keep-result`, inside the
/// hive's directory (ADR 0050).
pub async fn colmena(
    exec: &dyn Exec,
    dir: &Path,
    names: &[String],
    build_on_target: bool,
) -> Result<(), ExecError> {
    if names.is_empty() {
        return Ok(());
    }
    let on = names.join(",");
    let force = build_on_target.then_some("--build-on-target");
    let args = ["build"].into_iter().chain(force).chain(["--on", &on]);
    run(exec, colmena::colmena(dir, args)).await
}

/// Flake deployed mode, and each revision of revision mode: one
/// `nix build --keep-going <drv>^out…` on this machine, the results linked from `out_link`,
/// `out_link-1` and so on, inside the run's temporary directory so a concurrent garbage
/// collection cannot remove them mid-run.
pub async fn nix(exec: &dyn Exec, drvs: &[String], out_link: &Path) -> Result<(), ExecError> {
    if drvs.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "--extra-experimental-features".to_owned(),
        "nix-command".to_owned(),
        "build".to_owned(),
        "--keep-going".to_owned(),
        "--out-link".to_owned(),
        out_link.to_string_lossy().into_owned(),
    ];
    args.extend(drvs.iter().map(|drv| format!("{drv}^out")));
    run(exec, Command::new("nix", args)).await
}

/// `Err` only when the build could not run at all. The exit status says nothing the missing
/// toplevels will not.
async fn run(exec: &dyn Exec, mut command: Command) -> Result<(), ExecError> {
    command.stderr = Stderr::Inherit;
    exec.run(&Machine::Local, command).await.map(drop)
}
