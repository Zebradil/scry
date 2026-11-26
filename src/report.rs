//! The JSON form is a public interface (spec 8.1): renaming, removing or changing the meaning of
//! a field bumps [`SCHEMA_VERSION`], adding one does not. Fields beyond the spec are listed in
//! ADR 0006.
//!
//! Where the spec ties a field's presence to another field (`error` only on failures, sections
//! only on `done`, `base`/`head` only in revision mode), the types carry that rule in an enum, so
//! a report that breaks it cannot be built.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// One run's result. Text, JSON and the TUI render only this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Always [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `CARGO_PKG_VERSION`.
    pub tool_version: String,
    #[serde(flatten)]
    pub mode: Mode,
    /// In name order.
    pub configurations: Vec<Configuration>,
    /// Changes found identically in two or more `done` configurations (spec 8.2).
    pub shared: Vec<SharedChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Mode {
    Deployed,
    Revision { base: Base, head: Head },
    Paths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    /// `--base` as typed.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The full commit hash it resolved to.
    pub rev: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// The full hash of `HEAD`.
    pub rev: String,
    /// The working tree has uncommitted changes.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Configuration {
    pub name: String,
    pub source: Source,
    /// `unknown` until the new side's probe has run.
    pub kind: ClosureKind,
    /// Where the old side lives in deployed and paths mode, as `[user@]host`; absent for this
    /// machine and in revision mode. Known from the source before any side resolves, so the
    /// header can name it for a configuration that failed early (spec 8.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(flatten)]
    pub status: Status,
    /// `None` until resolved.
    pub old: Option<Side>,
    /// `None` until resolved.
    pub new: Option<Side>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Colmena,
    Flake,
    Paths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClosureKind {
    Nixos,
    NixDarwin,
    HomeManager,
    Unknown,
}

/// A configuration's final state, serialised as `status` plus the fields that state carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Status {
    /// Compared; old and new are different store paths.
    Done(Box<Sections>),
    /// Old and new resolve to the same store path.
    NoChanges,
    /// `error` is the message chain joined by `": "`.
    Error { error: String },
    /// The fetch phase outlived `--timeout`. `error` reads `timeout after N s`.
    Timeout { error: String },
    /// Revision mode: the configuration does not exist at base.
    AbsentAtBase,
}

impl Status {
    /// Counts as failed for the exit code (spec 4.7).
    pub fn failed(&self) -> bool {
        matches!(self, Self::Error { .. } | Self::Timeout { .. })
    }

    /// Old differs from new, which includes a configuration absent at base (spec 4.7).
    pub fn has_changes(&self) -> bool {
        matches!(self, Self::Done(_) | Self::AbsentAtBase)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sections {
    pub packages: Packages,
    pub files: Files,
    /// `None` (JSON `null`) for kinds other than NixOS.
    pub boot: Option<Boot>,
}

/// One toplevel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    /// Full store path of the toplevel.
    pub path: String,
    /// The trimmed marker file (spec 5); `None` for an unknown kind or before the probe.
    pub system_version: Option<String>,
    /// `local`, or the target as in `root@web-a`.
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packages {
    /// Closure size in bytes, summed `narSize`.
    pub size_old: u64,
    pub size_new: u64,
    /// By kind in [`PackageChangeKind`] order, then by pname (spec 7.2).
    pub changes: Vec<PackageChange>,
    pub rebuilt: Rebuilt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    pub kind: PackageChangeKind,
    pub pname: String,
    /// The full set of the pname's versions in the old closure, in byte order; empty when added.
    /// A versionless name has the version `""`. With several versions on a side, the text shows
    /// the set difference, old-only `->` new-only (spec 7.2); the full sets stay here.
    pub old_versions: Vec<String>,
    /// The full set in the new closure, in byte order; empty when removed.
    pub new_versions: Vec<String>,
    pub size_delta: i64,
}

/// Declared in display order, so sorting by kind gives the order of spec 7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageChangeKind {
    Upgraded,
    Downgraded,
    Changed,
    Added,
    Removed,
    Resized,
}

/// Packages with the same version set and new paths, collapsed in text (spec 7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rebuilt {
    pub count: usize,
    /// Always 0, as is each item's: a rebuild that changed size is resized (ADR 0032). Kept so
    /// schema version 1 stays valid.
    pub size_delta: i64,
    /// By pname.
    pub items: Vec<RebuiltPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuiltPackage {
    pub pname: String,
    pub versions: Vec<String>,
    pub size_delta: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Files {
    /// In display order (spec 7.3): by path in byte order, followed references right after
    /// their referrer. Churned changes are included and grouped by `class` when rendered.
    pub changes: Vec<FileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Relative to the toplevel, as in `etc/nix/nix.conf`. For a followed reference, the
    /// referrer's path; for a small rebuild without a referrer, `name/bin/script`.
    pub path: String,
    /// The followed chain after `path`, as in `["app-start/bin/app-start"]`; empty otherwise.
    pub via: Vec<String>,
    pub status: FileStatus,
    /// `real` for anything but a modified text file.
    pub class: ChurnClass,
    /// `None` where the path is absent.
    pub type_old: Option<FileType>,
    pub type_new: Option<FileType>,
    /// Fetched content is binary on either side; shown as `[B]` when modified.
    pub binary: bool,
    /// `false` when either side is over `--max-file-size`.
    pub content_fetched: bool,
    /// Bytes; `None` where the side has no regular file.
    pub size_old: Option<u64>,
    pub size_new: Option<u64>,
    /// Target text of an external or dangling link.
    pub link_old: Option<String>,
    pub link_new: Option<String>,
    /// Empty when there is no text to show.
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileStatus {
    Modified,
    Added,
    Removed,
    LinkChanged,
    TypeChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChurnClass {
    Real,
    HashOnly,
    VersionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileType {
    File,
    Directory,
    ExternalLink,
    DanglingLink,
}

/// A unified diff hunk, `@@ -old_start,old_lines +new_start,new_lines @@`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub op: LineOp,
    /// Without the line terminator.
    pub text: String,
    /// UTF-8 byte ranges `[start, end)` of the tokens shown bold.
    pub spans: Vec<(usize, usize)>,
    /// Paired with a line that differs from it only in store hashes: shown dimmed, without
    /// bold (spec 7.3). Serialised only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hash_only: bool,
    /// The file's last line, without a final newline: followed by `\ No newline at end of
    /// file`, as in `diff -u`. Serialised only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineOp {
    #[serde(rename = " ")]
    Context,
    #[serde(rename = "-")]
    Removed,
    #[serde(rename = "+")]
    Added,
}

/// NixOS only. An unchanged item is `None`; the section is hidden when all are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boot {
    pub kernel: Option<LinkChange>,
    pub initrd: Option<LinkChange>,
    pub kernel_modules: Option<LinkChange>,
    pub kernel_params: Option<ParamsChange>,
    pub specialisations: Option<Specialisations>,
}

/// A boot link whose target changed, by store name without hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkChange {
    /// `None` where the link is missing, shown as `(none)`.
    pub old: Option<String>,
    pub new: Option<String>,
    /// Same name, new hash.
    pub rebuilt: bool,
}

/// `kernel-params`, whitespace runs folded to one space and trimmed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParamsChange {
    pub old: String,
    pub new: String,
}

/// Specialisation names, each list sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Specialisations {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Same name, different target.
    pub changed: Vec<String>,
}

/// A change listed once with every configuration that has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedChange {
    pub item: SharedItem,
    /// Two or more, in name order. `(also in N)` is this length minus one.
    pub configurations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "section", content = "change", rename_all = "kebab-case")]
pub enum SharedItem {
    /// Size deltas are not compared, so they are not part of the item.
    Packages(SharedPackageChange),
    /// Class `real` only. The change as it appears in the first configuration by name.
    Files(FileChange),
    Boot(BootChange),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SharedPackageChange {
    pub kind: PackageChangeKind,
    pub pname: String,
    pub old_versions: Vec<String>,
    pub new_versions: Vec<String>,
}

/// One row of the Boot section.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "kebab-case")]
pub enum BootChange {
    Kernel(LinkChange),
    Initrd(LinkChange),
    KernelModules(LinkChange),
    KernelParams(ParamsChange),
    Specialisation {
        name: String,
        status: SpecialisationStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecialisationStatus {
    Added,
    Removed,
    Changed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The example of spec 8.1, elisions filled in.
    const SPEC_EXAMPLE: &str = r#"{
      "schema_version": 1,
      "tool_version": "1.0.0",
      "mode": "deployed",
      "configurations": [
        {
          "name": "web-a",
          "source": "colmena",
          "kind": "nixos",
          "status": "done",
          "old": { "path": "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-nixos-system-web-a-25.05.20250801.6027c30", "system_version": "25.05.20250801.6027c30", "location": "root@web-a" },
          "new": { "path": "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-nixos-system-web-a-25.11.20251001.9a8b7c6", "system_version": "25.11.20251001.9a8b7c6", "location": "local" },
          "packages": {
            "size_old": 1288490188, "size_new": 1331439861,
            "changes": [ { "kind": "upgraded", "pname": "openssl", "old_versions": ["3.0.13"], "new_versions": ["3.0.14"], "size_delta": 100 } ],
            "rebuilt": { "count": 412, "size_delta": 0, "items": [ { "pname": "bash", "versions": ["5.2p37"], "size_delta": 0 } ] }
          },
          "files": {
            "changes": [
              {
                "path": "etc/nix/nix.conf", "via": [], "status": "modified", "class": "real",
                "type_old": "file", "type_new": "file", "binary": false, "content_fetched": true,
                "size_old": 812, "size_new": 829, "link_old": null, "link_new": null,
                "hunks": [ { "old_start": 11, "old_lines": 3, "new_start": 11, "new_lines": 3, "lines": [
                  { "op": " ", "text": "substituters = https://cache.example", "spans": [] },
                  { "op": "-", "text": "trusted-public-keys = cache-a:AAAA", "spans": [] },
                  { "op": "+", "text": "trusted-public-keys = cache-a:AAAA cache-b:BBBB", "spans": [[34, 47]] },
                  { "op": " ", "text": "max-jobs = auto", "spans": [] }
                ] } ]
              }
            ]
          },
          "boot": {
            "kernel": { "old": "linux-6.12.40", "new": "linux-6.12.48", "rebuilt": false },
            "initrd": { "old": "initrd-linux-6.12.40", "new": "initrd-linux-6.12.48", "rebuilt": false },
            "kernel_modules": null,
            "kernel_params": null,
            "specialisations": { "added": ["debug"], "removed": [], "changed": [] }
          }
        },
        { "name": "web-b", "source": "colmena", "kind": "unknown", "status": "error", "error": "ssh root@web-b: connect to host web-b port 22: Connection refused", "old": null, "new": { "path": "/nix/store/h4qa0r1sxv7lgm2c9dyk5nfzbw8p6ij3-nixos-system-web-b-25.11.20251001.9a8b7c6", "system_version": null, "location": "local" } }
      ],
      "shared": [
        { "item": { "section": "packages", "change": { "kind": "upgraded", "pname": "openssl", "old_versions": ["3.0.13"], "new_versions": ["3.0.14"] } }, "configurations": ["web-a", "web-c"] }
      ]
    }"#;

    fn round_trip(value: &Value) -> Value {
        let report: Report = serde_json::from_value(value.clone()).expect("parses");
        serde_json::to_value(&report).expect("serialises")
    }

    #[test]
    fn spec_example_round_trips() {
        let value: Value = serde_json::from_str(SPEC_EXAMPLE).unwrap();
        assert_eq!(round_trip(&value), value);
    }

    #[test]
    fn other_statuses_modes_and_shared_items_round_trip() {
        let value: Value = serde_json::from_str(
            r#"{
            "schema_version": 1,
            "tool_version": "1.0.0",
            "mode": "revision",
            "base": { "ref": "main", "rev": "6027c30aa1b2c3d4e5f60718293a4b5c6d7e8f90" },
            "head": { "rev": "9a8b7c6aa1b2c3d4e5f60718293a4b5c6d7e8f90", "dirty": true },
            "configurations": [
                { "name": "db-a", "source": "flake", "kind": "nixos", "status": "no-changes",
                  "old": { "path": "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-x", "system_version": "25.11", "location": "local" },
                  "new": { "path": "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-x", "system_version": "25.11", "location": "local" } },
                { "name": "db-b", "source": "flake", "kind": "unknown", "target": "root@db-b", "status": "timeout",
                  "error": "timeout after 60 s", "old": null, "new": null },
                { "name": "db-c", "source": "flake", "kind": "home-manager", "status": "absent-at-base",
                  "old": null, "new": { "path": "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-home-manager-generation", "system_version": "25.11", "location": "local" } },
                { "name": "db-d", "source": "flake", "kind": "nix-darwin", "status": "done",
                  "old": { "path": "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-darwin-system-25.11.a", "system_version": "25.11.a", "location": "local" },
                  "new": { "path": "/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-darwin-system-25.11.b", "system_version": "25.11.b", "location": "local" },
                  "packages": { "size_old": 1, "size_new": 1, "changes": [], "rebuilt": { "count": 0, "size_delta": 0, "items": [] } },
                  "files": { "changes": [
                      { "path": "etc/mtab", "via": [], "status": "type-changed", "class": "real",
                        "type_old": "file", "type_new": "external-link", "binary": false, "content_fetched": false,
                        "size_old": 3, "size_new": null, "link_old": null, "link_new": "/dev/null",
                        "hunks": [ { "old_start": 1, "old_lines": 1, "new_start": 1, "new_lines": 1, "lines": [
                            { "op": "-", "text": "a", "spans": [], "hash_only": true, "no_newline": true } ] } ] } ] },
                  "boot": null }
            ],
            "shared": [
                { "item": { "section": "boot", "change": { "item": "kernel", "old": "linux-6.12.40", "new": "linux-6.12.48", "rebuilt": false } },
                  "configurations": ["db-a", "db-d"] },
                { "item": { "section": "boot", "change": { "item": "specialisation", "name": "debug", "status": "added" } },
                  "configurations": ["db-a", "db-d"] }
            ]
        }"#,
        )
        .unwrap();
        assert_eq!(round_trip(&value), value);
    }

    #[test]
    fn absent_fields_stay_absent() {
        let report = Report {
            schema_version: SCHEMA_VERSION,
            tool_version: "1.0.0".into(),
            mode: Mode::Paths,
            configurations: vec![Configuration {
                name: "localhost".into(),
                source: Source::Paths,
                kind: ClosureKind::Unknown,
                target: None,
                status: Status::NoChanges,
                old: None,
                new: None,
            }],
            shared: vec![],
        };
        let value = serde_json::to_value(&report).unwrap();
        for key in ["base", "head"] {
            assert!(value.get(key).is_none(), "{key}");
        }
        let configuration = &value["configurations"][0];
        for key in ["target", "error", "packages", "files", "boot"] {
            assert!(configuration.get(key).is_none(), "{key}");
        }
        assert_eq!(configuration["old"], Value::Null);
    }
}
