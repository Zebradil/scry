//! The binary's exit codes and `--version`, end to end.

use std::process::{Command, Output};

fn scry(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scry"))
        .args(args)
        .output()
        .expect("runs scry")
}

#[test]
fn version_names_the_revision() {
    let output = scry(&["--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let rev = stdout
        .trim_end()
        .strip_prefix(concat!("scry ", env!("CARGO_PKG_VERSION"), " ("))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("unexpected version line: {stdout:?}"));
    assert!(!rev.is_empty());
}

#[test]
fn fatal_error_exits_1_or_2_with_exit_code() {
    let output = scry(&["completions", "bash"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "scry: not implemented yet\n"
    );
    // A path that is not a toplevel fails its configuration; only `sh` runs.
    let paths = [
        "paths",
        "/nonexistent-old",
        "/nonexistent-new",
        "--format",
        "json",
    ];
    let output = scry(&paths);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("/nonexistent-new is not in the store on this machine"),
        "{stdout}"
    );
    let with_exit_code = [&paths[..], &["--exit-code"]].concat();
    assert_eq!(scry(&with_exit_code).status.code(), Some(2));
}

#[test]
fn usage_error_exits_2_with_and_without_exit_code() {
    assert_eq!(scry(&["flake", "--jobs", "0"]).status.code(), Some(2));
    assert_eq!(
        scry(&["flake", "--exit-code", "--target", "web-a"])
            .status
            .code(),
        Some(2)
    );
}
