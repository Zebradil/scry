/// This machine vs a target, the machine notion every command runs on.
pub mod machine;
/// Remote script helpers: single-quoting, the `PATH` preamble, the no-backslash rule (spec 6.1).
pub mod script;
/// `ssh` argument order, `NIX_SSHOPTS`, connection sharing and master cleanup (spec 6.1).
pub mod ssh;
