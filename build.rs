//! Stamps the git revision into the binary as `SCRY_REV` for `scry --version` (spec 4.6).

use std::process::Command;

fn main() {
    // Set by the flake: `nix build` copies the source without `.git`.
    println!("cargo::rerun-if-env-changed=SCRY_GIT_REV");
    let rev = std::env::var("SCRY_GIT_REV")
        .ok()
        .filter(|rev| !rev.is_empty())
        .or_else(git_rev)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo::rustc-env=SCRY_REV={rev}");
}

/// `<short rev>`, with `-dirty` when tracked files differ from HEAD, the form `self.dirtyShortRev`
/// has in the flake.
fn git_rev() -> Option<String> {
    // Any rerun-if directive turns off cargo's default of rerunning on every package change, so
    // list what moves the revision or the dirty flag: HEAD, the branch ref, the index, the sources.
    let mut watched = vec!["HEAD".to_owned(), "index".to_owned()];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    for path in ["build.rs", "Cargo.toml", "Cargo.lock", "src", "tests"] {
        println!("cargo::rerun-if-changed={path}");
    }

    let rev = git(&["rev-parse", "--short", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Some(if dirty { format!("{rev}-dirty") } else { rev })
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
