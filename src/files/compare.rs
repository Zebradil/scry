//! File changes from two dereferenced trees (spec 7.3).

use std::collections::{BTreeSet, HashMap};

use crate::files::{churn, diff, highlight};
use crate::report::{ChurnClass, FileChange, FileStatus};
use crate::tree::{Entry, Tree};

/// Fetched contents of one side, by absolute path ([`Entry::File`]'s `path`).
pub type Contents = HashMap<String, Vec<u8>>;

/// Absolute paths whose contents [`compare`] needs, per side.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fetch {
    pub old: Vec<String>,
    pub new: Vec<String>,
}

const BINARY_PROBE: usize = 8 * 1024;

/// Both sides of every modified file within `max_file_size` on both sides, and the one side of
/// every added or removed file within it. Sorted, without duplicates: several paths may
/// dereference to one file.
pub fn to_fetch(old: &Tree, new: &Tree, max_file_size: u64) -> Fetch {
    let mut fetch = Fetch::default();
    for (_, o, n, status) in differing(old, new) {
        if !matches!(
            status,
            FileStatus::Modified | FileStatus::Added | FileStatus::Removed
        ) {
            continue;
        }
        let within = |e: Option<&Entry>| match e {
            Some(Entry::File { size, .. }) => *size <= max_file_size,
            None => true,
            Some(_) => false,
        };
        if within(o) && within(n) {
            fetch.old.extend(path(o));
            fetch.new.extend(path(n));
        }
    }
    for paths in [&mut fetch.old, &mut fetch.new] {
        paths.sort();
        paths.dedup();
    }
    fetch
}

/// The file changes from `old` to `new`, sorted by path, with hunks highlighted and modified
/// text files classified. A file's content comes from the contents of its side; a file over
/// `max_file_size`, or missing from them, is not fetched.
pub fn compare(
    old: &Tree,
    new: &Tree,
    max_file_size: u64,
    old_contents: &Contents,
    new_contents: &Contents,
) -> Vec<FileChange> {
    differing(old, new)
        .map(|(path, o, n, status)| {
            let mut change = FileChange {
                path: path.to_owned(),
                via: Vec::new(),
                status,
                class: ChurnClass::Real,
                type_old: o.map(Entry::file_type),
                type_new: n.map(Entry::file_type),
                binary: false,
                content_fetched: false,
                size_old: size(o),
                size_new: size(n),
                link_old: link(o),
                link_new: link(n),
                hunks: Vec::new(),
            };
            if matches!(status, FileStatus::LinkChanged | FileStatus::TypeChanged) {
                return change;
            }
            let (Some(old_bytes), Some(new_bytes)) = (
                content(o, old_contents, max_file_size),
                content(n, new_contents, max_file_size),
            ) else {
                return change;
            };
            change.content_fetched = true;
            let (Some(old_text), Some(new_text)) = (text(old_bytes), text(new_bytes)) else {
                change.binary = true;
                return change;
            };
            change.hunks = diff::hunks(old_text, new_text);
            highlight::highlight(&mut change.hunks);
            if status == FileStatus::Modified {
                change.class = churn::classify(old_text, new_text);
            }
            change
        })
        .collect()
}

/// Display order (spec 7.3): by path in byte order, a referrer's followed references right
/// after it, chains depth first. Small rebuilds without a referrer sort by their own path.
pub fn sort(changes: &mut [FileChange]) {
    changes.sort_by(|a, b| (&a.path, &a.via).cmp(&(&b.path, &b.via)));
}

/// Every path whose entries differ, with its status, in path order. Directories only count
/// when the type changes: their files carry the change.
fn differing<'a>(
    old: &'a Tree,
    new: &'a Tree,
) -> impl Iterator<Item = (&'a str, Option<&'a Entry>, Option<&'a Entry>, FileStatus)> {
    let paths: BTreeSet<&str> = old.keys().chain(new.keys()).map(String::as_str).collect();
    paths.into_iter().filter_map(|path| {
        let (o, n) = (old.get(path), new.get(path));
        status(o, n).map(|status| (path, o, n, status))
    })
}

fn status(old: Option<&Entry>, new: Option<&Entry>) -> Option<FileStatus> {
    use Entry::*;
    let (old, new) = match (old, new) {
        (Some(o), Some(n)) => (o, n),
        (Some(Directory), None) | (None, Some(Directory) | None) => return None,
        (Some(_), None) => return Some(FileStatus::Removed),
        (None, Some(_)) => return Some(FileStatus::Added),
    };
    match (old, new) {
        (
            File {
                cksum: a, size: x, ..
            },
            File {
                cksum: b, size: y, ..
            },
        ) => ((a, x) != (b, y)).then_some(FileStatus::Modified),
        (Directory, Directory) => None,
        (ExternalLink(a), ExternalLink(b)) => (a != b).then_some(FileStatus::LinkChanged),
        // A new store target without new content is no change (spec 7.3), and a missing target
        // has no content to tell.
        (DanglingLink(_), DanglingLink(_)) => None,
        _ => Some(FileStatus::TypeChanged),
    }
}

/// The bytes to diff for one side: empty where the path is absent, `None` where there is no
/// fetched regular file.
fn content<'a>(entry: Option<&Entry>, contents: &'a Contents, max: u64) -> Option<&'a [u8]> {
    match entry {
        None => Some(&[]),
        Some(Entry::File { path, size, .. }) if *size <= max => {
            contents.get(path).map(Vec::as_slice)
        }
        Some(_) => None,
    }
}

/// `None` for binary content: a NUL byte in the first 8 KiB, or not UTF-8.
fn text(bytes: &[u8]) -> Option<&str> {
    if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn path(entry: Option<&Entry>) -> Option<String> {
    match entry {
        Some(Entry::File { path, .. }) => Some(path.clone()),
        _ => None,
    }
}

fn size(entry: Option<&Entry>) -> Option<u64> {
    match entry {
        Some(Entry::File { size, .. }) => Some(*size),
        _ => None,
    }
}

fn link(entry: Option<&Entry>) -> Option<String> {
    match entry {
        Some(Entry::ExternalLink(text) | Entry::DanglingLink(text)) => Some(text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{FileType, LineOp};

    const O: &str = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc";
    const N: &str = "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-etc/etc";
    const MAX: u64 = 10_000;

    fn file(base: &str, rel: &str, content: &[u8]) -> (String, Entry, Vec<u8>) {
        let path = format!("{base}/{rel}");
        // Content stands in for the checksum: equal content, equal (cksum, size).
        let cksum = content
            .iter()
            .fold(7u32, |h, &b| h.wrapping_mul(31).wrapping_add(b.into()));
        let entry = Entry::File {
            path: path.clone(),
            cksum,
            size: content.len() as u64,
        };
        (path, entry, content.to_vec())
    }

    struct Side {
        tree: Tree,
        contents: Contents,
    }

    fn side(base: &str, files: &[(&str, &[u8])], others: &[(&str, Entry)]) -> Side {
        let mut s = Side {
            tree: Tree::new(),
            contents: Contents::new(),
        };
        for (rel, content) in files {
            let (path, entry, bytes) = file(base, rel, content);
            s.tree.insert((*rel).into(), entry);
            s.contents.insert(path, bytes);
        }
        for (rel, entry) in others {
            s.tree.insert((*rel).into(), entry.clone());
        }
        s
    }

    fn fixture() -> (Side, Side) {
        // Valid UTF-8 with a NUL just past the first 8 KiB: text.
        let late_nul = |end: &[u8]| [&[b'a'; 8192][..], b"\0", end].concat();
        let old = side(
            O,
            &[
                ("same", b"same\n"),
                ("text", b"a\nb\n"),
                (
                    "churn",
                    b"x=/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-a/b\n",
                ),
                ("big", &[b'0'; 10_001]),
                ("grows", b"small\n"),
                ("shrinks", &[b'x'; 10_001]),
                ("late-nul", &late_nul(b"x\n")),
                ("nul", b"a\0b"),
                ("latin1", b"caf\xe9"),
                ("gone", b"bye\n"),
                ("was-file", b"x\n"),
                ("unit", b"[Unit]\n"),
            ],
            &[
                ("etc-dir", Entry::Directory),
                ("mtab", Entry::ExternalLink("/proc/mounts".into())),
                ("old-dir", Entry::Directory),
                (
                    "dangling",
                    Entry::DanglingLink("/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-x".into()),
                ),
            ],
        );
        let new = side(
            N,
            &[
                ("same", b"same\n"),
                ("text", b"a\nc\n"),
                (
                    "churn",
                    b"x=/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-a/b\n",
                ),
                ("big", &[b'0'; 10_002]),
                ("grows", &[b'x'; 10_001]),
                ("shrinks", b"small\n"),
                ("late-nul", &late_nul(b"y\n")),
                ("nul", b"a\0c"),
                ("latin1", b"caf\xe8"),
                ("added", b"hi\n"),
                ("added-bin", b"\0"),
            ],
            &[
                ("etc-dir", Entry::Directory),
                ("mtab", Entry::ExternalLink("/proc/self/mounts".into())),
                ("new-dir", Entry::Directory),
                ("was-file", Entry::Directory),
                ("unit", Entry::ExternalLink("/dev/null".into())),
                (
                    "dangling",
                    Entry::DanglingLink("/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-x".into()),
                ),
            ],
        );
        (old, new)
    }

    #[test]
    fn fetches_changed_files_within_the_limit() {
        let (old, new) = fixture();
        let fetch = to_fetch(&old.tree, &new.tree, MAX);
        let rels = |paths: &[String], base: &str| -> Vec<String> {
            paths
                .iter()
                .map(|p| p[base.len() + 1..].to_owned())
                .collect()
        };
        assert_eq!(
            rels(&fetch.old, O),
            ["churn", "gone", "late-nul", "latin1", "nul", "text"]
        );
        assert_eq!(
            rels(&fetch.new, N),
            [
                "added",
                "added-bin",
                "churn",
                "late-nul",
                "latin1",
                "nul",
                "text"
            ]
        );
    }

    #[test]
    fn statuses_and_content() {
        let (old, new) = fixture();
        let changes = compare(&old.tree, &new.tree, MAX, &old.contents, &new.contents);
        let summary: Vec<_> = changes
            .iter()
            .map(|c| {
                (
                    c.path.as_str(),
                    c.status,
                    c.class,
                    c.binary,
                    c.content_fetched,
                )
            })
            .collect();
        use ChurnClass::*;
        use FileStatus::*;
        assert_eq!(
            summary,
            [
                ("added", Added, Real, false, true),
                ("added-bin", Added, Real, true, true),
                ("big", Modified, Real, false, false),
                ("churn", Modified, HashOnly, false, true),
                ("gone", Removed, Real, false, true),
                ("grows", Modified, Real, false, false),
                ("late-nul", Modified, Real, false, true),
                ("latin1", Modified, Real, true, true),
                ("mtab", LinkChanged, Real, false, false),
                ("nul", Modified, Real, true, true),
                ("shrinks", Modified, Real, false, false),
                ("text", Modified, Real, false, true),
                ("unit", TypeChanged, Real, false, false),
                ("was-file", TypeChanged, Real, false, false),
            ]
        );
        let by_path = |p: &str| changes.iter().find(|c| c.path == p).unwrap();

        let added = by_path("added");
        assert_eq!((added.hunks[0].old_start, added.hunks[0].old_lines), (0, 0));
        assert_eq!(added.hunks[0].lines[0].op, LineOp::Added);
        assert_eq!((added.size_old, added.size_new), (None, Some(3)));
        assert!(by_path("added-bin").hunks.is_empty());

        let big = by_path("big");
        assert_eq!((big.size_old, big.size_new), (Some(10_001), Some(10_002)));
        assert!(big.hunks.is_empty());

        assert_eq!(by_path("late-nul").hunks.len(), 1);

        let text = by_path("text");
        let ops: Vec<_> = text.hunks[0].lines.iter().map(|l| l.op).collect();
        assert_eq!(ops, [LineOp::Context, LineOp::Removed, LineOp::Added]);
        assert_eq!(text.hunks[0].lines[2].spans, [(0, 1)]);
        assert!(by_path("churn").hunks[0].lines[0].hash_only);

        let mtab = by_path("mtab");
        assert_eq!(mtab.link_old.as_deref(), Some("/proc/mounts"));
        assert_eq!(mtab.link_new.as_deref(), Some("/proc/self/mounts"));
        let unit = by_path("unit");
        assert_eq!(unit.type_old, Some(FileType::File));
        assert_eq!(unit.type_new, Some(FileType::ExternalLink));
        assert_eq!(unit.link_new.as_deref(), Some("/dev/null"));
        assert_eq!(by_path("was-file").type_new, Some(FileType::Directory));
    }

    #[test]
    fn unfetched_content_is_reported_as_not_fetched() {
        let (old, new) = fixture();
        let changes = compare(&old.tree, &new.tree, MAX, &Contents::new(), &new.contents);
        let text = changes.iter().find(|c| c.path == "text").unwrap();
        assert!(!text.content_fetched && text.hunks.is_empty());
    }

    #[test]
    fn display_order_puts_references_after_their_referrer() {
        let (old, new) = fixture();
        let mut changes = compare(&old.tree, &new.tree, MAX, &old.contents, &new.contents);
        let template = changes[0].clone();
        let with = |path: &str, via: &[&str]| FileChange {
            path: path.into(),
            via: via.iter().map(|v| (*v).to_owned()).collect(),
            ..template.clone()
        };
        changes = vec![
            with("text b", &[]),
            with("text", &["z"]),
            with("app-start/bin/app-start", &[]),
            with("text", &["a", "b"]),
            with("text", &[]),
            with("text", &["a"]),
        ];
        sort(&mut changes);
        let order: Vec<String> = changes
            .iter()
            .map(crate::files::refs::display_path)
            .collect();
        assert_eq!(
            order,
            [
                "app-start/bin/app-start",
                "text",
                "text → a",
                "text → a → b",
                "text → z",
                "text b"
            ]
        );
    }
}
