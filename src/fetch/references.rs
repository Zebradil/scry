//! Step 4 of spec 6.2 and spec 7.4: followed references and small rebuilds. Each depth level is
//! one round trip per machine that lists the pairs' store paths and archives their files at
//! once, so a reference tree is listed one level deep: a store link out of it is left out, its
//! content unknown (ADR 0030).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};

use super::content::{gzip_arg, lines, tar_failure, unpack};
use super::{Host, SCRIPT, join, listing};
use crate::exec::{Command, Exec};
use crate::files::churn::Masks;
use crate::files::compare::{self, Contents};
use crate::files::refs;
use crate::packages::{SMALL_REBUILD_MAX_BYTES, SmallRebuild};
use crate::report::{FileChange, FileStatus, LineOp};
use crate::store::name::store_path_name;
use crate::tree::{Entry, LISTING, Tree, Walk};

/// Followed references are followed again up to this depth in total (spec 7.4, 12).
pub const DEPTH: usize = 2;

/// Where each side lives among the hosts, and its closure's `narSize`s.
pub(crate) struct Sides<'a> {
    pub old: usize,
    pub new: usize,
    pub old_sizes: &'a BTreeMap<String, u64>,
    pub new_sizes: &'a BTreeMap<String, u64>,
    /// Both toplevels and their specialisations: never followed, the comparison already
    /// covers what they hold.
    pub toplevels: &'a [&'a str],
    pub masks: &'a Masks,
}

/// One pair to compare for one referrer. A path referenced on one side only, and absent from
/// the other side's closure, pairs with `""` there: its content shows as added or removed.
struct Job {
    referrer: Option<FileChange>,
    old: String,
    new: String,
    /// A small rebuild nothing points at: only its `bin/` counts.
    small_rebuild: bool,
    /// The pairs from the first referrer down to this one, so a chain never loops.
    chain: Vec<(String, String)>,
}

impl Job {
    /// A small rebuild compares only `bin/`, a reference may compare a file: two results.
    fn key(&self) -> (String, String, bool) {
        (self.old.clone(), self.new.clone(), self.small_rebuild)
    }
}

/// What the round trip takes of a root (see [`super::SCRIPT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// A directory's `bin/`, or anything else whole.
    Whole,
    /// Only what is not a directory: a large pair is compared only as files.
    File,
    /// Only a directory's `bin/`: a small rebuild.
    Bin,
}

impl Take {
    fn letter(self) -> char {
        match self {
            Take::Whole => 'w',
            Take::File => 'f',
            Take::Bin => 'b',
        }
    }

    /// Two requests for one root: `Whole` covers any two different ones.
    fn merge(self, other: Take) -> Take {
        if self == other { self } else { Take::Whole }
    }
}

/// A pair's comparison, before it is named after a referrer.
struct Compared {
    changes: Vec<FileChange>,
    old_name: String,
    new_name: String,
    bin: bool,
}

/// The followed results of `changes` and `small_rebuilds`, named and chained, unsorted.
pub(crate) async fn follow(
    exec: &dyn Exec,
    hosts: &mut [Host],
    sides: Sides<'_>,
    max_file_size: u64,
    changes: &[FileChange],
    small_rebuilds: &[SmallRebuild],
) -> Result<Vec<FileChange>> {
    let mut jobs = referenced(changes, &[], &sides);
    let pointed_at: HashSet<(String, String)> = jobs
        .iter()
        .map(|j| (j.old.clone(), j.new.clone()))
        .collect();
    jobs.extend(
        small_rebuilds
            .iter()
            .filter(|s| !pointed_at.contains(&(s.old.clone(), s.new.clone())))
            .map(|s| Job {
                referrer: None,
                old: s.old.clone(),
                new: s.new.clone(),
                small_rebuild: true,
                // A script naming its own package must not lead back to it.
                chain: vec![(s.old.clone(), s.new.clone())],
            }),
    );
    let small = |path: &str, sizes: &BTreeMap<String, u64>| {
        path.is_empty()
            || sizes
                .get(path)
                .is_some_and(|&s| s <= SMALL_REBUILD_MAX_BYTES)
    };
    let absent = Tree::new();
    let mut compared: HashMap<(String, String, bool), Option<Compared>> = HashMap::new();
    let mut results = Vec::new();
    for _ in 0..DEPTH {
        // Roots of pairs not compared yet, per host.
        let mut wanted: Vec<BTreeMap<String, Take>> = vec![BTreeMap::new(); hosts.len()];
        for job in &jobs {
            if compared.contains_key(&job.key()) {
                continue;
            }
            let take = if job.small_rebuild {
                Take::Bin
            } else if small(&job.old, sides.old_sizes) && small(&job.new, sides.new_sizes) {
                Take::Whole
            } else {
                Take::File
            };
            for (host, root) in [(sides.old, &job.old), (sides.new, &job.new)] {
                if root.is_empty() {
                    continue;
                }
                let entry = wanted[host].entry(root.clone()).or_insert(take);
                *entry = entry.merge(take);
            }
        }
        let fetched = join(
            hosts
                .iter()
                .zip(&wanted)
                .map(|(host, wanted)| fetch(exec, host, wanted, max_file_size)),
        )
        .await;
        let mut trees = Vec::new();
        for (host, result) in hosts.iter_mut().zip(fetched) {
            let (host_trees, contents) = result?;
            host.contents.extend(contents);
            trees.push(host_trees);
        }

        let mut next = Vec::new();
        for job in jobs {
            let key = job.key();
            if !compared.contains_key(&key) {
                let tree = |host: usize, root: &String| {
                    if root.is_empty() {
                        &absent
                    } else {
                        &trees[host][root]
                    }
                };
                let pair = (tree(sides.old, &job.old), tree(sides.new, &job.new));
                let both_small =
                    small(&job.old, sides.old_sizes) && small(&job.new, sides.new_sizes);
                let result = compare_pair(&job, pair, both_small, hosts, &sides, max_file_size);
                compared.insert(key.clone(), result);
            }
            let Some(result) = &compared[&key] else {
                continue;
            };
            let named = name(result, job.referrer.as_ref());
            next.extend(referenced(&named, &job.chain, &sides));
            results.extend(named);
        }
        jobs = next;
        if jobs.is_empty() {
            break;
        }
    }
    Ok(results)
}

/// Pairs from every text file, with the file as referrer (spec 7.4), except pairs already on
/// the referrer's `chain` and references to a toplevel: the paired references of a modified
/// file, and every other reference on an added line that the old closure lacks, or on a
/// removed line that the new closure lacks, paired with `""`.
fn referenced(changes: &[FileChange], chain: &[(String, String)], sides: &Sides<'_>) -> Vec<Job> {
    changes
        .iter()
        .flat_map(|change| {
            let paired = if change.status == FileStatus::Modified {
                refs::pairs(change)
            } else {
                Vec::new()
            };
            let one_sided = |op, closure: &BTreeMap<String, u64>| -> Vec<String> {
                refs::on_lines(change, op)
                    .into_iter()
                    .filter(|path| {
                        !closure.contains_key(path)
                            && !paired.iter().any(|(o, n)| o == path || n == path)
                    })
                    .collect()
            };
            let added = one_sided(LineOp::Added, sides.old_sizes);
            let removed = one_sided(LineOp::Removed, sides.new_sizes);
            paired
                .into_iter()
                .chain(added.into_iter().map(|n| (String::new(), n)))
                .chain(removed.into_iter().map(|o| (o, String::new())))
                .filter(|pair| {
                    !chain.contains(pair)
                        && !sides.toplevels.contains(&pair.0.as_str())
                        && !sides.toplevels.contains(&pair.1.as_str())
                })
                .map(|pair| Job {
                    referrer: Some(change.clone()),
                    chain: chain.iter().cloned().chain([pair.clone()]).collect(),
                    old: pair.0,
                    new: pair.1,
                    small_rebuild: false,
                })
        })
        .collect()
}

/// As files when both are regular files, as `bin/` when both are small directories; a small
/// rebuild by its `bin/` only. An absent side (`""`) matches either. `None` when the pair is
/// neither.
fn compare_pair(
    job: &Job,
    (old, new): (&Tree, &Tree),
    both_small: bool,
    hosts: &[Host],
    sides: &Sides<'_>,
    max_file_size: u64,
) -> Option<Compared> {
    let (old, new, bin) = if job.small_rebuild {
        (below(old, "bin"), below(new, "bin"), true)
    } else {
        let root_is = |root: &str, tree: &Tree, dir: bool| {
            root.is_empty()
                || match tree.get("") {
                    Some(Entry::File { .. }) => !dir,
                    Some(Entry::Directory) => dir,
                    _ => false,
                }
        };
        let both = |dir| root_is(&job.old, old, dir) && root_is(&job.new, new, dir);
        if both(false) {
            (old.clone(), new.clone(), false)
        } else if both(true) && both_small {
            (below(old, "bin"), below(new, "bin"), true)
        } else {
            return None;
        }
    };
    let changes = compare::compare(
        &old,
        &new,
        max_file_size,
        &hosts[sides.old].contents,
        &hosts[sides.new].contents,
        sides.masks,
    );
    Some(Compared {
        changes,
        old_name: store_path_name(&job.old).to_owned(),
        new_name: store_path_name(&job.new).to_owned(),
        bin,
    })
}

/// `referrer → name…`, or `name/bin/script` without a referrer. A file only on the old side
/// carries the old store name (spec 7.4).
fn name(compared: &Compared, referrer: Option<&FileChange>) -> Vec<FileChange> {
    let (removed, kept): (Vec<_>, Vec<_>) = compared
        .changes
        .iter()
        .cloned()
        .partition(|c| c.status == FileStatus::Removed);
    let mut named = refs::label(kept, referrer, &compared.new_name, compared.bin);
    named.extend(refs::label(
        removed,
        referrer,
        &compared.old_name,
        compared.bin,
    ));
    named
}

/// The entries strictly below `dir` (`""` for the root), relative to it.
fn below(tree: &Tree, dir: &str) -> Tree {
    tree.iter()
        .filter_map(|(path, entry)| {
            let rel = if dir.is_empty() {
                Some(path.as_str()).filter(|p| !p.is_empty())
            } else {
                path.strip_prefix(dir)?.strip_prefix('/')
            };
            rel.map(|rel| (rel.to_owned(), entry.clone()))
        })
        .collect()
}

pub fn command(roots: &BTreeMap<String, Take>, gzip: bool, max_file_size: u64) -> Command {
    let limit = max_file_size.saturating_add(1).to_string();
    let mut command = SCRIPT.command(["refs", LISTING.text(), gzip_arg(gzip), &limit]);
    let requests: Vec<String> = roots
        .iter()
        .map(|(root, take)| format!("{} {root}", take.letter()))
        .collect();
    command.stdin = lines(requests.iter().map(String::as_str));
    command
}

/// One round trip: the trees of `roots`, one link level deep, and their files' contents.
async fn fetch(
    exec: &dyn Exec,
    host: &Host,
    roots: &BTreeMap<String, Take>,
    max_file_size: u64,
) -> Result<(HashMap<String, Tree>, Contents)> {
    if roots.is_empty() {
        return Ok((HashMap::new(), Contents::new()));
    }
    let output = exec
        .run(&host.machine, command(roots, host.gzip, max_file_size))
        .await?;
    let (listed, archive) = split_archive(&output.stdout);
    let (listed, failure) = super::split_failure(&host.machine, listed);
    if let Some(error) = failure
        .or_else(|| tar_failure(&host.machine, &output))
        .or_else(|| super::unexplained(&output))
    {
        return Err(error.into());
    }
    let archive = archive.with_context(|| format!("no archive from {}", host.machine))?;
    let mut walks: Vec<Walk> = roots.keys().map(|r| Walk::reference(r)).collect();
    let sets: Vec<HashSet<String>> = walks
        .iter_mut()
        .map(|w| w.next_roots().into_iter().collect())
        .collect();
    let parts = listing::split(listed, &sets);
    let contents = unpack(archive, host.gzip)?;
    let mut trees = HashMap::new();
    for ((root, walk), part) in roots.keys().zip(&mut walks).zip(parts) {
        walk.add_listing(&part)?;
        let mut tree = walk.tree();
        // A link out of the listed tree was not followed, so its content is unknown, not
        // missing; calling it dangling would be wrong.
        tree.retain(|_, entry| !matches!(entry, Entry::DanglingLink(_)));
        trees.insert(root.clone(), tree);
    }
    Ok((trees, contents))
}

/// The listing before the `::scry tar` line, and the archive after it.
fn split_archive(stdout: &[u8]) -> (&[u8], Option<&[u8]>) {
    const MARK: &[u8] = b"::scry tar\n";
    let at = if stdout.starts_with(MARK) {
        Some(0)
    } else {
        super::find(stdout, b"\n::scry tar\n").map(|i| i + 1)
    };
    match at {
        Some(at) => (&stdout[..at], Some(&stdout[at + MARK.len()..])),
        None => (stdout, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_strips_the_directory() {
        let file = |p: &str| Entry::File {
            path: p.into(),
            cksum: 1,
            size: 1,
        };
        let tree: Tree = [
            ("".to_owned(), Entry::Directory),
            ("bin".to_owned(), Entry::Directory),
            ("bin/app".to_owned(), file("/s/bin/app")),
            ("binary".to_owned(), file("/s/binary")),
            ("share/x".to_owned(), file("/s/share/x")),
        ]
        .into();
        assert_eq!(below(&tree, "bin").keys().collect::<Vec<_>>(), ["app"]);
        assert_eq!(below(&tree, "").len(), 4);
    }

    #[test]
    fn archive_follows_the_listing() {
        assert_eq!(
            split_archive(b"/a\n::scry tar\n\x00\x01"),
            (&b"/a\n"[..], Some(&b"\x00\x01"[..]))
        );
        assert_eq!(split_archive(b"::scry tar\n"), (&b""[..], Some(&b""[..])));
        assert_eq!(split_archive(b"/a\n::scry failed find 1\n").1, None);
    }
}
