//! The real seam. A local command runs its argv directly; a remote one runs
//! `ssh … <target> -- sh -c '…' sh '<argv>'…` over a shared connection (spec 6.1, ADR 0010).
//! A child with captured stderr leads its own process group, and dropping the command's future
//! kills the whole group, so `sh -c` scripts and tools such as `colmena` take their own
//! children with them (ADR 0012).

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::OnceCell;

use super::{BoxFuture, Command, Exec, ExecError, Output, Stderr};
use crate::transport::machine::{Machine, Remote};
use crate::transport::ssh::Sharing;
use crate::transport::{script, ssh};

/// How long stderr may stay open after the child was reaped. In the normal case the pipe holds
/// at most one buffer of unread output and reaches end of file at once.
const STDERR_GRACE: Duration = Duration::from_secs(1);

pub struct Process {
    ssh: String,
    nix_sshopts: Vec<String>,
    debug: bool,
    socket_dir: OnceCell<TempDir>,
    sockets: AtomicUsize,
    connections: Mutex<HashMap<Remote, Arc<ssh::Connection>>>,
}

impl Process {
    /// Reads `NIX_SSHOPTS` (spec 4.8) and `SCRY_DEBUG` (ADR 0004). Fails, fatally, when
    /// `NIX_SSHOPTS` does not split.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(
            "ssh",
            ssh::nix_sshopts(std::env::var("NIX_SSHOPTS").ok().as_deref())?,
            std::env::var_os("SCRY_DEBUG").is_some_and(|v| !v.is_empty()),
        ))
    }

    /// Without the `SCRY_DEBUG` log, for shell completion, whose stderr is the user's prompt.
    pub fn quiet(self) -> Self {
        Self {
            debug: false,
            ..self
        }
    }

    fn new(ssh: &str, nix_sshopts: Vec<String>, debug: bool) -> Self {
        Self {
            ssh: ssh.to_owned(),
            nix_sshopts,
            debug,
            socket_dir: OnceCell::new(),
            sockets: AtomicUsize::new(0),
            connections: Mutex::default(),
        }
    }

    async fn remote(
        &self,
        machine: &Machine,
        remote: &Remote,
        command: Command,
    ) -> Result<Output, ExecError> {
        let line = script::command_line(&command).map_err(|error| ExecError::Io {
            program: command.program.clone(),
            source: io::Error::new(io::ErrorKind::InvalidInput, error),
        })?;
        let connection = self.connection(remote);
        let sharing = connection
            .sharing
            .get_or_try_init(|| self.sharing(remote))
            .await?;
        let args = ssh::run_args(&self.nix_sshopts, remote, &sharing.options(), &line);
        let spawned = self.spawn(&self.ssh, &args, &command.stdin, command.stderr);
        let output = connection
            .run(
                spawned,
                |result| matches!(result, Ok(o) if o.code != Some(255)),
            )
            .await
            .map_err(|error| spawn_error(&self.ssh, error))?;
        classify(machine, output)
    }

    fn connection(&self, remote: &Remote) -> Arc<ssh::Connection> {
        let mut connections = self.connections.lock().expect("never poisoned");
        connections.entry(remote.clone()).or_default().clone()
    }

    /// `ssh -G` once per connection: the user's own `ControlPath` wins, else the tool's sharing.
    async fn sharing(&self, remote: &Remote) -> Result<Sharing, ExecError> {
        let args = ssh::config_args(&self.nix_sshopts, remote);
        let output = self
            .spawn(&self.ssh, &args, &[], Stderr::Capture)
            .await
            .map_err(|error| spawn_error(&self.ssh, error))?;
        if !output.success() {
            return Err(ssh_error(remote, &output.stderr));
        }
        if ssh::has_control_path(&String::from_utf8_lossy(&output.stdout)) {
            return Ok(Sharing::User);
        }
        let dir = self
            .socket_dir
            .get_or_try_init(|| async {
                // Not $TMPDIR: on macOS it is long enough to push socket paths past 104 bytes.
                tempfile::Builder::new()
                    .prefix("scry-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir_in("/tmp")
            })
            .await
            .map_err(|source| ExecError::Io {
                program: self.ssh.clone(),
                source,
            })?;
        let n = self.sockets.fetch_add(1, Ordering::Relaxed);
        Ok(Sharing::Own(dir.path().join(n.to_string())))
    }

    async fn spawn(
        &self,
        program: &str,
        args: &[String],
        stdin: &[u8],
        stderr: Stderr,
    ) -> io::Result<Output> {
        if self.debug {
            eprintln!("scry: debug: run {program} {args:?}");
        }
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(if stdin.is_empty() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let own_group = stderr == Stderr::Capture;
        if own_group {
            command.stderr(Stdio::piped()).process_group(0);
        } else {
            // A build stays in scry's foreground process group: in a background group, writing
            // to or setting up the terminal stops it (SIGTTOU), and builds have no timeout.
            command.stderr(Stdio::inherit());
        }
        let mut child = command.spawn()?;
        let mut group = KillGroup(child.id().filter(|_| own_group));
        let input = child.stdin.take();
        let mut out_pipe = child.stdout.take().expect("stdout is piped");
        let err_pipe = child.stderr.take();
        let write = async move {
            if let Some(mut input) = input {
                // A child may exit without reading all of its input.
                match input.write_all(stdin).await {
                    Err(error) if error.kind() != io::ErrorKind::BrokenPipe => return Err(error),
                    _ => {}
                }
            }
            Ok(())
        };
        let read_out = async move {
            let mut out = Vec::new();
            out_pipe.read_to_end(&mut out).await.map(|_| out)
        };
        let mut err = Vec::new();
        let read_err = async {
            if let Some(mut pipe) = err_pipe {
                // `read_buf` keeps what it has read when the grace period cuts it short.
                while pipe.read_buf(&mut err).await? != 0 {}
            }
            io::Result::Ok(())
        };
        let ((written, out, status), err_read) = {
            let main = async { tokio::join!(write, read_out, child.wait()) };
            tokio::pin!(main, read_err);
            tokio::select! {
                main = &mut main => {
                    // A process the child left behind may hold stderr open, such as ssh's
                    // `ControlPersist` master under `-v`.
                    let read = tokio::time::timeout(STDERR_GRACE, &mut read_err).await;
                    (main, read.unwrap_or(Ok(())))
                }
                read = &mut read_err => ((&mut main).await, read),
            }
        };
        let status = status?;
        // Reaped: the group id may belong to someone else from now on.
        group.0 = None;
        written?;
        let stdout = out?;
        err_read?;
        if self.debug {
            eprintln!(
                "scry: debug: {program} exited {:?}, stderr:\n{}",
                status.code(),
                String::from_utf8_lossy(&err)
            );
        }
        Ok(Output {
            code: status.code(),
            stdout,
            stderr: err,
        })
    }
}

impl Exec for Process {
    fn run<'a>(
        &'a self,
        machine: &'a Machine,
        command: Command,
    ) -> BoxFuture<'a, Result<Output, ExecError>> {
        Box::pin(async move {
            match machine {
                Machine::Local => {
                    let output = self
                        .spawn(
                            &command.program,
                            &command.args,
                            &command.stdin,
                            command.stderr,
                        )
                        .await
                        .map_err(|error| spawn_error(&command.program, error))?;
                    classify(machine, output)
                }
                Machine::Remote(remote) => self.remote(machine, remote, command).await,
            }
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let connections: Vec<_> = self
                .connections
                .lock()
                .expect("never poisoned")
                .iter()
                .map(|(remote, connection)| (remote.clone(), connection.clone()))
                .collect();
            for (remote, connection) in connections {
                if let Some(sharing @ Sharing::Own(_)) = connection.sharing.get()
                    && sharing.master_up()
                {
                    let args = ssh::exit_args(&self.nix_sshopts, &remote, &sharing.options());
                    let _ = self.spawn(&self.ssh, &args, &[], Stderr::Capture).await;
                }
            }
            if let Some(dir) = self.socket_dir.get() {
                let _ = std::fs::remove_dir_all(dir.path());
            }
        })
    }
}

/// Kills the process group led by the child unless disarmed.
struct KillGroup(Option<u32>);

impl Drop for KillGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0.and_then(|pid| libc::pid_t::try_from(pid).ok()) {
            // SAFETY: plain syscall; the child is not reaped yet, so the group is still ours.
            unsafe { libc::killpg(pid, libc::SIGKILL) };
        }
    }
}

fn spawn_error(program: &str, error: io::Error) -> ExecError {
    if error.kind() == io::ErrorKind::NotFound {
        ExecError::NotFound {
            program: program.to_owned(),
            machine: Machine::Local,
        }
    } else {
        ExecError::Io {
            program: program.to_owned(),
            source: error,
        }
    }
}

/// Turns the exit codes spec 9 words as errors into errors: 255 from `ssh`, 127 from a shell
/// that names the program it did not find. Any other 127, such as `xargs` failing to run its
/// utility, is an exit like any other. Shared with the fake, so both answer alike.
pub(crate) fn classify(machine: &Machine, output: Output) -> Result<Output, ExecError> {
    if let (Some(255), Machine::Remote(remote)) = (output.code, machine) {
        return Err(ssh_error(remote, &output.stderr));
    }
    if output.code == Some(127)
        && let Some(program) = missing_program(&output.stderr)
    {
        return Err(ExecError::NotFound {
            program,
            machine: machine.clone(),
        });
    }
    Ok(output)
}

fn ssh_error(remote: &Remote, stderr: &[u8]) -> ExecError {
    let stderr = String::from_utf8_lossy(stderr);
    let lines = || stderr.lines().filter(|l| !l.trim().is_empty());
    // ssh prints its verdict last, after warnings (`StrictHostKeyChecking=accept-new`) and
    // banners such as the `@@@` box of a changed host key.
    let line = lines()
        .rfind(|l| !l.starts_with("Warning:"))
        .or_else(|| lines().next_back())
        .unwrap_or("exit 255");
    ExecError::Ssh {
        target: remote.target.to_string(),
        line: line.strip_prefix("ssh: ").unwrap_or(line).to_owned(),
    }
}

/// The program a shell reports missing: `sh: 1: nix: not found` (dash),
/// `sh: line 1: nix: command not found` (bash), `sh: exec: nix: not found`.
fn missing_program(stderr: &[u8]) -> Option<String> {
    let stderr = String::from_utf8_lossy(stderr);
    stderr.lines().rev().find_map(|line| {
        let head = line
            .strip_suffix(": command not found")
            .or_else(|| line.strip_suffix(": not found"))?;
        let name = head.rsplit(": ").next()?;
        (!name.is_empty()).then(|| name.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use super::*;
    use crate::transport::script::Script;

    fn local() -> Process {
        Process::new("ssh", vec![], false)
    }

    fn remote(port: Option<u16>) -> Machine {
        Machine::Remote(Remote {
            target: "root@web-a".parse().unwrap(),
            port,
            ssh_options: vec![],
        })
    }

    fn sh(script: &'static str) -> Command {
        Script::new(script).command::<_, &str>([])
    }

    #[tokio::test]
    async fn local_stdout_stdin_and_exit_codes() {
        let process = local();
        let output = process
            .run(&Machine::Local, Command::new("printf", ["hi"]))
            .await
            .unwrap();
        assert_eq!(
            (output.code, output.stdout.as_slice()),
            (Some(0), &b"hi"[..])
        );

        // Larger than a pipe buffer, so writing and reading must overlap.
        let mut cat = Command::new("cat", [] as [&str; 0]);
        cat.stdin = vec![b'x'; 1 << 20];
        let output = process.run(&Machine::Local, cat.clone()).await.unwrap();
        assert_eq!(output.stdout, cat.stdin);

        let output = process
            .run(&Machine::Local, sh("echo oops >&2; exit 3"))
            .await
            .unwrap();
        assert_eq!(
            (output.code, output.stderr.as_slice()),
            (Some(3), &b"oops\n"[..])
        );
    }

    #[tokio::test]
    async fn local_missing_program() {
        let process = local();
        let error = process
            .run(&Machine::Local, Command::new("scry-no-such-program", ["x"]))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "`scry-no-such-program` not found on this machine"
        );
        let error = process
            .run(&Machine::Local, sh("scry-no-such-program"))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "`scry-no-such-program` not found on this machine"
        );
    }

    #[tokio::test]
    async fn dropping_the_future_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command =
            Script::new(r#"sleep 30 & echo $! > "$1"; wait"#).command([pid_file.to_str().unwrap()]);
        let process = local();
        let run = process.run(&Machine::Local, command);
        // Drop `run` only once the grandchild exists: a fixed delay races shell startup on a
        // loaded CI runner. The trailing newline marks the `echo` as complete.
        let read_pid = async {
            loop {
                match std::fs::read_to_string(&pid_file) {
                    Ok(text) if text.ends_with('\n') => break text.trim().parse::<libc::pid_t>(),
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        };
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                _ = run => panic!("the script exited before it was dropped"),
                pid = read_pid => pid.unwrap(),
            }
        })
        .await
        .expect("the script never wrote the grandchild's pid");
        for _ in 0..100 {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("grandchild {pid} survived");
    }

    /// The process group of a running child, read while the command runs.
    async fn group_of(stderr: Stderr) -> (libc::pid_t, libc::pid_t) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut command =
            Script::new(r#"echo $$ > "$1"; exec sleep 30"#).command([pid_file.to_str().unwrap()]);
        command.stderr = stderr;
        let process = local();
        let run = process.run(&Machine::Local, command);
        let pid = async {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let text = std::fs::read_to_string(&pid_file).unwrap_or_default();
                if let Ok(pid) = text.trim().parse::<libc::pid_t>() {
                    // SAFETY: plain syscall on a live child.
                    return (pid, unsafe { libc::getpgid(pid) });
                }
            }
        };
        tokio::select! {
            _ = run => panic!("the command ended"),
            ids = pid => ids,
        }
    }

    #[tokio::test]
    async fn only_captured_children_get_their_own_process_group() {
        let (pid, group) = group_of(Stderr::Capture).await;
        assert_eq!(group, pid);
        let (_, group) = group_of(Stderr::Inherit).await;
        // SAFETY: plain syscall.
        assert_eq!(group, unsafe { libc::getpgrp() });
    }

    #[tokio::test]
    async fn a_leftover_holding_stderr_does_not_hold_the_command() {
        let start = Instant::now();
        let output = local()
            .run(
                &Machine::Local,
                sh("echo err >&2; sleep 3 >/dev/null </dev/null &"),
            )
            .await
            .unwrap();
        assert!(start.elapsed() < Duration::from_millis(2500));
        assert_eq!(output.stderr, b"err\n");
    }

    #[test]
    fn classify_maps_spec_9_cases() {
        let out = |code, stderr: &str| Output {
            code: Some(code),
            stdout: vec![],
            stderr: stderr.into(),
        };
        for (stderr, line) in [
            (
                "ssh: connect to host web-a port 22: Connection refused\n",
                "connect to host web-a port 22: Connection refused",
            ),
            (
                "Warning: Permanently added 'web-a' (ED25519) to the list of known hosts.\n\
                 root@web-a: Permission denied (publickey).\n",
                "root@web-a: Permission denied (publickey).",
            ),
            (
                "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
                 @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!\n\
                 Host key for web-a has changed and you have requested strict checking.\n\
                 Host key verification failed.\n",
                "Host key verification failed.",
            ),
            ("", "exit 255"),
        ] {
            let error = classify(&remote(None), out(255, stderr)).unwrap_err();
            assert_eq!(error.to_string(), format!("ssh root@web-a: {line}"));
        }
        assert!(classify(&Machine::Local, out(255, "")).is_ok());
        assert!(classify(&remote(None), out(1, "")).is_ok());
        for stderr in [
            "sh: 1: exec: nix: not found",
            "sh: line 1: nix: command not found",
            "warning: x\nsh: nix: not found\n",
        ] {
            let error = classify(&remote(None), out(127, stderr)).unwrap_err();
            assert_eq!(
                error.to_string(),
                "`nix` not found on root@web-a",
                "{stderr:?}"
            );
        }
        for stderr in ["xargs: foo: No such file or directory\n", ""] {
            let output = classify(&remote(None), out(127, stderr)).unwrap();
            assert_eq!(output.code, Some(127));
        }
    }

    /// A stand-in `ssh` that runs the remote command line with the local `sh` as login shell,
    /// answers `-G`, creates and removes the tool's socket like a master would, and logs every
    /// call, so the remote path runs without a network.
    fn fake_ssh(dir: &Path) -> String {
        let path = dir.join("ssh");
        let log = dir.join("log");
        std::fs::write(
            &path,
            format!(
                r#"#!/bin/sh
echo "$*" >> "{log}"
socket=
for a; do case $a in ControlPath=/tmp/scry-*) socket=${{a#ControlPath=}};; esac; done
case " $* " in
  *" -G "*) case "$*" in *ControlPath=*) echo "controlpath /u/%C";; esac; exit 0;;
  *" -O exit "*) rm -f "$socket"; exit 0;;
  *unreachable*) echo "ssh: connect to host x port 22: Connection refused" >&2; exit 255;;
esac
[ -n "$socket" ] && : > "$socket"
for line; do :; done
exec sh -c "$line"
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn log(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("log"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn remote_commands_share_one_connection_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        let process = Process::new(&fake_ssh(dir.path()), vec![], false);
        for _ in 0..3 {
            let output = process
                .run(
                    &remote(None),
                    Command::new("printf", ["%s|", "a b", "$HOME"]),
                )
                .await
                .unwrap();
            assert_eq!(output.stdout, b"a b|$HOME|");
        }
        let error = process
            .run(
                &remote(None),
                Command::new("scry-no-such-program", [] as [&str; 0]),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "`scry-no-such-program` not found on root@web-a"
        );
        // Same target, other options: `%C` would be the same, the socket must not.
        process
            .run(&remote(Some(22)), Command::new("true", [] as [&str; 0]))
            .await
            .unwrap();

        let socket_dir = process.socket_dir.get().unwrap().path().to_owned();
        let mode = std::fs::metadata(&socket_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(socket_dir.to_str().unwrap().starts_with("/tmp/scry-"));

        process.shutdown().await;
        let calls = log(dir.path());
        assert_eq!(calls.iter().filter(|c| c.starts_with("-G ")).count(), 2);
        let first = format!("ControlPath={}/0 ", socket_dir.display());
        let second = format!("ControlPath={}/1 ", socket_dir.display());
        let runs: Vec<_> = calls.iter().filter(|c| !c.starts_with("-G ")).collect();
        assert!(runs[..4].iter().all(|c| c.contains(&first)), "{calls:#?}");
        assert!(
            runs[4].contains(&second) && runs[4].contains("-p 22"),
            "{calls:#?}"
        );
        let exits: Vec<_> = runs
            .iter()
            .filter(|c| c.ends_with("-O exit root@web-a"))
            .collect();
        assert_eq!(exits.len(), 2, "{calls:#?}");
        assert!(!socket_dir.exists());
    }

    #[tokio::test]
    async fn user_control_path_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let nix = vec!["-o".to_owned(), "ControlPath=/u/%C".to_owned()];
        let process = Process::new(&fake_ssh(dir.path()), nix, false);
        process
            .run(&remote(None), Command::new("true", [] as [&str; 0]))
            .await
            .unwrap();
        process.shutdown().await;
        let calls = log(dir.path());
        assert_eq!(calls.len(), 2, "{calls:#?}");
        assert!(!calls[1].contains("ControlMaster"));
        assert!(process.socket_dir.get().is_none());
    }

    #[tokio::test]
    async fn ssh_failure() {
        let dir = tempfile::tempdir().unwrap();
        let process = Process::new(&fake_ssh(dir.path()), vec![], false);
        let machine = Machine::Remote(Remote {
            target: "unreachable".parse().unwrap(),
            port: None,
            ssh_options: vec![],
        });
        let error = process
            .run(&machine, Command::new("true", [] as [&str; 0]))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "ssh unreachable: connect to host x port 22: Connection refused"
        );
    }
}
