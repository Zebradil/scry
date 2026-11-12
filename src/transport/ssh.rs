//! `ssh` command lines and the per-connection policy of spec 6.1. Spawning lives in
//! `exec::process`; this module builds argv and decides when a command may start.

use std::future::Future;
use std::path::PathBuf;

use anyhow::Context;
use tokio::sync::{Mutex, OnceCell, Semaphore};

use crate::transport::machine::Remote;

/// Commands at once over one connection. OpenSSH's server allows 10 sessions per connection
/// by default (`MaxSessions`); a configuration needs 2, one per side.
pub const SESSIONS_PER_CONNECTION: usize = 4;

/// `NIX_SSHOPTS` split as Nix splits it (spec 4.8), so a value that works for `nix copy` works
/// here. A value that does not split is a fatal error naming the variable.
pub fn nix_sshopts(value: Option<&str>) -> anyhow::Result<Vec<String>> {
    let value = value.unwrap_or_default();
    shell_split(value)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("while splitting NIX_SSHOPTS '{value}'"))
}

/// A port of Nix's `shellSplitString`: spaces and tabs separate words; single quotes keep
/// everything; double quotes take `\$`, `` \` ``, `\"` and `\\` as escapes and keep any other
/// backslash; outside quotes a backslash escapes the next character.
fn shell_split(s: &str) -> Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\\' => {
                // A trailing backslash is dropped, as Nix does.
                if let Some(next) = chars.next() {
                    word.push(next);
                    started = true;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated single quote"),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('$' | '`' | '"' | '\\')) => word.push(c),
                            Some(c) => word.extend(['\\', c]),
                            None => return Err("unterminated double quote"),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated double quote"),
                    }
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// `ssh -o BatchMode=yes <NIX_SSHOPTS…> <sshOptions…> <sharing…> [-p <port>] <target> -- <line>`.
/// ssh keeps the first value of an option, so the user's options beat `sharing`.
pub fn run_args(
    nix_sshopts: &[String],
    remote: &Remote,
    sharing: &[String],
    line: &str,
) -> Vec<String> {
    let mut args = options(nix_sshopts, remote, sharing);
    args.extend([remote.target.to_string(), "--".to_owned(), line.to_owned()]);
    args
}

/// `ssh -G …`: prints the effective configuration without connecting.
pub fn config_args(nix_sshopts: &[String], remote: &Remote) -> Vec<String> {
    let mut args = vec!["-G".to_owned()];
    args.extend(options(nix_sshopts, remote, &[]));
    args.push(remote.target.to_string());
    args
}

/// `ssh … -O exit <target>`: stops the master behind `sharing`'s socket.
pub fn exit_args(nix_sshopts: &[String], remote: &Remote, sharing: &[String]) -> Vec<String> {
    let mut args = options(nix_sshopts, remote, sharing);
    args.extend([
        "-O".to_owned(),
        "exit".to_owned(),
        remote.target.to_string(),
    ]);
    args
}

fn options(nix_sshopts: &[String], remote: &Remote, sharing: &[String]) -> Vec<String> {
    let mut args = vec!["-o".to_owned(), "BatchMode=yes".to_owned()];
    args.extend_from_slice(nix_sshopts);
    args.extend_from_slice(&remote.ssh_options);
    args.extend_from_slice(sharing);
    if let Some(port) = remote.port {
        args.extend(["-p".to_owned(), port.to_string()]);
    }
    args
}

/// Whether `ssh -G` output names a control path: the user's own multiplexing, used as is.
pub fn has_control_path(config: &str) -> bool {
    config.lines().any(|line| {
        line.split_once(' ')
            .is_some_and(|(key, value)| key == "controlpath" && value != "none")
    })
}

/// How a connection is shared, decided once from `ssh -G`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sharing {
    /// The user's own `ControlPath`, used as is.
    User,
    /// The tool's master on this socket, one per connection: `%C` would give two connections
    /// that differ only in options, such as an identity file, the same socket.
    Own(PathBuf),
}

impl Sharing {
    /// What goes after the user's options.
    pub fn options(&self) -> Vec<String> {
        match self {
            Self::User => Vec::new(),
            Self::Own(socket) => [
                "-o",
                "ControlMaster=auto",
                "-o",
                &format!("ControlPath={}", socket.display()),
                "-o",
                "ControlPersist=60",
            ]
            .map(str::to_owned)
            .into(),
        }
    }

    /// Whether a master listens. Assumed for the user's own, whose socket is theirs to manage.
    pub fn master_up(&self) -> bool {
        match self {
            Self::User => true,
            Self::Own(socket) => socket.exists(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No command has connected yet, or the master has gone since.
    Fresh,
    /// A master was up after the last command that ran alone.
    Shared,
    /// Commands connect, but no master stays (`-o ControlMaster=no`): nothing to wait for.
    Unshared,
}

/// One connection to a [`Remote`]: its sharing, found once, and the order in which commands
/// may use it.
#[derive(Debug)]
pub struct Connection {
    pub sharing: OnceCell<Sharing>,
    sessions: Semaphore,
    state: Mutex<State>,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            sharing: OnceCell::new(),
            sessions: Semaphore::new(SESSIONS_PER_CONNECTION),
            state: Mutex::new(State::Fresh),
        }
    }
}

impl Connection {
    /// Runs `command` once a session slot is free. While no master is known to listen,
    /// commands run one at a time: with `ControlMaster=auto`, clients that start before the
    /// master's socket exists each open their own connection, with its own handshake and key
    /// prompt. That holds at the start and again after an idle master exits (`ControlPersist`).
    ///
    /// shortcut: the rest wait for the whole first command, not just its handshake; wait for the
    /// socket instead if first commands to one target turn out to be slow and concurrent.
    pub async fn run<T>(
        &self,
        command: impl Future<Output = T>,
        connected: impl FnOnce(&T) -> bool,
    ) -> T {
        let _session = self
            .sessions
            .acquire()
            .await
            .expect("the semaphore is never closed");
        let master_up = || self.sharing.get().is_none_or(Sharing::master_up);
        let mut state = self.state.lock().await;
        let alone = match *state {
            State::Fresh => true,
            State::Shared => !master_up(),
            State::Unshared => false,
        };
        if !alone {
            drop(state);
            return command.await;
        }
        let result = command.await;
        if connected(&result) {
            *state = if master_up() {
                State::Shared
            } else {
                State::Unshared
            };
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|&w| w.to_owned()).collect()
    }

    fn remote(port: Option<u16>) -> Remote {
        Remote {
            target: "root@web-a".parse().unwrap(),
            port,
            ssh_options: strings(&["-o", "StrictHostKeyChecking=accept-new"]),
        }
    }

    /// The cases of Nix's own `shellSplitString` tests.
    #[test]
    fn splits_like_nix() {
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("foo", &["foo"]),
            ("'foo bar'", &["foo bar"]),
            ("'foo bar\"'", &["foo bar\""]),
            ("\"foo bar\"", &["foo bar"]),
            ("foo bar", &["foo", "bar"]),
            ("\"foo bar'\" 'baz\"'", &["foo bar'", "baz\""]),
            ("foo '' bar baz ''", &["foo", "", "bar", "baz", ""]),
            ("foo \"\" bar baz \"\"", &["foo", "", "bar", "baz", ""]),
            ("'foo\\\"bar'", &["foo\\\"bar"]),
            ("\"foo\\\"bar\"", &["foo\"bar"]),
            ("foo\\ bar baz qux\\ quux", &["foo bar", "baz", "qux quux"]),
            (
                "foo\\\"bar baz qux\\'quux",
                &["foo\"bar", "baz", "qux'quux"],
            ),
            ("\"a\\nb\"", &["a\\nb"]),
            (
                "  -o \"ProxyCommand=ssh -W %h:%p bastion\"\t-i key ",
                &["-o", "ProxyCommand=ssh -W %h:%p bastion", "-i", "key"],
            ),
        ];
        for (input, words) in cases {
            assert_eq!(shell_split(input).unwrap(), strings(words), "{input:?}");
        }
        for bad in ["foo'", "foo\"", "foo'bar", "foo\"bar", "foo\"bar\\\""] {
            assert!(shell_split(bad).is_err(), "{bad:?}");
        }
        let error = nix_sshopts(Some("-o 'x")).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "while splitting NIX_SSHOPTS '-o 'x': unterminated single quote"
        );
        assert!(nix_sshopts(None).unwrap().is_empty());
    }

    #[test]
    fn user_options_come_before_the_tools_defaults() {
        let nix = nix_sshopts(Some("-o ControlMaster=no -i key")).unwrap();
        let sharing = Sharing::Own("/tmp/scry-abc/0".into()).options();
        assert_eq!(
            run_args(&nix, &remote(Some(2222)), &sharing, "sh -c 'x'"),
            strings(&[
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlMaster=no",
                "-i",
                "key",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/scry-abc/0",
                "-o",
                "ControlPersist=60",
                "-p",
                "2222",
                "root@web-a",
                "--",
                "sh -c 'x'",
            ])
        );
    }

    #[test]
    fn no_port_no_sharing() {
        assert_eq!(
            run_args(&[], &remote(None), &Sharing::User.options(), "true"),
            strings(&[
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "root@web-a",
                "--",
                "true",
            ])
        );
    }

    #[test]
    fn config_and_exit_args() {
        let nix = strings(&["-F", "cfg"]);
        assert_eq!(
            config_args(&nix, &remote(Some(22))),
            strings(&[
                "-G",
                "-o",
                "BatchMode=yes",
                "-F",
                "cfg",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-p",
                "22",
                "root@web-a",
            ])
        );
        let sharing = Sharing::Own("/tmp/d/1".into()).options();
        let exit = exit_args(&nix, &remote(None), &sharing);
        assert_eq!(
            exit[exit.len() - 3..],
            strings(&["-O", "exit", "root@web-a"])
        );
        assert!(exit.contains(&"ControlPath=/tmp/d/1".to_owned()));
    }

    #[test]
    fn control_path_detection() {
        let user = "user root\ncontrolmaster auto\ncontrolpath /home/u/.ssh/cm-%C\n";
        assert!(has_control_path(user));
        assert!(!has_control_path("user root\ncontrolpath none\n"));
        assert!(!has_control_path("user root\ncontrolmaster false\n"));
    }

    /// Simulates `ssh` under `ControlMaster=auto`: a client that starts while no socket exists
    /// does a handshake and, unless sharing is off, leaves a master listening on the socket.
    struct Sim {
        connection: Connection,
        socket: PathBuf,
        sharing_on: bool,
        handshakes: AtomicUsize,
        active: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Sim {
        fn new(dir: &Path, sharing_on: bool) -> Arc<Self> {
            let socket = dir.join("0");
            let connection = Connection::default();
            connection
                .sharing
                .set(Sharing::Own(socket.clone()))
                .unwrap();
            Arc::new(Self {
                connection,
                socket,
                sharing_on,
                handshakes: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            })
        }

        async fn command(&self) {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            if !self.socket.exists() {
                self.handshakes.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                if self.sharing_on {
                    std::fs::write(&self.socket, "").unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
        }

        /// Starts `n` commands at once and returns the peak number running together.
        async fn batch(self: &Arc<Self>, n: usize) -> usize {
            self.peak.store(0, Ordering::SeqCst);
            let tasks: Vec<_> = (0..n)
                .map(|_| {
                    let sim = self.clone();
                    tokio::spawn(async move { sim.connection.run(sim.command(), |()| true).await })
                })
                .collect();
            for task in tasks {
                task.await.unwrap();
            }
            self.peak.load(Ordering::SeqCst)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_handshake_per_master_and_at_most_four_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let sim = Sim::new(dir.path(), true);
        let peak = sim.batch(10).await;
        assert_eq!(sim.handshakes.load(Ordering::SeqCst), 1);
        assert!((2..=SESSIONS_PER_CONNECTION).contains(&peak), "{peak}");

        // The idle master exits; the next batch must not race to several new ones.
        std::fs::remove_file(&sim.socket).unwrap();
        sim.batch(10).await;
        assert_eq!(sim.handshakes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn without_a_master_commands_still_run_together() {
        let dir = tempfile::tempdir().unwrap();
        let sim = Sim::new(dir.path(), false);
        sim.batch(1).await;
        let peak = sim.batch(10).await;
        assert!(peak >= 2, "{peak}");
    }

    #[tokio::test]
    async fn a_failed_first_command_leaves_the_next_one_first() {
        let connection = Connection::default();
        connection.run(async {}, |()| false).await;
        assert_eq!(*connection.state.lock().await, State::Fresh);
        connection.run(async {}, |()| true).await;
        assert_eq!(*connection.state.lock().await, State::Shared);
    }
}
