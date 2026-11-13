//! Step 3 of spec 6.2: the contents of changed files, both sides' paths on one machine in one
//! `tar -c -h -f - -T -` stream, through `gzip -1` when the probe found it, unpacked here.

use std::io::Read;

use anyhow::{Context, Result};

use super::SCRIPT;
use crate::exec::{Command, Exec, ExecError, Output};
use crate::files::compare::Contents;
use crate::transport::machine::Machine;

pub fn command(paths: &[String], gzip: bool) -> Command {
    let mut command = SCRIPT.command(["content", "", gzip_arg(gzip)]);
    command.stdin = lines(paths.iter().map(String::as_str));
    command
}

/// Fetches `paths`; nothing to fetch costs no round trip. GNU `tar` unescapes backslashes in
/// `-T` names, so it would fail on such a path or archive another file: those stay not
/// fetched.
pub async fn fetch(
    exec: &dyn Exec,
    machine: &Machine,
    paths: &[String],
    gzip: bool,
) -> Result<Contents> {
    let paths: Vec<String> = paths
        .iter()
        .filter(|p| !p.contains('\\'))
        .cloned()
        .collect();
    if paths.is_empty() {
        return Ok(Contents::new());
    }
    let output = exec.run(machine, command(&paths, gzip)).await?;
    if let Some(error) = tar_failure(machine, &output).or_else(|| super::unexplained(&output)) {
        return Err(error.into());
    }
    unpack(&output.stdout, gzip)
}

pub(crate) fn gzip_arg(gzip: bool) -> &'static str {
    if gzip { "gzip" } else { "" }
}

pub(crate) fn lines<'a>(items: impl Iterator<Item = &'a str>) -> Vec<u8> {
    items
        .flat_map(|i| [i.as_bytes(), b"\n"])
        .flatten()
        .copied()
        .collect()
}

/// `tar`'s own failure, which the script writes to stderr as `::scry failed tar <status>`.
pub(crate) fn tar_failure(machine: &Machine, output: &Output) -> Option<ExecError> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = stderr
        .lines()
        .find_map(|l| l.strip_prefix("::scry failed tar "))?;
    let rest: String = stderr
        .lines()
        .filter(|l| !l.starts_with(super::SENTINEL))
        .map(|l| format!("{l}\n"))
        .collect();
    Some(super::failure(machine, "tar", status, &rest))
}

/// File contents by absolute path. `tar` strips the leading `/` from member names, and stores
/// a file it already archived under another name (a hard link, common in an optimised store)
/// as a link to the first.
pub(crate) fn unpack(stream: &[u8], gzip: bool) -> Result<Contents> {
    let mut bytes = Vec::new();
    let stream = if gzip {
        flate2::read::GzDecoder::new(stream)
            .read_to_end(&mut bytes)
            .context("content stream is not gzip")?;
        &bytes[..]
    } else {
        stream
    };
    let mut contents = Contents::new();
    let mut archive = tar::Archive::new(stream);
    for entry in archive.entries().context("content stream is not tar")? {
        let mut entry = entry.context("content stream is not tar")?;
        let path = absolute(&entry.path_bytes());
        match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .context("content stream is cut short")?;
                contents.insert(path, data);
            }
            tar::EntryType::Link => {
                let target = entry
                    .link_name_bytes()
                    .map(|name| absolute(&name))
                    .and_then(|name| contents.get(&name).cloned());
                if let Some(data) = target {
                    contents.insert(path, data);
                }
            }
            _ => {}
        }
    }
    Ok(contents)
}

fn absolute(name: &[u8]) -> String {
    format!("/{}", String::from_utf8_lossy(name).trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::ok;

    /// The script against this machine's `tar` and `gzip` (bsdtar on macOS, GNU tar in the Nix
    /// build), with a hard link and a file reached through a symlink.
    #[test]
    fn real_tar_stream_unpacks() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name).to_str().unwrap().to_owned();
        std::fs::write(p("a"), "alpha\n").unwrap();
        std::fs::hard_link(p("a"), p("b")).unwrap();
        std::os::unix::fs::symlink(p("a"), p("l")).unwrap();
        std::fs::write(p("bin"), [0u8, 1, 2]).unwrap();
        let paths = [p("a"), p("b"), p("l"), p("bin")];
        for gzip in [false, true] {
            let command = command(&paths, gzip);
            let system = if cfg!(target_os = "macos") {
                "/usr/bin:/bin:"
            } else {
                ""
            };
            let mut child = std::process::Command::new(&command.program)
                .args(&command.args)
                .env(
                    "PATH",
                    format!("{system}{}", std::env::var("PATH").unwrap()),
                )
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&command.stdin)
                .unwrap();
            let out = child.wait_with_output().unwrap();
            let output = Output {
                code: out.status.code(),
                stdout: out.stdout,
                stderr: out.stderr,
            };
            assert!(
                tar_failure(&Machine::Local, &output).is_none(),
                "{output:?}"
            );
            let contents = unpack(&output.stdout, gzip).unwrap();
            // The temporary directory may sit behind a symlink (`/tmp` on macOS); names are as
            // given.
            for path in &paths[..3] {
                assert_eq!(contents[path], b"alpha\n", "{path}");
            }
            assert_eq!(contents[&paths[3]], [0, 1, 2]);
        }
    }

    #[test]
    fn tar_failure_and_missing_tar() {
        let mut output = ok("");
        output.stderr = b"tar: Removing leading `/' from member names\n\
tar: /nix/store/x: Cannot stat: No such file or directory\n::scry failed tar 2\n"
            .to_vec();
        assert_eq!(
            tar_failure(&Machine::Local, &output).unwrap().to_string(),
            "`tar` failed (exit 2): tar: /nix/store/x: Cannot stat: No such file or directory"
        );
        output.stderr = b"sh: 1: tar: not found\n::scry failed tar 127\n".to_vec();
        assert_eq!(
            tar_failure(&Machine::Local, &output).unwrap().to_string(),
            "`tar` not found on this machine"
        );
        output.stderr = b"tar: Removing leading '/' from member names\n".to_vec();
        assert!(tar_failure(&Machine::Local, &output).is_none());
    }
}
