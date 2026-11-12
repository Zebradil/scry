/// A store path's name and hash, and the pname/version split (spec 7.1).
pub mod name;
/// `nix path-info --json` output in both shapes Nix has emitted (spec 11).
pub mod pathinfo;
/// Version ordering: `builtins.compareVersions` plus the git-revision rule (spec 7.2).
pub mod version;
