//! Package comparison of two closures (spec 7.2), and the small rebuilds of spec 7.4.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::report::{PackageChange, PackageChangeKind, Packages, Rebuilt, RebuiltPackage};
use crate::store::name::package_id;
use crate::store::version::version_order;

/// A small rebuild's path is at most this big, on each side (spec 12).
pub const SMALL_REBUILD_MAX_BYTES: u64 = 1024 * 1024;

/// The Packages section, plus what the fetch needs beyond the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub packages: Packages,
    /// By pname.
    pub small_rebuilds: Vec<SmallRebuild>,
}

/// A rebuilt (or resized) package with exactly one path per side, each at most
/// [`SMALL_REBUILD_MAX_BYTES`] `narSize`: its `bin/` is compared by content (spec 7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmallRebuild {
    pub pname: String,
    /// Full store paths.
    pub old: String,
    pub new: String,
}

/// Compares two closures, each full store path to `narSize` as [`crate::store::pathinfo`]
/// parses it. `toplevels` holds both toplevels and their specialisations' toplevels: they count
/// towards the closure sizes but are never package rows (spec 7.1).
pub fn compare(
    old: &BTreeMap<String, u64>,
    new: &BTreeMap<String, u64>,
    toplevels: &[&str],
) -> Comparison {
    let (old_packages, new_packages) = (group(old, toplevels), group(new, toplevels));
    let pnames: BTreeSet<&str> = old_packages
        .keys()
        .chain(new_packages.keys())
        .copied()
        .collect();
    let absent = Package::default();
    let mut changes = Vec::new();
    let mut rebuilt = Rebuilt {
        count: 0,
        size_delta: 0,
        items: Vec::new(),
    };
    let mut small_rebuilds = Vec::new();

    for pname in pnames {
        let (o, n) = (old_packages.get(pname), new_packages.get(pname));
        let (old_side, new_side) = (o.unwrap_or(&absent), n.unwrap_or(&absent));
        let size_delta = new_side.size as i64 - old_side.size as i64;
        let kind = match (o, n) {
            (None, _) => PackageChangeKind::Added,
            (_, None) => PackageChangeKind::Removed,
            (Some(o), Some(n)) if o.versions == n.versions => {
                if o.paths.keys().eq(n.paths.keys()) {
                    continue;
                }
                if let (Some((old_path, old_size)), Some((new_path, new_size))) =
                    (single(&o.paths), single(&n.paths))
                    && old_size <= SMALL_REBUILD_MAX_BYTES
                    && new_size <= SMALL_REBUILD_MAX_BYTES
                {
                    small_rebuilds.push(SmallRebuild {
                        pname: pname.to_owned(),
                        old: old_path.to_owned(),
                        new: new_path.to_owned(),
                    });
                }
                // Store hashes have one length, so a rebuild that only swapped hashes keeps its
                // narSize to the byte; a size change is new content or a reference whose name
                // changed length (ADR 0032).
                if size_delta == 0 {
                    rebuilt.count += 1;
                    rebuilt.items.push(RebuiltPackage {
                        pname: pname.to_owned(),
                        versions: n.versions(),
                        size_delta: 0,
                    });
                    continue;
                }
                PackageChangeKind::Resized
            }
            (Some(o), Some(n)) if o.versions.len() == 1 && n.versions.len() == 1 => {
                match version_order(o.versions.first().unwrap(), n.versions.first().unwrap()) {
                    Some(Ordering::Less) => PackageChangeKind::Upgraded,
                    Some(Ordering::Greater) => PackageChangeKind::Downgraded,
                    _ => PackageChangeKind::Changed,
                }
            }
            (Some(_), Some(_)) => PackageChangeKind::Changed,
        };
        changes.push(PackageChange {
            kind,
            pname: pname.to_owned(),
            old_versions: old_side.versions(),
            new_versions: new_side.versions(),
            size_delta,
        });
    }
    // Stable, so pnames stay in order within a kind.
    changes.sort_by_key(|change| change.kind);

    Comparison {
        packages: Packages {
            size_old: old.values().sum(),
            size_new: new.values().sum(),
            changes,
            rebuilt,
        },
        small_rebuilds,
    }
}

/// All paths of one pname on one side.
#[derive(Default)]
struct Package<'a> {
    versions: BTreeSet<&'a str>,
    paths: BTreeMap<&'a str, u64>,
    size: u64,
}

impl Package<'_> {
    fn versions(&self) -> Vec<String> {
        self.versions.iter().map(|&v| v.to_owned()).collect()
    }
}

fn group<'a>(
    closure: &'a BTreeMap<String, u64>,
    toplevels: &[&str],
) -> BTreeMap<&'a str, Package<'a>> {
    let mut packages = BTreeMap::<&str, Package>::new();
    for (path, &size) in closure {
        if toplevels.contains(&path.as_str()) {
            continue;
        }
        let Some((pname, version)) = package_id(path) else {
            continue;
        };
        let package = packages.entry(pname).or_default();
        package.versions.insert(version);
        package.paths.insert(path, size);
        package.size += size;
    }
    packages
}

/// The only path of a side with exactly one.
fn single<'a>(paths: &BTreeMap<&'a str, u64>) -> Option<(&'a str, u64)> {
    let mut entries = paths.iter();
    match (entries.next(), entries.next()) {
        (Some((&key, &value)), None) => Some((key, value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PackageChangeKind::*;

    const MIB: u64 = 1024 * 1024;

    /// A store path with a placeholder hash: `hash` repeated, from Nix's base32 alphabet.
    fn path(hash: char, name: &str) -> String {
        format!("/nix/store/{}-{name}", hash.to_string().repeat(32))
    }

    fn closure(entries: &[(char, &str, u64)]) -> BTreeMap<String, u64> {
        entries
            .iter()
            .map(|&(hash, name, size)| (path(hash, name), size))
            .collect()
    }

    fn change(
        kind: PackageChangeKind,
        pname: &str,
        old: &[&str],
        new: &[&str],
        delta: i64,
    ) -> PackageChange {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        PackageChange {
            kind,
            pname: pname.into(),
            old_versions: strings(old),
            new_versions: strings(new),
            size_delta: delta,
        }
    }

    #[test]
    fn every_kind_in_order() {
        let old = closure(&[
            ('0', "nixos-system-web-1-25.05.1", 1000),
            ('0', "nixos-system-web-1-debug-25.05.1", 1000),
            ('0', "openssl-3.0.13", 4000),
            ('0', "openssl-3.0.13-bin", 1000),
            ('0', "foo-2.0", 100),
            ('0', "my-tool-1.2.0-1a2b3c4", 100),
            ('0', "lib-1.0", 100),
            ('0', "lib-2.0", 100),
            ('0', "same-1.0", 100),
            ('0', "old-lib-0.9", 300 * 1024),
            ('0', "linux-firmware-20250801", 100 * MIB),
            ('0', "big-1", 100 * MIB),
            ('0', "bash-5.2p37", 1000),
            ('0', "tiny-1", 1000),
            ('0', "unchanged-1", 1000),
            ('0', "hello-2.12.drv", 1000),
        ]);
        let new = closure(&[
            ('1', "nixos-system-web-1-25.11.1", 1100),
            ('1', "nixos-system-web-1-debug-25.11.1", 1100),
            ('1', "openssl-3.0.14", 4000),
            ('1', "openssl-3.0.14-bin", 1100),
            ('1', "foo-1.9", 100),
            ('1', "my-tool-1.2.0-9f8e7d6", 4196),
            ('1', "lib-2.0", 100),
            ('1', "lib-3.0", 100),
            ('1', "same-1.00", 100),
            ('1', "new-agent-1.4.0", 2 * MIB),
            ('1', "linux-firmware-20250801", 112 * MIB),
            ('1', "big-1", 105 * MIB),
            ('1', "bash-5.2p37", 1000),
            ('1', "tiny-1", 2000),
            ('0', "unchanged-1", 1000),
            ('1', "hello-2.12.drv", 1000),
        ]);
        let toplevels = [
            path('0', "nixos-system-web-1-25.05.1"),
            path('0', "nixos-system-web-1-debug-25.05.1"),
            path('1', "nixos-system-web-1-25.11.1"),
            path('1', "nixos-system-web-1-debug-25.11.1"),
        ];
        let toplevels: Vec<&str> = toplevels.iter().map(String::as_str).collect();

        let comparison = compare(&old, &new, &toplevels);

        let packages = &comparison.packages;
        assert_eq!(packages.size_old, old.values().sum::<u64>());
        assert_eq!(packages.size_new, new.values().sum::<u64>());
        assert_eq!(
            packages.changes,
            vec![
                change(Upgraded, "openssl", &["3.0.13"], &["3.0.14"], 100),
                change(Downgraded, "foo", &["2.0"], &["1.9"], 0),
                change(Changed, "lib", &["1.0", "2.0"], &["2.0", "3.0"], 0),
                change(
                    Changed,
                    "my-tool",
                    &["1.2.0-1a2b3c4"],
                    &["1.2.0-9f8e7d6"],
                    4096
                ),
                change(Changed, "same", &["1.0"], &["1.00"], 0),
                change(Added, "new-agent", &[], &["1.4.0"], 2 * MIB as i64),
                change(Removed, "old-lib", &["0.9"], &[], -300 * 1024),
                change(Resized, "big", &["1"], &["1"], 5 * MIB as i64),
                change(
                    Resized,
                    "linux-firmware",
                    &["20250801"],
                    &["20250801"],
                    12 * MIB as i64
                ),
                change(Resized, "tiny", &["1"], &["1"], 1000),
            ]
        );
        assert_eq!(
            packages.rebuilt,
            Rebuilt {
                count: 1,
                size_delta: 0,
                items: vec![RebuiltPackage {
                    pname: "bash".into(),
                    versions: vec!["5.2p37".into()],
                    size_delta: 0
                }],
            }
        );
        assert_eq!(
            comparison.small_rebuilds,
            vec![
                SmallRebuild {
                    pname: "bash".into(),
                    old: path('0', "bash-5.2p37"),
                    new: path('1', "bash-5.2p37")
                },
                SmallRebuild {
                    pname: "tiny".into(),
                    old: path('0', "tiny-1"),
                    new: path('1', "tiny-1")
                },
            ]
        );
    }

    #[test]
    fn resized_on_any_size_change() {
        let cases = [
            (10 * MIB, 10 * MIB, None),
            (10 * MIB, 10 * MIB + 8, Some(Resized)),
            (20 * MIB, 18 * MIB, Some(Resized)),
        ];
        for (old_size, new_size, expected) in cases {
            let old = closure(&[('0', "pkg-1", old_size)]);
            let new = closure(&[('1', "pkg-1", new_size)]);
            let packages = compare(&old, &new, &[]).packages;
            assert_eq!(
                packages.changes.first().map(|c| c.kind),
                expected,
                "{old_size} -> {new_size}"
            );
            assert_eq!(packages.rebuilt.count, usize::from(expected.is_none()));
        }
    }

    #[test]
    fn small_rebuild_needs_one_small_path_per_side() {
        let cases = [
            (vec![('0', "s-1", MIB)], vec![('1', "s-1", MIB)], true),
            (vec![('0', "s-1", MIB)], vec![('1', "s-1", MIB + 1)], false),
            (vec![('0', "s-1", MIB + 1)], vec![('1', "s-1", MIB)], false),
            (
                vec![('0', "s-1", 10), ('0', "s-1-man", 10)],
                vec![('1', "s-1", 10)],
                false,
            ),
            (
                vec![('0', "s-1", 10), ('0', "s-1-man", 10)],
                vec![('1', "s-1", 10), ('1', "s-1-man", 10)],
                false,
            ),
            // Only the man page was rebuilt: still two paths per side.
            (
                vec![('0', "s-1", 10), ('0', "s-1-man", 10)],
                vec![('0', "s-1", 10), ('1', "s-1-man", 10)],
                false,
            ),
        ];
        for (old, new, expected) in cases {
            let comparison = compare(&closure(&old), &closure(&new), &[]);
            assert_eq!(
                !comparison.small_rebuilds.is_empty(),
                expected,
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn same_version_set_with_several_versions_is_rebuilt() {
        let old = closure(&[('0', "lib-1.0", 10), ('0', "lib-2.0", 10)]);
        let new = closure(&[('0', "lib-1.0", 10), ('1', "lib-2.0", 10)]);
        let packages = compare(&old, &new, &[]).packages;
        assert!(packages.changes.is_empty());
        assert_eq!(packages.rebuilt.items[0].versions, ["1.0", "2.0"]);
    }

    #[test]
    fn identical_closures_have_no_changes() {
        let old = closure(&[('0', "bash-5.2p37", 10), ('0', "hello", 10)]);
        let packages = compare(&old, &old, &[]).packages;
        assert_eq!(
            packages,
            Packages {
                size_old: 20,
                size_new: 20,
                changes: vec![],
                rebuilt: Rebuilt {
                    count: 0,
                    size_delta: 0,
                    items: vec![]
                }
            }
        );
    }
}
