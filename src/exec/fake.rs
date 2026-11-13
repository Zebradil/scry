//! Canned answers per invocation, for pipeline tests and the fixture run mode (spec 11).
//!
//! A rule looks at the machine and the command (program, args, stdin) and returns the output,
//! or `None` to pass; the first rule that answers wins. Answers go through the same mapping as
//! the real seam, so exit 255 on a target is an ssh error and exit 127 a missing program. Every
//! remote command must pass the quoting rule of spec 6.1, which makes each pipeline test a check
//! of the scripts it runs.

use super::process::classify;
use super::{BoxFuture, Command, Exec, ExecError, Output, Stderr};
use crate::transport::machine::Machine;
use crate::transport::script;

/// `Some(None)` answers by never finishing, like a target that hangs.
type Rule = Box<dyn Fn(&Machine, &Command) -> Option<Option<Output>> + Send + Sync>;

#[derive(Default)]
pub struct Fake {
    rules: Vec<Rule>,
}

impl Fake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a rule after the existing ones.
    pub fn rule(
        mut self,
        rule: impl Fn(&Machine, &Command) -> Option<Output> + Send + Sync + 'static,
    ) -> Self {
        self.rules.push(Box::new(move |machine, command| {
            rule(machine, command).map(Some)
        }));
        self
    }

    /// Adds a rule after the existing ones: the commands it accepts never finish, until
    /// dropped by a timeout or Ctrl-C.
    pub fn hang(
        mut self,
        when: impl Fn(&Machine, &Command) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.rules.push(Box::new(move |machine, command| {
            when(machine, command).then_some(None)
        }));
        self
    }

    /// Answers exactly `program args…` on `machine`, whatever the stdin.
    pub fn on(self, machine: Machine, program: &str, args: &[&str], output: Output) -> Self {
        let program = program.to_owned();
        let args: Vec<String> = args.iter().map(|&a| a.to_owned()).collect();
        self.rule(move |m, c| {
            (*m == machine && c.program == program && c.args == args).then(|| output.clone())
        })
    }
}

/// Exit 0 with `stdout`.
pub fn ok(stdout: impl Into<Vec<u8>>) -> Output {
    Output {
        code: Some(0),
        stdout: stdout.into(),
        stderr: Vec::new(),
    }
}

/// Exit `code` with `stderr`.
pub fn exit(code: i32, stderr: impl Into<Vec<u8>>) -> Output {
    Output {
        code: Some(code),
        stdout: Vec::new(),
        stderr: stderr.into(),
    }
}

impl Exec for Fake {
    /// Panics on an unmatched invocation or a remote command that cannot be quoted: both are
    /// bugs in the test or the fixture, not outcomes to report.
    fn run<'a>(
        &'a self,
        machine: &'a Machine,
        command: Command,
    ) -> BoxFuture<'a, Result<Output, ExecError>> {
        if matches!(machine, Machine::Remote(_))
            && let Err(error) = script::command_line(&command)
        {
            panic!("{error}");
        }
        let answer = self
            .rules
            .iter()
            .find_map(|rule| rule(machine, &command))
            .unwrap_or_else(|| {
                panic!(
                    "no fake rule for {:?} {:?} on {machine}",
                    command.program, command.args
                )
            });
        let Some(mut output) = answer else {
            return Box::pin(std::future::pending());
        };
        if command.stderr == Stderr::Inherit {
            output.stderr.clear();
        }
        Box::pin(async move { classify(machine, output) })
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::machine::Remote;
    use crate::transport::script::Script;

    fn web_a() -> Machine {
        Machine::Remote(Remote {
            target: "root@web-a".parse().unwrap(),
            port: None,
            ssh_options: vec![],
        })
    }

    #[tokio::test]
    async fn first_matching_rule_answers_and_may_read_stdin() {
        const CAT: Script = Script::new("cat");
        let fake = Fake::new()
            .on(web_a(), "nix", &["--version"], ok("nix (Nix) 2.31.0\n"))
            .rule(|_, c| (c.program == "sh").then(|| ok(c.stdin.to_ascii_uppercase())))
            .rule(|_, _| Some(exit(1, "unreachable rule")));
        let output = fake
            .run(&web_a(), Command::new("nix", ["--version"]))
            .await
            .unwrap();
        assert_eq!(output.stdout, b"nix (Nix) 2.31.0\n");
        let mut command = CAT.command::<_, &str>([]);
        command.stdin = b"/nix/store/a\n".to_vec();
        let output = fake.run(&Machine::Local, command).await.unwrap();
        assert_eq!(output.stdout, b"/NIX/STORE/A\n");
    }

    #[tokio::test]
    async fn answers_map_like_the_real_seam() {
        let fake = Fake::new()
            .on(
                web_a(),
                "nix",
                &[],
                exit(127, "sh: 1: exec: nix: not found\n"),
            )
            .on(
                web_a(),
                "true",
                &[],
                exit(255, "ssh: Could not resolve hostname web-a\n"),
            );
        let error = fake
            .run(&web_a(), Command::new("nix", [] as [&str; 0]))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "`nix` not found on root@web-a");
        let error = fake
            .run(&web_a(), Command::new("true", [] as [&str; 0]))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "ssh root@web-a: Could not resolve hostname web-a"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn hang_never_answers() {
        let fake = Fake::new()
            .hang(|m, _| *m == web_a())
            .rule(|_, _| Some(ok("")));
        let target = web_a();
        let hung = fake.run(&target, Command::new("true", [] as [&str; 0]));
        let timeout = std::time::Duration::from_secs(60);
        assert!(tokio::time::timeout(timeout, hung).await.is_err());
        let local = fake.run(&Machine::Local, Command::new("true", [] as [&str; 0]));
        assert!(local.await.is_ok());
    }

    #[tokio::test]
    #[should_panic(expected = "no fake rule")]
    async fn unmatched_invocation_panics() {
        let _ = Fake::new()
            .run(&Machine::Local, Command::new("nix", ["build"]))
            .await;
    }

    #[tokio::test]
    #[should_panic(expected = "backslash or a single quote")]
    async fn unquotable_remote_command_panics() {
        let fake = Fake::new().rule(|_, _| Some(ok("")));
        let _ = fake.run(&web_a(), Command::new("printf", ["it's"])).await;
    }
}
