//! The colmena hive (spec 4.2) in two steps, both `colmena eval -E` without building: one cheap
//! call for every node's deployment options, then one evaluation of the toplevel per selected
//! node, since evaluating a whole hive in one process risks running out of memory.
//!
//! colmena finds the hive by searching upwards from its working directory for `flake.nix` or
//! `hive.nix`. Every call runs in a given directory through [`IN_DIR`], so the base worktree of
//! revision mode is searched from the same relative directory as the working tree (spec 4.5)
//! with colmena's own rule.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{Evaluated, OUT_AND_DRV, Selected, locate, nix_string, success};
use crate::exec::{Command, Exec};
use crate::report::ClosureKind;
use crate::transport::machine::{Machine, Remote, Target};
use crate::transport::script::Script;

/// Runs the argv after the directory in that directory.
pub const IN_DIR: Script = Script::new(r#"cd "$1" && shift && exec "$@""#);

/// Step 1: every node's deployment options. `buildOnTarget` is absent in older colmena.
const DEPLOYMENT: &str = "{ nodes, ... }: builtins.mapAttrs (name: node: \
    let d = node.config.deployment; in { \
    inherit (d) targetHost targetUser targetPort sshOptions tags; \
    buildOnTarget = d.buildOnTarget or false; }) nodes";

/// One node of the hive, from step 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// `deployment.tags`, for `@tag` selection.
    pub tags: Vec<String>,
    /// Where the node runs: this machine for a null `targetHost`, or why its target is unusable.
    pub machine: Result<Machine, String>,
    pub build_on_target: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deployment {
    target_host: Option<String>,
    target_user: Option<String>,
    target_port: Option<i64>,
    #[serde(default)]
    ssh_options: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    build_on_target: bool,
}

/// `colmena --version`, before anything else (spec 4.2).
pub async fn check(exec: &dyn Exec) -> Result<()> {
    let output = exec
        .run(&Machine::Local, Command::new("colmena", ["--version"]))
        .await;
    match output {
        Ok(output) if output.success() => Ok(()),
        _ => bail!("colmena not found on PATH"),
    }
}

/// Refuses a flake hive whose `flake.lock` is missing or out of date. colmena 0.5 evaluates it
/// with a `nix eval` that would write the lock into the user's checkout, and offers no way to
/// pass `--no-write-lock-file` (ADR 0050). Finds the hive from `dir` as colmena does; a
/// `hive.nix` hive, or none, passes.
pub async fn check_lock(exec: &dyn Exec, dir: &Path) -> Result<()> {
    let hive = locate(exec, dir, Path::new("/"), &["flake.nix", "hive.nix"]).await?;
    let Some(flake) = hive.as_deref().and_then(|h| h.strip_suffix("/flake.nix")) else {
        return Ok(());
    };
    let flake = if flake.is_empty() { "/" } else { flake };
    let metadata = Command::new(
        "nix",
        [
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "metadata",
            "--no-update-lock-file",
            flake,
        ],
    );
    let output = exec.run(&Machine::Local, metadata).await?;
    if String::from_utf8_lossy(&output.stderr).contains("requires lock file changes") {
        bail!(
            "the hive's flake.lock in {flake} is missing or out of date, and colmena would \
             write it; run `nix flake lock` there first"
        );
    }
    success("nix", output).context("reading the hive's flake")?;
    Ok(())
}

/// `colmena <args…>` on this machine, in `dir`.
pub(crate) fn colmena<'a>(dir: &Path, args: impl IntoIterator<Item = &'a str>) -> Command {
    let dir = dir.to_string_lossy().into_owned();
    IN_DIR.command(
        [dir, "colmena".to_owned()]
            .into_iter()
            .chain(args.into_iter().map(str::to_owned)),
    )
}

/// Step 1: the hive's nodes in name order.
pub async fn nodes(exec: &dyn Exec, dir: &Path) -> Result<Vec<Node>> {
    let output = exec
        .run(&Machine::Local, colmena(dir, ["eval", "-E", DEPLOYMENT]))
        .await?;
    let stdout = success("colmena", output)?;
    let nodes: BTreeMap<String, Deployment> =
        serde_json::from_slice(&stdout).context("reading the hive's deployment options")?;
    Ok(nodes
        .into_iter()
        .map(|(name, d)| Node {
            machine: machine(&d),
            tags: d.tags,
            build_on_target: d.build_on_target,
            name,
        })
        .collect())
}

/// `targetUser@targetHost` (or `targetHost`), `-p targetPort`, and `sshOptions` (spec 4.2).
fn machine(d: &Deployment) -> Result<Machine, String> {
    let Some(host) = &d.target_host else {
        return Ok(Machine::Local);
    };
    let target = match &d.target_user {
        Some(user) => format!("{user}@{host}"),
        None => host.clone(),
    };
    let target: Target = target.parse()?;
    let port = d
        .target_port
        .map(|port| u16::try_from(port).map_err(|_| format!("targetPort {port} is not a port")))
        .transpose()?;
    Ok(Machine::Remote(Remote {
        target,
        port,
        ssh_options: d.ssh_options.clone(),
    }))
}

/// Where the node's sides live in deployed mode (spec 4.3): the old side on its machine, the
/// new side there too with `buildOnTarget`, else on this machine.
pub fn selected(node: &Node) -> Selected {
    let (old, new, error) = match &node.machine {
        Ok(machine) if node.build_on_target => (machine.clone(), machine.clone(), None),
        Ok(machine) => (machine.clone(), Machine::Local, None),
        Err(error) => (Machine::Local, Machine::Local, Some(error.clone())),
    };
    Selected {
        name: node.name.clone(),
        kind: ClosureKind::Nixos,
        old,
        new,
        error,
    }
}

/// Step 2: one node's toplevel.
pub async fn evaluate(exec: &dyn Exec, dir: &Path, name: &str) -> Result<Evaluated> {
    let expression = format!(
        "{{ nodes, ... }}: let t = nodes.{}.config.system.build.toplevel; in {OUT_AND_DRV}",
        nix_string(name)
    );
    let output = exec
        .run(&Machine::Local, colmena(dir, ["eval", "-E", &expression]))
        .await?;
    Evaluated::parse(&success("colmena", output)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment(json: &str) -> Deployment {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn targets_from_deployment_options() {
        let d = deployment(
            r#"{"targetHost":"web-a","targetUser":"root","targetPort":2222,"sshOptions":["-i","k"],"tags":[]}"#,
        );
        assert!(!d.build_on_target);
        let Ok(Machine::Remote(remote)) = machine(&d) else {
            panic!("not remote");
        };
        assert_eq!(remote.target.as_str(), "root@web-a");
        assert_eq!(remote.port, Some(2222));
        assert_eq!(remote.ssh_options, ["-i", "k"]);

        let d = deployment(r#"{"targetHost":"web-a","targetUser":null,"targetPort":null}"#);
        let Ok(Machine::Remote(remote)) = machine(&d) else {
            panic!("not remote");
        };
        assert_eq!(remote.target.as_str(), "web-a");
        assert_eq!(remote.port, None);

        let d = deployment(r#"{"targetHost":null,"targetUser":"root","buildOnTarget":true}"#);
        assert_eq!(machine(&d), Ok(Machine::Local));

        let d = deployment(r#"{"targetHost":"-oProxyCommand=x","targetUser":null}"#);
        assert_eq!(
            machine(&d),
            Err("target `-oProxyCommand=x` starts with `-`".to_owned())
        );
        let d = deployment(r#"{"targetHost":"web-a","targetPort":70000}"#);
        assert_eq!(
            machine(&d),
            Err("targetPort 70000 is not a port".to_owned())
        );
    }
}
