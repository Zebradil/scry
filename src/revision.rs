//! Revision mode (spec 4.5): the base is a temporary `git worktree add --detach` at `--base`,
//! outside the checkout, and the source is found at the same relative directory inside it.
//! Head is the working directory as it is.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};

use crate::exec::{Command, Exec};
use crate::exit::UsageError;
use crate::report::{Base, Head};
use crate::source::{locate, success};
use crate::transport::machine::Machine;

/// What `--base` resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub base: Base,
    pub head: Head,
    /// Where base's source is when it has one: the directory of head's flake or hive, at the
    /// same path relative to the checkout, inside the base worktree.
    pub dir: PathBuf,
}

/// The base worktree to remove when the run ends. It is registered before `git worktree add`
/// runs, so an add cut short by Ctrl-C is cleaned up too.
#[derive(Debug, Default)]
pub struct Cleanup(Mutex<Option<Added>>);

#[derive(Debug)]
struct Added {
    checkout: PathBuf,
    worktree: PathBuf,
}

/// `git -C <dir> <args…>`.
pub(crate) fn git<'a>(dir: &Path, args: impl IntoIterator<Item = &'a str>) -> Command {
    let mut command = Command::new("git", args);
    let dir = dir.to_string_lossy().into_owned();
    command.args.splice(0..0, ["-C".to_owned(), dir]);
    command
}

async fn stdout(exec: &dyn Exec, command: Command) -> Result<String> {
    let output = exec.run(&Machine::Local, command).await?;
    let stdout = success("git", output)?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
}

/// Resolves `reference` for the checkout holding `source`, finds head's flake or hive (one of
/// `files`, in `source` or a directory above it inside the checkout, as colmena and Nix search),
/// and adds the base worktree at `worktree`, which must not exist yet. A source outside a git
/// checkout and a reference git does not know are usage errors; no flake or hive is a failure.
pub async fn open(
    exec: &dyn Exec,
    source: &Path,
    files: &[&str],
    reference: &str,
    worktree: &Path,
    cleanup: &Cleanup,
) -> Result<Revision> {
    let local = Machine::Local;
    let top = exec
        .run(&local, git(source, ["rev-parse", "--show-toplevel"]))
        .await?;
    if !top.success() {
        return Err(UsageError(format!(
            "--base needs a source inside a git checkout, and {} is not in one",
            source.display()
        ))
        .into());
    }
    let checkout = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    // No flake or hive is an unreadable source (spec 9), as without `--base`.
    let Some(found) = locate(exec, source, &checkout, files).await? else {
        bail!(
            "no {} in {} or above it inside the checkout {}",
            files.join(" or "),
            source.display(),
            checkout.display()
        );
    };
    let commit = format!("{reference}^{{commit}}");
    let rev = exec
        .run(
            &local,
            git(
                source,
                ["rev-parse", "--verify", "--end-of-options", &commit],
            ),
        )
        .await?;
    if !rev.success() {
        return Err(UsageError(format!(
            "--base {reference}: not a revision of {}",
            checkout.display()
        ))
        .into());
    }
    let rev = String::from_utf8_lossy(&rev.stdout).trim().to_owned();
    let head = stdout(exec, git(source, ["rev-parse", "HEAD"])).await?;
    // Nix sees tracked files only, so untracked ones do not make head dirty.
    let status = git(source, ["status", "--porcelain", "--untracked-files=no"]);
    let dirty = !stdout(exec, status).await?.is_empty();

    *cleanup.0.lock().expect("never poisoned") = Some(Added {
        checkout: checkout.clone(),
        worktree: worktree.to_owned(),
    });
    let path = worktree.to_string_lossy();
    stdout(
        exec,
        git(&checkout, ["worktree", "add", "--detach", &path, &rev]),
    )
    .await
    .with_context(|| format!("adding a worktree at {reference}"))?;

    // `/sub/flake.nix` relative to the checkout: base's source is `<worktree>/sub`.
    let relative = found.rsplit_once('/').map_or("", |(dir, _)| dir);
    Ok(Revision {
        base: Base {
            reference: reference.to_owned(),
            rev,
        },
        head: Head { rev: head, dirty },
        dir: PathBuf::from(format!("{}{relative}", worktree.display())),
    })
}

impl Revision {
    /// Whether base has one of `files` in [`Revision::dir`]. A source added after `--base` is
    /// absent there, and so is every configuration in it (spec 4.5).
    pub async fn has_source(&self, exec: &dyn Exec, files: &[&str]) -> Result<bool> {
        Ok(locate(exec, &self.dir, &self.dir, files).await?.is_some())
    }
}

impl Cleanup {
    /// Removes the base worktree, if one was added: `git worktree remove --force --force`, which
    /// also takes a worktree whose add was cut short and is still locked. When that fails, as
    /// for an add that died before writing the worktree's `.git`: `git worktree unlock`, since
    /// `prune` skips locked entries, the directory, then `git worktree prune`. Never fails; the
    /// run is over.
    pub async fn remove(&self, exec: &dyn Exec) {
        let Some(added) = self.0.lock().expect("never poisoned").take() else {
            return;
        };
        let path = added.worktree.to_string_lossy();
        let removed = exec
            .run(
                &Machine::Local,
                git(
                    &added.checkout,
                    ["worktree", "remove", "--force", "--force", &path],
                ),
            )
            .await;
        if !matches!(removed, Ok(output) if output.success()) {
            let unlock = git(&added.checkout, ["worktree", "unlock", &path]);
            let _ = exec.run(&Machine::Local, unlock).await;
            let _ = std::fs::remove_dir_all(&added.worktree);
            let prune = git(&added.checkout, ["worktree", "prune"]);
            let _ = exec.run(&Machine::Local, prune).await;
        }
    }
}
