//! Step 1 of spec 6.2, one round trip per machine for every side on it: the toplevel resolved
//! with `cd -P`, its first listing level, the marker files and `kernel-params`, its closure from
//! `nix path-info --json -r`, and whether `gzip` exists. Boot links and specialisations come from
//! the listing's `ls -ldn` lines, so they cost no extra command.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use super::{SCRIPT, SENTINEL, Toplevel};
use crate::boot;
use crate::exec::{Command, Exec, Output};
use crate::report::ClosureKind;
use crate::store::pathinfo::{self, PathInfo};
use crate::transport::machine::Machine;
use crate::tree::{self, LISTING, Listed};

/// The current system of NixOS, nix-darwin and the unknown kind (spec 4.3).
pub const CURRENT_SYSTEM: &str = "/run/current-system";

/// One side to probe: a path, or `None` for the home-manager current system, which the script
/// finds from the machine's `XDG_STATE_HOME`, `HOME` and `USER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request(pub Option<String>);

impl Request {
    pub fn of(toplevel: &Toplevel) -> Self {
        Self(match toplevel {
            Toplevel::Path { path, .. } => Some(path.clone()),
            Toplevel::Current(ClosureKind::HomeManager) => None,
            Toplevel::Current(_) => Some(CURRENT_SYSTEM.to_owned()),
        })
    }
}

/// What one probe round trip found on a machine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// In request order.
    pub sides: Vec<ProbedSide>,
    /// By resolved toplevel; two sides that resolve to one toplevel share an entry.
    pub toplevels: HashMap<String, ToplevelData>,
    pub gzip: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProbedSide {
    /// The path tried: as requested, or the home-manager profile the script picked.
    pub path: String,
    /// `cd -P <path> && pwd -P`; `None` when that failed.
    pub toplevel: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ToplevelData {
    /// [`LISTING`] output for the toplevel itself: the walk's first level.
    pub listing: Vec<u8>,
    /// Marker files and `kernel-params` that exist, by name.
    pub files: BTreeMap<String, String>,
    pub path_info: PathInfo,
}

/// Marker files in detection order (spec 5).
const MARKERS: [(&str, ClosureKind); 3] = [
    ("nixos-version", ClosureKind::Nixos),
    ("darwin-version", ClosureKind::NixDarwin),
    ("hm-version", ClosureKind::HomeManager),
];

impl ToplevelData {
    pub fn kind(&self) -> ClosureKind {
        MARKERS
            .iter()
            .find(|(file, _)| self.files.contains_key(*file))
            .map_or(ClosureKind::Unknown, |&(_, kind)| kind)
    }

    /// The trimmed marker file (spec 5); `None` for an unknown kind.
    pub fn system_version(&self) -> Option<String> {
        MARKERS
            .iter()
            .find_map(|(file, _)| self.files.get(*file))
            .map(|text| text.trim().to_owned())
    }

    /// Boot links, `kernel-params` and specialisations of the toplevel at `toplevel`.
    pub fn boot(&self, toplevel: &str) -> boot::Probe {
        let mut probe = boot::Probe {
            kernel_params: self.files.get("kernel-params").cloned(),
            ..boot::Probe::default()
        };
        let listing = String::from_utf8_lossy(&self.listing);
        let specialisation = format!("{toplevel}/specialisation/");
        for (path, listed) in listing.lines().filter_map(tree::parse_line) {
            let Listed::Link(target) = listed else {
                continue;
            };
            let slot = match path.strip_prefix(toplevel) {
                Some("/kernel") => &mut probe.kernel,
                Some("/initrd") => &mut probe.initrd,
                Some("/kernel-modules") => &mut probe.kernel_modules,
                _ => {
                    if let Some(name) = path.strip_prefix(&specialisation)
                        && !name.contains('/')
                    {
                        probe.specialisations.insert(name.to_owned(), target);
                    }
                    continue;
                }
            };
            *slot = Some(target);
        }
        probe
    }
}

pub fn command(requests: &[Request]) -> Command {
    let mut command = SCRIPT.command(["probe", LISTING.text()]);
    for Request(path) in requests {
        command.stdin.extend(path.as_deref().unwrap_or("").bytes());
        command.stdin.push(b'\n');
    }
    command
}

pub async fn run(exec: &dyn Exec, machine: &Machine, requests: Vec<Request>) -> Result<Probe> {
    let output = exec.run(machine, command(&requests)).await?;
    parse(machine, &output)
}

enum Section {
    None,
    Listing,
    File(String),
    PathInfo,
}

pub fn parse(machine: &Machine, output: &Output) -> Result<Probe> {
    let (stdout, failure) = super::split_failure(machine, &output.stdout);
    if let Some(error) = failure.or_else(|| super::unexplained(output)) {
        return Err(error.into());
    }
    let mut probe = Probe::default();
    let mut current: Option<String> = None;
    let mut section = Section::None;
    let mut path_info: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut files: BTreeMap<(String, String), Vec<u8>> = BTreeMap::new();
    for line in stdout.split(|&b| b == b'\n') {
        if let Some(rest) = line.strip_prefix(SENTINEL.as_bytes()) {
            let rest = String::from_utf8_lossy(rest);
            let (word, value) = rest.split_once(' ').unwrap_or((&rest, ""));
            section = Section::None;
            match word {
                "side" => probe.sides.push(ProbedSide::default()),
                "path" => last(&mut probe.sides).path = value.to_owned(),
                "toplevel" => {
                    last(&mut probe.sides).toplevel = Some(value.to_owned());
                    probe.toplevels.entry(value.to_owned()).or_default();
                    current = Some(value.to_owned());
                }
                "listing" => section = Section::Listing,
                "file" => section = Section::File(value.to_owned()),
                "pathinfo" => section = Section::PathInfo,
                "gzip" => probe.gzip = true,
                _ => {}
            }
            continue;
        }
        let Some(toplevel) = &current else {
            continue;
        };
        let buffer = match &section {
            Section::None => continue,
            Section::Listing => &mut probe.toplevels.get_mut(toplevel).expect("added").listing,
            Section::File(name) => files.entry((toplevel.clone(), name.clone())).or_default(),
            Section::PathInfo => path_info.entry(toplevel.clone()).or_default(),
        };
        buffer.extend_from_slice(line);
        buffer.push(b'\n');
    }
    for ((toplevel, name), text) in files {
        let text = String::from_utf8_lossy(&text).into_owned();
        probe
            .toplevels
            .get_mut(&toplevel)
            .expect("added")
            .files
            .insert(name, text);
    }
    for (toplevel, json) in path_info {
        probe.toplevels.get_mut(&toplevel).expect("added").path_info = pathinfo::parse(&json)?;
    }
    Ok(probe)
}

fn last(sides: &mut Vec<ProbedSide>) -> &mut ProbedSide {
    if sides.is_empty() {
        sides.push(ProbedSide::default());
    }
    sides.last_mut().expect("not empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::ok;

    const T: &str = "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-nixos-system-web-a-26.05";
    const K: &str = "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-linux-6.12.48/bzImage";

    fn stdout() -> String {
        format!(
            "::scry side\n::scry path /run/current-system\n::scry toplevel {T}\n::scry listing\n\
             {T}\n{T}/specialisation\n\
             lrwxrwxrwx 1 0 0 {} Jan  1  1970 {T}/kernel -> {K}\n\
             lrwxrwxrwx 1 0 0 2 Jan  1  1970 {T}/specialisation/debug -> /x\n\
             1 6 {T}/kernel-params\n\
             ::scry file nixos-version\n26.05.20261102.7dcdd87\n\n\
             ::scry file kernel-params\nquiet  loglevel=4\n\
             ::scry pathinfo\n{{\"{T}\":{{\"narSize\":5}}}}\n\n\
             ::scry side\n::scry path /nix/store/missing\n\
             ::scry side\n::scry path {T}\n::scry toplevel {T}\n::scry gzip\n",
            K.len()
        )
    }

    #[test]
    fn parses_sides_toplevels_and_boot() {
        let probe = parse(&Machine::Local, &ok(stdout())).unwrap();
        assert_eq!(probe.sides.len(), 3);
        assert_eq!(probe.sides[0].toplevel.as_deref(), Some(T));
        assert_eq!(probe.sides[1].path, "/nix/store/missing");
        assert_eq!(probe.sides[1].toplevel, None);
        assert_eq!(probe.sides[2].toplevel.as_deref(), Some(T));
        assert!(probe.gzip);
        let data = &probe.toplevels[T];
        assert_eq!(data.kind(), ClosureKind::Nixos);
        assert_eq!(
            data.system_version().as_deref(),
            Some("26.05.20261102.7dcdd87")
        );
        assert_eq!(data.path_info.sizes[T], 5);
        let boot = data.boot(T);
        assert_eq!(boot.kernel.as_deref(), Some(K));
        assert_eq!(boot.initrd, None);
        assert_eq!(boot.kernel_params.as_deref(), Some("quiet  loglevel=4\n"));
        assert_eq!(boot.specialisations["debug"], "/x");
        assert!(data.listing.starts_with(format!("{T}\n").as_bytes()));
    }

    #[test]
    fn failures_carry_the_program() {
        let out = ok(format!(
            "::scry side\n::scry toplevel {T}\n::scry pathinfo\n::scry failed nix 1\n\
             warning: '--json' without '--json-format' is deprecated\nerror: cannot open connection\n"
        ));
        let error = parse(&Machine::Local, &out).unwrap_err();
        assert_eq!(
            error.to_string(),
            "`nix` failed (exit 1): error: cannot open connection"
        );
    }

    #[test]
    fn command_sends_paths_and_home_manager_on_stdin() {
        let requests = [
            Request::of(&Toplevel::Current(ClosureKind::HomeManager)),
            Request::of(&Toplevel::Current(ClosureKind::Nixos)),
        ];
        assert_eq!(command(&requests).stdin, b"\n/run/current-system\n");
    }
}
