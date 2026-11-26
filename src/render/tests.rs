//! Snapshot tests of text and JSON output on hand-built reports.

use std::collections::HashSet;
use std::ffi::OsStr;

use insta::assert_snapshot;

use super::text::{self, Options};
use super::view::{self, FILE_PATH_SPAN, Kind, Node, PackageGroup};
use super::{color_enabled, json};
use crate::cli::{Color, WordHighlight};
use crate::report::{
    Base, Boot, ChurnClass, ClosureKind, Configuration, FileChange, FileStatus, FileType, Files,
    Head, Hunk, Line, LineOp, LinkChange, Mode, PackageChange, PackageChangeKind, Packages,
    ParamsChange, Rebuilt, RebuiltPackage, Report, SCHEMA_VERSION, Sections, Side, Source,
    Specialisations, Status,
};
use crate::shared;

const H: [&str; 4] = [
    "1b9p07z77phvv2hf6gm9f28syp39f1ag",
    "9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g",
    "h4qa0r1sxv7lgm2c9dyk5nfzbw8p6ij3",
    "0c2xa4r6mpqv8bdn5fhy3wz7gil9ks1j",
];
const MIB: u64 = 1024 * 1024;

fn side(hash: usize, name: &str, version: Option<&str>, location: &str) -> Side {
    Side {
        path: format!("/nix/store/{}-{name}", H[hash]),
        system_version: version.map(Into::into),
        location: location.into(),
    }
}

fn pkg(
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

fn packages(
    size_old: u64,
    size_new: u64,
    changes: Vec<PackageChange>,
    rebuilt: Rebuilt,
) -> Packages {
    Packages {
        size_old,
        size_new,
        changes,
        rebuilt,
    }
}

fn no_rebuilt() -> Rebuilt {
    Rebuilt {
        count: 0,
        size_delta: 0,
        items: vec![],
    }
}

fn line(op: char, text: &str) -> Line {
    Line {
        op: match op {
            '-' => LineOp::Removed,
            '+' => LineOp::Added,
            _ => LineOp::Context,
        },
        text: text.into(),
        spans: vec![],
        hash_only: false,
        no_newline: false,
    }
}

fn marked(op: char, text: &str, spans: &[(usize, usize)]) -> Line {
    Line {
        spans: spans.to_vec(),
        ..line(op, text)
    }
}

fn hunk(old: (usize, usize), new: (usize, usize), lines: Vec<Line>) -> Hunk {
    Hunk {
        old_start: old.0,
        old_lines: old.1,
        new_start: new.0,
        new_lines: new.1,
        lines,
    }
}

/// A text change of `status` with both sides' types and sizes filled as that status has them.
fn file(path: &str, status: FileStatus) -> FileChange {
    let (old, new) = match status {
        FileStatus::Added => (None, Some(FileType::File)),
        FileStatus::Removed => (Some(FileType::File), None),
        _ => (Some(FileType::File), Some(FileType::File)),
    };
    FileChange {
        path: path.into(),
        via: vec![],
        status,
        class: ChurnClass::Real,
        type_old: old,
        type_new: new,
        binary: false,
        content_fetched: true,
        size_old: old.map(|_| 812),
        size_new: new.map(|_| 829),
        link_old: None,
        link_new: None,
        hunks: vec![],
    }
}

fn churned(path: &str, class: ChurnClass, hunks: Vec<Hunk>) -> FileChange {
    FileChange {
        class,
        hunks,
        ..file(path, FileStatus::Modified)
    }
}

fn done(
    name: &str,
    kind: ClosureKind,
    sides: (Side, Side),
    packages: Packages,
    files: Vec<FileChange>,
    boot: Option<Boot>,
) -> Configuration {
    Configuration {
        name: name.into(),
        source: Source::Colmena,
        kind,
        target: None,
        status: Status::Done(Box::new(Sections {
            packages,
            files: Files { changes: files },
            boot,
        })),
        old: Some(sides.0),
        new: Some(sides.1),
    }
}

fn failed(name: &str, target: Option<&str>, status: Status) -> Configuration {
    Configuration {
        name: name.into(),
        source: Source::Colmena,
        kind: ClosureKind::Nixos,
        target: target.map(Into::into),
        status,
        old: None,
        new: None,
    }
}

fn nixos_sides(name: &str, location: &str) -> (Side, Side) {
    (
        side(
            0,
            &format!("nixos-system-{name}-25.05.20250801.6027c30"),
            Some("25.05.20250801.6027c30"),
            location,
        ),
        side(
            1,
            &format!("nixos-system-{name}-25.11.20251001.9a8b7c6"),
            Some("25.11.20251001.9a8b7c6"),
            "local",
        ),
    )
}

fn report(mode: Mode, configurations: Vec<Configuration>) -> Report {
    Report {
        schema_version: SCHEMA_VERSION,
        tool_version: "1.0.0".into(),
        mode,
        shared: shared::shared(&configurations),
        configurations,
    }
}

fn render(report: &Report, color: bool, expand_churn: bool) -> String {
    let mut out = Vec::new();
    let options = Options {
        color,
        expand_churn,
        ..Options::default()
    };
    text::write(report, options, &mut out).unwrap();
    String::from_utf8(out).unwrap().replace('\x1b', "\\e")
}

fn nix_conf() -> FileChange {
    FileChange {
        hunks: vec![hunk(
            (11, 3),
            (11, 3),
            vec![
                line(' ', "substituters = https://cache.example"),
                line('-', "trusted-public-keys = cache-a:AAAA"),
                marked(
                    '+',
                    "trusted-public-keys = cache-a:AAAA cache-b:BBBB",
                    &[(34, 47)],
                ),
                line(' ', "max-jobs = auto"),
            ],
        )],
        ..file("etc/nix/nix.conf", FileStatus::Modified)
    }
}

fn openssl(delta: i64) -> PackageChange {
    pkg(
        PackageChangeKind::Upgraded,
        "openssl",
        &["3.0.13"],
        &["3.0.14"],
        delta,
    )
}

fn link(old: Option<&str>, new: Option<&str>, rebuilt: bool) -> Option<LinkChange> {
    Some(LinkChange {
        old: old.map(Into::into),
        new: new.map(Into::into),
        rebuilt,
    })
}

/// Spec 8.3's normative example, the elided `web-c` filled in.
fn normative() -> Report {
    let old_size = 1_341_103_638;
    let web_a_packages = packages(
        old_size,
        old_size + 42_949_673,
        vec![
            openssl(100),
            pkg(
                PackageChangeKind::Upgraded,
                "systemd",
                &["257.5"],
                &["258.1"],
                1_258_291,
            ),
            pkg(
                PackageChangeKind::Changed,
                "my-tool",
                &["1.2.0-1a2b3c4"],
                &["1.2.0-9f8e7d6"],
                4096,
            ),
            pkg(
                PackageChangeKind::Added,
                "new-agent",
                &[],
                &["1.4.0"],
                2 * MIB as i64,
            ),
            pkg(
                PackageChangeKind::Removed,
                "old-lib",
                &["0.9"],
                &[],
                -300 * 1024,
            ),
            pkg(
                PackageChangeKind::Resized,
                "linux-firmware",
                &["20250801"],
                &["20250801"],
                12 * MIB as i64,
            ),
        ],
        Rebuilt {
            count: 412,
            size_delta: 0,
            items: vec![RebuiltPackage {
                pname: "bash".into(),
                versions: vec!["5.2p37".into()],
                size_delta: 0,
            }],
        },
    );
    let mut web_a_files = vec![
        FileChange {
            type_old: Some(FileType::ExternalLink),
            type_new: Some(FileType::ExternalLink),
            link_old: Some("/proc/mounts".into()),
            link_new: Some("/proc/self/mounts".into()),
            content_fetched: false,
            size_old: None,
            size_new: None,
            ..file("etc/mtab", FileStatus::LinkChanged)
        },
        nix_conf(),
        FileChange {
            hunks: vec![hunk(
                (0, 0),
                (1, 1),
                vec![line('+', "ssh-ed25519 AAAA… deploy@example")],
            )],
            ..file("etc/ssh/authorized_keys.d/deploy", FileStatus::Added)
        },
        FileChange {
            via: vec!["app-start/bin/app-start".into()],
            hunks: vec![hunk(
                (1, 4),
                (1, 4),
                vec![
                    line(' ', "#!/bin/sh"),
                    line(' ', "set -eu"),
                    marked('-', "exec app --workers 4", &[(19, 20)]),
                    marked('+', "exec app --workers 8", &[(19, 20)]),
                ],
            )],
            ..file("etc/systemd/system/app.service", FileStatus::Modified)
        },
        FileChange {
            type_new: Some(FileType::ExternalLink),
            link_new: Some("/dev/null".into()),
            content_fetched: false,
            size_new: None,
            ..file(
                "etc/systemd/system/debug-shell.service",
                FileStatus::TypeChanged,
            )
        },
        FileChange {
            content_fetched: false,
            size_old: Some(14_155_776),
            size_new: Some(14_260_634),
            ..file("etc/udev/hwdb.bin", FileStatus::Modified)
        },
    ];
    for i in 0..57 {
        let path = format!("etc/static/hash-{i:02}");
        web_a_files.push(churned(&path, ChurnClass::HashOnly, vec![]));
    }
    for i in 0..3 {
        let path = format!("etc/static/version-{i}");
        web_a_files.push(churned(&path, ChurnClass::VersionOnly, vec![]));
    }
    web_a_files.sort_by(|a, b| (&a.path, &a.via).cmp(&(&b.path, &b.via)));
    let web_a_boot = Boot {
        kernel: link(Some("linux-6.12.40"), Some("linux-6.12.48"), false),
        initrd: link(
            Some("initrd-linux-6.12.40"),
            Some("initrd-linux-6.12.48"),
            false,
        ),
        kernel_modules: link(
            Some("linux-6.12.48-modules"),
            Some("linux-6.12.48-modules"),
            true,
        ),
        kernel_params: Some(ParamsChange {
            old: "quiet loglevel=4".into(),
            new: "quiet loglevel=4 mitigations=auto".into(),
        }),
        specialisations: Some(Specialisations {
            added: vec!["debug".into()],
            removed: vec![],
            changed: vec![],
        }),
    };
    let unchanged_boot = Boot {
        kernel: None,
        initrd: None,
        kernel_modules: None,
        kernel_params: None,
        specialisations: None,
    };
    let web_d = side(
        2,
        "nixos-system-web-d-25.11.20251001.9a8b7c6",
        Some("25.11.20251001.9a8b7c6"),
        "root@web-d",
    );
    report(
        Mode::Deployed,
        vec![
            done(
                "web-a",
                ClosureKind::Nixos,
                nixos_sides("web-a", "root@web-a"),
                web_a_packages,
                web_a_files,
                Some(web_a_boot),
            ),
            failed(
                "web-b",
                Some("root@web-b"),
                Status::Error {
                    error: "ssh root@web-b: connect to host web-b port 22: Connection refused"
                        .into(),
                },
            ),
            done(
                "web-c",
                ClosureKind::Nixos,
                nixos_sides("web-c", "root@web-c"),
                packages(old_size, old_size + 2048, vec![openssl(120)], no_rebuilt()),
                vec![nix_conf()],
                Some(unchanged_boot),
            ),
            Configuration {
                status: Status::NoChanges,
                kind: ClosureKind::Nixos,
                old: Some(web_d.clone()),
                new: Some(Side {
                    location: "local".into(),
                    ..web_d
                }),
                ..failed("web-d", None, Status::NoChanges)
            },
        ],
    )
}

#[test]
fn normative_example() {
    assert_snapshot!("normative", render(&normative(), false, false));
}

#[test]
fn normative_example_in_colour() {
    assert_snapshot!("normative_colour", render(&normative(), true, false));
}

fn revision() -> Report {
    let darwin = |hash, version: &str| {
        side(
            hash,
            &format!("darwin-system-{version}"),
            Some(version),
            "local",
        )
    };
    let small_packages = packages(
        3 * MIB,
        3 * MIB,
        vec![pkg(
            PackageChangeKind::Downgraded,
            "python3",
            &["3.12.4"],
            &["3.11.9"],
            0,
        )],
        no_rebuilt(),
    );
    let files = vec![
        file("etc/motd", FileStatus::Added),
        FileChange {
            binary: true,
            size_old: Some(2048),
            ..file("etc/old.bin", FileStatus::Removed)
        },
    ];
    let hm = side(3, "home-manager-generation", Some("25.11"), "local");
    let flake = |c| Configuration {
        source: Source::Flake,
        ..c
    };
    report(
        Mode::Revision {
            base: Base {
                reference: "main".into(),
                rev: "6027c30aa1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
            },
            head: Head {
                rev: "9a8b7c6aa1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
                dirty: true,
            },
        },
        [
            done(
                "db-a",
                ClosureKind::NixDarwin,
                (darwin(0, "25.11.a"), darwin(1, "25.11.b")),
                small_packages,
                files,
                None,
            ),
            Configuration {
                kind: ClosureKind::HomeManager,
                new: Some(hm.clone()),
                ..failed("db-b", None, Status::AbsentAtBase)
            },
            Configuration {
                kind: ClosureKind::HomeManager,
                old: Some(hm.clone()),
                new: Some(hm),
                ..failed("db-c", None, Status::NoChanges)
            },
            failed(
                "db-d",
                None,
                Status::Timeout {
                    error: "timeout after 60 s".into(),
                },
            ),
        ]
        .into_iter()
        .map(flake)
        .collect(),
    )
}

#[test]
fn revision_mode() {
    assert_snapshot!("revision", render(&revision(), false, false));
}

#[test]
fn json_output() {
    let report = revision();
    let mut out = Vec::new();
    json::write(&report, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let back: Report = serde_json::from_str(&text).unwrap();
    assert_eq!(back, report);
    assert_snapshot!("revision_json", text);
}

#[test]
fn paths_mode() {
    let hm = |hash| side(hash, "home-manager-generation", Some("25.11"), "root@web-a");
    let files = vec![
        FileChange {
            type_new: Some(FileType::ExternalLink),
            link_new: Some("/proc/mounts".into()),
            size_new: None,
            content_fetched: false,
            ..file("home-files/.mtab", FileStatus::Added)
        },
        FileChange {
            hunks: vec![hunk(
                (1, 1),
                (1, 1),
                vec![
                    Line {
                        no_newline: true,
                        ..line('-', "set -g mouse on")
                    },
                    line('+', "set -g mouse on"),
                ],
            )],
            ..file("home-files/.tmux.conf", FileStatus::Modified)
        },
        FileChange {
            type_new: Some(FileType::DanglingLink),
            link_new: Some(format!("/nix/store/{}-gone", H[2])),
            size_new: None,
            content_fetched: false,
            ..file("home-files/.zshrc", FileStatus::TypeChanged)
        },
    ];
    let configuration = Configuration {
        source: Source::Paths,
        target: Some("root@web-a".into()),
        ..done(
            "root@web-a",
            ClosureKind::HomeManager,
            (hm(0), hm(1)),
            packages(MIB, MIB, vec![], no_rebuilt()),
            files,
            None,
        )
    };
    let report = report(Mode::Paths, vec![configuration.clone()]);
    assert_snapshot!("paths", render(&report, false, false));

    let same = Configuration {
        status: Status::NoChanges,
        new: configuration.old.clone(),
        ..configuration
    };
    let text = render(&self::report(Mode::Paths, vec![same]), false, false);
    assert!(
        text.contains(&format!(
            "  no changes, same toplevel /nix/store/{}-home-manager-generation\n",
            H[0]
        )),
        "{text}"
    );
}

#[test]
fn statuses() {
    let minimal = done(
        "web-a",
        ClosureKind::Unknown,
        (
            side(0, "toplevel-a", None, "root@web-a"),
            side(1, "toplevel-b", None, "local"),
        ),
        packages(MIB, MIB + 512, vec![], no_rebuilt()),
        vec![],
        None,
    );
    let configurations = vec![
        minimal,
        failed(
            "web-b",
            Some("root@web-b"),
            Status::Error {
                error: "evaluating toplevel: attribute missing".into(),
            },
        ),
        failed(
            "web-c",
            None,
            Status::Timeout {
                error: "timeout after 60 s".into(),
            },
        ),
        Configuration {
            old: Some(side(2, "toplevel-c", None, "root@web-d")),
            new: Some(side(2, "toplevel-c", None, "local")),
            ..failed("web-d", Some("root@web-d"), Status::NoChanges)
        },
    ];
    assert_snapshot!(
        "statuses",
        render(&report(Mode::Deployed, configurations), false, false)
    );
}

#[test]
fn several_versions_per_side() {
    use PackageChangeKind::*;
    let changes = vec![
        pkg(Upgraded, "foo", &[""], &["1.0"], 10),
        pkg(
            Changed,
            "glibc",
            &["2.39", "2.40"],
            &["2.40"],
            -5 * MIB as i64,
        ),
        pkg(
            Changed,
            "perl",
            &["5.38.2", "5.40.0"],
            &["5.40.0", "5.42.0"],
            0,
        ),
        pkg(
            Changed,
            "python3",
            &["3.12.4"],
            &["3.12.4", "3.13.1"],
            30 * MIB as i64,
        ),
        pkg(Added, "qt", &[], &["6.7.0", "6.8.0"], 50 * MIB as i64),
        pkg(Added, "unit-foo.service", &[], &[""], 1200),
        pkg(Removed, "libbar", &["0.9"], &[], -1),
        pkg(
            Resized,
            "firmware",
            &["20250801"],
            &["20250801"],
            3 * MIB as i64,
        ),
    ];
    let rebuilt = Rebuilt {
        count: 2,
        size_delta: 0,
        items: vec![],
    };
    let configuration = done(
        "web-a",
        ClosureKind::Nixos,
        nixos_sides("web-a", "root@web-a"),
        packages(GIB, GIB + 80 * MIB, changes, rebuilt),
        vec![],
        None,
    );
    assert_snapshot!(
        "several_versions",
        render(&report(Mode::Deployed, vec![configuration]), false, false)
    );
}

const GIB: u64 = 1024 * MIB;

fn shared_fleet() -> Report {
    let conf = |hash: usize| FileChange {
        hunks: vec![hunk(
            (3, 1),
            (3, 1),
            vec![
                line('-', "ExecStart=/usr/bin/true"),
                marked(
                    '+',
                    &format!("ExecStart=/nix/store/{}-app-1.0/bin/app", H[hash]),
                    &[(10, 50)],
                ),
            ],
        )],
        ..file("etc/systemd/system/app.service", FileStatus::Modified)
    };
    let mtab = FileChange {
        type_old: Some(FileType::ExternalLink),
        type_new: Some(FileType::ExternalLink),
        link_old: Some("/proc/mounts".into()),
        link_new: Some("/proc/self/mounts".into()),
        content_fetched: false,
        size_old: None,
        size_new: None,
        ..file("etc/mtab", FileStatus::LinkChanged)
    };
    let hash_only = churned("etc/static/x", ChurnClass::HashOnly, vec![]);
    let boot = |new: &str| Boot {
        kernel: link(Some("linux-6.12.40"), Some(new), false),
        initrd: None,
        kernel_modules: None,
        kernel_params: None,
        specialisations: None,
    };
    let host = |name: &str, packages_: Vec<PackageChange>, files: Vec<FileChange>, kernel: &str| {
        done(
            name,
            ClosureKind::Nixos,
            nixos_sides(name, &format!("root@{name}")),
            packages(GIB, GIB, packages_, no_rebuilt()),
            files,
            Some(boot(kernel)),
        )
    };
    let curl = pkg(
        PackageChangeKind::Upgraded,
        "curl",
        &["8.9.0"],
        &["8.10.1"],
        0,
    );
    report(
        Mode::Deployed,
        vec![
            host(
                "web-a",
                vec![curl.clone(), openssl(100)],
                vec![conf(0), hash_only.clone()],
                "linux-6.12.48",
            ),
            host(
                "web-b",
                vec![openssl(90)],
                vec![mtab.clone(), nix_conf(), hash_only.clone()],
                "linux-6.12.48",
            ),
            host(
                "web-c",
                vec![curl, openssl(80)],
                vec![mtab, conf(1), hash_only],
                "linux-6.12.47",
            ),
            failed(
                "web-d",
                Some("root@web-d"),
                Status::Error {
                    error: "boom".into(),
                },
            ),
        ],
    )
}

#[test]
fn shared_changes_and_also_in() {
    assert_snapshot!("shared", render(&shared_fleet(), false, false));
}

fn churn() -> Report {
    let hash_line = |op, hash: usize| Line {
        hash_only: true,
        ..line(op, &format!("ExecStart=/nix/store/{}-app/bin/app", H[hash]))
    };
    let files = vec![
        churned(
            "etc/a.conf",
            ChurnClass::HashOnly,
            vec![hunk(
                (1, 1),
                (1, 1),
                vec![hash_line('-', 0), hash_line('+', 1)],
            )],
        ),
        churned(
            "etc/b.conf",
            ChurnClass::VersionOnly,
            vec![hunk(
                (2, 1),
                (2, 1),
                vec![
                    marked(
                        '-',
                        &format!("/nix/store/{}-app-1.0", H[0]),
                        &[(11, 43), (50, 51)],
                    ),
                    marked(
                        '+',
                        &format!("/nix/store/{}-app-1.1", H[1]),
                        &[(11, 43), (50, 51)],
                    ),
                ],
            )],
        ),
        churned("etc/c.conf", ChurnClass::HashOnly, vec![]),
        FileChange {
            hunks: vec![hunk(
                (1, 2),
                (1, 2),
                vec![
                    line(' ', "port = 80"),
                    marked('-', "workers = 4", &[(10, 11)]),
                    marked('+', "workers = 8", &[(10, 11)]),
                ],
            )],
            ..file("etc/d.conf", FileStatus::Modified)
        },
    ];
    let configuration = done(
        "web-a",
        ClosureKind::Nixos,
        nixos_sides("web-a", "root@web-a"),
        packages(GIB, GIB, vec![], no_rebuilt()),
        files,
        None,
    );
    report(Mode::Deployed, vec![configuration])
}

#[test]
fn churn_collapsed() {
    assert_snapshot!("churn_collapsed", render(&churn(), false, false));
}

#[test]
fn churn_expanded() {
    assert_snapshot!("churn_expanded", render(&churn(), false, true));
}

#[test]
fn churn_expanded_in_colour() {
    assert_snapshot!("churn_expanded_colour", render(&churn(), true, true));
}

#[test]
fn word_highlight_styles_in_colour() {
    let mut text = String::new();
    for style in [
        WordHighlight::Dim,
        WordHighlight::Background,
        WordHighlight::Reverse,
        WordHighlight::Underline,
        WordHighlight::Bold,
    ] {
        let options = Options {
            color: true,
            word_highlight: style,
            ..Options::default()
        };
        let mut out = Vec::new();
        text::write(&normative(), options, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap().replace('\x1b', "\\e");
        text += &format!("{style:?}\n");
        // A pair with spans on the added side only, and one with spans on both.
        for line in out.lines().filter(|l| {
            ["trusted-public-keys", "--workers"]
                .iter()
                .any(|w| l.contains(w))
        }) {
            text += line;
            text += "\n";
        }
    }
    assert_snapshot!("word_highlight_styles_colour", text);
}

#[test]
fn colour_decision() {
    let set = Some(OsStr::new("1"));
    let empty = Some(OsStr::new(""));
    let cases = [
        (Color::Auto, None, true, true),
        (Color::Auto, empty, true, true),
        (Color::Auto, set, true, false),
        (Color::Auto, None, false, false),
        (Color::Always, set, false, true),
        (Color::Never, None, true, false),
    ];
    for (choice, no_color, terminal, expected) in cases {
        assert_eq!(
            color_enabled(choice, no_color, terminal),
            expected,
            "{choice:?} {no_color:?} {terminal}"
        );
    }
}

/// What the TUI relies on: file rows carry their display path at [`FILE_PATH_SPAN`], and
/// collapsible kinds are unique within one configuration.
#[test]
fn view_identity_for_the_tui() {
    let report = normative();
    let nodes = view::configuration(
        &report.mode,
        &report.shared,
        &report.configurations[0],
        WordHighlight::default(),
    );
    let mut collapsible = HashSet::new();
    let mut stack: Vec<&Node> = nodes.iter().collect();
    let mut files = 0;
    while let Some(node) = stack.pop() {
        if let Kind::File { path, via } = &node.kind {
            files += 1;
            assert!(node.spans[FILE_PATH_SPAN].text.starts_with(path.as_str()));
            assert!(
                via.iter()
                    .all(|v| node.spans[FILE_PATH_SPAN].text.contains(v.as_str()))
            );
        }
        if node.collapsible() {
            assert!(collapsible.insert(node.kind.clone()), "{:?}", node.kind);
        }
        stack.extend(&node.children);
    }
    let Status::Done(sections) = &report.configurations[0].status else {
        panic!("web-a is done")
    };
    assert_eq!(
        files,
        sections.files.changes.len(),
        "one node per file change"
    );
    assert!(collapsible.contains(&Kind::PackageGroup(PackageGroup::Changed)));
    assert!(collapsible.contains(&Kind::Rebuilt));
    assert!(collapsible.contains(&Kind::Churn(ChurnClass::HashOnly)));
}

#[test]
fn nodes_under_any_open_state() {
    let report = normative();
    let nodes = view::configuration(
        &report.mode,
        &report.shared,
        &report.configurations[0],
        WordHighlight::default(),
    );
    let mut out = Vec::new();
    text::write_nodes(&nodes, |_| true, false, &mut out).unwrap();
    let all_open = String::from_utf8(out).unwrap();
    assert!(
        all_open.contains("\n  412 rebuilt (same version, new hash)\n    bash  5.2p37\n"),
        "{all_open}"
    );
    assert!(
        all_open.contains("\n  57 hash-only (store hash churn)\n    [M] etc/static/hash-00\n"),
        "{all_open}"
    );

    // `Y`: the configuration exactly as text output prints it.
    let mut out = Vec::new();
    text::write_as_text(&nodes, Options::default(), &mut out).unwrap();
    let full = render(&report, false, false);
    let block = full.split("\n\n").nth(1).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{block}\n"));

    assert_eq!(view::rebuilt_label(3), "3 rebuilt (same version, new hash)");
    assert_eq!(
        view::churn_label(ChurnClass::VersionOnly, 2, false),
        "2 version-only (version bumps)"
    );
}

#[test]
fn escape_makes_controls_visible() {
    let raw = "a\tb\x1b[2J\r\n\x7f\u{9b}31m é";
    assert_eq!(view::escape(raw), "a\tb^[[2J^M^J^?\\u{9b}31m é");
    assert_eq!(view::escape(&view::escape(raw)), view::escape(raw));
}

/// Every row kind not in the other snapshots, multi-byte highlight and display widths, and
/// control characters from file contents, paths and messages.
fn row_kinds() -> Report {
    let injection = "a\x1b]52;c;ZXZpbA==\x07b\x1b]0;pwned\x07c\x1b[2J\rX\u{9b}31mY";
    let x = injection.rfind('X').unwrap();
    let names = |path: &str| FileChange {
        hunks: vec![hunk(
            (1, 1),
            (1, 1),
            vec![
                marked('-', "名前 = 太郎", &[(9, 15)]),
                marked('+', "名前 = 花子", &[(9, 15)]),
            ],
        )],
        ..file(path, FileStatus::Modified)
    };
    let web_a_files = vec![
        names("etc/abcdefgh.conf"),
        FileChange {
            binary: true,
            size_old: Some(1024),
            size_new: Some(2048),
            ..file("etc/blob.bin", FileStatus::Modified)
        },
        file("etc/evil\x1b[31m.conf", FileStatus::Added),
        FileChange {
            hunks: vec![hunk(
                (1, 2),
                (0, 0),
                vec![line('-', "a = 1"), line('-', "b = 2")],
            )],
            ..file("etc/gone.conf", FileStatus::Removed)
        },
        FileChange {
            hunks: vec![hunk(
                (1, 1),
                (1, 1),
                vec![line('-', "plain"), marked('+', injection, &[(x, x + 1)])],
            )],
            ..file("etc/inject.conf", FileStatus::Modified)
        },
        FileChange {
            type_old: Some(FileType::Directory),
            type_new: Some(FileType::ExternalLink),
            link_new: Some("/etc/static/ssl/certs".into()),
            content_fetched: false,
            size_old: None,
            size_new: None,
            ..file("etc/ssl/certs", FileStatus::TypeChanged)
        },
        names("etc/日本語.conf"),
    ];
    let boot = Boot {
        kernel: link(None, Some("linux-6.12.48"), false),
        initrd: link(Some("initrd-linux-6.12.40"), None, false),
        kernel_modules: None,
        kernel_params: Some(ParamsChange {
            old: "".into(),
            new: "quiet".into(),
        }),
        specialisations: Some(Specialisations {
            added: vec![],
            removed: vec!["old".into()],
            changed: vec!["gpu".into()],
        }),
    };
    let empty = || packages(MIB, MIB, vec![], no_rebuilt());
    report(
        Mode::Deployed,
        vec![
            done(
                "web-a",
                ClosureKind::Nixos,
                nixos_sides("web-a", "root@web-a"),
                empty(),
                web_a_files,
                Some(boot),
            ),
            done(
                "web-b",
                ClosureKind::Nixos,
                nixos_sides("web-b", "root@web-b"),
                empty(),
                vec![names("etc/abcdefgh.conf"), names("etc/日本語.conf")],
                None,
            ),
            failed(
                "web-c",
                Some("root@web-c"),
                Status::Error {
                    error:
                        "ssh root@web-c: banner \x1b]0;pwned\x07\nPermission denied (publickey)."
                            .into(),
                },
            ),
        ],
    )
}

#[test]
fn row_kinds_in_colour() {
    let report = row_kinds();
    assert_snapshot!("row_kinds_colour", render(&report, true, false));
    let plain = render(&report, false, false);
    assert!(
        plain.chars().all(|c| c == '\n' || !c.is_control()),
        "{plain:?}"
    );
}

#[test]
fn json_escapes_del_and_c1() {
    let report = report(
        Mode::Deployed,
        vec![failed(
            "web-a",
            None,
            Status::Error {
                error: "a\x1b\x7f\u{85}\u{9f}é".into(),
            },
        )],
    );
    let mut out = Vec::new();
    json::write(&report, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(r#""error": "a\u001b\u007f\u0085\u009fé""#),
        "{text}"
    );
    assert!(text.chars().all(|c| c == '\n' || !c.is_control()));
    assert_eq!(serde_json::from_str::<Report>(&text).unwrap(), report);
}
