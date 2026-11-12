//! The remote login shell may be bash, zsh or fish (spec 6.1). The one string `ssh` sends is
//! `sh -c '<wrapper>' sh '<program>' '<arg>'…`: the login shell parses only single-quoted words,
//! and the wrapper extends `PATH` and `exec`s the argv, so `sh` alone interprets everything.
//!
//! fish treats `\\` and `\'` as escapes inside single quotes and POSIX shells do not, so no
//! quoted word may contain a backslash or a single quote. Scripts are [`Script`] consts, checked
//! when the crate compiles; every other word is checked when the command line is built, and the
//! fake seam builds it for every remote command a test runs. See ADR 0011.

use crate::exec::Command;

/// Runs before the argv on a target: a non-interactive session may lack the Nix profile on
/// `PATH` (spec 6.1).
const WRAPPER: &str = r#"PATH="$PATH:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:$HOME/.nix-profile/bin"; export PATH; exec "$@""#;

/// A script for `sh -c`. Declare each one as a const so the quoting rule is checked at compile
/// time: `const LIST: Script = Script::new("find \"$@\" -print");`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script(&'static str);

impl Script {
    /// Panics, which in a const is a compile error, when `text` holds a backslash or a single
    /// quote.
    pub const fn new(text: &'static str) -> Self {
        if !quotable(text) {
            panic!("a remote script must not contain a backslash or a single quote (spec 6.1)");
        }
        Self(text)
    }

    pub fn text(self) -> &'static str {
        self.0
    }

    /// `sh -c <script> sh <args…>`: inside the script the arguments are `"$@"`. Path lists go
    /// on the command's stdin instead (spec 6.1, argv limits).
    pub fn command<I, S>(self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let fixed = ["-c", self.0, "sh"].map(String::from);
        Command::new(
            "sh",
            fixed.into_iter().chain(args.into_iter().map(Into::into)),
        )
    }
}

/// Whether `word` survives single quoting in bash, zsh and fish alike.
pub const fn quotable(word: &str) -> bool {
    let bytes = word.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' || bytes[i] == b'\'' {
            return false;
        }
        i += 1;
    }
    true
}

#[derive(Debug, thiserror::Error)]
#[error("cannot send {0:?} to a remote shell: it contains a backslash or a single quote")]
pub struct Unquotable(pub String);

/// The command line `ssh` sends for `command`, every word but the fixed `sh -c` and `$0`
/// single-quoted.
pub fn command_line(command: &Command) -> Result<String, Unquotable> {
    let mut line = format!("sh -c '{WRAPPER}' sh");
    for word in std::iter::once(&command.program).chain(&command.args) {
        if !quotable(word) {
            return Err(Unquotable(word.clone()));
        }
        line.push_str(" '");
        line.push_str(word);
        line.push('\'');
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through_sh(command: &Command) -> String {
        let line = command_line(command).unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &line])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn quoting_rule() {
        for ok in ["", "a b", "\"$@\"", "$HOME/*", "{} +", "a;b|c"] {
            assert!(quotable(ok), "{ok:?}");
        }
        for bad in ["a'b", "a\\b", "'", "\\"] {
            assert!(!quotable(bad), "{bad:?}");
        }
    }

    #[test]
    fn wrapper_obeys_the_rule_and_extends_path_before_exec() {
        assert!(quotable(WRAPPER));
        let path = WRAPPER.find("PATH=").unwrap();
        assert!(path < WRAPPER.find("exec").unwrap());
    }

    #[test]
    fn command_line_quotes_every_word() {
        let command = Command::new("nix", ["path-info", "--json", "-r", "/nix/store/x"]);
        assert_eq!(
            command_line(&command).unwrap(),
            format!("sh -c '{WRAPPER}' sh 'nix' 'path-info' '--json' '-r' '/nix/store/x'"),
        );
    }

    #[test]
    fn command_line_refuses_what_it_cannot_quote() {
        let command = Command::new("printf", ["%s", "it's"]);
        assert_eq!(command_line(&command).unwrap_err().0, "it's");
        let command = Command::new("printf", ["a\\nb"]);
        assert!(command_line(&command).is_err());
    }

    #[test]
    #[should_panic(expected = "backslash or a single quote")]
    fn script_refuses_what_it_cannot_quote() {
        let text = "echo 'x'";
        let _ = Script::new(text);
    }

    /// The login shell here is `sh`; fish, bash and zsh were checked by hand (ADR 0011).
    #[test]
    fn arguments_arrive_verbatim() {
        let args = ["a b", "$HOME", "\"q\"", "*", "", "x;y", "line\nbreak"];
        let command = Command::new("printf", ["%s|"].into_iter().chain(args));
        assert_eq!(through_sh(&command), format!("{}|", args.join("|")));
    }

    #[test]
    fn scripts_get_their_arguments_and_the_nix_path() {
        const SHOW: Script = Script::new(r#"printf "%s|" "$@"; printf "%s" "$PATH""#);
        let command = SHOW.command(["x y", "z"]);
        let home = std::env::var("HOME").unwrap();
        let output = through_sh(&command);
        let (args, path) = output.split_at(output.find("|z|").unwrap() + 3);
        assert_eq!(args, "x y|z|");
        assert!(
            path.ends_with(&format!(
                ":/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:{home}/.nix-profile/bin"
            )),
            "{path}"
        );
    }
}
