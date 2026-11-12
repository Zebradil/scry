//! Tree listings and the level walk (spec 5, 6.2 step 2).
//!
//! Each level runs [`listing_command`] on the side's machine (GNU or BSD userland). [`Walk`]
//! decides the roots of each level and rebuilds the dereferenced tree from the listings; the
//! caller runs the commands.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::exec::Command;
use crate::report::{ClosureKind, FileType};
use crate::store::name::STORE_PREFIX;
use crate::transport::script::Script;

/// Lists the roots read from stdin, one per line, without following links (spec 6.2):
/// - roots that do not exist are skipped, since `find` fails on them; their links are dangling;
/// - `find` runs on at most 500 roots at a time: a few hundred bytes each stay far below
///   `ARG_MAX` (1 MiB on macOS);
/// - a name with a newline is reported by the first alternative, because `ls` prints the newline
///   as `?` and `cksum` and `-print` split the line;
/// - `ls -ldn` prints numeric ids, so an owner or group name with a space cannot shift fields;
/// - `-a` binds tighter than `-o`, so no parentheses are needed.
pub const LISTING: Script = Script::new(
    r#"LC_ALL=C
export LC_ALL
nl="
"
list() {
  [ $# -eq 0 ] || find "$@" -name "*$nl*" -exec echo newline: {} ";" -o -type l -exec ls -ldn {} + -o -type f -exec cksum {} + -o -type d -print
}
set --
while IFS= read -r r; do
  if [ -e "$r" ] || [ -h "$r" ]; then set -- "$@" "$r"; fi
  if [ $# -ge 500 ]; then list "$@" || exit; set --; fi
done
list "$@""#,
);

/// The marker [`LISTING`] prints before a path whose name holds a newline.
const NEWLINE_MARKER: &str = "newline: ";

/// [`LISTING`] with `roots` on stdin.
pub fn listing_command(roots: &[String]) -> Command {
    let mut command = LISTING.command::<_, &str>([]);
    command.stdin = roots
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>()
        .into();
    command
}

/// Paths relative to the toplevel that Files never shows, with everything below them (spec 5):
/// the marker files and `kernel-params`, shown elsewhere, and per-user profiles, which link to
/// package trees.
pub const EXCLUDED: [&str; 6] = [
    "nixos-version",
    "darwin-version",
    "hm-version",
    "darwin-version.json",
    "kernel-params",
    "etc/profiles/per-user",
];

/// Trees under the toplevel whose files are compared, store links dereferenced (spec 5).
pub fn followed_trees(kind: ClosureKind) -> &'static [&'static str] {
    match kind {
        ClosureKind::Nixos => &["etc"],
        ClosureKind::NixDarwin => &[
            "etc",
            "Library/LaunchAgents",
            "Library/LaunchDaemons",
            "user/Library/LaunchAgents",
        ],
        ClosureKind::HomeManager => &["home-files", "LaunchAgents"],
        ClosureKind::Unknown => &[],
    }
}

/// One listing line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    File {
        cksum: u32,
        size: u64,
    },
    Directory,
    /// The link's target text.
    Link(String),
}

/// A path of the dereferenced tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// `path` is the absolute path holding the content, store links dereferenced: what a
    /// content fetch asks for.
    File {
        path: String,
        cksum: u32,
        size: u64,
    },
    Directory,
    /// Target outside the store, compared by its text and never followed.
    ExternalLink(String),
    /// Store target missing from the listings.
    DanglingLink(String),
}

impl Entry {
    pub fn file_type(&self) -> FileType {
        match self {
            Entry::File { .. } => FileType::File,
            Entry::Directory => FileType::Directory,
            Entry::ExternalLink(_) => FileType::ExternalLink,
            Entry::DanglingLink(_) => FileType::DanglingLink,
        }
    }
}

/// Relative path to entry. The toplevel's tree holds its own regular files under their names
/// and the followed trees below theirs.
pub type Tree = BTreeMap<String, Entry>;

/// Parses one listing line: a directory path, `cksum` output (`<crc> <size> <path>`), or
/// `ls -ldn` output of a link. Paths are absolute. `None` for anything else.
pub fn parse_line(line: &str) -> Option<(&str, Listed)> {
    match line.as_bytes().first()? {
        b'/' => Some((line, Listed::Directory)),
        b'0'..=b'9' => {
            let (crc, rest) = line.split_once(' ')?;
            let (size, path) = rest.split_once(' ')?;
            let file = Listed::File {
                cksum: crc.parse().ok()?,
                size: size.parse().ok()?,
            };
            path.starts_with('/').then_some((path, file))
        }
        b'l' => parse_ls(line),
        _ => None,
    }
}

/// `<mode> <links> <uid> <gid> <size> <date> <path> -> <target>`. The date's format varies
/// (`Jan  1  1970`, `Nov 12 22:34`) but holds no `/`, so the path starts at the first ` /`. A
/// link's size is its target's byte length, which tells ` -> ` inside a name or target from
/// the separator.
fn parse_ls(line: &str) -> Option<(&str, Listed)> {
    let mut rest = line;
    let mut size = "";
    for _ in 0..5 {
        rest = rest.trim_start_matches(' ');
        let end = rest.find(' ')?;
        size = &rest[..end];
        rest = &rest[end..];
    }
    let size: usize = size.parse().ok()?;
    let rest = &rest[rest.find(" /")? + 1..];
    let split = rest.len().checked_sub(size)?;
    let path = rest.get(..split)?.strip_suffix(" -> ")?;
    Some((path, Listed::Link(rest.get(split..)?.to_owned())))
}

/// The level walk of one side: which roots to list next, and the dereferenced tree.
///
/// ```text
/// let mut walk = Walk::toplevel(toplevel, kind);
/// loop {
///     let roots = walk.next_roots();
///     if roots.is_empty() { break }
///     walk.add_listing(&run(listing_command(&roots))?.stdout)?;
/// }
/// let tree = walk.tree();
/// ```
#[derive(Debug)]
pub struct Walk {
    root: String,
    followed: &'static [&'static str],
    excluded: &'static [&'static str],
    listed: BTreeMap<String, Listed>,
    /// Every root requested so far, with the name of its tree for error messages.
    roots: BTreeMap<String, String>,
}

impl Walk {
    /// A toplevel: its own regular files, and the followed trees of `kind` with store links at
    /// or below them dereferenced. Store links elsewhere are ignored.
    pub fn toplevel(path: &str, kind: ClosureKind) -> Self {
        Self::new(path, followed_trees(kind), &EXCLUDED)
    }

    /// A followed reference or a `bin/` directory (spec 7.4): everything at and below `path`,
    /// store links dereferenced. A regular file is the tree's root, at path `""`.
    pub fn reference(path: &str) -> Self {
        Self::new(path, &[""], &[])
    }

    fn new(
        path: &str,
        followed: &'static [&'static str],
        excluded: &'static [&'static str],
    ) -> Self {
        Self {
            root: normalize(path),
            followed,
            excluded,
            listed: BTreeMap::new(),
            roots: BTreeMap::new(),
        }
    }

    /// The roots to list next, absolute and sorted: the toplevel first, then the store link
    /// targets not listed or requested yet. Empty when the walk is done.
    pub fn next_roots(&mut self) -> Vec<String> {
        let missing = if self.roots.is_empty() {
            BTreeMap::from([(self.root.clone(), self.root.clone())])
        } else {
            self.resolve().missing
        };
        let next = missing.keys().cloned().collect();
        self.roots.extend(missing);
        next
    }

    /// Adds the output of [`LISTING`] for requested roots. A name with a newline fails with an
    /// error naming the tree, as does any line that is not a listing line under a requested
    /// root: a newline in a directory's name also splits the lines below it.
    pub fn add_listing(&mut self, output: &[u8]) -> Result<()> {
        let output = output.strip_suffix(b"\n").unwrap_or(output);
        if output.is_empty() {
            return Ok(());
        }
        let mut tree = "the listing";
        for raw in output.split(|&b| b == b'\n') {
            let Ok(line) = std::str::from_utf8(raw) else {
                bail!(
                    "file name not valid UTF-8 in {tree}: {:?}",
                    String::from_utf8_lossy(raw)
                );
            };
            if let Some(path) = line.strip_prefix(NEWLINE_MARKER) {
                let tree = root_of(&self.roots, path).unwrap_or(tree);
                bail!("file name with a newline in {tree}: {path:?}");
            }
            let parsed = parse_line(line)
                .and_then(|(path, listed)| root_of(&self.roots, path).map(|n| (path, listed, n)));
            let Some((path, listed, name)) = parsed else {
                bail!("file name with a newline in {tree}: unexpected listing line {line:?}");
            };
            tree = name;
            self.listed.insert(path.to_owned(), listed);
        }
        Ok(())
    }

    /// The dereferenced tree. Complete once [`Walk::next_roots`] returned no roots; until then,
    /// links to targets not listed yet show as dangling.
    pub fn tree(&self) -> Tree {
        self.resolve().tree
    }

    fn resolve(&self) -> Resolve<'_> {
        let mut r = Resolve {
            walk: self,
            tree: Tree::new(),
            missing: BTreeMap::new(),
            chain: Vec::new(),
        };
        // A root that does not exist, such as a reference missing on one side, is an empty tree.
        if self.listed.contains_key(&self.root) {
            r.follow(&self.root, "", &self.root);
        }
        r
    }

    fn region(&self, rel: &str) -> Region {
        if self.excluded.iter().any(|e| at_or_below(rel, e)) {
            Region::Excluded
        } else if self.followed.iter().any(|f| at_or_below(rel, f)) {
            Region::Followed
        } else {
            Region::Own
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Excluded,
    /// At or below a followed tree: everything counts, store links are dereferenced.
    Followed,
    /// The toplevel outside followed trees: only regular files count.
    Own,
}

struct Resolve<'a> {
    walk: &'a Walk,
    tree: Tree,
    /// Store targets neither listed nor requested, with the tree name of their link.
    missing: BTreeMap<String, String>,
    /// Paths being dereferenced, to stop at link cycles.
    chain: Vec<String>,
}

impl Resolve<'_> {
    /// Places the listed path `abs` and everything below it at `rel`. `text` is the target text
    /// of the link that led here.
    fn follow(&mut self, abs: &str, rel: &str, text: &str) {
        if self.chain.iter().any(|c| at_or_below(c, abs)) {
            // A link into its own ancestor: following it never ends, so it is compared by its
            // text like an external link.
            self.tree
                .insert(rel.to_owned(), Entry::ExternalLink(text.to_owned()));
            return;
        }
        let walk = self.walk;
        let Some(listed) = walk.listed.get(abs) else {
            if !walk.roots.contains_key(abs) && !in_listed_directory(&walk.listed, abs) {
                self.missing
                    .entry(abs.to_owned())
                    .or_insert_with(|| rel.to_owned());
            }
            self.tree
                .insert(rel.to_owned(), Entry::DanglingLink(text.to_owned()));
            return;
        };
        self.chain.push(abs.to_owned());
        self.place(abs, rel, listed);
        if *listed == Listed::Directory {
            // `find` lists real directories completely, so every descendant is in `listed`.
            let prefix = format!("{abs}/");
            for (path, listed) in walk.listed.range(prefix.clone()..) {
                let Some(sub) = path.strip_prefix(&prefix) else {
                    break;
                };
                let rel = if rel.is_empty() {
                    sub.to_owned()
                } else {
                    format!("{rel}/{sub}")
                };
                self.place(path, &rel, listed);
            }
        }
        self.chain.pop();
    }

    fn place(&mut self, abs: &str, rel: &str, listed: &Listed) {
        let entry = match (self.walk.region(rel), listed) {
            (Region::Excluded, _) | (Region::Own, Listed::Directory | Listed::Link(_)) => return,
            (_, &Listed::File { cksum, size }) => Entry::File {
                path: abs.to_owned(),
                cksum,
                size,
            },
            (Region::Followed, Listed::Directory) => Entry::Directory,
            (Region::Followed, Listed::Link(text)) => {
                let target = resolve_link(abs, text);
                if target.len() > STORE_PREFIX.len() && target.starts_with(STORE_PREFIX) {
                    return self.follow(&target, rel, text);
                }
                Entry::ExternalLink(text.clone())
            }
        };
        self.tree.insert(rel.to_owned(), entry);
    }
}

/// Whether the nearest listed ancestor of `path` is a directory. `find` lists real directories
/// completely, so a path missing below one does not exist, and listing it again is a wasted
/// round trip.
fn in_listed_directory(listed: &BTreeMap<String, Listed>, path: &str) -> bool {
    let mut prefix = path;
    while let Some(slash) = prefix.rfind('/') {
        prefix = &prefix[..slash];
        if let Some(entry) = listed.get(prefix) {
            return *entry == Listed::Directory;
        }
    }
    false
}

/// The tree name of the requested root that `path` lies at or below.
fn root_of<'a>(roots: &'a BTreeMap<String, String>, path: &str) -> Option<&'a str> {
    let mut prefix = path;
    loop {
        if let Some(name) = roots.get(prefix) {
            return Some(name);
        }
        prefix = &prefix[..prefix.rfind('/')?];
    }
}

fn at_or_below(path: &str, base: &str) -> bool {
    base.is_empty()
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The absolute target of the link at `link`: a relative target is taken from the link's
/// directory. Lexical, like the listings, which never resolve links.
fn resolve_link(link: &str, target: &str) -> String {
    if target.starts_with('/') {
        normalize(target)
    } else {
        let dir = &link[..link.rfind('/').unwrap_or(0)];
        normalize(&format!("{dir}/{target}"))
    }
}

/// Drops empty and `.` components and applies `..`, so equal paths compare equal.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured with [`LISTING`] on synthetic files (mtime 1, as in the store), macOS BSD find,
    /// ls and cksum; the scratch directory prefix and the uid replaced.
    const BSD: &str = "\
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/ssh
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/sub dir
lrwxr-xr-x 1 0 0  7 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/l -> k -> /x -> y
lrwxr-xr-x 1 0 0 12 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/mtab -> /proc/mounts
lrwxr-xr-x 1 0 0 18 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/rel link -> sub dir/with space
lrwxr-xr-x 1 0 0 72 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/ssh/moduli -> /nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-openssh-9.8p1/etc/ssh/moduli
4294967295 0 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/empty
2418082923 2 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/a -> b
3015617425 6 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/hosts
2192966820 2 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/sub dir/with space
";

    /// The same tree with GNU findutils 4.11.0 and coreutils 9.12: same line formats, other
    /// order.
    const GNU: &str = "\
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/ssh
/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/sub dir
lrwxr-xr-x 1 0 0  7 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/l -> k -> /x -> y
lrwxr-xr-x 1 0 0 12 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/mtab -> /proc/mounts
lrwxr-xr-x 1 0 0 18 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/rel link -> sub dir/with space
lrwxr-xr-x 1 0 0 72 Jan  1  1970 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/ssh/moduli -> /nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-openssh-9.8p1/etc/ssh/moduli
4294967295 0 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/empty
2192966820 2 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/sub dir/with space
2418082923 2 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/a -> b
3015617425 6 /nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc/etc/hosts
";

    const ROOT: &str = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-etc";

    fn link(target: &str) -> Listed {
        Listed::Link(target.into())
    }

    #[test]
    fn parses_bsd_and_gnu_listings() {
        for output in [BSD, GNU] {
            let mut walk = Walk::reference(ROOT);
            assert_eq!(walk.next_roots(), [ROOT]);
            walk.add_listing(output.as_bytes()).unwrap();
            let e = |rel: &str| walk.listed[&format!("{ROOT}/etc/{rel}")].clone();
            assert_eq!(e("l -> k"), link("/x -> y"));
            assert_eq!(e("mtab"), link("/proc/mounts"));
            assert_eq!(e("rel link"), link("sub dir/with space"));
            assert_eq!(
                e("ssh/moduli"),
                link("/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-openssh-9.8p1/etc/ssh/moduli")
            );
            assert_eq!(
                e("a -> b"),
                Listed::File {
                    cksum: 2418082923,
                    size: 2
                }
            );
            assert_eq!(
                e("sub dir/with space"),
                Listed::File {
                    cksum: 2192966820,
                    size: 2
                }
            );
            assert_eq!(
                e("empty"),
                Listed::File {
                    cksum: 4294967295,
                    size: 0
                }
            );
            assert_eq!(e("sub dir"), Listed::Directory);
            assert_eq!(walk.listed.len(), 12);
        }
    }

    #[test]
    fn parses_ls_variants() {
        let cases = [
            // A link modified this year shows the time instead of the year.
            "lrwxr-xr-x 1 0 0 12 Nov 12 22:34 /a/mtab -> /proc/mounts",
            // Extended attributes (macOS), SELinux context (GNU), long-iso time style.
            "lrwxr-xr-x@ 1 0 0 12 Jan  1  1970 /a/mtab -> /proc/mounts",
            "lrwxrwxrwx. 1 0 0 12 Jan  1  1970 /a/mtab -> /proc/mounts",
            "lrwxrwxrwx 1 0 0 12 1970-01-01 00:00 /a/mtab -> /proc/mounts",
        ];
        for line in cases {
            assert_eq!(
                parse_line(line),
                Some(("/a/mtab", link("/proc/mounts"))),
                "{line}"
            );
        }
        let arrows = "lrwxrwxrwx 1 0 0 7 Jan  1  1970 /a/x -> y -> /b -> c";
        assert_eq!(parse_line(arrows), Some(("/a/x -> y", link("/b -> c"))));
        for bad in [
            "lrwxrwxrwx 1 0 0 99 Jan  1  1970 /a/x -> y",
            "lrwxrwxrwx 1 0 0 1 Jan  1  1970 /a/x",
            "12 /a",
            "12 x /a",
            "x",
            "",
        ] {
            assert_eq!(parse_line(bad), None, "{bad}");
        }
    }

    #[test]
    fn newline_in_a_name_fails_naming_the_tree() {
        let t = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-nixos-system-web-a";
        let e = "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-etc/etc";
        let mut walk = Walk::toplevel(t, ClosureKind::Nixos);
        walk.next_roots();
        let level1 = format!("{t}\n{}", ls(&format!("{t}/etc"), e));
        walk.add_listing(level1.as_bytes()).unwrap();
        assert_eq!(walk.next_roots(), [e]);
        // The script marks a name with a newline.
        let err = walk
            .add_listing(format!("{e}\nnewline: {e}/ssh/new\nline\n").as_bytes())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("file name with a newline in etc: \"{e}/ssh/new\"")
        );
        // A directory's newline also splits the `cksum` and `find -print` lines below it.
        for output in [
            format!("{e}\n12 3 {e}/new\nline\n"),
            format!("{e}\n{e}/new\nline\n"),
        ] {
            let err = walk.add_listing(output.as_bytes()).unwrap_err();
            assert_eq!(
                err.to_string(),
                "file name with a newline in etc: unexpected listing line \"line\""
            );
        }
        let err = walk.add_listing(b"1 2 /nix/store/x/\xff\n").unwrap_err();
        assert!(
            err.to_string().starts_with("file name not valid UTF-8 in "),
            "{err}"
        );
    }

    #[test]
    fn listing_command_sends_roots_on_stdin() {
        let command = listing_command(&["/nix/store/a".into(), "/nix/store/b c".into()]);
        assert_eq!(command.program, "sh");
        assert_eq!(command.args[1], LISTING.text());
        assert_eq!(command.stdin, b"/nix/store/a\n/nix/store/b c\n");
    }

    /// The script against this machine's `find`, `ls` and `cksum`: GNU in the Nix build, BSD on
    /// a Mac.
    #[test]
    fn listing_script_runs_with_the_parser() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let r = |rel: &str| root.join(rel);
        std::fs::create_dir_all(r("sub dir")).unwrap();
        std::fs::write(r("sub dir/a -> b"), "x\n").unwrap();
        symlink("/x -> y", r("l -> k")).unwrap();
        symlink(
            "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-moduli",
            r("moduli"),
        )
        .unwrap();

        let run = |roots: &[String]| {
            let command = listing_command(roots);
            let mut child = std::process::Command::new(&command.program)
                .args(&command.args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&command.stdin)
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            output.stdout
        };

        let mut walk = Walk::reference(root.to_str().unwrap());
        let roots = walk.next_roots();
        walk.add_listing(&run(&roots)).unwrap();
        // The store target does not exist here: the script skips it, and the link dangles.
        let roots = walk.next_roots();
        assert_eq!(
            roots,
            ["/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-moduli"]
        );
        walk.add_listing(&run(&roots)).unwrap();
        assert!(walk.next_roots().is_empty());
        let tree = walk.tree();
        let types: Vec<(&str, FileType)> = tree
            .iter()
            .map(|(p, e)| (p.as_str(), e.file_type()))
            .collect();
        assert_eq!(
            types,
            [
                ("", FileType::Directory),
                ("l -> k", FileType::ExternalLink),
                ("moduli", FileType::DanglingLink),
                ("sub dir", FileType::Directory),
                ("sub dir/a -> b", FileType::File),
            ]
        );
        assert_eq!(tree["l -> k"], Entry::ExternalLink("/x -> y".into()));

        std::fs::write(r("sub dir/new\nline"), "x\n").unwrap();
        let mut walk = Walk::reference(root.to_str().unwrap());
        let roots = walk.next_roots();
        let err = walk.add_listing(&run(&roots)).unwrap_err();
        assert!(
            err.to_string().starts_with("file name with a newline in "),
            "{err}"
        );
    }

    fn ls(path: &str, target: &str) -> String {
        format!(
            "lrwxrwxrwx 1 0 0 {} Jan  1  1970 {path} -> {target}\n",
            target.len()
        )
    }

    fn ck(path: &str, cksum: u32, size: u64) -> String {
        format!("{cksum} {size} {path}\n")
    }

    fn file(path: &str, cksum: u32, size: u64) -> Entry {
        Entry::File {
            path: path.into(),
            cksum,
            size,
        }
    }

    #[test]
    fn walks_levels_until_every_store_link_is_listed() {
        let t = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-nixos-system-web-a-25.11";
        let e = "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-etc/etc";
        let moduli = "/nix/store/h4qa0r1sxv7lgm2c9dyk5nfzbw8p6ij3-openssh-9.8p1/etc/ssh/moduli";
        let dir = "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-dir";
        let deep = "/nix/store/zyxwvsrqpnmlkjihgfdcba9876543210-deep";
        let gone = "/nix/store/00000000000000000000000000000000-gone/x";

        let mut walk = Walk::toplevel(t, ClosureKind::Nixos);
        assert_eq!(walk.next_roots(), [t]);
        let level1 = [
            format!("{t}\n{t}/bin\n{t}/specialisation\n"),
            ls(&format!("{t}/etc"), e),
            ls(
                &format!("{t}/sw"),
                "/nix/store/0000000000000000000000000000000a-system-path",
            ),
            ls(
                &format!("{t}/kernel"),
                "/nix/store/0000000000000000000000000000000b-linux/bzImage",
            ),
            ls(
                &format!("{t}/specialisation/debug"),
                "/nix/store/0000000000000000000000000000000c-x",
            ),
            ck(&format!("{t}/activate"), 1, 10),
            ck(&format!("{t}/nixos-version"), 2, 20),
            ck(&format!("{t}/kernel-params"), 3, 30),
            ck(&format!("{t}/bin/switch-to-configuration"), 4, 40),
        ]
        .concat();
        walk.add_listing(level1.as_bytes()).unwrap();
        assert_eq!(walk.next_roots(), [e]);

        let level2 = [
            format!("{e}\n{e}/ssh\n{e}/profiles\n{e}/profiles/per-user\n"),
            ls(
                &format!("{e}/profiles/per-user/alice"),
                "/nix/store/0000000000000000000000000000000d-env",
            ),
            ck(&format!("{e}/hosts"), 5, 50),
            ls(&format!("{e}/ssh/moduli"), moduli),
            ls(&format!("{e}/ssh/moduli-again"), moduli),
            ls(&format!("{e}/mtab"), "/proc/mounts"),
            ls(&format!("{e}/dir"), &format!("{dir}/")),
            ls(&format!("{e}/missing"), gone),
            ls(&format!("{e}/hosts-alias"), "./hosts"),
            // Missing inside a listed directory: dangling without another round trip.
            ls(&format!("{e}/nope-alias"), "./nope"),
            // Below a listed link: only a listing through the link can tell.
            ls(&format!("{e}/through"), &format!("{e}/ssh/moduli/x")),
            ls(&format!("{e}/loop"), "../etc"),
        ]
        .concat();
        walk.add_listing(level2.as_bytes()).unwrap();
        let through = format!("{e}/ssh/moduli/x");
        assert_eq!(walk.next_roots(), [gone, dir, &through, moduli]);

        // Roots may be listed in chunks; `gone` does not exist and is skipped by the script.
        walk.add_listing(ck(moduli, 6, 60).as_bytes()).unwrap();
        let level3 = [
            format!("{dir}\n"),
            ck(&format!("{dir}/a"), 7, 70),
            ls(&format!("{dir}/b"), deep),
        ];
        walk.add_listing(level3.concat().as_bytes()).unwrap();
        assert_eq!(walk.next_roots(), [deep]);

        walk.add_listing(ck(deep, 8, 80).as_bytes()).unwrap();
        assert!(walk.next_roots().is_empty());

        let expected = Tree::from([
            ("activate".into(), file(&format!("{t}/activate"), 1, 10)),
            (
                "bin/switch-to-configuration".into(),
                file(&format!("{t}/bin/switch-to-configuration"), 4, 40),
            ),
            ("etc".into(), Entry::Directory),
            ("etc/dir".into(), Entry::Directory),
            ("etc/dir/a".into(), file(&format!("{dir}/a"), 7, 70)),
            ("etc/dir/b".into(), file(deep, 8, 80)),
            ("etc/hosts".into(), file(&format!("{e}/hosts"), 5, 50)),
            ("etc/hosts-alias".into(), file(&format!("{e}/hosts"), 5, 50)),
            ("etc/loop".into(), Entry::ExternalLink("../etc".into())),
            ("etc/missing".into(), Entry::DanglingLink(gone.into())),
            (
                "etc/mtab".into(),
                Entry::ExternalLink("/proc/mounts".into()),
            ),
            (
                "etc/nope-alias".into(),
                Entry::DanglingLink("./nope".into()),
            ),
            ("etc/profiles".into(), Entry::Directory),
            ("etc/ssh".into(), Entry::Directory),
            ("etc/ssh/moduli".into(), file(moduli, 6, 60)),
            ("etc/ssh/moduli-again".into(), file(moduli, 6, 60)),
            ("etc/through".into(), Entry::DanglingLink(through.clone())),
        ]);
        assert_eq!(walk.tree(), expected);
    }

    #[test]
    fn followed_trees_below_real_directories() {
        let t = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-darwin-system-26.11";
        let agents = "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-launchd/Library/LaunchAgents";
        let mut walk = Walk::toplevel(t, ClosureKind::NixDarwin);
        walk.next_roots();
        let level1 = [
            format!("{t}\n{t}/Library\n"),
            ls(&format!("{t}/Library/LaunchAgents"), agents),
            ls(
                &format!("{t}/Library/Fonts"),
                "/nix/store/0000000000000000000000000000000a-fonts",
            ),
            ck(&format!("{t}/darwin-version"), 1, 1),
            ck(&format!("{t}/darwin-version.json"), 1, 1),
        ];
        walk.add_listing(level1.concat().as_bytes()).unwrap();
        assert_eq!(walk.next_roots(), [agents]);
        walk.add_listing(
            format!("{agents}\n{}", ck(&format!("{agents}/a.plist"), 2, 2)).as_bytes(),
        )
        .unwrap();
        assert!(walk.next_roots().is_empty());
        assert_eq!(
            walk.tree(),
            Tree::from([
                ("Library/LaunchAgents".into(), Entry::Directory),
                (
                    "Library/LaunchAgents/a.plist".into(),
                    file(&format!("{agents}/a.plist"), 2, 2)
                ),
            ])
        );
    }

    #[test]
    fn missing_root_is_an_empty_tree() {
        let mut walk = Walk::reference("/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-gone/bin");
        walk.next_roots();
        walk.add_listing(b"").unwrap();
        assert!(walk.next_roots().is_empty());
        assert!(walk.tree().is_empty());
    }

    #[test]
    fn reference_walk_of_a_file() {
        let f = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-app.conf";
        let mut walk = Walk::reference(f);
        assert_eq!(walk.next_roots(), [f]);
        walk.add_listing(ck(f, 1, 2).as_bytes()).unwrap();
        assert!(walk.next_roots().is_empty());
        assert_eq!(walk.tree(), Tree::from([(String::new(), file(f, 1, 2))]));
    }
}
