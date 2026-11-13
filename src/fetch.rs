//! The fetch pipeline of one configuration (spec 6.2): probe, level listing, content, followed
//! references and small rebuilds, each one round trip per machine that holds a side. Where both
//! sides are on one machine, they share every round trip (spec 6.1), so a machine never sees more
//! than 2 + link depth + reference depth of them. See ADR 0030.
//!
//! [`configuration`] is the entry point: one call per configuration, every failure inside it
//! becomes that configuration's error status.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use crate::exec::{Exec, ExecError, Output};
use crate::files::compare::{self, Contents};
use crate::report::{ClosureKind, Configuration, Sections, Side, Source, Status};
use crate::transport::machine::Machine;
use crate::transport::script::Script;
use crate::tree::Walk;
use crate::{boot, packages};

/// Changed files' contents in one `tar` stream per machine (spec 6.2 step 3).
pub mod content;
/// Tree listing, one round trip per link level (spec 6.2 step 2).
pub mod listing;
/// Toplevel resolution, markers, boot links, closure path-info (spec 6.2 step 1).
pub mod probe;
/// Followed references and small rebuilds, one round trip per depth (spec 6.2 step 4, 7.4).
pub mod references;

/// What the run hands over for one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub source: Source,
    pub old: Wanted,
    pub new: Wanted,
    /// `--max-file-size`: files larger than this on either side are not fetched.
    pub max_file_size: u64,
    /// `--timeout`: the whole fetch phase, from the first probe to the last fetch (spec 6.3).
    pub timeout: Duration,
}

/// One side: the machine it lives on, and which toplevel there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub machine: Machine,
    pub toplevel: Toplevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toplevel {
    /// A path known before the fetch: a build result, or one given by hand.
    Path {
        path: String,
        /// The build step meant to produce this path; it words the error when the path is
        /// missing (spec 9). `None` when nothing builds it, as in `scry paths`.
        build: Option<BuildStep>,
    },
    /// The machine's current system for the closure kind the source expects (spec 4.3),
    /// resolved on that machine with that machine's environment.
    Current(ClosureKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
    /// The build ran for this side.
    Ran,
    /// `--no-build` skipped it.
    Skipped,
}

/// One script for every round trip, the step chosen by its first argument:
///
/// - `probe`: for each line on stdin (a toplevel path, or an empty line for the home-manager
///   current system), resolves it with `cd -P`, keeps it only when it is a store path, and, once
///   per distinct toplevel, lists it with [`crate::tree::LISTING`] (the first link level, so the
///   probe holds no extra round trip), prints the marker files and `kernel-params`, and runs
///   `nix path-info --json -r`. Then whether `gzip` exists.
/// - `list`: [`crate::tree::LISTING`] on the roots on stdin.
/// - `refs`: lines `<mode> <path>`. A directory stands for its `bin/`, the only part a followed
///   reference or small rebuild compares: `w` takes a directory's `bin/` or anything else
///   whole, `f` only what is not a directory, `b` only a directory's `bin/`. Prints each such
///   directory, lists the roots, then archives their regular files of at most `limit - 1` bytes,
///   all in one round trip.
/// - `content`: archives the files on stdin.
///
/// The listing's and `nix`'s stderr are captured and reported on stdout as
/// `::scry failed <program> <status>` plus their stderr, so neither `ssh` chatter nor the
/// `--json` deprecation warning of Nix 2.35 passes for a failure. `tar` writes its status to
/// stderr instead, as stdout carries the archive. `tar` strips the leading `/` of member names
/// and may say so on stderr: not a failure. `COPYFILE_DISABLE` keeps macOS `tar` from adding
/// `._name` entries for extended attributes. GNU `tar` unescapes backslashes in `-T` names, so
/// the reference archive leaves out names with `[`, `\`, `]` or `^` (the range `[-^`); the
/// content step filters exactly, before sending.
pub const SCRIPT: Script = Script::new(
    r#"mode=$1
listing=$2
gzip=$3
limit=$4
nl="
"
LC_ALL=C
COPYFILE_DISABLE=1
export LC_ALL COPYFILE_DISABLE
exec 3>&1
list() {
  e=$(sh -c "$listing" 2>&1 >&3)
  s=$?
  if [ $s -ne 0 ] || [ -n "$e" ]; then
    echo "::scry failed find $s"
    printf "%s$nl" "$e"
    return 1
  fi
}
archive() {
  tar -c -h -f - -T -
  s=$?
  if [ $s -ne 0 ]; then echo "::scry failed tar $s" >&2; fi
}
pack() {
  if [ "$gzip" = gzip ]; then archive | gzip -1; else archive; fi
}
probe() {
  seen=$nl
  while IFS= read -r p; do
    echo "::scry side"
    if [ -z "$p" ]; then
      p=${XDG_STATE_HOME:-$HOME/.local/state}/nix/profiles/home-manager
      if [ ! -e "$p" ] && [ -e "/nix/var/nix/profiles/per-user/$USER/home-manager" ]; then
        p=/nix/var/nix/profiles/per-user/$USER/home-manager
      fi
    fi
    printf "::scry path %s$nl" "$p"
    t=$(cd -P "$p" 2>/dev/null && pwd -P) || continue
    case $t in /nix/store/*/*) continue;; /nix/store/?*) ;; *) continue;; esac
    printf "::scry toplevel %s$nl" "$t"
    case $seen in *"$nl$t$nl"*) continue;; esac
    seen=$seen$t$nl
    echo "::scry listing"
    printf "%s$nl" "$t" | list || return
    for f in nixos-version darwin-version hm-version kernel-params; do
      if [ -f "$t/$f" ]; then echo "::scry file $f"; cat "$t/$f"; echo; fi
    done
    echo "::scry pathinfo"
    e=$(nix --extra-experimental-features nix-command path-info --json -r "$t" 2>&1 >&3 </dev/null)
    s=$?
    if [ $s -ne 0 ]; then echo "::scry failed nix $s"; printf "%s$nl" "$e"; return; fi
    echo
  done
  if command -v gzip >/dev/null 2>&1; then echo "::scry gzip"; fi
}
files() {
  set --
  while IFS= read -r r; do
    if [ -e "$r" ]; then set -- "$@" "$r"; fi
    if [ $# -ge 500 ]; then find "$@" -type f -size -"$limit"c ! -name "*[[-^]*"; set --; fi
  done
  if [ $# -gt 0 ]; then find "$@" -type f -size -"$limit"c ! -name "*[[-^]*"; fi
}
refs() {
  roots=
  while IFS= read -r line; do
    r=${line#??}
    if [ -d "$r" ]; then
      case $line in f*) continue;; esac
      printf "%s$nl" "$r"
      r=$r/bin
    else
      case $line in b*) continue;; esac
    fi
    roots=$roots$r$nl
  done
  printf "%s" "$roots" | list || return
  echo "::scry tar"
  printf "%s" "$roots" | files | pack
}
case $mode in
  probe) probe;;
  list) list;;
  refs) refs;;
  content) pack;;
esac"#,
);

/// Starts every line the script writes about itself; no listing line, path-info JSON or
/// marker file starts with it.
pub(crate) const SENTINEL: &str = "::scry ";

/// Runs the fetch phase of one configuration within `job.timeout`. Never fails: a failure or
/// the timeout becomes the configuration's status, with the sides and kind known by then
/// (spec 6.3, 8.1). The timeout, like dropping this future, kills every command it started.
pub async fn configuration(exec: &dyn Exec, job: &Job) -> Configuration {
    let known = Mutex::new(Known::new(job));
    let status = match tokio::time::timeout(job.timeout, compare(exec, job, &known)).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => Status::Error {
            error: format!("{error:#}"),
        },
        Err(_) => Status::Timeout {
            error: format!("timeout after {} s", job.timeout.as_secs()),
        },
    };
    let known = known
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Configuration {
        name: job.name.clone(),
        source: job.source,
        kind: known.kind,
        target: match &job.old.machine {
            Machine::Local => None,
            Machine::Remote(remote) => Some(remote.target.to_string()),
        },
        status,
        old: known.old,
        new: known.new,
    }
}

/// What the report can say about a configuration that fails part way.
struct Known {
    kind: ClosureKind,
    old: Option<Side>,
    new: Option<Side>,
}

impl Known {
    /// A side given by path is known before its probe; a current system only after it.
    fn new(job: &Job) -> Self {
        let side = |wanted: &Wanted| match &wanted.toplevel {
            Toplevel::Path { path, .. } => Some(Side {
                path: path.clone(),
                system_version: None,
                location: wanted.machine.location(),
            }),
            Toplevel::Current(_) => None,
        };
        Self {
            kind: ClosureKind::Unknown,
            old: side(&job.old),
            new: side(&job.new),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Old,
    New,
}

/// A machine that holds one or both sides, with what its round trips learnt.
pub(crate) struct Host {
    pub machine: Machine,
    pub gzip: bool,
    /// Fetched contents by absolute path, from every content and reference round trip.
    pub contents: Contents,
}

async fn compare(exec: &dyn Exec, job: &Job, known: &Mutex<Known>) -> Result<Status> {
    // The new side first: its probe decides the kind, and its errors are reported first.
    let mut groups: Vec<(Machine, Vec<Which>)> = vec![(job.new.machine.clone(), vec![Which::New])];
    if job.old.machine == job.new.machine {
        groups[0].1.push(Which::Old);
    } else {
        groups.push((job.old.machine.clone(), vec![Which::Old]));
    }
    let wanted = |which| match which {
        Which::Old => &job.old,
        Which::New => &job.new,
    };
    let group = |which| usize::from(which == Which::Old && groups.len() == 2);

    // Each machine records its sides as soon as its probe returns, so a timeout while the
    // other machine is still probing keeps them.
    let results = join(groups.iter().map(|(machine, sides)| async move {
        let requests: Vec<probe::Request> = sides
            .iter()
            .map(|&w| probe::Request::of(&wanted(w).toplevel))
            .collect();
        let probe = probe::run(exec, machine, requests).await?;
        let mut known = known.lock().expect("never held across a failure");
        for (side, &which) in probe.sides.iter().zip(sides) {
            let Some(toplevel) = &side.toplevel else {
                continue;
            };
            let data = &probe.toplevels[toplevel];
            let info = Side {
                path: toplevel.clone(),
                system_version: data.system_version(),
                location: machine.location(),
            };
            match which {
                Which::New => {
                    known.kind = data.kind();
                    known.new = Some(info);
                }
                Which::Old => known.old = Some(info),
            }
        }
        Ok(probe)
    }))
    .await;

    // The first error in group order wins: the new side's first.
    let mut probes: Vec<probe::Probe> = Vec::new();
    let mut first_error = None;
    // Side, toplevel, and the path tried for it.
    let mut resolved: Vec<(Which, String, String)> = Vec::new();
    for ((machine, sides), result) in groups.iter().zip(results) {
        let probe = match result {
            Ok(probe) => probe,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        for (i, &which) in sides.iter().enumerate() {
            let side = &probe.sides[i];
            match &side.toplevel {
                Some(toplevel) => resolved.push((which, toplevel.clone(), side.path.clone())),
                None => {
                    first_error.get_or_insert_with(|| {
                        missing(machine, &wanted(which).toplevel, &side.path)
                    });
                }
            }
        }
        probes.push(probe);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    let side = |which| {
        resolved
            .iter()
            .find(|(w, ..)| *w == which)
            .expect("every side resolved")
    };
    let toplevel = |which| side(which).1.as_str();
    let (old_t, new_t) = (toplevel(Which::Old), toplevel(Which::New));
    if old_t == new_t {
        return Ok(Status::NoChanges);
    }
    let data = |which| &probes[group(which)].toplevels[toplevel(which)];
    let (old_kind, kind) = (data(Which::Old).kind(), data(Which::New).kind());
    if old_kind != ClosureKind::Unknown && old_kind != kind {
        bail!(
            "closure kinds differ: old is {}, new is {}",
            kind_name(old_kind),
            kind_name(kind)
        );
    }

    let mut sizes = Vec::new();
    for which in [Which::Old, Which::New] {
        let info = &data(which).path_info;
        if !info.sizes.contains_key(toplevel(which)) {
            let machine = &groups[group(which)].0;
            return Err(missing(machine, &wanted(which).toplevel, &side(which).2));
        }
        sizes.push(&info.sizes);
    }
    let (old_boot, new_boot) = (data(Which::Old).boot(old_t), data(Which::New).boot(new_t));
    let specialisations = old_boot
        .specialisations
        .values()
        .chain(new_boot.specialisations.values());
    let toplevels: Vec<&str> = [old_t, new_t]
        .into_iter()
        .chain(specialisations.map(String::as_str))
        .collect();
    let packages = packages::compare(sizes[0], sizes[1], &toplevels);

    // Level one of each walk came with the probe.
    let mut walks: Vec<Vec<Walk>> = Vec::new();
    for (_, sides) in &groups {
        let mut group_walks = Vec::new();
        for &which in sides {
            let mut walk = Walk::toplevel(toplevel(which), kind);
            walk.next_roots();
            walk.add_listing(&data(which).listing)?;
            group_walks.push(walk);
        }
        walks.push(group_walks);
    }
    let listed = join(
        groups
            .iter()
            .zip(walks.iter_mut())
            .map(|((machine, _), walks)| listing::walk(exec, machine, walks)),
    )
    .await;
    listed.into_iter().collect::<Result<Vec<()>>>()?;
    let tree = |which: Which| {
        let (_, sides) = &groups[group(which)];
        let i = sides
            .iter()
            .position(|&w| w == which)
            .expect("side in group");
        walks[group(which)][i].tree()
    };
    let (old_tree, new_tree) = (tree(Which::Old), tree(Which::New));

    let mut hosts: Vec<Host> = groups
        .iter()
        .zip(&probes)
        .map(|((machine, _), probe)| Host {
            machine: machine.clone(),
            gzip: probe.gzip,
            contents: Contents::new(),
        })
        .collect();
    let fetch = compare::to_fetch(&old_tree, &new_tree, job.max_file_size);
    let mut paths: Vec<Vec<String>> = vec![Vec::new(); hosts.len()];
    paths[group(Which::Old)].extend(fetch.old);
    paths[group(Which::New)].extend(fetch.new);
    let fetched = join(
        hosts
            .iter()
            .zip(&paths)
            .map(|(host, paths)| content::fetch(exec, &host.machine, paths, host.gzip)),
    )
    .await;
    for (host, contents) in hosts.iter_mut().zip(fetched) {
        host.contents = contents?;
    }
    let (old_contents, new_contents) = (
        &hosts[group(Which::Old)].contents,
        &hosts[group(Which::New)].contents,
    );
    let mut changes = compare::compare(
        &old_tree,
        &new_tree,
        job.max_file_size,
        old_contents,
        new_contents,
    );

    let followed = references::follow(
        exec,
        &mut hosts,
        references::Sides {
            old: group(Which::Old),
            new: group(Which::New),
            old_sizes: sizes[0],
            new_sizes: sizes[1],
            toplevels: &toplevels,
        },
        job.max_file_size,
        &changes,
        &packages.small_rebuilds,
    )
    .await?;
    changes.extend(followed);
    compare::sort(&mut changes);

    let boot = (kind == ClosureKind::Nixos).then(|| boot::compare(&old_boot, &new_boot));
    Ok(Status::Done(Box::new(Sections {
        packages: packages.packages,
        files: crate::report::Files { changes },
        boot,
    })))
}

/// Spec 9: a missing toplevel, with the build step's suffix, or a missing current system.
fn missing(machine: &Machine, toplevel: &Toplevel, path: &str) -> anyhow::Error {
    match toplevel {
        Toplevel::Current(_) => anyhow!("no current system at {path} on {machine}"),
        Toplevel::Path { path: given, build } => {
            let suffix = match build {
                None => "",
                Some(BuildStep::Ran) => "; the build did not produce it",
                Some(BuildStep::Skipped) => "; run without --no-build",
            };
            // A given path that resolved is named as given: that is the path the user knows.
            anyhow!("{given} is not in the store on {machine}{suffix}")
        }
    }
}

/// The kind as the report's JSON writes it.
fn kind_name(kind: ClosureKind) -> &'static str {
    match kind {
        ClosureKind::Nixos => "nixos",
        ClosureKind::NixDarwin => "nix-darwin",
        ClosureKind::HomeManager => "home-manager",
        ClosureKind::Unknown => "unknown",
    }
}

/// Runs one future per machine at once; there are one or two.
async fn join<T, F: Future<Output = T>>(futures: impl IntoIterator<Item = F>) -> Vec<T> {
    let mut futures: Vec<F> = futures.into_iter().collect();
    match futures.len() {
        0 => Vec::new(),
        1 => vec![futures.pop().expect("one").await],
        2 => {
            let second = futures.pop().expect("two");
            let first = futures.pop().expect("two");
            let (a, b) = tokio::join!(first, second);
            vec![a, b]
        }
        n => unreachable!("{n} machines for one configuration"),
    }
}

/// A failure the script reported as `::scry failed <program> <status>` with the program's
/// stderr, in the wording of spec 9: exit 127 is a missing program.
pub(crate) fn failure(machine: &Machine, program: &str, status: &str, stderr: &str) -> ExecError {
    let code = status.trim().parse().ok();
    if code == Some(127) {
        return ExecError::NotFound {
            program: program.to_owned(),
            machine: machine.clone(),
        };
    }
    ExecError::Failed {
        program: program.to_owned(),
        code,
        line: first_line(stderr),
    }
}

/// The line of a failed command's stderr worth showing: Nix's `error:` line when there is one,
/// else the first line that is not a warning (Nix's `--json` deprecation, ssh's host key
/// notice, tar's leading `/`).
pub(crate) fn first_line(stderr: &str) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    let noise = |l: &&str| {
        l.starts_with("warning:") || l.starts_with("Warning:") || l.contains("Removing leading")
    };
    lines()
        .find(|l| l.starts_with("error:"))
        .or_else(|| lines().find(|l| !noise(l)))
        .or_else(|| lines().next())
        .unwrap_or_default()
        .to_owned()
}

/// Splits `stdout` at a `::scry failed …` frame: what came before, and the error it reports.
pub(crate) fn split_failure<'a>(
    machine: &Machine,
    stdout: &'a [u8],
) -> (&'a [u8], Option<ExecError>) {
    let marker = b"::scry failed ";
    let at = if stdout.starts_with(marker) {
        Some(0)
    } else {
        find(stdout, b"\n::scry failed ").map(|i| i + 1)
    };
    let Some(at) = at else {
        return (stdout, None);
    };
    let frame = String::from_utf8_lossy(&stdout[at + marker.len()..]);
    let (head, stderr) = frame.split_once('\n').unwrap_or((&frame, ""));
    let (program, status) = head.split_once(' ').unwrap_or((head, ""));
    (
        &stdout[..at],
        Some(failure(machine, program, status, stderr)),
    )
}

/// An exit the script does not explain: the shell itself failed.
pub(crate) fn unexplained(output: &Output) -> Option<ExecError> {
    (!output.success()).then(|| ExecError::Failed {
        program: "sh".to_owned(),
        code: output.code,
        line: first_line(&String::from_utf8_lossy(&output.stderr)),
    })
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_warnings() {
        let nix = "warning: '--json' without '--json-format' is deprecated\nerror: path '/x' is not valid\n";
        assert_eq!(first_line(nix), "error: path '/x' is not valid");
        let tar = "tar: Removing leading `/' from member names\ntar: /nix/store/x: Cannot stat: No such file or directory\n";
        assert_eq!(
            first_line(tar),
            "tar: /nix/store/x: Cannot stat: No such file or directory"
        );
        assert_eq!(
            first_line("\n  find: /a: Permission denied\n"),
            "find: /a: Permission denied"
        );
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn failure_frames() {
        let out = b"/a\n12 3 /a/b\n::scry failed find 1\nfind: /a/c: Permission denied\n";
        let (before, error) = split_failure(&Machine::Local, out);
        assert_eq!(before, b"/a\n12 3 /a/b\n");
        assert_eq!(
            error.unwrap().to_string(),
            "`find` failed (exit 1): find: /a/c: Permission denied"
        );
        let out = b"::scry failed nix 127\nsh: 1: nix: not found\n";
        let (before, error) = split_failure(&Machine::Local, out);
        assert!(before.is_empty());
        assert_eq!(
            error.unwrap().to_string(),
            "`nix` not found on this machine"
        );
        assert!(split_failure(&Machine::Local, b"/a\n").1.is_none());
    }
}
