# Glossary

Terms as used in the spec, code, issues and docs. Prefer these over synonyms.

- **Closure**: a store path plus everything it references, transitively.
- **Toplevel**: the store path at the root of a configuration's closure. NixOS:
  `nixos-system-<name>-<version>`, the output of `config.system.build.toplevel`. nix-darwin:
  `darwin-system-<version>`, same attribute. home-manager: `home-manager-generation`, the
  `activationPackage`.
- **Configuration**: one deployable unit that owns a toplevel: an entry of `nixosConfigurations`,
  `darwinConfigurations` or `homeConfigurations` in a flake, or a node of a colmena hive. The unit of
  selection, of reporting and of failure.
- **Source**: where configurations come from: a colmena hive (`scry colmena`), a flake (`scry flake`),
  or two store paths given by hand (`scry paths`).
- **Side**: one of the two toplevels being compared, **old** or **new**. Each side lives on one
  machine, its **location**: this machine or a target. The two sides of one configuration may live on
  different machines.
- **Target**: a remote machine reached with the `ssh` binary, written `[user@]host`.
- **Mode**: what the old side is.
  - **Deployed mode** (default): old is the current system on the configuration's target, new is the
    candidate built from the working tree.
  - **Revision mode** (`--base REV`): old is the same configuration built from git revision `REV`,
    new is the same configuration built from the working tree. No target is contacted.
- **Base / head**: in revision mode, the revision given with `--base` and the working tree.
- **Current system**: the toplevel a machine runs now for a closure kind: `/run/current-system` for
  NixOS and nix-darwin, the user's home-manager profile for home-manager.
- **Closure kind**: NixOS, nix-darwin, home-manager or unknown. Detected from a marker file at the top
  of the new toplevel (`nixos-version`, `darwin-version`, `hm-version`). Decides the followed trees,
  whether a Boot section exists and the default old side.
- **Section**: one layer of a configuration's diff: **Packages**, **Files** or **Boot**.
- **Package**: all store paths of a closure that share one *pname*, with their versions, paths and
  summed size.
- **Package change**: upgraded, downgraded, changed, added, removed, resized, or rebuilt.
- **Rebuilt**: same pname and same version set, different store paths. Collapsed into one summary line.
- **Resized**: a rebuilt package whose size moved by at least 1 MiB and at least 10 % of its old size.
  Listed instead of collapsed.
- **Small rebuild**: a rebuilt package with exactly one path per side, each at most 1 MiB: a script
  rather than a package, so its `bin/` is compared by content.
- **Followed tree**: a directory under the toplevel whose files are compared by content, store symlinks
  dereferenced. An allow-list per closure kind.
- **File change**: a difference in one path of a followed tree, a toplevel regular file, or a followed
  reference: modified, added, removed, type change, or link change.
- **External link**: a symlink in a followed tree whose target is outside `/nix/store`, such as
  `etc/mtab -> /proc/mounts` or a unit masked with `-> /dev/null`. Compared by its target text, never
  followed.
- **Link depth**: the longest chain of store symlinks under a followed tree. Each level costs one
  listing round trip.
- **Hash-only change**: a modified file that is equal on both sides once every store hash is replaced
  by a placeholder.
- **Version-only change**: a modified file that is not hash-only but is equal once the version in every
  store path name is replaced too.
- **Churn**: hash-only and version-only changes together. Collapsed by default.
- **Followed reference**: a store path referenced from a modified file whose old and new versions differ;
  compared by content and shown as `referrer → name`.
- **Shared change**: a change that appears identically in two or more configurations of one run.
- **Report**: the structured result of a run: every configuration with its status and sections, plus the
  shared changes. Every output format renders a report.
- **Command seam**: the single interface through which every external command runs, real or faked.
