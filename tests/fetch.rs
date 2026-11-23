//! The fetch pipeline end to end on the synthetic store: assertions on the resulting
//! `report::Configuration`, and on round trips per machine against the budget of spec 6.1.

use std::sync::Arc;
use std::time::Duration;

use scry::fetch::{self, BuildStep, Job, Toplevel, Wanted};
use scry::fixture::{self, Fleet, Host, current_system, nixos, store_path as sp};
use scry::report::{ClosureKind, Configuration, FileStatus, Source, Status};
use scry::transport::machine::{Machine, Remote};

const MAX: u64 = 4096;

fn remote(target: &str) -> Machine {
    Machine::Remote(Remote {
        target: target.parse().unwrap(),
        port: None,
        ssh_options: vec![],
    })
}

fn web_a() -> Machine {
    remote("root@web-a")
}

fn built(machine: Machine, path: &str) -> Wanted {
    Wanted {
        machine,
        toplevel: Toplevel::Path {
            path: path.to_owned(),
            build: Some(BuildStep::Ran),
        },
    }
}

fn current(machine: Machine, kind: ClosureKind) -> Wanted {
    Wanted {
        machine,
        toplevel: Toplevel::Current(kind),
    }
}

fn job(old: Wanted, new: Wanted) -> Job {
    Job {
        name: "web-a".to_owned(),
        source: Source::Colmena,
        old,
        new,
        max_file_size: MAX,
        timeout: Duration::from_secs(60),
    }
}

async fn run(fleet: &Arc<Fleet>, job: &Job) -> Configuration {
    let fake = fleet.fake();
    fetch::configuration(&fake, job).await
}

fn error(configuration: &Configuration) -> &str {
    match &configuration.status {
        Status::Error { error } => error,
        other => panic!("not an error: {other:?}"),
    }
}

fn changes(configuration: &Configuration) -> Vec<String> {
    let Status::Done(sections) = &configuration.status else {
        panic!("not done: {:?}", configuration.status);
    };
    sections
        .files
        .changes
        .iter()
        .map(|c| {
            let via: String = c.via.iter().map(|v| format!(" → {v}")).collect();
            format!("{:?} {:?} {}{via}", c.status, c.class, c.path)
        })
        .collect()
}

/// Old side: the current system on `web-a`. New side: built here.
fn two_machines() -> (Arc<Fleet>, Job) {
    let (mut target, mut local) = (Host::new(), Host::new());
    let old = nixos(&mut target, true);
    current_system(&mut target, &old);
    let new = nixos(&mut local, false);
    let fleet = Fleet::new()
        .host(web_a(), target)
        .host(Machine::Local, local);
    let job = job(
        current(web_a(), ClosureKind::Nixos),
        built(Machine::Local, &new),
    );
    (Arc::new(fleet), job)
}

#[tokio::test]
async fn nixos_bump_on_two_machines() {
    let (fleet, job) = two_machines();
    let configuration = run(&fleet, &job).await;
    insta::assert_json_snapshot!(configuration);
    // probe (with the first level) + 3 link levels + content + 2 reference levels: exactly the
    // budget of 2 + link depth 3 + reference depth 2.
    assert_eq!(fleet.round_trips(&web_a()), 7);
    assert_eq!(fleet.round_trips(&Machine::Local), 7);
}

#[tokio::test]
async fn both_sides_on_one_machine_share_round_trips() {
    let mut host = Host::new();
    let old = nixos(&mut host, true);
    let new = nixos(&mut host, false);
    let fleet = Arc::new(Fleet::new().host(web_a(), host));
    let path = |path: &str| Wanted {
        machine: web_a(),
        toplevel: Toplevel::Path {
            path: path.to_owned(),
            build: None,
        },
    };
    let one = run(&fleet, &job(path(&old), path(&new))).await;
    assert_eq!(fleet.round_trips(&web_a()), 7);

    let (fleet, job) = two_machines();
    let two = run(&fleet, &job).await;
    assert_eq!(one.status, two.status);
    assert_eq!(one.kind, ClosureKind::Nixos);
    assert_eq!(one.old.unwrap().location, "root@web-a");
    assert_eq!(one.new.unwrap().location, "root@web-a");
}

#[tokio::test]
async fn churn_references_and_small_rebuilds() {
    let (fleet, job) = two_machines();
    let configuration = run(&fleet, &job).await;
    // Display order: by path, followed references right after their referrer; the small
    // rebuild `app-helper` on its own; `app-start` only under the unit that points at it, its
    // binary dropped; `etc/profiles/per-user` and the `sw` tree not followed. `boot.json`,
    // `etc/issue` and `nixos-version` differ only by the system, revision and kernel versions.
    assert_eq!(
        changes(&configuration),
        [
            "Modified VersionOnly activate",
            "Modified Real app-helper/bin/app-helper",
            "Modified VersionOnly boot.json",
            "Modified Real etc/binary.db",
            "Modified VersionOnly etc/issue",
            "LinkChanged Real etc/mtab",
            "Modified Real etc/nix/nix.conf",
            "Modified HashOnly etc/profile",
            "TypeChanged Real etc/resolv.conf",
            "Added Real etc/ssh/authorized_keys.d/deploy",
            "Modified VersionOnly etc/ssl/openssl.cnf",
            "Modified Real etc/systemd/system-generators/dev-disk-by\\x2dlabel.mount",
            "Modified HashOnly etc/systemd/system/app.service",
            "Modified Real etc/systemd/system/app.service → app-start/bin/app-start",
            "Modified Real etc/systemd/system/app.service → app-start/bin/app-start → app.json",
            "TypeChanged Real etc/systemd/system/debug-shell.service",
            "Modified Real etc/udev/hwdb.bin",
            "Modified VersionOnly nixos-version/bin/nixos-version",
            "Modified Real user-environment/bin/tool",
        ]
    );
}

#[tokio::test]
async fn no_changes_costs_only_the_probe() {
    let mut host = Host::new();
    let t = nixos(&mut host, true);
    current_system(&mut host, &t);
    let fleet = Arc::new(Fleet::new().host(web_a(), host));
    let configuration = run(
        &fleet,
        &job(current(web_a(), ClosureKind::Nixos), built(web_a(), &t)),
    )
    .await;
    assert_eq!(configuration.status, Status::NoChanges);
    assert_eq!(configuration.old.unwrap().path, t);
    assert_eq!(fleet.round_trips(&web_a()), 1);
}

#[tokio::test]
async fn missing_toplevel_names_the_build_step() {
    let mut target = Host::new();
    let old = nixos(&mut target, true);
    current_system(&mut target, &old);
    let fleet = Arc::new(
        Fleet::new()
            .host(web_a(), target)
            .host(Machine::Local, Host::new()),
    );
    let missing = sp("n", "nixos-system-web-a-26.11");
    for (build, suffix) in [
        (Some(BuildStep::Ran), "; the build did not produce it"),
        (Some(BuildStep::Skipped), "; run without --no-build"),
        (None, ""),
    ] {
        let new = Wanted {
            machine: Machine::Local,
            toplevel: Toplevel::Path {
                path: missing.clone(),
                build,
            },
        };
        let configuration = run(&fleet, &job(current(web_a(), ClosureKind::Nixos), new)).await;
        assert_eq!(
            error(&configuration),
            format!("{missing} is not in the store on this machine{suffix}")
        );
        assert_eq!(configuration.kind, ClosureKind::Unknown);
        assert_eq!(configuration.new.unwrap().path, missing);
        // The old side resolved on its own machine, so the report keeps it.
        assert_eq!(configuration.old.unwrap().path, old);
        assert_eq!(configuration.target.as_deref(), Some("root@web-a"));
    }
}

#[tokio::test]
async fn missing_current_system() {
    let mut local = Host::new();
    let new = nixos(&mut local, false);
    let fleet = Arc::new(
        Fleet::new()
            .host(web_a(), Host::new())
            .host(Machine::Local, local),
    );
    let configuration = run(
        &fleet,
        &job(
            current(web_a(), ClosureKind::Nixos),
            built(Machine::Local, &new),
        ),
    )
    .await;
    assert_eq!(
        error(&configuration),
        "no current system at /run/current-system on root@web-a"
    );
    // The new side resolved first: its kind and version are known, the old side is not.
    assert_eq!(configuration.kind, ClosureKind::Nixos);
    assert_eq!(
        configuration.new.unwrap().system_version.as_deref(),
        Some("26.11.20261110.9a8b7c6")
    );
    assert_eq!(configuration.old, None);
}

#[tokio::test]
async fn unreachable_target_and_missing_nix() {
    let mut local = Host::new();
    let new = nixos(&mut local, false);
    let mut down = Host::new();
    down.unreachable = Some("connect to host web-b port 22: Connection refused".to_owned());
    let mut no_nix = Host::new();
    let old = nixos(&mut no_nix, true);
    current_system(&mut no_nix, &old);
    no_nix.nix = false;
    let fleet = Arc::new(
        Fleet::new()
            .host(remote("root@web-b"), down)
            .host(web_a(), no_nix)
            .host(Machine::Local, local),
    );
    let configuration = run(
        &fleet,
        &job(
            current(remote("root@web-b"), ClosureKind::Nixos),
            built(Machine::Local, &new),
        ),
    )
    .await;
    assert_eq!(
        error(&configuration),
        "ssh root@web-b: connect to host web-b port 22: Connection refused"
    );
    assert_eq!(configuration.old, None);
    assert_eq!(configuration.target.as_deref(), Some("root@web-b"));

    let configuration = run(
        &fleet,
        &job(
            current(web_a(), ClosureKind::Nixos),
            built(Machine::Local, &new),
        ),
    )
    .await;
    assert_eq!(error(&configuration), "`nix` not found on root@web-a");
}

#[tokio::test]
async fn failing_find_fails_the_configuration() {
    let (mut target, mut local) = (Host::new(), Host::new());
    let old = nixos(&mut target, true);
    current_system(&mut target, &old);
    let new = nixos(&mut local, false);
    let locked = format!("{}/etc/locked", sp("n", "etc"));
    local.unreadable(&locked);
    let fleet = Arc::new(
        Fleet::new()
            .host(web_a(), target)
            .host(Machine::Local, local),
    );
    let configuration = run(
        &fleet,
        &job(
            current(web_a(), ClosureKind::Nixos),
            built(Machine::Local, &new),
        ),
    )
    .await;
    assert_eq!(
        error(&configuration),
        format!("`find` failed (exit 1): find: {locked}: Permission denied")
    );
    assert_eq!(configuration.kind, ClosureKind::Nixos);
}

#[tokio::test]
async fn kinds_differ() {
    let mut host = Host::new();
    let old = nixos(&mut host, true);
    current_system(&mut host, &old);
    let darwin = sp("n", "darwin-system-26.11.ff0e21a");
    host.file(&format!("{darwin}/darwin-version"), "26.11.ff0e21a\n");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, host));
    let configuration = run(
        &fleet,
        &job(
            current(Machine::Local, ClosureKind::NixDarwin),
            built(Machine::Local, &darwin),
        ),
    )
    .await;
    assert_eq!(
        error(&configuration),
        "closure kinds differ: old is nixos, new is nix-darwin"
    );
    assert_eq!(configuration.kind, ClosureKind::NixDarwin);
    assert_eq!(configuration.target, None);
}

/// A toplevel without a marker: Packages and its own regular files only, no followed trees.
#[tokio::test]
async fn unknown_kind() {
    let mut host = Host::new();
    let (old, new) = (
        fixture::unknown(&mut host, "o", "hi\n"),
        fixture::unknown(&mut host, "n", "hello\n"),
    );
    let fleet = Arc::new(Fleet::new().host(Machine::Local, host));
    let path = |path: &str| Wanted {
        machine: Machine::Local,
        toplevel: Toplevel::Path {
            path: path.to_owned(),
            build: None,
        },
    };
    let configuration = run(&fleet, &job(path(&old), path(&new))).await;
    insta::assert_json_snapshot!(configuration);
    // probe + content + one reference level for the small rebuild `tool`: 2 + 0 + 1.
    assert_eq!(fleet.round_trips(&Machine::Local), 3);
}

/// nix-darwin: launchd plists under `Library` and `user/Library`, `Applications` not followed.
#[tokio::test]
async fn nix_darwin() {
    let mut host = Host::new();
    let (old, new) = (
        fixture::nix_darwin(&mut host, "o", "3600"),
        fixture::nix_darwin(&mut host, "n", "7200"),
    );
    current_system(&mut host, &old);
    let fleet = Arc::new(Fleet::new().host(Machine::Local, host));
    let configuration = run(
        &fleet,
        &job(
            current(Machine::Local, ClosureKind::NixDarwin),
            built(Machine::Local, &new),
        ),
    )
    .await;
    insta::assert_json_snapshot!(configuration);
}

/// home-manager: `home-files` and `LaunchAgents` followed, the current system from the target's
/// `XDG_STATE_HOME`, or the per-user profile when that is missing.
#[tokio::test]
async fn home_manager() {
    let db_a = remote("alice@db-a");
    for per_user in [false, true] {
        let mut target = Host::new();
        target.home = "/home/alice".to_owned();
        target.user = "alice".to_owned();
        target.xdg_state_home = Some("/home/alice/.state".to_owned());
        let old = fixture::home_manager(&mut target, "o", "alice@example.org");
        let profile = if per_user {
            "/nix/var/nix/profiles/per-user/alice/home-manager"
        } else {
            "/home/alice/.state/nix/profiles/home-manager"
        };
        target.link(profile, &old);
        let mut local = Host::new();
        let new = fixture::home_manager(&mut local, "n", "alice@example.com");
        let fleet = Arc::new(
            Fleet::new()
                .host(db_a.clone(), target)
                .host(Machine::Local, local),
        );
        let configuration = run(
            &fleet,
            &job(
                current(db_a.clone(), ClosureKind::HomeManager),
                built(Machine::Local, &new),
            ),
        )
        .await;
        assert_eq!(configuration.old.as_ref().unwrap().path, old);
        insta::assert_json_snapshot!(configuration);
    }
    // Neither profile: the error names the XDG one.
    let mut target = Host::new();
    target.xdg_state_home = Some("/home/alice/.state".to_owned());
    let mut local = Host::new();
    let new = fixture::home_manager(&mut local, "n", "alice@example.com");
    let fleet = Arc::new(
        Fleet::new()
            .host(db_a.clone(), target)
            .host(Machine::Local, local),
    );
    let configuration = run(
        &fleet,
        &job(
            current(db_a.clone(), ClosureKind::HomeManager),
            built(Machine::Local, &new),
        ),
    )
    .await;
    assert_eq!(
        error(&configuration),
        "no current system at /home/alice/.state/nix/profiles/home-manager on alice@db-a"
    );
}

/// home-manager's launchd domains, restart triggers and `putter.json` (a link to one store
/// file) are followed too: a change there shows in Files.
#[tokio::test]
async fn home_manager_agent_domains_and_putter() {
    let mut local = Host::new();
    let mut generation = |seed: &str, domain: &str, target: &str| {
        let t = fixture::home_manager(&mut local, seed, "alice@example.org");
        let domains = sp(seed, "home-manager-agent-domains");
        let triggers = sp(seed, "home-manager-agent-restart-triggers");
        let putter = sp(seed, "hm-putter.json");
        local
            .file(&format!("{domains}/org.example.agent.domain"), domain)
            .dir(&triggers)
            .file(&putter, format!("{{\"target\":\"{target}\"}}\n"))
            .link(&format!("{t}/LaunchAgentDomains"), &domains)
            .link(&format!("{t}/LaunchAgentRestartTriggers"), &triggers)
            .link(&format!("{t}/putter.json"), &putter);
        t
    };
    let old = generation("o", "gui\n", "/home/alice/.a");
    let new = generation("n", "user\n", "/home/alice/.b");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let configuration = run(
        &fleet,
        &job(built(Machine::Local, &old), built(Machine::Local, &new)),
    )
    .await;
    let changes = changes(&configuration);
    for path in ["LaunchAgentDomains/org.example.agent.domain", "putter.json"] {
        assert!(
            changes.contains(&format!("Modified Real {path}")),
            "{path} missing: {changes:#?}"
        );
    }
}

#[tokio::test]
async fn binary_and_over_limit_files() {
    let (fleet, job) = two_machines();
    let configuration = run(&fleet, &job).await;
    let Status::Done(sections) = &configuration.status else {
        panic!()
    };
    let by_path = |p: &str| {
        sections
            .files
            .changes
            .iter()
            .find(|c| c.path == p && c.via.is_empty())
            .unwrap()
    };
    let binary = by_path("etc/binary.db");
    assert!(binary.binary && binary.content_fetched && binary.hunks.is_empty());
    let hwdb = by_path("etc/udev/hwdb.bin");
    assert!(!hwdb.content_fetched && !hwdb.binary);
    assert_eq!((hwdb.size_old, hwdb.size_new), (Some(5000), Some(5000)));
    assert_eq!(by_path("etc/resolv.conf").status, FileStatus::TypeChanged);
    // GNU tar would unescape the backslash in its file list: not fetched rather than wrong.
    let mount = by_path("etc/systemd/system-generators/dev-disk-by\\x2dlabel.mount");
    assert!(!mount.content_fetched && mount.hunks.is_empty());
}

/// Never answers on one machine, like a target that hangs.
struct Hang {
    fake: scry::exec::fake::Fake,
    machine: Machine,
}

impl scry::exec::Exec for Hang {
    fn run<'a>(
        &'a self,
        machine: &'a Machine,
        command: scry::exec::Command,
    ) -> scry::exec::BoxFuture<'a, Result<scry::exec::Output, scry::exec::ExecError>> {
        if *machine == self.machine {
            Box::pin(std::future::pending())
        } else {
            self.fake.run(machine, command)
        }
    }

    fn shutdown(&self) -> scry::exec::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_keeps_what_is_known() {
    let mut local = Host::new();
    let new = nixos(&mut local, false);
    let web_b = remote("root@web-b");
    let fleet = Arc::new(
        Fleet::new()
            .host(web_b.clone(), Host::new())
            .host(Machine::Local, local),
    );
    let hang = Hang {
        fake: fleet.fake(),
        machine: web_b.clone(),
    };
    let job = job(
        current(web_b, ClosureKind::Nixos),
        built(Machine::Local, &new),
    );
    let configuration = fetch::configuration(&hang, &job).await;
    assert_eq!(
        configuration.status,
        Status::Timeout {
            error: "timeout after 60 s".to_owned()
        }
    );
    // The new side's probe returned before the old one timed out.
    assert_eq!(configuration.kind, ClosureKind::Nixos);
    let side = configuration.new.unwrap();
    assert_eq!(
        side.system_version.as_deref(),
        Some("26.11.20261110.9a8b7c6")
    );
    assert_eq!(configuration.old, None);
    assert_eq!(configuration.target.as_deref(), Some("root@web-b"));
}
