//! Step 2 of spec 6.2: the level walk of every side on one machine, one round trip per link
//! level. The walks' roots go to one [`crate::tree::LISTING`] run, and its output is split back
//! by root, so two sides on one machine share each level (spec 6.1).

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use super::SCRIPT;
use crate::exec::{Command, Exec};
use crate::transport::machine::Machine;
use crate::tree::{self, LISTING, Walk};

/// The newline marker of [`LISTING`].
const NEWLINE_MARKER: &str = "newline: ";

pub fn command(roots: &BTreeSet<String>) -> Command {
    let mut command = SCRIPT.command(["list", LISTING.text()]);
    command.stdin = super::content::lines(roots.iter().map(String::as_str));
    command
}

/// Lists level after level until no walk has roots left.
pub async fn walk(exec: &dyn Exec, machine: &Machine, walks: &mut [Walk]) -> Result<()> {
    loop {
        let roots: Vec<HashSet<String>> = walks
            .iter_mut()
            .map(|w| w.next_roots().into_iter().collect())
            .collect();
        let all: BTreeSet<String> = roots.iter().flatten().cloned().collect();
        if all.is_empty() {
            return Ok(());
        }
        let output = exec.run(machine, command(&all)).await?;
        for (walk, part) in walks.iter_mut().zip(list_output(machine, &output, &roots)?) {
            walk.add_listing(&part)?;
        }
    }
}

/// The listing in `output`, split per set of roots; a failed `find` is an error (spec 6.2).
pub(crate) fn list_output(
    machine: &Machine,
    output: &crate::exec::Output,
    roots: &[HashSet<String>],
) -> Result<Vec<Vec<u8>>> {
    let (stdout, failure) = super::split_failure(machine, &output.stdout);
    if let Some(error) = failure.or_else(|| super::unexplained(output)) {
        return Err(error.into());
    }
    Ok(split(stdout, roots))
}

/// Gives each line to every root set holding a root at or above its path. A line that does not
/// parse continues the name of the line before it (a newline in a name), so it goes where that
/// one went; the walk then reports it. A line under no root goes everywhere, for the same
/// reason.
pub(crate) fn split(output: &[u8], roots: &[HashSet<String>]) -> Vec<Vec<u8>> {
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, set) in roots.iter().enumerate() {
        for root in set {
            index.entry(root).or_default().push(i);
        }
    }
    let mut parts = vec![Vec::new(); roots.len()];
    let everyone: Vec<usize> = (0..roots.len()).collect();
    let mut owners = everyone.clone();
    for line in output.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let path = std::str::from_utf8(line).ok().and_then(|line| {
            line.strip_prefix(NEWLINE_MARKER)
                .or_else(|| tree::parse_line(line).map(|(path, _)| path))
        });
        if let Some(path) = path {
            owners = prefixes(path)
                .filter_map(|p| index.get(p))
                .flatten()
                .copied()
                .collect::<BTreeSet<usize>>()
                .into_iter()
                .collect();
            if owners.is_empty() {
                owners.clone_from(&everyone);
            }
        }
        for &i in &owners {
            parts[i].extend_from_slice(line);
            parts[i].push(b'\n');
        }
    }
    parts
}

/// `path` and each of its parents but `/`.
fn prefixes(path: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(path), |p| {
        p.rfind('/').filter(|&i| i > 0).map(|i| &p[..i])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(roots: &[&str]) -> HashSet<String> {
        roots.iter().map(|&r| r.to_owned()).collect()
    }

    #[test]
    fn lines_go_to_every_walk_that_asked_for_their_root() {
        let output = b"/s/a\n1 2 /s/a/f\n/s/b\n1 2 /s/b/g\n\
lrwxrwxrwx 1 0 0 2 Jan  1  1970 /s/a/sub/l -> /x\nnewline: /s/b/n\nline\n";
        let parts = split(output, &[set(&["/s/a"]), set(&["/s/a/sub", "/s/b"])]);
        assert_eq!(
            String::from_utf8(parts[0].clone()).unwrap(),
            "/s/a\n1 2 /s/a/f\nlrwxrwxrwx 1 0 0 2 Jan  1  1970 /s/a/sub/l -> /x\n"
        );
        assert_eq!(
            String::from_utf8(parts[1].clone()).unwrap(),
            "/s/b\n1 2 /s/b/g\nlrwxrwxrwx 1 0 0 2 Jan  1  1970 /s/a/sub/l -> /x\nnewline: /s/b/n\nline\n"
        );
        let stray = split(b"1 2 /elsewhere\n", &[set(&["/s/a"]), set(&["/s/b"])]);
        assert!(stray.iter().all(|p| p == b"1 2 /elsewhere\n"));
    }
}
