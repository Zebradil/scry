//! A flake's configurations (spec 4.2): listed without evaluating them, the default selection
//! for this machine by the rules `nh` and `home-manager` use, `REF#name`, one evaluation per
//! selected configuration, and where each one's old side lives.

use anyhow::{Context, Result};

use super::{Evaluated, OUT_AND_DRV, Selected, nix_string, success};
use crate::cli::TargetOverride;
use crate::exec::{Command, Exec};
use crate::exit::UsageError;
use crate::report::ClosureKind;
use crate::transport::machine::{Machine, Remote, Target};

/// The attribute sets a flake lists configurations in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Set {
    Nixos,
    Darwin,
    Home,
}

impl Set {
    pub const ALL: [Self; 3] = [Self::Nixos, Self::Darwin, Self::Home];

    pub fn attribute(self) -> &'static str {
        match self {
            Self::Nixos => "nixosConfigurations",
            Self::Darwin => "darwinConfigurations",
            Self::Home => "homeConfigurations",
        }
    }

    pub fn kind(self) -> ClosureKind {
        match self {
            Self::Nixos => ClosureKind::Nixos,
            Self::Darwin => ClosureKind::NixDarwin,
            Self::Home => ClosureKind::HomeManager,
        }
    }

    /// The toplevel inside one configuration.
    fn toplevel(self) -> &'static str {
        match self {
            Self::Nixos | Self::Darwin => "config.system.build.toplevel",
            Self::Home => "activationPackage",
        }
    }
}

/// One configuration of the flake.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entry {
    pub name: String,
    pub set: Set,
}

/// `REF#name` split at the first `#`, as Nix splits a flake reference from its fragment. An
/// empty name is a usage error.
pub fn split(reference: &str) -> Result<(&str, Option<&str>), UsageError> {
    match reference.split_once('#') {
        None => Ok((reference, None)),
        Some((_, "")) => Err(UsageError(format!(
            "{reference}: no configuration name after `#`"
        ))),
        Some((flake, name)) => Ok((flake, Some(name))),
    }
}

/// `nix eval --json <installable> --apply <apply>` with the flags of a call that takes a flake
/// reference (spec 6.1). `--no-write-lock-file`: a missing or outdated `flake.lock` would
/// otherwise be written into the user's checkout, and `git add`ed (spec 4.4).
fn eval(installable: &str, apply: &str) -> Command {
    Command::new(
        "nix",
        [
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--json",
            "--no-write-lock-file",
            installable,
            "--apply",
            apply,
        ],
    )
}

/// Every configuration of the three sets, by name, without evaluating any. A set the flake
/// does not have is skipped.
pub async fn list(exec: &dyn Exec, flake: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for set in Set::ALL {
        let installable = format!("{flake}#{}", set.attribute());
        let output = exec
            .run(&Machine::Local, eval(&installable, "builtins.attrNames"))
            .await?;
        if !output.success()
            && String::from_utf8_lossy(&output.stderr).contains("does not provide attribute")
        {
            continue;
        }
        let stdout = success("nix", output).with_context(|| format!("listing {installable}"))?;
        let names: Vec<String> = serde_json::from_slice(&stdout)
            .with_context(|| format!("reading the names of {installable}"))?;
        entries.extend(names.into_iter().map(|name| Entry { name, set }));
    }
    entries.sort();
    Ok(entries)
}

/// This machine's names: `hostname -s`, and on macOS also `scutil --get LocalHostName`, which
/// may be unset (spec 4.2).
pub async fn hosts(exec: &dyn Exec, macos: bool) -> Result<Vec<String>> {
    let local = Machine::Local;
    let output = exec.run(&local, Command::new("hostname", ["-s"])).await?;
    let mut hosts = vec![
        String::from_utf8_lossy(&success("hostname", output)?)
            .trim()
            .to_owned(),
    ];
    if macos {
        let scutil = Command::new("scutil", ["--get", "LocalHostName"]);
        if let Ok(output) = exec.run(&local, scutil).await
            && output.success()
        {
            hosts.push(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
    }
    hosts.retain(|h| !h.is_empty());
    hosts.dedup();
    Ok(hosts)
}

/// Who this machine is, for the default selection and local targets.
#[derive(Debug, Clone, Copy)]
pub struct Here<'a> {
    pub hosts: &'a [String],
    /// `$USER`.
    pub user: Option<&'a str>,
}

impl Here<'_> {
    fn host(&self, name: &str) -> bool {
        self.hosts.iter().any(|h| h == name)
    }

    fn user_at_host(&self, name: &str) -> bool {
        self.user.is_some_and(|user| {
            name.strip_prefix(user)
                .and_then(|rest| rest.strip_prefix('@'))
                .is_some_and(|host| self.host(host))
        })
    }

    /// The configuration belongs to this machine: a NixOS or nix-darwin configuration named
    /// after it, or a home-manager one named `$USER@<host>` or `$USER`.
    pub fn is_local(&self, entry: &Entry) -> bool {
        match entry.set {
            Set::Nixos | Set::Darwin => self.host(&entry.name),
            Set::Home => self.user_at_host(&entry.name) || self.user == Some(&entry.name),
        }
    }

    /// The default selection of deployed mode (spec 4.2): this machine's configurations, a
    /// bare `$USER` only when no `$USER@<host>` exists. Indices into `entries`.
    pub fn select(&self, entries: &[Entry]) -> Vec<usize> {
        let user_at_host = entries
            .iter()
            .any(|e| e.set == Set::Home && self.user_at_host(&e.name));
        let wanted = |e: &Entry| match e.set {
            Set::Nixos | Set::Darwin => self.host(&e.name),
            Set::Home if user_at_host => self.user_at_host(&e.name),
            Set::Home => self.user == Some(&e.name),
        };
        (0..entries.len())
            .filter(|&i| wanted(&entries[i]))
            .collect()
    }

    /// Where the configuration's sides live in deployed mode (spec 4.3): this machine's own
    /// configurations entirely here; any other's old side on `--target NAME=TARGET`, else on
    /// the target its name gives, a bare name resolving through `~/.ssh/config`.
    pub fn selected(&self, entry: &Entry, targets: &[TargetOverride]) -> Selected {
        let mut selected = Selected::local(&entry.name, entry.set.kind());
        if self.is_local(entry) {
            return selected;
        }
        let target = match targets.iter().rev().find(|t| t.name == entry.name) {
            Some(given) => Ok(given.target.clone()),
            None => entry.name.parse::<Target>(),
        };
        match target {
            Ok(target) => {
                selected.old = Machine::Remote(Remote {
                    target,
                    port: None,
                    ssh_options: Vec::new(),
                })
            }
            Err(error) => selected.error = Some(format!("{error}; give one with --target")),
        }
        selected
    }
}

/// The configurations named `name`: exactly one, else a usage error (spec 4.2).
pub fn by_name(entries: &[Entry], name: &str) -> Result<usize, UsageError> {
    let found: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].name == name)
        .collect();
    match found[..] {
        [one] => Ok(one),
        [] => Err(UsageError(format!("#{name}: no such configuration"))),
        _ => {
            let sets: Vec<&str> = found.iter().map(|&i| entries[i].set.attribute()).collect();
            Err(UsageError(format!("#{name} is in {}", sets.join(" and "))))
        }
    }
}

/// One configuration's toplevel: one `nix eval` (spec 4.2), the name quoted in the
/// attribute path.
pub async fn evaluate(exec: &dyn Exec, flake: &str, entry: &Entry) -> Result<Evaluated> {
    let installable = format!("{flake}#{}", entry.set.attribute());
    let apply = format!(
        "c: let t = c.{}.{}; in {OUT_AND_DRV}",
        nix_string(&entry.name),
        entry.set.toplevel()
    );
    let output = exec
        .run(&Machine::Local, eval(&installable, &apply))
        .await?;
    Evaluated::parse(&success("nix", output)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[(Set, &str)]) -> Vec<Entry> {
        let mut entries: Vec<Entry> = names
            .iter()
            .map(|&(set, name)| Entry {
                name: name.to_owned(),
                set,
            })
            .collect();
        entries.sort();
        entries
    }

    fn names(entries: &[Entry], indices: &[usize]) -> Vec<String> {
        indices.iter().map(|&i| entries[i].name.clone()).collect()
    }

    #[test]
    fn default_selection_follows_nh_and_home_manager() {
        let hosts = ["laptop".to_owned(), "Laptop-Local".to_owned()];
        let here = Here {
            hosts: &hosts,
            user: Some("alice"),
        };
        let all = entries(&[
            (Set::Nixos, "laptop"),
            (Set::Nixos, "web-a"),
            (Set::Darwin, "Laptop-Local"),
            (Set::Home, "alice"),
            (Set::Home, "alice@laptop"),
            (Set::Home, "bob@laptop"),
        ]);
        assert_eq!(
            names(&all, &here.select(&all)),
            ["Laptop-Local", "alice@laptop", "laptop"]
        );
        // Without `$USER@<host>`, the bare `$USER`.
        let bare = entries(&[(Set::Home, "alice"), (Set::Home, "alice@desk")]);
        assert_eq!(names(&bare, &here.select(&bare)), ["alice"]);
        assert!(here.select(&entries(&[(Set::Nixos, "web-a")])).is_empty());
    }

    #[test]
    fn targets() {
        let hosts = ["laptop".to_owned()];
        let here = Here {
            hosts: &hosts,
            user: Some("bob"),
        };
        let entry = |set, name: &str| Entry {
            name: name.to_owned(),
            set,
        };
        let old = |entry: &Entry, targets: &[TargetOverride]| {
            let s = here.selected(entry, targets);
            assert_eq!(s.new, Machine::Local);
            s.error.unwrap_or_else(|| s.old.location())
        };
        assert_eq!(old(&entry(Set::Nixos, "laptop"), &[]), "local");
        assert_eq!(old(&entry(Set::Home, "bob@laptop"), &[]), "local");
        assert_eq!(old(&entry(Set::Home, "bob"), &[]), "local");
        assert_eq!(old(&entry(Set::Home, "alice@laptop"), &[]), "alice@laptop");
        assert_eq!(old(&entry(Set::Nixos, "web-a"), &[]), "web-a");
        let given: TargetOverride = "web-a=root@10.0.0.1".parse().unwrap();
        assert_eq!(old(&entry(Set::Nixos, "web-a"), &[given]), "root@10.0.0.1");
        assert_eq!(
            old(&entry(Set::Nixos, "-web"), &[]),
            "target `-web` starts with `-`; give one with --target"
        );
    }

    #[test]
    fn names_after_the_hash() {
        assert_eq!(split(".").unwrap(), (".", None));
        assert_eq!(split(".#web-a").unwrap(), (".", Some("web-a")));
        assert_eq!(
            split("github:o/r#a#b").unwrap(),
            ("github:o/r", Some("a#b"))
        );
        assert_eq!(
            split(".#").unwrap_err().0,
            ".#: no configuration name after `#`"
        );
        let all = entries(&[
            (Set::Nixos, "web-a"),
            (Set::Home, "web-a"),
            (Set::Home, "bob"),
        ]);
        assert_eq!(all[by_name(&all, "bob").unwrap()].name, "bob");
        assert_eq!(
            by_name(&all, "x").unwrap_err().0,
            "#x: no such configuration"
        );
        assert_eq!(
            by_name(&all, "web-a").unwrap_err().0,
            "#web-a is in nixosConfigurations and homeConfigurations"
        );
    }
}
