//! The viewer driven through its state, without a terminal (spec 11), and render snapshots of
//! representative screens on `TestBackend`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use insta::assert_snapshot;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::app::{App, Focus, KeyCode, KeyEvent};
use super::terminal::{Guard, finish, lock};
use super::{Options, draw, osc52};
use crate::report::{
    Boot, ChurnClass, ClosureKind, Configuration, FileChange, FileStatus, FileType, Files, Hunk,
    Line, LineOp, LinkChange, Mode, PackageChange, PackageChangeKind, Packages, Rebuilt,
    RebuiltPackage, Sections, Side, Source, Status,
};

const MIB: u64 = 1024 * 1024;

fn side(hash: char, name: &str, location: &str) -> Side {
    Side {
        path: format!(
            "/nix/store/{}-nixos-system-{name}",
            hash.to_string().repeat(32)
        ),
        system_version: Some("25.11".into()),
        location: location.into(),
    }
}

fn upgraded(pname: &str, old: &str, new: &str, delta: i64) -> PackageChange {
    PackageChange {
        kind: PackageChangeKind::Upgraded,
        pname: pname.into(),
        old_versions: vec![old.into()],
        new_versions: vec![new.into()],
        size_delta: delta,
    }
}

fn line(op: LineOp, text: &str) -> Line {
    Line {
        op,
        text: text.into(),
        spans: vec![],
        hash_only: false,
        no_newline: false,
    }
}

fn modified(path: &str, via: &[&str], class: ChurnClass, lines: Vec<Line>) -> FileChange {
    FileChange {
        path: path.into(),
        via: via.iter().map(|v| v.to_string()).collect(),
        status: FileStatus::Modified,
        class,
        type_old: Some(FileType::File),
        type_new: Some(FileType::File),
        binary: false,
        content_fetched: true,
        size_old: Some(100),
        size_new: Some(100),
        link_old: None,
        link_new: None,
        hunks: vec![Hunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines,
        }],
    }
}

fn nix_conf() -> FileChange {
    modified(
        "etc/nix/nix.conf",
        &[],
        ChurnClass::Real,
        vec![
            line(
                LineOp::Removed,
                "substituters = https://cache.example https://old.cache.example/with/a/long/path",
            ),
            line(LineOp::Added, "substituters = https://cache.example"),
        ],
    )
}

fn link(path: &str, old: &str, new: &str) -> FileChange {
    FileChange {
        status: FileStatus::LinkChanged,
        type_old: Some(FileType::ExternalLink),
        type_new: Some(FileType::ExternalLink),
        size_old: None,
        size_new: None,
        link_old: Some(old.into()),
        link_new: Some(new.into()),
        hunks: vec![],
        ..modified(path, &[], ChurnClass::Real, vec![])
    }
}

fn done(
    name: &str,
    hash: char,
    changes: Vec<PackageChange>,
    rebuilt: &[&str],
    files: Vec<FileChange>,
) -> Configuration {
    let items: Vec<RebuiltPackage> = rebuilt
        .iter()
        .map(|pname| RebuiltPackage {
            pname: pname.to_string(),
            versions: vec!["1.0".into()],
            size_delta: 1024,
        })
        .collect();
    Configuration {
        name: name.into(),
        source: Source::Colmena,
        kind: ClosureKind::Nixos,
        target: Some(format!("root@{name}")),
        status: Status::Done(Box::new(Sections {
            packages: Packages {
                size_old: 1200 * MIB,
                size_new: 1241 * MIB,
                changes,
                rebuilt: Rebuilt {
                    count: items.len(),
                    size_delta: 1024 * items.len() as i64,
                    items,
                },
            },
            files: Files { changes: files },
            boot: Some(Boot {
                kernel: Some(LinkChange {
                    old: Some("linux-6.12.40".into()),
                    new: Some("linux-6.12.48".into()),
                    rebuilt: false,
                }),
                initrd: None,
                kernel_modules: None,
                kernel_params: None,
                specialisations: None,
            }),
        })),
        old: Some(side(
            hash,
            &format!("{name}-25.05"),
            &format!("root@{name}"),
        )),
        new: Some(side('9', &format!("{name}-25.11"), "local")),
    }
}

fn web_a() -> Configuration {
    done(
        "web-a",
        '1',
        vec![
            upgraded("openssl", "3.0.13", "3.0.14", 100),
            upgraded("systemd", "257.5", "258.1", 1_200_000),
        ],
        &["bash", "coreutils", "zlib"],
        vec![
            link("etc/mtab", "/proc/mounts", "/proc/self/mounts"),
            nix_conf(),
            modified(
                "etc/systemd/system/app.service",
                &["app-start/bin/app-start"],
                ChurnClass::Real,
                vec![
                    line(LineOp::Removed, "exec app --workers 4"),
                    line(LineOp::Added, "exec app --workers 8"),
                ],
            ),
            modified(
                "etc/static/a",
                &[],
                ChurnClass::HashOnly,
                vec![line(LineOp::Added, "x")],
            ),
            modified(
                "etc/static/b",
                &[],
                ChurnClass::HashOnly,
                vec![line(LineOp::Added, "y")],
            ),
        ],
    )
}

fn web_b() -> Configuration {
    done(
        "web-b",
        '2',
        vec![upgraded("openssl", "3.0.13", "3.0.14", 100)],
        &[],
        vec![nix_conf()],
    )
}

fn db_a() -> Configuration {
    Configuration {
        name: "db-a".into(),
        source: Source::Colmena,
        kind: ClosureKind::Unknown,
        target: Some("root@db-a".into()),
        status: Status::Error {
            error: "ssh root@db-a: connect to host db-a port 22: Connection refused".into(),
        },
        old: None,
        new: None,
    }
}

fn fleet(arrived: Vec<Configuration>) -> App {
    let names = ["db-a", "web-a", "web-b"].map(String::from).to_vec();
    let mut app = App::new(Mode::Deployed, names, Options::default());
    app.resize(100, 30);
    for c in arrived {
        app.arrive(c);
    }
    app
}

fn press(app: &mut App, keys: &str) {
    for c in keys.chars() {
        let code = match c {
            '\n' => KeyCode::Enter,
            '\t' => KeyCode::Tab,
            '\x1b' => KeyCode::Esc,
            '\x08' => KeyCode::Backspace,
            c => KeyCode::Char(c),
        };
        app.key(code.into());
    }
}

fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent {
        code: KeyCode::Char(c),
        ctrl: true,
        alt: false,
    });
}

fn shown(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.text()).collect()
}

fn cursor(app: &App) -> usize {
    app.views[app.selected].cursor
}

fn cursor_text(app: &App) -> String {
    app.rows()[cursor(app)].text().trim().to_owned()
}

/// The text an OSC 52 escape carries.
fn clipboard(app: &mut App) -> String {
    let payload = app.clipboard.take().expect("something was copied");
    let data = payload
        .strip_prefix("\x1b]52;c;")
        .and_then(|p| p.strip_suffix('\x07'))
        .expect("an OSC 52 escape");
    String::from_utf8(STANDARD.decode(data).unwrap()).unwrap()
}

/// The screen as the event loop draws it: resized to the terminal first.
fn screen(app: &mut App, width: u16, height: u16) -> String {
    app.resize(width, height);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw::draw(app, frame)).unwrap();
    terminal.backend().to_string()
}

fn statuses(app: &App) -> Vec<String> {
    app.list()
        .iter()
        .map(|r| {
            let status: String = r.status.iter().map(|s| s.text.as_str()).collect();
            format!("{} {} {status}", r.index, r.name).trim().to_owned()
        })
        .collect()
}

#[test]
fn osc52_encodes_the_text() {
    assert_eq!(osc52("hi\n"), "\x1b]52;c;aGkK\x07");
}

#[test]
fn list_shows_all_then_configurations_with_live_status() {
    let mut app = fleet(vec![]);
    assert_eq!(
        statuses(&app),
        [
            "All 0 shared",
            "1 db-a fetching…",
            "2 web-a fetching…",
            "3 web-b fetching…"
        ]
    );
    app.arrive(web_a());
    app.arrive(db_a());
    assert_eq!(
        statuses(&app)[1..3],
        ["1 db-a error", "2 web-a done +41.0 MiB"]
    );
    // Shared changes and `(also in N)` follow the arrivals.
    press(&mut app, "2");
    assert!(!shown(&app).iter().any(|r| r.contains("(also in")));
    app.arrive(web_b());
    assert_eq!(statuses(&app)[0], "All 3 shared");
    assert!(shown(&app).contains(&"  [U] openssl  +100 B   3.0.13 -> 3.0.14  (also in 1)".into()));
}

#[test]
fn list_statuses_follow_the_table() {
    let names = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
    let mut app = App::new(Mode::Deployed, names, Options::default());
    let status = |name: &str, status: Status| Configuration {
        name: name.into(),
        status,
        ..db_a()
    };
    app.arrive(status("a", Status::NoChanges));
    app.arrive(status(
        "b",
        Status::Timeout {
            error: "timeout after 60 s".into(),
        },
    ));
    app.arrive(status("c", Status::AbsentAtBase));
    app.arrive(status("d", db_a().status));
    assert_eq!(
        statuses(&app),
        [
            "All 0 shared",
            "1 a no changes",
            "2 b timeout",
            "3 c absent at base",
            "4 d error",
            "5 e fetching…"
        ]
    );
    let list = app.list();
    let style = |i: usize| list[i].status[0].style;
    assert!(style(1).dim && style(3).dim && style(5).dim);
    assert_eq!(style(2).color, Some(crate::render::view::Color::Red));
}

#[test]
fn one_configuration_has_no_all_entry() {
    let app = App::new(Mode::Paths, vec!["localhost".into()], Options::default());
    assert_eq!(statuses(&app), ["1 localhost fetching…"]);
    assert_eq!(shown(&app), ["fetching…"]);
}

#[test]
fn list_width_is_the_widest_row_at_most_a_third() {
    let mut app = fleet(vec![web_a()]);
    // Borders, `> 1 `, `web-a`, two spaces, `done +41.0 MiB`.
    assert_eq!(app.areas().list.width, 2 + 4 + 5 + 2 + 14);
    app.resize(60, 20);
    assert_eq!(app.areas().list.width, 20);
}

#[test]
fn quitting_early_reports_what_arrived() {
    let mut app = fleet(vec![web_b(), db_a()]);
    press(&mut app, "q");
    assert!(app.quit);
    let finished = app.finish();
    assert!(!finished.all_finished);
    let names: Vec<_> = finished
        .configurations
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["db-a", "web-b"]);

    let app = fleet(vec![web_b(), db_a(), web_a()]);
    assert!(app.finish().all_finished);
}

#[test]
fn a_configuration_queued_before_quitting_counts_as_finished() {
    let mut app = fleet(vec![web_b(), db_a()]);
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    sender.send(web_a()).unwrap();
    press(&mut app, "q");
    assert!(finish(app, &mut events).all_finished);
}

#[test]
fn ctrl_c_quits_also_while_editing_the_filter() {
    let mut app = fleet(vec![]);
    press(&mut app, "/q");
    assert!(!app.quit, "q is typed into the filter");
    ctrl(&mut app, 'c');
    assert!(app.quit);
}

#[test]
fn selection_and_focus() {
    let mut app = fleet(vec![web_a(), web_b(), db_a()]);
    assert_eq!((app.selected, app.focus), (0, Focus::List));
    press(&mut app, "jj");
    assert_eq!(app.title(), "web-a");
    press(&mut app, "kkk");
    assert_eq!(app.title(), "All");
    press(&mut app, "J");
    assert_eq!(app.selected, 1, "J moves one row in the list");
    press(&mut app, "3");
    assert_eq!(app.title(), "web-b");
    press(&mut app, "9");
    assert_eq!(app.title(), "web-b", "no ninth configuration");
    press(&mut app, "\t");
    assert_eq!(app.focus, Focus::Detail);
    app.key(KeyCode::BackTab.into());
    assert_eq!(app.focus, Focus::List);
    press(&mut app, "\n");
    assert_eq!(app.focus, Focus::Detail);
    press(&mut app, "\t ");
    assert_eq!(app.focus, Focus::Detail, "space focuses the detail");
    press(&mut app, "\tl");
    assert_eq!(app.focus, Focus::Detail);
}

/// web-a's detail with the default open state and tree view.
const WEB_A: [&str; 18] = [
    "web-a · nixos (root@web-a)",
    "  old /nix/store/11111111111111111111111111111111-nixos-system-web-a-25.05",
    "  new /nix/store/99999999999999999999999999999999-nixos-system-web-a-25.11",
    "  system 25.11",
    "Packages  1.2 GiB -> 1.2 GiB (+41.0 MiB)",
    "  [U] openssl  +100 B   3.0.13 -> 3.0.14  (also in 1)",
    "  [U] systemd  +1.1 MiB 257.5 -> 258.1",
    "  3 rebuilt (same version, new hash, +3.0 KiB)",
    "Files",
    "  etc/  (3)",
    "    [L] mtab  /proc/mounts -> /proc/self/mounts",
    "    nix/  (1)",
    "      [M] nix.conf  (also in 1)",
    "    systemd/system/  (1)",
    "      [M] app.service → app-start/bin/app-start",
    "  2 hash-only (store hash churn)",
    "Boot",
    "  kernel: linux-6.12.40 -> linux-6.12.48  (also in 1)",
];

fn web_a_detail() -> App {
    let mut app = fleet(vec![web_a(), web_b(), db_a()]);
    press(&mut app, "2\t");
    app
}

#[test]
fn detail_shows_the_configuration_with_default_open_state() {
    assert_eq!(shown(&web_a_detail()), WEB_A);
}

#[test]
fn expand_churn_opens_churn_groups() {
    let names = vec!["web-a".to_owned()];
    let options = Options {
        expand_churn: true,
        color: false,
    };
    let mut app = App::new(Mode::Deployed, names, options);
    app.arrive(web_a());
    let rows = shown(&app);
    let at = rows.iter().position(|r| r.contains("hash-only")).unwrap();
    assert_eq!(
        rows[at + 1..at + 4],
        ["    etc/static/  (2)", "      [M] a", "      [M] b"]
    );
}

#[test]
fn j_k_g_and_capital_j_k_move_in_detail() {
    let mut app = web_a_detail();
    press(&mut app, "jj");
    assert_eq!(cursor(&app), 2);
    press(&mut app, "J");
    assert_eq!(
        cursor_text(&app),
        "[U] openssl  +100 B   3.0.13 -> 3.0.14  (also in 1)"
    );
    press(&mut app, "JJ");
    assert_eq!(
        cursor_text(&app),
        "3 rebuilt (same version, new hash, +3.0 KiB)"
    );
    press(&mut app, "J");
    assert_eq!(cursor_text(&app), "etc/  (3)", "a directory is collapsible");
    press(&mut app, "K");
    assert_eq!(cursor(&app), 7);
    press(&mut app, "G");
    assert_eq!(cursor(&app), WEB_A.len() - 1);
    press(&mut app, "J");
    assert_eq!(cursor(&app), WEB_A.len() - 1, "nothing further");
    press(&mut app, "g");
    assert_eq!(cursor(&app), 0);
    press(&mut app, "K");
    assert_eq!(cursor(&app), 0);
    app.key(KeyCode::Down.into());
    assert_eq!(cursor(&app), 1);
}

#[test]
fn g_works_on_the_detail_from_the_list() {
    let mut app = web_a_detail();
    press(&mut app, "\tG");
    assert_eq!((app.focus, cursor(&app)), (Focus::List, WEB_A.len() - 1));
}

#[test]
fn enter_toggles_and_on_a_leaf_collapses_its_parent() {
    let mut app = web_a_detail();
    let nix_conf = 12;
    app.views[app.selected].cursor = nix_conf;
    press(&mut app, "\n");
    assert_eq!(
        shown(&app)[13..16],
        [
            "          @@ -1,1 +1,1 @@",
            "          -substituters = https://cache.example https://old.cache.example/with/a/long/path",
            "          +substituters = https://cache.example",
        ]
    );
    press(&mut app, "jj ");
    assert_eq!(cursor(&app), nix_conf);
    assert_eq!(shown(&app), WEB_A);
    // mtab is a leaf in `etc/`: Enter folds the directory.
    press(&mut app, "kk\n");
    assert_eq!(cursor_text(&app), "etc/  (3)");
    assert_eq!(shown(&app)[10], "  2 hash-only (store hash churn)");
    press(&mut app, "\n");
    assert_eq!(shown(&app), WEB_A);
    // A leaf under a header that does not fold stays put.
    press(&mut app, "g\n");
    assert_eq!(cursor(&app), 0);
    app.views[app.selected].cursor = 5;
    press(&mut app, "\n");
    assert_eq!(
        (cursor(&app), shown(&app)),
        (5, WEB_A.map(String::from).to_vec())
    );
}

#[test]
fn h_collapses_then_goes_to_the_parent_then_to_the_list() {
    let mut app = web_a_detail();
    app.views[app.selected].cursor = 12;
    press(&mut app, "lj");
    assert_eq!(cursor_text(&app), "@@ -1,1 +1,1 @@");
    press(&mut app, "h");
    assert_eq!(cursor(&app), 12, "to the parent");
    press(&mut app, "h");
    assert_eq!(shown(&app), WEB_A, "collapsed");
    press(&mut app, "h");
    assert_eq!(cursor_text(&app), "nix/  (1)");
    press(&mut app, "hh");
    assert_eq!(cursor_text(&app), "etc/  (3)");
    press(&mut app, "hh");
    assert_eq!(cursor_text(&app), "Files");
    assert_eq!(app.focus, Focus::Detail);
    press(&mut app, "h");
    assert_eq!(app.focus, Focus::List);
    press(&mut app, "l");
    assert_eq!(app.focus, Focus::Detail);
    assert_eq!(cursor_text(&app), "Files", "l from the list only focuses");
}

#[test]
fn l_expands_and_never_collapses() {
    let mut app = web_a_detail();
    app.views[app.selected].cursor = 7;
    press(&mut app, "l");
    assert_eq!(
        shown(&app)[8..11],
        [
            "    bash       +1.0 KiB 1.0",
            "    coreutils  +1.0 KiB 1.0",
            "    zlib       +1.0 KiB 1.0",
        ]
    );
    press(&mut app, "l");
    assert_eq!(shown(&app).len(), WEB_A.len() + 3);
}

#[test]
fn open_state_is_kept_per_configuration_across_selection_and_filter() {
    let mut app = web_a_detail();
    app.views[app.selected].cursor = 12;
    press(&mut app, "l");
    let open = shown(&app);
    press(&mut app, "\t3");
    assert!(
        !shown(&app).iter().any(|r| r.contains("@@")),
        "web-b keeps its own"
    );
    press(&mut app, "2");
    assert_eq!(shown(&app), open);
    press(&mut app, "/nix\n");
    assert!(shown(&app).iter().any(|r| r.contains("@@")));
    press(&mut app, "/\x08\x08\x08\n");
    assert_eq!(shown(&app), open);
}

#[test]
fn filter_editing() {
    let mut app = web_a_detail();
    press(&mut app, "/SSL");
    assert_eq!(app.editing.as_deref(), Some("SSL"));
    assert_eq!(shown(&app), WEB_A, "not applied while editing");
    press(&mut app, "\n");
    assert_eq!(app.title(), "web-a [filter: SSL]");
    assert_eq!(
        shown(&app)[4..],
        [
            "Packages  1.2 GiB -> 1.2 GiB (+41.0 MiB)",
            "  [U] openssl  +100 B   3.0.13 -> 3.0.14  (also in 1)",
            "Boot",
            "  kernel: linux-6.12.40 -> linux-6.12.48  (also in 1)",
        ]
    );
    // Esc keeps the previous filter; Backspace deletes.
    press(&mut app, "/x\x1b");
    assert_eq!(app.filter, "SSL");
    press(&mut app, "/\x08\x08\x08static\n");
    assert_eq!(
        shown(&app)[4..],
        [
            "Files",
            "  2 hash-only (store hash churn)",
            "Boot",
            "  kernel: linux-6.12.40 -> linux-6.12.48  (also in 1)",
        ]
    );
    // The filter holds in every entry, All included.
    press(&mut app, &format!("/{}nix\n\tkk", "\x08".repeat(6)));
    assert_eq!(app.title(), "All [filter: nix]");
    let rows = shown(&app);
    assert_eq!(
        rows[..2],
        ["Shared across configurations", "  etc/nix/  (1)"]
    );
    assert!(rows[2].starts_with("    [M] nix.conf  ") && rows[2].ends_with("web-a, web-b"));
    assert!(rows[3].starts_with("  kernel: "), "Boot rows always show");
    assert_eq!(rows.len(), 4);
}

#[test]
fn the_cursor_stays_on_its_row_across_filter_tree_view_and_arrivals() {
    let mut app = web_a_detail();
    app.views[app.selected].cursor = 12;
    press(&mut app, "/nix\n");
    assert_eq!(cursor_text(&app), "[M] nix.conf  (also in 1)");
    press(&mut app, "t");
    assert_eq!(cursor_text(&app), "[M] etc/nix/nix.conf  (also in 1)");
    // In All, rows shift as shared changes arrive.
    let names = ["web-a", "web-b", "web-c", "web-d"]
        .map(String::from)
        .to_vec();
    let mut app = App::new(Mode::Deployed, names, Options::default());
    app.arrive(web_a());
    app.arrive(web_b());
    press(&mut app, "\tG");
    let last = cursor_text(&app);
    assert!(last.starts_with("kernel: "), "{last}");
    for name in ["web-c", "web-d"] {
        let mut c = web_b();
        c.name = name.into();
        if let Status::Done(s) = &mut c.status {
            s.packages
                .changes
                .insert(0, upgraded("curl", "8.1", "8.2", 1));
        }
        app.arrive(c);
    }
    assert!(shown(&app)[1].contains("curl"));
    assert_eq!(
        cursor_text(&app),
        last.replace("web-a, web-b", "web-a, web-b, web-c, web-d")
    );
}

#[test]
fn all_aligns_the_names_column_in_tree_view() {
    let app = fleet(vec![web_a(), web_b()]);
    let rows = shown(&app);
    assert_eq!(rows[2], "  etc/nix/  (1)");
    let column: Vec<_> = [1, 3, 4].map(|i| rows[i].find("web-a, web-b")).into();
    assert_eq!(column, [Some(42); 3], "{rows:#?}");
}

#[test]
fn w_and_t_toggle_wrap_and_tree_view() {
    let mut app = web_a_detail();
    press(&mut app, "t");
    assert!(shown(&app).contains(&"  [M] etc/nix/nix.conf  (also in 1)".into()));
    press(&mut app, "t");
    assert_eq!(shown(&app), WEB_A);
    app.resize(60, 30);
    let rows = app.rows();
    let height = |app: &App| *app.starts(&rows).last().unwrap();
    let wrapped = height(&app);
    assert!(wrapped > rows.len());
    press(&mut app, "w");
    assert_eq!(height(&app), rows.len());
}

/// web-a with its rebuilt group open over 60 packages: a detail longer than the pane.
fn long_detail(width: u16) -> App {
    let names: Vec<String> = (0..60).map(|i| format!("pkg-{i:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = App::new(Mode::Deployed, vec!["web-a".into()], Options::default());
    app.resize(width, 30);
    app.arrive(done("web-a", '1', vec![], &names, vec![]));
    press(&mut app, "\t");
    app.views[0].cursor = 5;
    press(&mut app, "lg");
    app
}

#[test]
fn scrolling_keeps_five_lines_of_margin() {
    let mut app = long_detail(120);
    let height = app.detail_size().1;
    assert_eq!(height, 27);
    let top = |app: &App| app.views[0].top;
    press(&mut app, &"j".repeat(21));
    assert_eq!((cursor(&app), top(&app)), (21, 0));
    press(&mut app, "j");
    assert_eq!((cursor(&app), top(&app)), (22, 1));
    press(&mut app, "G");
    assert_eq!(top(&app), app.rows().len() - height);
    assert_eq!((cursor(&app), top(&app)), (67, 41));
    press(&mut app, &"k".repeat(21));
    assert_eq!((cursor(&app), top(&app)), (46, 41));
    press(&mut app, "k");
    assert_eq!((cursor(&app), top(&app)), (45, 40));
    press(&mut app, "g");
    assert_eq!(top(&app), 0);
}

#[test]
fn half_pages_move_in_screen_lines() {
    let mut app = long_detail(120);
    ctrl(&mut app, 'd');
    assert_eq!((cursor(&app), app.views[0].top), (13, 13 - 5));
    app.key(KeyCode::PageDown.into());
    assert_eq!(cursor(&app), 26);
    ctrl(&mut app, 'u');
    assert_eq!(cursor(&app), 13);
    app.key(KeyCode::PageUp.into());
    assert_eq!(cursor(&app), 0);
    // Wrapped rows take several lines: a half page skips fewer rows.
    let mut app = long_detail(100);
    app.views[0].cursor = 0;
    app.resize(30, 30);
    let starts = app.starts(&app.rows());
    ctrl(&mut app, 'd');
    let half = app.detail_size().1 / 2;
    let at = starts[cursor(&app)];
    assert!(at <= half && half < starts[cursor(&app) + 1], "{at} {half}");
    // In the list, half the list.
    let mut app = fleet(vec![]);
    ctrl(&mut app, 'd');
    assert_eq!(app.selected, 3);
}

#[test]
fn y_copies_the_pane_as_shown_unwrapped_without_colour() {
    let mut app = web_a_detail();
    app.options.color = true;
    app.resize(50, 30);
    app.views[app.selected].cursor = 12;
    press(&mut app, "l/nix\ny");
    let copied = clipboard(&mut app);
    assert_eq!(
        copied,
        "web-a · nixos (root@web-a)
  old /nix/store/11111111111111111111111111111111-nixos-system-web-a-25.05
  new /nix/store/99999999999999999999999999999999-nixos-system-web-a-25.11
  system 25.11
Files
  etc/nix/  (1)
    [M] nix.conf  (also in 1)
        @@ -1,1 +1,1 @@
        -substituters = https://cache.example https://old.cache.example/with/a/long/path
        +substituters = https://cache.example
Boot
  kernel: linux-6.12.40 -> linux-6.12.48  (also in 1)
"
    );
    let size = osc52(&copied).len() as f64 / 1024.0;
    assert_eq!(
        app.notice.as_deref(),
        Some(format!("copied 12 lines ({size:.1} KiB)").as_str())
    );
    press(&mut app, "j");
    assert_eq!(app.notice, None, "transient");
}

#[test]
fn capital_y_copies_the_text_output_without_colour() {
    let mut app = web_a_detail();
    app.options.color = true;
    press(&mut app, "/nix\nY");
    let copied = clipboard(&mut app);
    assert!(!copied.contains('\x1b'));
    assert!(copied.contains("\n  3 rebuilt (same version, new hash, +3.0 KiB)\n"));
    assert!(copied.contains("\n  [M] etc/nix/nix.conf  (also in 1)\n      @@ -1,1 +1,1 @@\n"));
    assert!(copied.contains("\n  2 hash-only (store hash churn; --expand-churn shows them)\n"));
    // All copies the shared section as text output prints it.
    press(&mut app, "\tkkY");
    let row = |label: &str| format!("{label:<42}web-a, web-b\n");
    assert_eq!(
        clipboard(&mut app),
        "Shared across configurations\n".to_owned()
            + &row("  [U] openssl  3.0.13 -> 3.0.14")
            + &row("  [M] etc/nix/nix.conf")
            + &row("  kernel: linux-6.12.40 -> linux-6.12.48")
    );
}

#[test]
fn copy_notices() {
    let mut app = fleet(vec![web_a()]);
    press(&mut app, "3y");
    assert_eq!(app.notice.as_deref(), Some("nothing to copy yet"));
    press(&mut app, "Y");
    assert_eq!(app.notice.as_deref(), Some("nothing to copy yet"));
    assert_eq!(app.clipboard, None);
    press(&mut app, "kkky");
    assert_eq!(shown(&app), ["no shared changes yet"]);
    assert_eq!(app.notice.as_deref(), Some("nothing to copy yet"));
    press(&mut app, "Y");
    assert_eq!(app.notice.as_deref(), Some("nothing to copy yet"));
    app.arrive(db_a());
    app.arrive(web_b());
    app.shared.clear();
    press(&mut app, "y");
    assert_eq!(app.notice.as_deref(), Some("nothing to copy"));
    assert_eq!(app.clipboard, None);
    // A large payload gets the warning.
    let big = (0..2000)
        .map(|i| line(LineOp::Added, &format!("{i:060}")))
        .collect();
    let mut c = web_b();
    if let Status::Done(s) = &mut c.status {
        s.files.changes = vec![modified("etc/big", &[], ChurnClass::Real, big)];
    }
    let mut app = App::new(Mode::Deployed, vec!["web-b".into()], Options::default());
    app.arrive(c);
    press(&mut app, "Y");
    let notice = app.notice.clone().unwrap();
    assert!(
        notice.ends_with("KiB), may exceed the terminal's clipboard limit"),
        "{notice}"
    );
}

#[test]
fn screen_all_while_fetching() {
    let mut app = fleet(vec![web_a(), db_a()]);
    assert_snapshot!(screen(&mut app, 100, 12));
}

#[test]
fn screen_configuration_detail() {
    let mut app = web_a_detail();
    app.views[app.selected].cursor = 12;
    press(&mut app, "l");
    assert_snapshot!(screen(&mut app, 100, 26));
}

#[test]
fn screen_filter_and_wrap_on_a_narrow_terminal() {
    let mut app = web_a_detail();
    app.resize(64, 18);
    app.views[app.selected].cursor = 12;
    press(&mut app, "l/nix\n");
    assert_snapshot!(screen(&mut app, 64, 18));
    press(&mut app, "w");
    assert_snapshot!(screen(&mut app, 64, 18));
}

#[test]
fn screen_editing_the_filter() {
    let mut app = App::new(Mode::Paths, vec!["localhost".into()], Options::default());
    app.resize(48, 6);
    press(&mut app, "/web");
    assert_snapshot!(screen(&mut app, 48, 6));
}

#[test]
fn half_pages_scroll_through_a_row_taller_than_the_pane() {
    let long = "x".repeat(3000);
    let file = modified(
        "etc/env",
        &[],
        ChurnClass::Real,
        vec![line(LineOp::Added, &long)],
    );
    let mut app = App::new(Mode::Deployed, vec!["web-a".into()], Options::default());
    app.resize(80, 24);
    app.arrive(done("web-a", '1', vec![], &[], vec![file]));
    let at = app.rows().iter().position(|r| r.text().contains("[M] env"));
    app.views[0].cursor = at.unwrap();
    press(&mut app, "\tl");
    let rows = app.rows();
    let tall = rows.iter().position(|r| r.text().contains("xxx")).unwrap();
    let starts = app.starts(&rows);
    let (start, end) = (starts[tall], starts[tall + 1]);
    let height = app.detail_size().1;
    assert!(end - start > 2 * height);
    let mut seen = vec![false; end - start];
    let mut mark = |app: &App| {
        let top = app.views[0].top;
        for line in (top..top + height).filter(|l| (start..end).contains(l)) {
            seen[line - start] = true;
        }
    };
    for _ in 0..(end - start) {
        if cursor(&app) > tall {
            break;
        }
        ctrl(&mut app, 'd');
        mark(&app);
    }
    assert!(cursor(&app) > tall, "the cursor leaves the row at last");
    assert!(
        seen.iter().all(|&s| s),
        "every line of the row shows on the way down"
    );
    seen.fill(false);
    let mut mark = |app: &App| {
        let top = app.views[0].top;
        for line in (top..top + height).filter(|l| (start..end).contains(l)) {
            seen[line - start] = true;
        }
    };
    mark(&app);
    for _ in 0..(end - start) {
        if cursor(&app) < tall {
            break;
        }
        ctrl(&mut app, 'u');
        mark(&app);
    }
    assert!(cursor(&app) < tall);
    assert!(seen.iter().all(|&s| s), "and on the way up");
}

#[test]
fn the_cursor_finds_its_own_hunk_line_again() {
    let mut app = web_a_detail();
    // Both hunks start `@@ -1,1 +1,1 @@` at the same indent.
    for path in ["nix.conf", "app.service"] {
        let at = app.rows().iter().position(|r| r.text().contains(path));
        app.views[app.selected].cursor = at.unwrap();
        press(&mut app, "l");
    }
    press(&mut app, "j");
    let on_app_service_hunk = |app: &App| {
        let rows = app.rows();
        let row = &rows[cursor(app)];
        row.text().trim() == "@@ -1,1 +1,1 @@"
            && rows[row.parent.unwrap()].text().contains("app.service")
    };
    assert!(on_app_service_hunk(&app));
    press(&mut app, "/etc\n");
    assert!(
        !shown(&app).iter().any(|r| r.starts_with("Packages")),
        "rows shifted"
    );
    assert!(on_app_service_hunk(&app));
    press(&mut app, "t");
    assert!(
        on_app_service_hunk(&app),
        "the tree view changed its indent"
    );

    // Alike rows in one file: the same occurrence.
    let brace = vec![line(LineOp::Added, "}"), line(LineOp::Added, "}")];
    let file = modified("etc/a", &[], ChurnClass::Real, brace);
    let mut app = App::new(Mode::Deployed, vec!["web-a".into()], Options::default());
    app.arrive(done("web-a", '1', vec![], &[], vec![file]));
    let braces = |app: &App| -> Vec<usize> {
        let rows = app.rows();
        (0..rows.len())
            .filter(|&i| rows[i].text().trim() == "+}")
            .collect()
    };
    let at = app.rows().iter().position(|r| r.text().contains("[M] a"));
    app.views[0].cursor = at.unwrap();
    press(&mut app, "\tl");
    app.views[0].cursor = braces(&app)[1];
    press(&mut app, "t");
    assert_eq!(cursor(&app), braces(&app)[1]);
}

static RESTORED: AtomicUsize = AtomicUsize::new(0);

fn count_restore() {
    RESTORED.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn other_threads_panics_wait_for_the_terminal_and_the_hook_goes_away() {
    let guard = Guard::install(count_restore);
    let held = Arc::clone(&guard.held);
    let worker = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| panic!("boom"))
        .unwrap();
    assert!(worker.join().is_err());
    assert_eq!(
        RESTORED.load(Ordering::SeqCst),
        0,
        "the viewer keeps the terminal"
    );
    let messages = lock(&held).clone().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("thread 'worker' panicked at") && m.ends_with("boom")),
        "{messages:?}"
    );
    // A panic on the viewer's thread restores the terminal first, then passes everything on.
    assert!(std::panic::catch_unwind(|| panic!("viewer")).is_err());
    assert_eq!(RESTORED.load(Ordering::SeqCst), 1);
    assert!(lock(&held).is_none());
    drop(guard);
    assert_eq!(RESTORED.load(Ordering::SeqCst), 2);
    assert_eq!(Arc::strong_count(&held), 1, "the hook is removed");
}
