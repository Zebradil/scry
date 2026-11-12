//! Shared changes (spec 8.2): changes found identically in two or more status-`done`
//! configurations, and the `(also in N)` lookup of the per-configuration views.
//!
//! Identity per item:
//!
//! - package change: kind, pname, old and new versions; size deltas are not compared;
//! - file change of class `real`: the whole change with sizes dropped and hunk lines compared
//!   by text after `files::churn::normalize_hashes`, so path, `via`, status and hunks, plus the
//!   link targets and types that `[L]` and `[T]` rows have instead of hunks;
//! - Boot item: one row of the Boot section, its item and old and new names.
//!
//! Churned files, rebuilt packages, the system version and sizes are never shared items.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::files::churn::normalize_hashes;
use crate::report::{
    Boot, BootChange, ChurnClass, Configuration, FileChange, PackageChange, Sections, SharedChange,
    SharedItem, SharedPackageChange, SpecialisationStatus, Status,
};

/// The shared changes of `configurations`: each listed once with every `done` configuration
/// that has it, in name order, the item as the first of them has it. Ordered by section
/// (packages, files, boot), then as the sections order their rows.
pub fn shared(configurations: &[Configuration]) -> Vec<SharedChange> {
    let mut done: Vec<(&str, &Sections)> = configurations
        .iter()
        .filter_map(|c| match &c.status {
            Status::Done(sections) => Some((c.name.as_str(), &**sections)),
            _ => None,
        })
        .collect();
    done.sort_by_key(|&(name, _)| name);

    let mut index: HashMap<String, usize> = HashMap::new();
    let mut found: Vec<SharedChange> = Vec::new();
    for (name, sections) in done {
        for item in items(sections) {
            match index.entry(key(&item)) {
                Entry::Occupied(at) => {
                    let names = &mut found[*at.get()].configurations;
                    if names.last().is_none_or(|last| last != name) {
                        names.push(name.to_owned());
                    }
                }
                Entry::Vacant(at) => {
                    at.insert(found.len());
                    found.push(SharedChange {
                        item,
                        configurations: vec![name.to_owned()],
                    });
                }
            }
        }
    }
    found.retain(|change| change.configurations.len() >= 2);
    // Stable: equal rows (one path, different hunks) keep the order of their first configuration.
    found.sort_by(|a, b| order(&a.item).cmp(&order(&b.item)));
    found
}

/// Answers `(also in N)` for a configuration's rows: `N` is the number of other configurations
/// sharing the row, `None` when it is not shared.
pub struct AlsoIn(HashMap<String, usize>);

impl AlsoIn {
    pub fn new(shared: &[SharedChange]) -> Self {
        Self(
            shared
                .iter()
                .map(|s| (key(&s.item), s.configurations.len().saturating_sub(1)))
                .collect(),
        )
    }

    pub fn package(&self, change: &PackageChange) -> Option<usize> {
        self.get(key(&SharedItem::Packages(package_item(change))))
    }

    pub fn file(&self, change: &FileChange) -> Option<usize> {
        self.get(file_key(change))
    }

    pub fn boot(&self, row: &BootChange) -> Option<usize> {
        self.get(key(&SharedItem::Boot(row.clone())))
    }

    fn get(&self, key: String) -> Option<usize> {
        self.0.get(&key).copied()
    }
}

/// The rows of a Boot section in display order: kernel, initrd, kernel-modules, kernel-params,
/// then added, removed and changed specialisations, each by name.
pub fn boot_rows(boot: &Boot) -> Vec<BootChange> {
    let mut rows: Vec<BootChange> = [
        boot.kernel.clone().map(BootChange::Kernel),
        boot.initrd.clone().map(BootChange::Initrd),
        boot.kernel_modules.clone().map(BootChange::KernelModules),
        boot.kernel_params.clone().map(BootChange::KernelParams),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(s) = &boot.specialisations {
        for (names, status) in [
            (&s.added, SpecialisationStatus::Added),
            (&s.removed, SpecialisationStatus::Removed),
            (&s.changed, SpecialisationStatus::Changed),
        ] {
            rows.extend(names.iter().map(|name| BootChange::Specialisation {
                name: name.clone(),
                status,
            }));
        }
    }
    rows
}

fn items(sections: &Sections) -> Vec<SharedItem> {
    let packages = sections.packages.changes.iter().map(package_item);
    let files = sections
        .files
        .changes
        .iter()
        .filter(|c| c.class == ChurnClass::Real);
    let boot = sections.boot.as_ref().map(boot_rows).unwrap_or_default();
    packages
        .map(SharedItem::Packages)
        .chain(files.cloned().map(SharedItem::Files))
        .chain(boot.into_iter().map(SharedItem::Boot))
        .collect()
}

fn package_item(change: &PackageChange) -> SharedPackageChange {
    SharedPackageChange {
        kind: change.kind,
        pname: change.pname.clone(),
        old_versions: change.old_versions.clone(),
        new_versions: change.new_versions.clone(),
    }
}

/// Equal for items that count as identical.
fn key(item: &SharedItem) -> String {
    match item {
        SharedItem::Files(change) => file_key(change),
        other => serde_json::to_string(other).expect("report items serialise"),
    }
}

fn file_key(change: &FileChange) -> String {
    let mut change = change.clone();
    (change.size_old, change.size_new) = (None, None);
    for line in change.hunks.iter_mut().flat_map(|h| &mut h.lines) {
        line.text = normalize_hashes(&line.text);
        // Derived from the text, and they differ with the hashes.
        line.spans.clear();
        line.hash_only = false;
    }
    serde_json::to_string(&SharedItem::Files(change)).expect("report items serialise")
}

fn order(item: &SharedItem) -> (u8, u8, Vec<&str>) {
    match item {
        SharedItem::Packages(p) => (0, p.kind as u8, vec![p.pname.as_str()]),
        SharedItem::Files(f) => (
            1,
            0,
            std::iter::once(&f.path)
                .chain(&f.via)
                .map(String::as_str)
                .collect(),
        ),
        SharedItem::Boot(row) => match row {
            BootChange::Kernel(_) => (2, 0, vec![]),
            BootChange::Initrd(_) => (2, 1, vec![]),
            BootChange::KernelModules(_) => (2, 2, vec![]),
            BootChange::KernelParams(_) => (2, 3, vec![]),
            BootChange::Specialisation { name, status } => (2, 4 + *status as u8, vec![name]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{
        ClosureKind, FileStatus, FileType, Files, Hunk, Line, LineOp, LinkChange,
        PackageChangeKind, Packages, Rebuilt, Source, Specialisations,
    };

    const H1: &str = "1b9p07z77phvv2hf6gm9f28syp39f1ag";
    const H2: &str = "9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g";

    fn upgrade(pname: &str, old: &str, new: &str, delta: i64) -> PackageChange {
        PackageChange {
            kind: PackageChangeKind::Upgraded,
            pname: pname.into(),
            old_versions: vec![old.into()],
            new_versions: vec![new.into()],
            size_delta: delta,
        }
    }

    fn modified(path: &str, class: ChurnClass, old: &str, new: &str, size: u64) -> FileChange {
        let line = |op, text: &str| Line {
            op,
            text: text.into(),
            spans: vec![],
            hash_only: false,
            no_newline: false,
        };
        FileChange {
            path: path.into(),
            via: vec![],
            status: FileStatus::Modified,
            class,
            type_old: Some(FileType::File),
            type_new: Some(FileType::File),
            binary: false,
            content_fetched: true,
            size_old: Some(size),
            size_new: Some(size + 1),
            link_old: None,
            link_new: None,
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![line(LineOp::Removed, old), line(LineOp::Added, new)],
            }],
        }
    }

    fn done(
        name: &str,
        packages: Vec<PackageChange>,
        files: Vec<FileChange>,
        boot: Option<Boot>,
    ) -> Configuration {
        Configuration {
            name: name.into(),
            source: Source::Colmena,
            kind: ClosureKind::Nixos,
            target: None,
            status: Status::Done(Box::new(Sections {
                packages: Packages {
                    size_old: 1,
                    size_new: 2,
                    changes: packages,
                    rebuilt: Rebuilt {
                        count: 0,
                        size_delta: 0,
                        items: vec![],
                    },
                },
                files: Files { changes: files },
                boot,
            })),
            old: None,
            new: None,
        }
    }

    fn names(change: &SharedChange) -> Vec<&str> {
        change.configurations.iter().map(String::as_str).collect()
    }

    #[test]
    fn packages_ignore_size_and_need_equal_versions() {
        let configurations = [
            done(
                "web-c",
                vec![upgrade("openssl", "3.0.13", "3.0.14", 7)],
                vec![],
                None,
            ),
            done(
                "web-a",
                vec![upgrade("openssl", "3.0.13", "3.0.14", 100)],
                vec![],
                None,
            ),
            done(
                "web-b",
                vec![upgrade("openssl", "3.0.12", "3.0.14", 100)],
                vec![],
                None,
            ),
        ];
        let shared = shared(&configurations);
        assert_eq!(shared.len(), 1);
        assert_eq!(names(&shared[0]), ["web-a", "web-c"]);
        let SharedItem::Packages(item) = &shared[0].item else {
            panic!("not a package")
        };
        assert_eq!(item.old_versions, ["3.0.13"]);

        let also = AlsoIn::new(&shared);
        assert_eq!(
            also.package(&upgrade("openssl", "3.0.13", "3.0.14", 1)),
            Some(1)
        );
        assert_eq!(
            also.package(&upgrade("openssl", "3.0.12", "3.0.14", 1)),
            None
        );
    }

    #[test]
    fn real_files_compare_hunks_without_hashes_and_sizes() {
        let a = modified(
            "etc/app.conf",
            ChurnClass::Real,
            &format!("x=/nix/store/{H1}-app-1.0 a"),
            &format!("x=/nix/store/{H2}-app-1.0 b"),
            10,
        );
        let mut b = a.clone();
        b.hunks[0].lines[0].text = format!("x=/nix/store/{H2}-app-1.0 a");
        b.size_old = Some(99);
        let other_text = modified("etc/app.conf", ChurnClass::Real, "a", "c", 10);
        let churned = modified("etc/churn.conf", ChurnClass::HashOnly, H1, H2, 10);
        let configurations = [
            done("web-a", vec![], vec![a.clone(), churned.clone()], None),
            done("web-b", vec![], vec![b, churned.clone()], None),
            done("web-c", vec![], vec![other_text.clone()], None),
        ];
        let shared = shared(&configurations);
        assert_eq!(shared.len(), 1, "{shared:#?}");
        assert_eq!(names(&shared[0]), ["web-a", "web-b"]);
        assert_eq!(shared[0].item, SharedItem::Files(a.clone()));

        let also = AlsoIn::new(&shared);
        assert_eq!(also.file(&a), Some(1));
        assert_eq!(also.file(&other_text), None);
        assert_eq!(also.file(&churned), None);
    }

    #[test]
    fn link_changes_compare_targets() {
        let link = |new: &str| FileChange {
            status: FileStatus::LinkChanged,
            type_old: Some(FileType::ExternalLink),
            type_new: Some(FileType::ExternalLink),
            link_old: Some("/proc/mounts".into()),
            link_new: Some(new.into()),
            content_fetched: false,
            size_old: None,
            size_new: None,
            hunks: vec![],
            ..modified("etc/mtab", ChurnClass::Real, "", "", 0)
        };
        let configurations = [
            done("web-a", vec![], vec![link("/proc/self/mounts")], None),
            done("web-b", vec![], vec![link("/dev/null")], None),
        ];
        assert!(shared(&configurations).is_empty());
    }

    #[test]
    fn boot_rows_by_item_and_names() {
        let boot = |new: &str, debug: bool| Boot {
            kernel: Some(LinkChange {
                old: Some("linux-6.12.40".into()),
                new: Some(new.into()),
                rebuilt: false,
            }),
            initrd: None,
            kernel_modules: None,
            kernel_params: None,
            specialisations: Some(Specialisations {
                added: if debug { vec!["debug".into()] } else { vec![] },
                removed: vec![],
                changed: vec![],
            }),
        };
        let configurations = [
            done("web-a", vec![], vec![], Some(boot("linux-6.12.48", true))),
            done("web-b", vec![], vec![], Some(boot("linux-6.12.48", false))),
            done("web-c", vec![], vec![], Some(boot("linux-6.12.47", true))),
        ];
        let shared = shared(&configurations);
        let rows: Vec<_> = shared.iter().map(|s| (&s.item, names(s))).collect();
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert!(matches!(rows[0].0, SharedItem::Boot(BootChange::Kernel(_))));
        assert_eq!(rows[0].1, ["web-a", "web-b"]);
        assert!(matches!(
            rows[1].0,
            SharedItem::Boot(BootChange::Specialisation { .. })
        ));
        assert_eq!(rows[1].1, ["web-a", "web-c"]);
    }

    #[test]
    fn only_done_configurations_count() {
        let mut failed = done("web-b", vec![upgrade("openssl", "1", "2", 0)], vec![], None);
        failed.status = Status::Error {
            error: "boom".into(),
        };
        let configurations = [
            done("web-a", vec![upgrade("openssl", "1", "2", 0)], vec![], None),
            failed,
        ];
        assert!(shared(&configurations).is_empty());
    }

    #[test]
    fn ordered_by_section_then_row() {
        let both = || {
            done(
                "x",
                vec![
                    upgrade("zlib", "1", "2", 0),
                    PackageChange {
                        kind: PackageChangeKind::Added,
                        ..upgrade("aaa", "", "1", 0)
                    },
                ],
                vec![modified("etc/b", ChurnClass::Real, "1", "2", 1)],
                None,
            )
        };
        let (mut a, mut b) = (both(), both());
        (a.name, b.name) = ("web-a".into(), "web-b".into());
        let shared = shared(&[a, b]);
        let order: Vec<_> = shared
            .iter()
            .map(|s| match &s.item {
                SharedItem::Packages(p) => p.pname.clone(),
                SharedItem::Files(f) => f.path.clone(),
                SharedItem::Boot(_) => "boot".into(),
            })
            .collect();
        assert_eq!(order, ["zlib", "aaa", "etc/b"]);
        assert_eq!(AlsoIn::new(&shared).0.values().max(), Some(&1));
    }
}
