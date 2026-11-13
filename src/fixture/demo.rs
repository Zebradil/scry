//! The fixture run mode's fleet (spec 11, ADR 0060): one fake seam that answers every command
//! `scry colmena` and `scry flake` run, in deployed and revision mode, for five configurations.
//!
//! - `db-a`: this machine, already running its new toplevel: no changes in either mode.
//! - `web-a`: on its target, before a nixpkgs bump and a config change: churn, a binary file,
//!   a Boot change and a new specialisation, all shared with `web-b`.
//! - `web-b`: like `web-a`; in the hive it builds on its target, where its new side also has a
//!   file of its own.
//! - `web-c`: added by the change, so unreachable in deployed mode and absent at base in
//!   revision mode.
//! - `mail-a`: fails to evaluate.
//!
//! The hive tags them `@db`, `@mail`, `@web` and `@eu`; the flake lists them under
//! `nixosConfigurations`, and this machine's host name is `db-a`. Base is any revision; its
//! worktree is told apart from the working tree by its path.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Fleet, Host, current_system, nixos_of, store_path};
use crate::exec::Output;
use crate::exec::fake::{Fake, exit, ok};
use crate::source::LOCATE;
use crate::source::colmena::IN_DIR;
use crate::transport::machine::{Machine, Remote};

/// A non-empty value turns the fixture run mode on.
pub const ENV: &str = "SCRY_FIXTURE";

/// Whether [`ENV`] asks for the fixture run mode.
pub fn enabled() -> bool {
    std::env::var_os(ENV).is_some_and(|v| !v.is_empty())
}

const BASE_REV: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
const HEAD_REV: &str = "9f8e7d6c5b4a39281706f5e4d3c2b1a098f7e6d5";
const MAIL_ERROR: &str = "error:\n       … while evaluating the attribute 'config.system.build.toplevel'\n\n       error: The option `services.postfix.relayHost' was accessed but has no value defined.\n";

/// Every node: name, tags, whether it has a target, whether it builds there.
const NODES: [(&str, &[&str], bool, bool); 5] = [
    ("db-a", &["db"], false, false),
    ("mail-a", &["mail"], true, false),
    ("web-a", &["web"], true, false),
    ("web-b", &["web", "eu"], true, true),
    ("web-c", &["web", "eu"], true, false),
];

fn remote(target: &str) -> Machine {
    Machine::Remote(Remote {
        target: target.parse().expect("a valid target"),
        port: None,
        ssh_options: Vec::new(),
    })
}

/// The seam of the fixture run mode.
pub fn fake() -> Fake {
    let mut local = Host::new();
    let db = nixos_of(&mut local, false, "db-a");
    current_system(&mut local, &db);
    let mut head = BTreeMap::from([("db-a", db.clone())]);
    let mut base = head.clone();
    for name in ["web-a", "web-b"] {
        base.insert(name, nixos_of(&mut local, true, name));
        head.insert(name, nixos_of(&mut local, false, name));
    }
    head.insert("web-c", nixos_of(&mut local, false, "web-c"));

    let mut web_a = Host::new();
    let old = nixos_of(&mut web_a, true, "web-a");
    current_system(&mut web_a, &old);
    let mut web_b = Host::new();
    let old = nixos_of(&mut web_b, true, "web-b");
    current_system(&mut web_b, &old);
    nixos_of(&mut web_b, false, "web-b");
    web_b.file(
        &format!("{}/etc/nginx/limits.conf", store_path("n", "etc")),
        "worker_connections 4096;\n",
    );
    let mut web_c = Host::new();
    web_c.unreachable = Some("connect to host web-c port 22: Connection refused".to_owned());

    // The hive's targets are `root@<name>`; the flake's are the bare names.
    let mut fleet = Fleet::new().host(Machine::Local, local);
    for (name, host) in [("web-a", web_a), ("web-b", web_b), ("web-c", web_c)] {
        fleet = fleet
            .host(remote(&format!("root@{name}")), host.clone())
            .host(remote(name), host);
    }
    let (head, base) = (Arc::new(head), Arc::new(base));
    let (colmena_head, colmena_base) = (Arc::clone(&head), Arc::clone(&base));

    Arc::new(fleet)
        .fake()
        .rule(|_, c| {
            let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
            let ["-c", script, "sh", _, _, ref files @ ..] = args[..] else {
                return None;
            };
            (c.program == "sh" && script == LOCATE.text())
                .then(|| ok(format!("/{}", files.last().copied().unwrap_or_default())))
        })
        .rule(move |_, c| {
            let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
            let ["-c", script, "sh", dir, "colmena", ref args @ ..] = args[..] else {
                return None;
            };
            if c.program != "sh" || script != IN_DIR.text() {
                return None;
            }
            let at_base = is_base(dir);
            Some(match args {
                ["build", ..] => ok(""),
                ["eval", "-E", e] if e.contains("buildOnTarget") => ok(deployment(at_base)),
                ["eval", "-E", e] => {
                    let toplevels = if at_base { &colmena_base } else { &colmena_head };
                    evaluated(toplevels, quoted_after(e, "nodes.")?)
                }
                _ => return None,
            })
        })
        .on(Machine::Local, "colmena", &["--version"], ok("colmena 0.5.0\n"))
        .rule(move |_, c| {
            let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
            if c.program != "nix" {
                return None;
            }
            Some(match args[..] {
                ["--extra-experimental-features", _, "build", ..] => ok(""),
                [.., "eval", "--json", "--no-write-lock-file", installable, "--apply", apply] => {
                    let (reference, set) = installable.split_once('#')?;
                    if set != "nixosConfigurations" {
                        return Some(exit(
                            1,
                            format!(
                                "error: flake 'path:{reference}' does not provide attribute '{set}'\n"
                            ),
                        ));
                    }
                    let toplevels = if is_base(reference) { &base } else { &head };
                    if apply == "builtins.attrNames" {
                        let mut names: Vec<&str> = toplevels.keys().copied().collect();
                        names.push("mail-a");
                        names.sort();
                        return Some(ok(serde_json::to_string(&names).expect("strings")));
                    }
                    evaluated(toplevels, quoted_after(apply, "c.")?)
                }
                _ => return None,
            })
        })
        .rule(|_, c| {
            let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
            let ["-C", dir, ref args @ ..] = args[..] else {
                return None;
            };
            if c.program != "git" {
                return None;
            }
            Some(match args {
                ["rev-parse", "--show-toplevel"] => ok(format!("{dir}\n")),
                ["rev-parse", "--verify", ..] => ok(format!("{BASE_REV}\n")),
                ["rev-parse", "HEAD"] => ok(format!("{HEAD_REV}\n")),
                ["status", ..] => ok(" M hive.nix\n"),
                ["worktree", ..] => ok(""),
                ["for-each-ref", ..] => ok("main\nrelease-26.05\nv0.1.0\n"),
                _ => return None,
            })
        })
        .on(Machine::Local, "hostname", &["-s"], ok("db-a\n"))
        // Anything else is not part of the fixture: a missing program here, an unknown target
        // elsewhere.
        .rule(|machine, _| {
            Some(match machine {
                Machine::Local => exit(127, ""),
                Machine::Remote(_) => exit(255, "ssh: not part of the fixture fleet\n"),
            })
        })
}

/// The base worktree is `<run's temporary directory>/worktree`, and the source is at its root.
fn is_base(dir: &str) -> bool {
    dir.ends_with("/worktree")
}

/// The name in `<prefix>"<name>"`, as the sources quote it.
fn quoted_after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = &text[text.find(&format!("{prefix}\""))? + prefix.len() + 1..];
    Some(&rest[..rest.find('"')?])
}

/// Step 1 of the hive at head, or at base without `web-c`.
fn deployment(at_base: bool) -> String {
    let nodes: serde_json::Map<String, serde_json::Value> = NODES
        .iter()
        .filter(|(name, ..)| !(at_base && *name == "web-c"))
        .map(|&(name, tags, remote, build_on_target)| {
            let node = serde_json::json!({
                "buildOnTarget": build_on_target,
                "sshOptions": [],
                "tags": tags,
                "targetHost": remote.then_some(name),
                "targetPort": null,
                "targetUser": "root",
            });
            (name.to_owned(), node)
        })
        .collect();
    serde_json::Value::Object(nodes).to_string()
}

/// `[ outPath drvPath ]` of `name`, or `mail-a`'s evaluation error.
fn evaluated(toplevels: &BTreeMap<&str, String>, name: &str) -> Output {
    match toplevels.get(name) {
        Some(out) => ok(serde_json::json!([out, format!("{out}.drv")]).to_string()),
        None if name == "mail-a" => exit(1, MAIL_ERROR),
        None => exit(1, format!("error: attribute '{name}' missing\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_from_quoted_attribute_paths() {
        assert_eq!(
            quoted_after(r#"c: let t = c."web-a".config"#, "c."),
            Some("web-a")
        );
        assert_eq!(quoted_after("nodes.web-a", "nodes."), None);
    }
}
