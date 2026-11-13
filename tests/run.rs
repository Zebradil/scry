//! Whole runs on the fake seam: canned `colmena eval`, `nix eval`, `nix build`, `git` and
//! `hostname` output, the synthetic fleet for fetching, and assertions on the rendered report
//! and the exit codes with and without `--exit-code`.

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use scry::cli::Cli;
use scry::exec::Output;
use scry::exec::fake::{Fake, exit, ok};
use scry::exit::Outcome;
use scry::fetch;
use scry::fixture::{self, Fleet, Host, current_system, nixos};
use scry::report::{Configuration, Report};
use scry::run::{self, Env};
use scry::source::LOCATE;
use scry::source::colmena::IN_DIR;
use scry::transport::machine::{Machine, Remote};
use scry::tui::{self, Finished};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;

const BASE: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
const HEAD: &str = "9f8e7d6c5b4a39281706f5e4d3c2b1a098f7e6d5";

struct Ran {
    stdout: String,
    stderr: String,
    result: anyhow::Result<Outcome>,
    /// The run's temporary directories still on disk.
    left: usize,
}

impl Ran {
    /// The exit codes without and with `--exit-code`.
    fn codes(&self) -> (u8, u8) {
        let outcome = match &self.result {
            Ok(outcome) => *outcome,
            Err(error) => Outcome::of_error(error),
        };
        (outcome.exit_code(false), outcome.exit_code(true))
    }

    /// `scry: <message>` as `run::main` prints it.
    fn error(&self) -> String {
        format!("{:#}", self.result.as_ref().unwrap_err())
    }
}

fn env(temp: &TempDir) -> Env {
    Env {
        cwd: PathBuf::from("/repo"),
        user: Some("alice".to_owned()),
        macos: false,
        no_color: None,
        stdout_is_terminal: false,
        temp_dir: temp.path().to_owned(),
        viewer: scry::tui::run,
    }
}

async fn scry(fake: Fake, env: Env, args: &[&str]) -> Ran {
    scry_until(fake, env, args, std::future::pending()).await
}

async fn scry_until(
    fake: Fake,
    env: Env,
    args: &[&str],
    interrupt: impl Future<Output = ()>,
) -> Ran {
    let cli = Cli::try_parse_from(std::iter::once("scry").chain(args.iter().copied())).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = run::run(
        Arc::new(fake),
        &cli.command,
        &env,
        &mut out,
        &mut err,
        interrupt,
    )
    .await;
    Ran {
        stdout: String::from_utf8(out).unwrap(),
        stderr: String::from_utf8(err).unwrap(),
        result,
        left: std::fs::read_dir(&env.temp_dir).unwrap().count(),
    }
}

fn remote(target: &str) -> Machine {
    Machine::Remote(Remote {
        target: target.parse().unwrap(),
        port: None,
        ssh_options: vec![],
    })
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let rest = &text[text.find(start).unwrap() + start.len()..];
    &rest[..rest.find(end).unwrap()]
}

/// An evaluated toplevel: its `outPath` and a `drvPath`.
fn pair(out: &str) -> Output {
    ok(serde_json::json!([out, format!("{out}.drv")]).to_string())
}

fn missing_attribute(name: &str) -> Output {
    exit(
        1,
        format!(
            "error:\n       … while calling the 'toJSON' builtin\n\n       error: attribute '{name}' missing\n"
        ),
    )
}

/// Whether `dir` is in the base worktree rather than the working tree.
fn at_base(dir: &str) -> bool {
    dir.contains("/worktree")
}

/// The command is [`LOCATE`] from `dir` up to `top`, for `files`.
fn located(c: &scry::exec::Command) -> Option<(&str, &str, Vec<&str>)> {
    let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
    let ["-c", script, "sh", dir, top, ref files @ ..] = args[..] else {
        return None;
    };
    (c.program == "sh" && script == LOCATE.text()).then(|| (dir, top, files.to_vec()))
}

/// colmena in `/repo` or, with `base`, in the base worktree: step 1 answers `deployment`,
/// step 2 the toplevel of each node in `toplevels` (any other node fails to evaluate), and
/// `colmena build` succeeds. Head's hive is `/repo/hive.nix`.
fn hive(
    fake: Fake,
    base: bool,
    deployment: &'static str,
    toplevels: Vec<(&'static str, String)>,
) -> Fake {
    let fake = fake.rule(move |_, c| {
        let (_, top, _) = located(c)?;
        (!base && top == "/").then(|| ok("/repo/hive.nix"))
    });
    fake.rule(move |_, c| {
        let is_colmena = c.program == "sh"
            && c.args.get(1).map(String::as_str) == Some(IN_DIR.text())
            && c.args.get(4).map(String::as_str) == Some("colmena");
        if !is_colmena || at_base(&c.args[3]) != base {
            return None;
        }
        let args: Vec<&str> = c.args[5..].iter().map(String::as_str).collect();
        match args[..] {
            ["eval", "-E", e] if e.contains("buildOnTarget") => Some(ok(deployment)),
            ["eval", "-E", e] => {
                let name = between(e, "nodes.\"", "\"");
                let toplevel = toplevels.iter().find(|(n, _)| *n == name);
                Some(toplevel.map_or_else(|| missing_attribute(name), |(_, out)| pair(out)))
            }
            ["build", "--on", _] => Some(ok("")),
            _ => None,
        }
    })
}

/// One attribute set of a flake: each name with its toplevel, `None` when it fails to
/// evaluate.
type Set = (&'static str, Vec<(&'static str, Option<String>)>);

/// The flake of the working tree or, with `base`, of the base worktree. An attribute set not
/// in `sets` is missing.
fn flake(fake: Fake, base: bool, sets: Vec<Set>) -> Fake {
    fake.rule(move |_, c| {
        let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
        let [
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--json",
            "--no-write-lock-file",
            installable,
            "--apply",
            apply,
        ] = args[..]
        else {
            return None;
        };
        let (reference, set) = installable.split_once('#')?;
        if c.program != "nix" || at_base(reference) != base {
            return None;
        }
        let Some((_, names)) = sets.iter().find(|(s, _)| *s == set) else {
            return Some(exit(
                1,
                format!("error: flake 'path:{reference}' does not provide attribute '{set}'\n"),
            ));
        };
        if apply == "builtins.attrNames" {
            let names: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
            return Some(ok(serde_json::to_string(&names).unwrap()));
        }
        let name = between(apply, "c.\"", "\"");
        Some(match names.iter().find(|(n, _)| *n == name) {
            Some((_, Some(out))) => pair(out),
            Some((_, None)) => exit(
                1,
                "error:\n       … while evaluating\n\n       error: infinite recursion encountered\n",
            ),
            None => missing_attribute(name),
        })
    })
}

/// `nix build` succeeds, except into the out-links named in `failing` (`new`, `old`).
fn builds(fake: Fake, failing: &'static [&'static str]) -> Fake {
    fake.rule(move |_, c| {
        let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
        let [
            "--extra-experimental-features",
            "nix-command",
            "build",
            "--keep-going",
            "--out-link",
            link,
            ref drvs @ ..,
        ] = args[..]
        else {
            return None;
        };
        let built = c.program == "nix" && drvs.iter().all(|d| d.ends_with(".drv^out"));
        let fails = failing.iter().any(|f| link.ends_with(&format!("/{f}")));
        built.then(|| if fails { exit(1, "") } else { ok("") })
    })
}

fn hosts(fake: Fake, hostname: &str, local_host_name: Option<&str>) -> Fake {
    let scutil = match local_host_name {
        Some(name) => ok(format!("{name}\n")),
        None => exit(1, "LocalHostName: not set\n"),
    };
    fake.on(
        Machine::Local,
        "hostname",
        &["-s"],
        ok(format!("{hostname}\n")),
    )
    .on(
        Machine::Local,
        "scutil",
        &["--get", "LocalHostName"],
        scutil,
    )
}

/// The checkout `/repo` the base worktree is added to.
#[derive(Default)]
struct Repository {
    worktrees: Mutex<HashSet<String>>,
    /// The directory of head's flake or hive relative to the checkout: `""` or `"/sub"`.
    source: &'static str,
    /// Base has a flake or hive there.
    source_at_base: bool,
    /// `git worktree add` dies before writing the worktree's `.git`: the entry stays locked,
    /// `remove` fails validation, and `prune` skips it until it is unlocked.
    stuck: bool,
    locked: Mutex<HashSet<String>>,
}

impl Repository {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            source_at_base: true,
            ..Self::default()
        })
    }
}

/// `git` on the checkout, whose `main` is [`BASE`] and `HEAD` [`HEAD`], and whether base has
/// a source.
fn git(fake: Fake, repository: &Arc<Repository>, dirty: bool) -> Fake {
    let (repository, present) = (Arc::clone(repository), Arc::clone(repository));
    fake.rule(move |_, c| {
        let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
        let ["-C", _, ref args @ ..] = args[..] else {
            return None;
        };
        if c.program != "git" {
            return None;
        }
        let mut worktrees = repository.worktrees.lock().unwrap();
        let mut locked = repository.locked.lock().unwrap();
        Some(match args {
            ["rev-parse", "--show-toplevel"] => ok("/repo\n"),
            ["rev-parse", "--verify", "--end-of-options", "main^{commit}"] => {
                ok(format!("{BASE}\n"))
            }
            ["rev-parse", "--verify", "--end-of-options", _] => {
                exit(128, "fatal: Needed a single revision\n")
            }
            ["rev-parse", "HEAD"] => ok(format!("{HEAD}\n")),
            ["status", "--porcelain", "--untracked-files=no"] => {
                ok(if dirty { " M flake.nix\n" } else { "" })
            }
            ["worktree", "add", "--detach", path, BASE] if worktrees.insert(path.to_string()) => {
                if repository.stuck {
                    locked.insert(path.to_string());
                }
                ok("")
            }
            ["worktree", "remove", ..] if repository.stuck => exit(
                128,
                "fatal: validation failed, cannot remove working tree\n",
            ),
            ["worktree", "remove", "--force", "--force", path] if worktrees.remove(*path) => ok(""),
            ["worktree", "unlock", path] if locked.remove(*path) => ok(""),
            ["worktree", "prune"] => {
                worktrees.retain(|w| locked.contains(w));
                ok("")
            }
            _ => return None,
        })
    })
    .rule(move |_, c| {
        let (dir, top, files) = located(c)?;
        let file = files.first()?;
        if at_base(dir) {
            let found = present.source_at_base
                && dir == top
                && dir.ends_with(&format!("/worktree{}", present.source));
            return Some(if found {
                ok(format!("/{file}"))
            } else {
                exit(1, "")
            });
        }
        (top == "/repo").then(|| ok(format!("{}/{file}", present.source)))
    })
}

const HIVE: &str = r#"{
  "db-a": {"buildOnTarget": false, "sshOptions": [], "tags": ["db"], "targetHost": null, "targetPort": null, "targetUser": "root"},
  "mail-a": {"buildOnTarget": false, "sshOptions": [], "tags": ["mail"], "targetHost": "mail-a", "targetPort": null, "targetUser": "root"},
  "web-a": {"buildOnTarget": false, "sshOptions": [], "tags": ["web"], "targetHost": "web-a", "targetPort": null, "targetUser": "root"},
  "web-b": {"buildOnTarget": true, "sshOptions": ["-o", "ConnectTimeout=5"], "tags": ["web"], "targetHost": "web-b", "targetPort": 2222, "targetUser": "root"},
  "web-c": {"buildOnTarget": false, "sshOptions": [], "tags": ["web", "eu"], "targetHost": "web-c", "targetPort": null, "targetUser": "root"}
}"#;

fn web_b() -> Machine {
    Machine::Remote(Remote {
        target: "root@web-b".parse().unwrap(),
        port: Some(2222),
        ssh_options: vec!["-o".to_owned(), "ConnectTimeout=5".to_owned()],
    })
}

/// `db-a` already runs its new toplevel here; `web-a`'s old side is on its target and its
/// new side here; `web-b` builds on its target, so both sides are there; `web-c` is
/// unreachable.
fn colmena_fleet() -> (Fake, String) {
    let mut local = Host::new();
    let new = nixos(&mut local, false);
    current_system(&mut local, &new);
    let mut web_a = Host::new();
    let old = nixos(&mut web_a, true);
    current_system(&mut web_a, &old);
    let mut web_b_host = Host::new();
    nixos(&mut web_b_host, false);
    let old_b = nixos(&mut web_b_host, true);
    current_system(&mut web_b_host, &old_b);
    let mut web_c = Host::new();
    web_c.unreachable = Some("connect to host web-c port 22: Connection refused".to_owned());
    let fleet = Arc::new(
        Fleet::new()
            .host(Machine::Local, local)
            .host(remote("root@web-a"), web_a)
            .host(web_b(), web_b_host)
            .host(remote("root@web-c"), web_c),
    );
    let fake = fleet.fake().on(
        Machine::Local,
        "colmena",
        &["--version"],
        ok("colmena 0.5.0\n"),
    );
    (fake, new)
}

#[tokio::test]
async fn colmena_with_tags_and_build_on_target() {
    let temp = TempDir::new().unwrap();
    let (fake, new) = colmena_fleet();
    let toplevels = ["db-a", "mail-a", "web-a", "web-b", "web-c"]
        .into_iter()
        .map(|name| (name, new.clone()))
        .collect();
    let fake = hive(fake, false, HIVE, toplevels);
    let args = ["colmena", "--on", "@web,db-a", "--max-file-size", "4096"];
    let ran = scry(fake, env(&temp), &args).await;
    insta::assert_snapshot!(ran.stdout);
    assert_eq!(ran.codes(), (1, 2));
    assert_eq!(ran.left, 0);

    // Where each side lived (spec 4.3).
    let (fake, new) = colmena_fleet();
    let toplevels = vec![
        ("db-a", new.clone()),
        ("web-a", new.clone()),
        ("web-b", new),
    ];
    let fake = hive(fake, false, HIVE, toplevels);
    let args = ["colmena", "--on", "db-a,web-a,web-b", "--format", "json"];
    let ran = scry(fake, env(&temp), &args).await;
    let report: serde_json::Value = serde_json::from_str(&ran.stdout).unwrap();
    let sides: Vec<(&str, &str, &str)> = report["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let location = |side: &str| c[side]["location"].as_str().unwrap();
            (
                c["name"].as_str().unwrap(),
                location("old"),
                location("new"),
            )
        })
        .collect();
    assert_eq!(
        sides,
        [
            ("db-a", "local", "local"),
            ("web-a", "root@web-a", "local"),
            ("web-b", "root@web-b", "root@web-b"),
        ]
    );
}

#[tokio::test]
async fn colmena_evaluation_failure_fails_only_its_node() {
    let temp = TempDir::new().unwrap();
    let (fake, new) = colmena_fleet();
    let fake = hive(fake, false, HIVE, vec![("db-a", new.clone())]);
    let ran = scry(fake, env(&temp), &["colmena", "--on", "db-a,web-a"]).await;
    assert_eq!(
        ran.stdout,
        format!(
            "db-a · nixos (local)\n  no changes, already running {new}\n\n\
             web-a · unknown (root@web-a)\n  \
             error: `colmena` failed (exit 1): error: attribute 'web-a' missing\n"
        )
    );
    assert_eq!(ran.codes(), (1, 2));
}

#[tokio::test]
async fn colmena_selection_and_version_errors() {
    let temp = TempDir::new().unwrap();
    let (fake, _) = colmena_fleet();
    let fake = hive(fake, false, HIVE, vec![]);
    let ran = scry(fake, env(&temp), &["colmena", "--on", "web-*,@dbx"]).await;
    assert_eq!(ran.error(), "--on @dbx: matches no configuration");
    assert_eq!(ran.codes(), (2, 2));
    assert_eq!(ran.stdout, "");

    let missing = Fake::new().on(
        Machine::Local,
        "colmena",
        &["--version"],
        exit(127, "sh: 1: colmena: not found\n"),
    );
    let ran = scry(missing, env(&temp), &["colmena"]).await;
    assert_eq!(ran.error(), "colmena not found on PATH");
    assert_eq!(ran.codes(), (1, 2));
    assert_eq!(ran.left, 0);
}

/// `web-a` (NixOS) and `alice@web-a` (home-manager) belong to this machine, the bare `alice`
/// does not since `alice@web-a` exists, and `bob@web-a` and `web-b` are other machines'.
fn this_machine() -> (Fake, Vec<Set>) {
    let mut local = Host::new();
    local.home = "/home/alice".to_owned();
    local.user = "alice".to_owned();
    let old = nixos(&mut local, true);
    current_system(&mut local, &old);
    let new = nixos(&mut local, false);
    let old_hm = fixture::home_manager(&mut local, "o", "alice@example.org");
    local.link(
        "/home/alice/.local/state/nix/profiles/home-manager",
        &old_hm,
    );
    let new_hm = fixture::home_manager(&mut local, "n", "alice@example.com");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let sets = vec![
        (
            "nixosConfigurations",
            vec![("web-a", Some(new.clone())), ("web-b", Some(new))],
        ),
        (
            "homeConfigurations",
            vec![
                ("alice", Some(new_hm.clone())),
                ("alice@web-a", Some(new_hm.clone())),
                ("bob@web-a", Some(new_hm)),
            ],
        ),
    ];
    (fleet.fake(), sets)
}

#[tokio::test]
async fn flake_default_selection_on_this_host() {
    let temp = TempDir::new().unwrap();
    let (fake, sets) = this_machine();
    let fake = builds(flake(hosts(fake, "web-a", None), false, sets), &[]);
    let ran = scry(
        fake,
        env(&temp),
        &[
            "flake",
            "--target",
            "web-b=root@web-b",
            "--max-file-size",
            "4096",
        ],
    )
    .await;
    insta::assert_snapshot!(ran.stdout);
    assert_eq!(
        ran.stderr,
        "scry: listing the configurations of .\n\
         scry: warning: --target web-b=root@web-b: web-b is not selected\n\
         scry: evaluating 2 configurations\n"
    );
    assert_eq!(ran.codes(), (0, 1));
    assert_eq!(ran.left, 0);
}

#[tokio::test]
async fn flake_default_selection_by_local_host_name_on_macos() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let old = fixture::nix_darwin(&mut local, "o", "3600");
    current_system(&mut local, &old);
    let new = fixture::nix_darwin(&mut local, "n", "7200");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let sets = vec![(
        "darwinConfigurations",
        vec![("Alices-MacBook", Some(new.clone())), ("desk", Some(new))],
    )];
    let fake = || {
        let fake = hosts(fleet.fake(), "alices-mbp", Some("Alices-MacBook"));
        builds(flake(fake, false, sets.clone()), &[])
    };
    let macos = Env {
        macos: true,
        ..env(&temp)
    };
    let ran = scry(fake(), macos, &["flake"]).await;
    assert!(
        ran.stdout
            .starts_with("Alices-MacBook · nix-darwin (local)\n  old "),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.codes(), (0, 1));

    // Elsewhere only `hostname -s` counts, which matches nothing.
    let ran = scry(fake(), env(&temp), &["flake"]).await;
    assert_eq!(
        ran.error(),
        "no configuration of . belongs to this machine (host alices-mbp, user alice); \
         select from Alices-MacBook, desk with --on"
    );
    assert_eq!(ran.codes(), (2, 2));
}

#[tokio::test]
async fn flake_reference_with_a_name() {
    let temp = TempDir::new().unwrap();
    let mut target = Host::new();
    let old = nixos(&mut target, true);
    current_system(&mut target, &old);
    let mut local = Host::new();
    let new = nixos(&mut local, false);
    let fleet = Arc::new(
        Fleet::new()
            .host(remote("root@web-b"), target)
            .host(Machine::Local, local),
    );
    let sets = vec![
        ("nixosConfigurations", vec![("web-b", Some(new.clone()))]),
        ("homeConfigurations", vec![("web-b", Some(new.clone()))]),
        ("darwinConfigurations", vec![("desk", Some(new))]),
    ];
    let fake = || {
        let fake = hosts(fleet.fake(), "web-a", None);
        builds(flake(fake, false, sets.clone()), &[])
    };
    let ran = scry(
        fake(),
        env(&temp),
        &[
            "flake",
            ".#desk",
            "--target",
            "desk=root@web-b",
            "--format",
            "json",
        ],
    )
    .await;
    let report: serde_json::Value = serde_json::from_str(&ran.stdout).unwrap();
    let desk = &report["configurations"][0];
    assert_eq!(report["mode"], "deployed");
    assert_eq!(report["configurations"].as_array().unwrap().len(), 1);
    assert_eq!(desk["name"], "desk");
    assert_eq!(desk["source"], "flake");
    assert_eq!(desk["status"], "done");
    assert_eq!(desk["target"], "root@web-b");
    assert_eq!(desk["old"]["location"], "root@web-b");
    assert_eq!(desk["new"]["location"], "local");
    assert_eq!(ran.codes(), (0, 1));

    for (args, error) in [
        (&["flake", ".#"][..], ".#: no configuration name after `#`"),
        (&["flake", ".#nope"], "#nope: no such configuration"),
        (
            &["flake", ".#web-b"],
            "#web-b is in nixosConfigurations and homeConfigurations",
        ),
        (
            &["flake", ".#desk", "--on", "desk"],
            ".#desk: give either REF#name or --on, not both",
        ),
        (
            &["flake", "--on", "@web"],
            "--on @web: `@` selects colmena tags, and a flake has none",
        ),
        (
            &["flake", "--on", "web-?,db"],
            "--on db: matches no configuration",
        ),
    ] {
        let ran = scry(fake(), env(&temp), args).await;
        assert_eq!(ran.error(), error, "{args:?}");
        assert_eq!(ran.codes(), (2, 2), "{args:?}");
        assert_eq!(ran.stdout, "");
    }
}

#[tokio::test]
async fn flake_build_failure_and_no_build() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let old = nixos(&mut local, true);
    current_system(&mut local, &old);
    let new = nixos(&mut local, false);
    let mut target = Host::new();
    current_system(&mut target, &nixos(&mut Host::new(), true));
    target.dir("/nix/store");
    let fleet = Arc::new(
        Fleet::new()
            .host(Machine::Local, local)
            .host(remote("web-a"), target),
    );
    let missing = fixture::store_path("n", "nixos-system-web-a-26.11");
    let sets = vec![(
        "nixosConfigurations",
        vec![("web-a", Some(new)), ("web-a-next", Some(missing.clone()))],
    )];
    let flake = || flake(hosts(fleet.fake(), "web-a", None), false, sets.clone());

    // The build fails: `web-a` was built anyway, `web-a-next` was not.
    let ran = scry(
        builds(flake(), &["new"]),
        env(&temp),
        &["flake", "--on", "web-a*", "--target", "web-a-next=web-a"],
    )
    .await;
    assert!(
        ran.stdout.starts_with("web-a · nixos (local)\n"),
        "{}",
        ran.stdout
    );
    let failed = format!(
        "web-a-next · unknown (web-a)\n  error: {missing} is not in the store on this machine; \
         the build did not produce it\n"
    );
    assert!(ran.stdout.ends_with(&failed), "{}", ran.stdout);
    assert_eq!(ran.codes(), (1, 2));

    // Nothing is built, which the fake would refuse: it has no rule for `nix build`.
    let ran = scry(
        flake(),
        env(&temp),
        &[
            "flake",
            "--on",
            "web-a-next",
            "--target",
            "web-a-next=web-a",
            "--no-build",
        ],
    )
    .await;
    assert_eq!(
        ran.stdout,
        format!(
            "web-a-next · unknown (web-a)\n  error: {missing} is not in the store on this \
             machine; run without --no-build\n"
        )
    );
    assert_eq!(ran.codes(), (1, 2));
}

#[tokio::test(start_paused = true)]
async fn timeout_and_unreachable_among_working_targets() {
    let temp = TempDir::new().unwrap();
    let (fake, new) = colmena_fleet();
    let toplevels = vec![
        ("web-a", new.clone()),
        ("mail-a", new.clone()),
        ("web-c", new),
    ];
    let fake = hive(fake, false, HIVE, toplevels).hang(|m, _| *m == remote("root@mail-a"));
    let ran = scry(
        fake,
        env(&temp),
        &["colmena", "--on", "mail-a,web-a,web-c", "--timeout", "5"],
    )
    .await;
    let statuses: Vec<&str> = ran
        .stdout
        .lines()
        .filter(|l| l.contains(" · ") || l.starts_with("  error") || l.starts_with("  timeout"))
        .collect();
    assert_eq!(
        statuses,
        [
            "mail-a · nixos (root@mail-a)",
            "  timeout after 5 s",
            "web-a · nixos (root@web-a)",
            "web-c · nixos (root@web-c)",
            "  error: ssh root@web-c: connect to host web-c port 22: Connection refused",
        ]
    );
    assert_eq!(ran.codes(), (1, 2));
}

#[tokio::test]
async fn paths_on_one_machine() {
    let temp = TempDir::new().unwrap();
    let mut host = Host::new();
    let old = fixture::unknown(&mut host, "o", "hi\n");
    let new = fixture::unknown(&mut host, "n", "hello\n");
    host.link("/nix/var/nix/profiles/system-1-link", &old);
    current_system(&mut host, &new);
    let fleet = Arc::new(
        Fleet::new()
            .host(Machine::Local, host.clone())
            .host(remote("root@web-a"), host),
    );
    let ran = scry(
        fleet.fake(),
        env(&temp),
        &[
            "paths",
            "/nix/var/nix/profiles/system-1-link",
            "/run/current-system",
            "--format",
            "json",
        ],
    )
    .await;
    insta::assert_snapshot!(ran.stdout);
    assert_eq!(ran.codes(), (0, 1));

    let ran = scry(
        fleet.fake(),
        env(&temp),
        &[
            "paths",
            "--target",
            "root@web-a",
            "/run/current-system",
            &new,
        ],
    )
    .await;
    assert_eq!(
        ran.stdout,
        format!("root@web-a · unknown (root@web-a)\n  no changes, same toplevel {new}\n")
    );
    assert_eq!(ran.codes(), (0, 0));
    assert_eq!(ran.left, 0);
}

/// Head has `web-a`, `web-b` and `alice@web-a`; base has no `web-b`, and its build fails, which
/// leaves `alice@web-a`'s base toplevel missing.
#[tokio::test]
async fn flake_revision_mode() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let (old, new) = (
        fixture::unknown(&mut local, "o", "hi\n"),
        fixture::unknown(&mut local, "n", "hello\n"),
    );
    let new_hm = fixture::home_manager(&mut local, "n", "alice@example.com");
    let old_hm = fixture::store_path("o", "home-manager-generation");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let repository = Repository::new();
    let fake = git(fleet.fake(), &repository, true);
    let fake = flake(
        fake,
        false,
        vec![
            (
                "nixosConfigurations",
                vec![("web-a", Some(new.clone())), ("web-b", Some(new))],
            ),
            ("homeConfigurations", vec![("alice@web-a", Some(new_hm))]),
        ],
    );
    let fake = flake(
        fake,
        true,
        vec![
            ("nixosConfigurations", vec![("web-a", Some(old))]),
            (
                "homeConfigurations",
                vec![("alice@web-a", Some(old_hm.clone()))],
            ),
        ],
    );
    let fake = builds(fake, &["old"]);
    let ran = scry(
        fake,
        env(&temp),
        &["flake", "--base", "main", "--format", "json"],
    )
    .await;
    let report: serde_json::Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(report["mode"], "revision");
    assert_eq!(
        report["base"],
        serde_json::json!({"ref": "main", "rev": BASE})
    );
    assert_eq!(
        report["head"],
        serde_json::json!({"rev": HEAD, "dirty": true})
    );
    let statuses: Vec<(&str, &str)> = report["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        statuses,
        [
            ("alice@web-a", "error"),
            ("web-a", "done"),
            ("web-b", "absent-at-base"),
        ]
    );
    assert_eq!(
        report["configurations"][0]["error"],
        format!("{old_hm} is not in the store on this machine; the build did not produce it")
    );
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());
    assert_eq!(ran.left, 0);
}

/// The source lives in `sub/` and was added after base: base has none there, so every
/// configuration is absent at base, in both sources.
#[tokio::test]
async fn revision_mode_with_no_source_at_base() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let new = fixture::unknown(&mut local, "n", "hello\n");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let repository = Arc::new(Repository {
        source: "/sub",
        ..Repository::default()
    });
    let fake = git(fleet.fake(), &repository, false);
    let fake = flake(
        fake,
        false,
        vec![("nixosConfigurations", vec![("web-a", Some(new.clone()))])],
    );
    let fake = hive(fake, false, HIVE, vec![("web-a", new.clone())]).on(
        Machine::Local,
        "colmena",
        &["--version"],
        ok("colmena 0.5.0\n"),
    );
    let fake = Arc::new(builds(fake, &[]));
    let expected = "web-a · unknown (1a2b3c4 -> working tree)\n  not present at base 1a2b3c4\n";
    for args in [
        &["flake", "./sub", "--base", "main"][..],
        &["colmena", "--on", "web-a", "--base", "main"],
    ] {
        let cli = Cli::try_parse_from(std::iter::once("scry").chain(args.iter().copied())).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exec: Arc<dyn scry::exec::Exec> = fake.clone();
        let outcome = run::run(
            exec,
            &cli.command,
            &env(&temp),
            &mut out,
            &mut err,
            std::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), expected, "{args:?}");
        assert_eq!((outcome.exit_code(false), outcome.exit_code(true)), (0, 1));
    }
    assert!(repository.worktrees.lock().unwrap().is_empty());
}

#[tokio::test]
async fn colmena_revision_mode() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let (old, new) = (
        fixture::unknown(&mut local, "o", "hi\n"),
        fixture::unknown(&mut local, "n", "hello\n"),
    );
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let repository = Repository::new();
    let fake = fleet.fake().on(
        Machine::Local,
        "colmena",
        &["--version"],
        ok("colmena 0.5.0\n"),
    );
    let fake = git(fake, &repository, false);
    let head = vec![
        ("web-a", new.clone()),
        ("web-b", new.clone()),
        ("web-c", new),
    ];
    let fake = hive(fake, false, HIVE, head);
    let base_hive = r#"{
      "web-a": {"targetHost": "web-a", "targetUser": "root", "targetPort": null, "sshOptions": [], "tags": ["web"]},
      "web-c": {"targetHost": "web-c", "targetUser": "root", "targetPort": null, "sshOptions": [], "tags": ["web"]}
    }"#;
    let fake = hive(fake, true, base_hive, vec![("web-a", old)]);
    let fake = builds(fake, &[]);
    let ran = scry(
        fake,
        env(&temp),
        &["colmena", "--on", "@web", "--base", "main"],
    )
    .await;
    insta::assert_snapshot!(ran.stdout);
    assert!(
        ran.stderr
            .starts_with("scry: evaluating the hive\nscry: evaluating 3 nodes at head and base\n"),
        "{}",
        ran.stderr
    );
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());

    for (base, error) in [
        ("nope", "--base nope: not a revision of /repo"),
        (
            "main",
            "--base needs a source inside a git checkout, and /repo is not in one",
        ),
    ] {
        let outside = base == "main";
        let fake = Fake::new()
            .on(
                Machine::Local,
                "colmena",
                &["--version"],
                ok("colmena 0.5.0\n"),
            )
            .rule(move |_, c| {
                (outside && c.args.contains(&"--show-toplevel".to_owned()))
                    .then(|| exit(128, "fatal: not a git repository\n"))
            });
        let fake = hive(git(fake, &repository, false), false, HIVE, vec![]);
        let ran = scry(fake, env(&temp), &["colmena", "--base", base]).await;
        assert_eq!(ran.error(), error);
        assert_eq!(ran.codes(), (2, 2));
    }
    let ran = scry(
        Fake::new(),
        env(&temp),
        &["flake", "github:o/r", "--base", "main"],
    )
    .await;
    assert_eq!(
        ran.error(),
        "--base needs a flake in a local git checkout, such as `.`; github:o/r is not a path"
    );
    assert_eq!(ran.codes(), (2, 2));
    assert_eq!(ran.left, 0);
}

/// Ctrl-C while fetching: the run ends as failed, and the base worktree and the temporary
/// directory are gone.
#[tokio::test(start_paused = true)]
async fn ctrl_c_cleans_up() {
    let temp = TempDir::new().unwrap();
    let repository = Repository::new();
    let fake = git(Fake::new(), &repository, false);
    let fake = flake(
        fake,
        false,
        vec![(
            "nixosConfigurations",
            vec![("web-a", Some("/nix/store/x-new".to_owned()))],
        )],
    );
    let fake = flake(
        fake,
        true,
        vec![(
            "nixosConfigurations",
            vec![("web-a", Some("/nix/store/x-old".to_owned()))],
        )],
    );
    let fake = builds(fake, &[]).hang(|_, c| c.program == "sh");
    let interrupt = tokio::time::sleep(Duration::from_secs(1));
    let ran = scry_until(fake, env(&temp), &["flake", "--base", "main"], interrupt).await;
    assert_eq!(ran.error(), "interrupted");
    assert_eq!(ran.codes(), (1, 2));
    assert_eq!(ran.stdout, "");
    assert!(repository.worktrees.lock().unwrap().is_empty());
    assert_eq!(ran.left, 0);
}

/// The working directory `hosts/web-c` exists only at head; the flake is at the checkout's
/// root, in base too, so base is evaluated there.
#[tokio::test]
async fn revision_from_a_directory_only_at_head() {
    let temp = TempDir::new().unwrap();
    let mut local = Host::new();
    let old = fixture::unknown(&mut local, "o", "hi\n");
    let new = fixture::unknown(&mut local, "n", "hello\n");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let repository = Repository::new();
    let fake = git(fleet.fake(), &repository, false);
    let fake = flake(
        fake,
        false,
        vec![("nixosConfigurations", vec![("web-a", Some(new))])],
    );
    let fake = flake(
        fake,
        true,
        vec![("nixosConfigurations", vec![("web-a", Some(old))])],
    );
    let env = Env {
        cwd: PathBuf::from("/repo/hosts/web-c"),
        ..env(&temp)
    };
    let ran = scry(builds(fake, &[]), env, &["flake", ".", "--base", "main"]).await;
    assert!(
        ran.stdout
            .starts_with("web-a · unknown (1a2b3c4 -> working tree)\n  old "),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.codes(), (0, 1));
    assert!(repository.worktrees.lock().unwrap().is_empty());
}

/// A flake hive whose lock needs changes stops the run before colmena could write it.
#[tokio::test]
async fn colmena_flake_hive_with_a_stale_lock() {
    let temp = TempDir::new().unwrap();
    for stale in [true, false] {
        let (fake, new) = colmena_fleet();
        let fake = fake
            .rule(|_, c| {
                let (_, top, _) = located(c)?;
                (top == "/").then(|| ok("/repo/flake.nix"))
            })
            .rule(move |_, c| {
                let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
                let metadata = args[..]
                    == [
                        "--extra-experimental-features",
                        "nix-command flakes",
                        "flake",
                        "metadata",
                        "--no-update-lock-file",
                        "/repo",
                    ];
                (c.program == "nix" && metadata).then(|| {
                    if stale {
                        exit(
                            1,
                            "error: flake 'git+file:///repo' requires lock file changes but \
                             they're not allowed due to '--no-update-lock-file'\n",
                        )
                    } else {
                        ok("")
                    }
                })
            });
        let fake = hive(fake, false, HIVE, vec![("db-a", new)]);
        let ran = scry(fake, env(&temp), &["colmena", "--on", "db-a", "--no-build"]).await;
        if stale {
            assert_eq!(
                ran.error(),
                "the hive's flake.lock in /repo is missing or out of date, and colmena would \
                 write it; run `nix flake lock` there first"
            );
            assert_eq!(ran.codes(), (1, 2));
        } else {
            assert!(
                ran.stdout.starts_with("db-a · nixos (local)\n"),
                "{}",
                ran.stdout
            );
            assert_eq!(ran.codes(), (0, 0));
        }
        assert_eq!(ran.left, 0);
    }
}

/// How long a stand-in viewer, or a run that shows it, may take before the test fails instead
/// of hanging.
const LIMIT: Duration = Duration::from_secs(5);

/// The next configuration, or `None` once the channel closes or [`LIMIT`] has passed `since`.
fn next(events: &mut UnboundedReceiver<Configuration>, since: Instant) -> Option<Configuration> {
    while since.elapsed() < LIMIT {
        match events.try_recv() {
            Ok(configuration) => return Some(configuration),
            Err(TryRecvError::Disconnected) => return None,
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    None
}

/// Stand-ins for the viewer.
fn watch_all(
    _: Report,
    names: Vec<String>,
    mut events: UnboundedReceiver<Configuration>,
    _: tui::Options,
    _: &AtomicBool,
) -> std::io::Result<Finished> {
    let (mut configurations, since) = (Vec::new(), Instant::now());
    while let Some(configuration) = next(&mut events, since) {
        configurations.push(configuration);
    }
    Ok(Finished {
        all_finished: configurations.len() == names.len(),
        configurations,
    })
}

fn quit_after_one(
    _: Report,
    _: Vec<String>,
    mut events: UnboundedReceiver<Configuration>,
    _: tui::Options,
    _: &AtomicBool,
) -> std::io::Result<Finished> {
    let first = next(&mut events, Instant::now());
    Ok(Finished {
        all_finished: false,
        configurations: first.into_iter().collect(),
    })
}

/// How often [`until_told`] was told to quit.
static TOLD: AtomicUsize = AtomicUsize::new(0);

fn until_told(
    _: Report,
    _: Vec<String>,
    _: UnboundedReceiver<Configuration>,
    _: tui::Options,
    quit: &AtomicBool,
) -> std::io::Result<Finished> {
    let since = Instant::now();
    while !quit.load(Ordering::Relaxed) && since.elapsed() < LIMIT {
        std::thread::sleep(Duration::from_millis(5));
    }
    if quit.load(Ordering::Relaxed) {
        TOLD.fetch_add(1, Ordering::Relaxed);
    }
    Ok(Finished {
        all_finished: false,
        configurations: Vec::new(),
    })
}

/// Lets [`stuck`] return, so the test's runtime can shut down.
static UNSTUCK: AtomicBool = AtomicBool::new(false);

/// A viewer blocked on a terminal that hung up: it never looks at the quit flag.
fn stuck(
    _: Report,
    _: Vec<String>,
    _: UnboundedReceiver<Configuration>,
    _: tui::Options,
    _: &AtomicBool,
) -> std::io::Result<Finished> {
    while !UNSTUCK.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
    Err(std::io::Error::other("unstuck"))
}

/// Revision mode with `web-a` to fetch and `web-b` absent at base, the fetch answered by
/// `fetching`.
fn viewed(fetching: Fake, repository: &Arc<Repository>) -> Fake {
    let (old, new) = (
        fixture::store_path("o", "custom-system"),
        fixture::store_path("n", "custom-system"),
    );
    let fake = git(fetching, repository, false);
    let fake = flake(
        fake,
        false,
        vec![(
            "nixosConfigurations",
            vec![("web-a", Some(new.clone())), ("web-b", Some(new))],
        )],
    );
    let fake = flake(
        fake,
        true,
        vec![("nixosConfigurations", vec![("web-a", Some(old))])],
    );
    builds(fake, &[])
}

/// Fails the test when the run outlives the stand-in viewer.
async fn bounded(run: impl Future<Output = Ran>) -> Ran {
    tokio::time::timeout(2 * LIMIT, run)
        .await
        .expect("the run ends with its viewer")
}

fn is_fetch(c: &scry::exec::Command) -> bool {
    c.program == "sh" && c.args.get(1).map(String::as_str) == Some(fetch::SCRIPT.text())
}

#[tokio::test]
async fn viewer_quits_and_the_run_cleans_up() {
    let temp = TempDir::new().unwrap();
    let viewer = |viewer| Env {
        stdout_is_terminal: true,
        viewer,
        ..env(&temp)
    };
    let args = ["flake", "--base", "main"];

    // Everything arrives before the viewer quits.
    let mut local = Host::new();
    fixture::unknown(&mut local, "o", "hi\n");
    fixture::unknown(&mut local, "n", "hello\n");
    let fleet = Arc::new(Fleet::new().host(Machine::Local, local));
    let repository = Repository::new();
    let ran = bounded(scry(
        viewed(fleet.fake(), &repository),
        viewer(watch_all),
        &args,
    ))
    .await;
    assert_eq!(ran.stdout, "", "the viewer writes the screen, not stdout");
    assert_eq!(ran.codes(), (0, 1));
    assert!(repository.worktrees.lock().unwrap().is_empty());

    // Quit while `web-a` still fetches: it counts as failed.
    let repository = Repository::new();
    let hanging = Fake::new().hang(|_, c| is_fetch(c));
    let ran = bounded(scry(
        viewed(hanging, &repository),
        viewer(quit_after_one),
        &args,
    ))
    .await;
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());

    // A signal tells the viewer to quit, and the run ends as interrupted.
    let repository = Repository::new();
    let hanging = Fake::new().hang(|_, c| is_fetch(c));
    let signal = tokio::time::sleep(Duration::from_millis(50));
    let ran = bounded(scry_until(
        viewed(hanging, &repository),
        viewer(until_told),
        &args,
        signal,
    ))
    .await;
    assert_eq!(ran.error(), "interrupted");
    assert_eq!(
        TOLD.load(Ordering::Relaxed),
        1,
        "the viewer was told to quit"
    );
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());

    // A fetch task that panics fails the run instead of leaving the viewer up.
    let repository = Repository::new();
    let panicking = Fake::new().rule(|_, c| is_fetch(c).then(|| panic!("boom")));
    let ran = bounded(scry(
        viewed(panicking, &repository),
        viewer(until_told),
        &args,
    ))
    .await;
    assert!(ran.error().contains("panicked"), "{}", ran.error());
    assert_eq!(
        TOLD.load(Ordering::Relaxed),
        2,
        "the viewer was told to quit"
    );
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());
    assert_eq!(ran.left, 0);
}

#[tokio::test]
async fn a_stuck_viewer_does_not_keep_the_run_alive() {
    let temp = TempDir::new().unwrap();
    let env = Env {
        stdout_is_terminal: true,
        viewer: stuck,
        ..env(&temp)
    };
    let repository = Repository::new();
    let hanging = Fake::new().hang(|_, c| is_fetch(c));
    let signal = tokio::time::sleep(Duration::from_millis(50));
    let ran = bounded(scry_until(
        viewed(hanging, &repository),
        env,
        &["flake", "--base", "main"],
        signal,
    ))
    .await;
    UNSTUCK.store(true, Ordering::Relaxed);
    assert_eq!(ran.error(), "interrupted");
    assert_eq!(ran.codes(), (1, 2));
    assert!(repository.worktrees.lock().unwrap().is_empty());
    assert_eq!(ran.left, 0);
}

/// Head has no flake: an unreadable source (spec 9), as without `--base`, not a usage error.
#[tokio::test]
async fn revision_without_a_flake_or_hive_fails() {
    let temp = TempDir::new().unwrap();
    for (args, error) in [
        (
            &["flake", ".", "--base", "main"][..],
            "no flake.nix in /repo/. or above it inside the checkout /repo",
        ),
        (
            &["colmena", "--base", "main"],
            "no flake.nix or hive.nix in /repo or above it inside the checkout /repo",
        ),
    ] {
        let repository = Repository::new();
        let fake = Fake::new()
            .on(
                Machine::Local,
                "colmena",
                &["--version"],
                ok("colmena 0.5.0\n"),
            )
            .rule(|_, c| {
                let (_, top, _) = located(c)?;
                (top == "/repo").then(|| exit(1, ""))
            });
        let fake = hive(git(fake, &repository, false), false, HIVE, vec![]);
        let ran = scry(fake, env(&temp), args).await;
        assert_eq!(ran.error(), error);
        assert_eq!(ran.codes(), (1, 2));
        assert_eq!(ran.left, 0);
    }
}

/// `git worktree add` died before writing the worktree's `.git`: the entry is still removed.
#[tokio::test]
async fn a_worktree_left_locked_is_removed() {
    let temp = TempDir::new().unwrap();
    let repository = Arc::new(Repository {
        stuck: true,
        ..Repository::default()
    });
    let fake = git(Fake::new(), &repository, false);
    let fake = flake(
        fake,
        false,
        vec![(
            "nixosConfigurations",
            vec![("web-a", Some("/nix/store/x-new".to_owned()))],
        )],
    );
    let ran = scry(builds(fake, &[]), env(&temp), &["flake", "--base", "main"]).await;
    assert_eq!(
        ran.stdout,
        "web-a · unknown (1a2b3c4 -> working tree)\n  not present at base 1a2b3c4\n"
    );
    assert!(repository.worktrees.lock().unwrap().is_empty());
    assert_eq!(ran.left, 0);
}
