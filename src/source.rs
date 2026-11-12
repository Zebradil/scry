/// `scry colmena`: two-step `colmena eval`, targets from deployment options (spec 4.2).
pub mod colmena;
/// `scry flake`: listing, default selection for this machine, `REF#name`, targets (spec 4.2).
pub mod flake;
/// `scry paths`: two toplevels on one machine (spec 4.2).
pub mod paths;
/// `--on` patterns: whole-name globs and `@tag`s, unmatched patterns as usage errors (spec 4.1).
pub mod select;
