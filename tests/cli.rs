//! The binary's exit codes and `--version`, end to end.

use std::process::{Command, Output};

fn scry(args: &[&str]) -> Output {
    scry_with(&[], args)
}

fn scry_with(env: &[(&str, &str)], args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scry"))
        .args(args)
        .env_remove("SCRY_FIXTURE")
        .env_remove("COMPLETE")
        .envs(env.iter().copied())
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
    let output = scry_with(&[("NIX_SSHOPTS", "-o 'x")], &["paths", "/a", "/b"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "scry: while splitting NIX_SSHOPTS '-o 'x': unterminated single quote\n"
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

#[test]
fn fixture_mode_runs_on_the_synthetic_fleet() {
    let fixture = [("SCRY_FIXTURE", "1")];
    let output = scry_with(&fixture, &["colmena", "--format", "json"]);
    // `mail-a` fails to evaluate and `web-c` is unreachable.
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<&str> = report["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["db-a", "mail-a", "web-a", "web-b", "web-c"]);
    assert!(!report["shared"].as_array().unwrap().is_empty());

    let output = scry_with(&fixture, &["flake", "--base", "main", "--on", "web-*"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("web-c · unknown (1a2b3c4 -> working tree)\n  not present at base"),
        "{stdout}"
    );
}

#[test]
fn completion_scripts_and_candidates() {
    for shell in ["bash", "zsh", "fish"] {
        let output = scry(&["completions", shell]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("COMPLETE"));
    }
    let env = [("COMPLETE", "fish"), ("SCRY_FIXTURE", "1")];
    let output = scry_with(&env, &["--", "scry", "colmena", "--on", "@w"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "@web\n");
    assert!(output.stderr.is_empty());

    // The real seam, which would log every command under SCRY_DEBUG.
    let env = [("COMPLETE", "fish"), ("SCRY_DEBUG", "1")];
    let output = scry_with(&env, &["--", "scry", "colmena", "--base", ""]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

/// The bash script in bash, which splits words at `@`, `:` and `=`: `$2` is bash's current
/// word, and what the script puts in COMPREPLY replaces it, quoted as typed and with the
/// space after a finished word. Needs `bash` on `PATH`; the flake's check inputs provide it.
#[test]
fn bash_completes_across_its_word_breaks() {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["my dir", "plain"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
    }
    for file in ["my file.txt", "it's.nix"] {
        std::fs::write(dir.path().join(file), "").unwrap();
    }
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_scry"))
        .parent()
        .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let script = r#"
        eval "$(scry completions bash)"
        t() { COMP_LINE=$1; COMP_POINT=${#1}; _scry scry "$2" x; printf '%s|' "${COMPREPLY[@]}"; echo; }
        t 'scry colmena --on @w' w
        t 'scry colmena --on web-a,@e' e
        t 'scry colmena --on=db' db
        t 'scry flake github:o/r#web-a' 'o/r#web-a'
        t 'scry flake --base m' m
        t 'scry paths my\ d' 'my\ d'
        t 'scry paths my' my
        t "scry paths 'my d" 'my d'
        t 'scry paths it' it
        t 'scry paths pl' pl
    "#;
    let output = Command::new("bash")
        .args(["-c", script])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("SCRY_FIXTURE", "1")
        .env_remove("COMPLETE")
        .output()
        .expect("bash runs");
    let expected = [
        "web |",
        "eu |",
        "db-a |",
        "o/r#web-a |",
        "main |",
        r"my\ dir/|",
        r"my\ dir/|my\ file.txt |",
        "my dir/|",
        r"it\'s.nix |",
        "plain/|",
    ];
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected.map(|line| format!("{line}\n")).concat(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
