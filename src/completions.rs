//! Dynamic shell completion (spec 10) on clap_complete's engine (ADR 0060). At each Tab the
//! registration script calls `COMPLETE=<shell> scry -- …`; scry prints the candidates and
//! exits. Subcommands, flags, possible values and paths come from the clap definition; `--on`,
//! `--base` and `REF#…` ask the source through the command seam, reusing the sources' own
//! listing. Any failure, or a lookup slower than [`LIMIT`], completes nothing.
//!
//! zsh and fish use clap_complete's scripts; zsh's words arrive with the user's quoting, which
//! is removed here. bash has a script of its own: bash splits words at `COMP_WORDBREAKS`
//! (`@`, `:`, `=`), which cut `@tag`, `REF#user@host` and `--on=…` apart, so the script sends
//! the line up to the cursor, scry splits it as the shell will, and each candidate is trimmed
//! to the part bash replaces.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{CommandFactory, ValueHint};
use clap_complete::engine::{
    ArgValueCompleter, CompletionCandidate, PathCompleter, ValueCompleter,
};
use clap_complete::env::{CompleteEnv, Shells};

use crate::cli::{Cli, Shell};
use crate::exec::Exec;
use crate::exec::process::Process;
use crate::revision::git;
use crate::source::{colmena, flake, success};
use crate::transport::machine::Machine;

/// The variable the registration script sets to ask for candidates.
const VAR: &str = "COMPLETE";

/// How long a lookup through the seam may take before the Tab completes nothing.
const LIMIT: Duration = Duration::from_secs(3);

/// bash's registration: `_SCRY_WORD` is bash's own current word, up to the cursor. scry
/// quotes the candidates and adds the space after a finished word itself (`nospace`), since
/// bash 3.2 has no `compopt`; without candidates bash completes as it would by default, which
/// covers `$HOME/…`.
const BASH: &str = r#"_scry() {
    local candidate
    COMPREPLY=()
    while IFS= read -r candidate; do
        COMPREPLY+=("$candidate")
    done < <(_SCRY_WORD="$2" COMPLETE=bash scry -- "${COMP_LINE:0:COMP_POINT}" 2>/dev/null)
}
complete -o bashdefault -o default -o nospace -F _scry scry
"#;

/// Answers the shell and exits when [`VAR`] is set; returns otherwise.
pub fn complete_from_env() {
    let Some(shell) = std::env::var_os(VAR).filter(|v| !v.is_empty()) else {
        return;
    };
    let mut words: Vec<OsString> = std::env::args_os()
        .skip_while(|a| a != "--")
        .skip(1)
        .collect();
    // The debug log and the seam's stderr would land on the user's prompt.
    let exec: Option<Arc<dyn Exec>> = if crate::fixture::demo::enabled() {
        Some(Arc::new(crate::fixture::demo::fake()))
    } else {
        Process::from_env()
            .ok()
            .map(|p| Arc::new(p.quiet()) as Arc<dyn Exec>)
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    if shell == "bash" {
        let line = words.first().map(|w| w.to_string_lossy().into_owned());
        let word = std::env::var("_SCRY_WORD").unwrap_or_default();
        let mut out = std::io::stdout().lock();
        for candidate in bash(exec, &cwd, &line.unwrap_or_default(), &word, LIMIT) {
            let _ = writeln!(out, "{candidate}");
        }
        let _ = out.flush();
        std::process::exit(0);
    }
    if shell == "zsh" {
        words = words
            .iter()
            .map(|w| split(&w.to_string_lossy(), false).0.concat().into())
            .collect();
    }
    let args: Vec<OsString> = ["scry".into(), "--".into()]
        .into_iter()
        .chain(words.iter().cloned())
        .collect();
    let line = Arc::new(Line {
        exec,
        cwd: cwd.clone(),
        words,
        limit: LIMIT,
    });
    let completed = CompleteEnv::with_factory(move || command(&line))
        .var(VAR)
        .try_complete(args, Some(&cwd));
    std::process::exit(if completed.is_ok() { 0 } else { 1 });
}

/// `scry completions <shell>`: the script that registers [`complete_from_env`]. It calls
/// `scry` from `PATH`, so it survives an upgrade of the package.
pub fn registration(shell: Shell, out: &mut impl Write) -> std::io::Result<()> {
    let name = match shell {
        Shell::Bash => return out.write_all(BASH.as_bytes()),
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
    };
    let shells = Shells::builtins();
    let shell = shells.completer(name).expect("a built-in shell");
    shell.write_registration(VAR, "scry", "scry", "scry", out)
}

/// The candidates for the word at `index` of `words` (`scry` first), as the engine gives them.
pub fn candidates(exec: Arc<dyn Exec>, cwd: &Path, words: &[&str], index: usize) -> Vec<String> {
    let words: Vec<OsString> = words.iter().map(OsString::from).collect();
    complete(Some(exec), cwd, words, index, LIMIT)
}

fn complete(
    exec: Option<Arc<dyn Exec>>,
    cwd: &Path,
    words: Vec<OsString>,
    index: usize,
    limit: Duration,
) -> Vec<String> {
    let line = Arc::new(Line {
        exec,
        cwd: cwd.to_owned(),
        words: words.clone(),
        limit,
    });
    clap_complete::engine::complete(&mut command(&line), words, index, Some(cwd))
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.get_value().to_string_lossy().into_owned())
        .collect()
}

/// bash: the candidates for the last word of `line` (the line up to the cursor), each as the
/// text that replaces `word`, bash's current word: what precedes `word` in scry's word stays,
/// and the rest is quoted as typed.
fn bash(
    exec: Option<Arc<dyn Exec>>,
    cwd: &Path,
    line: &str,
    word: &str,
    limit: Duration,
) -> Vec<String> {
    let (mut words, last) = split(line, true);
    let raw = match last {
        Some(start) => &line[start..],
        None => {
            words.push(String::new());
            ""
        }
    };
    // `NAME=value scry …`: the assignments are not scry's words.
    let assignments = words[..words.len() - 1]
        .iter()
        .take_while(|w| w.contains('='))
        .count();
    words.drain(..assignments);
    let index = words.len() - 1;
    let kept_raw = raw.strip_suffix(word).unwrap_or("");
    let kept = split(kept_raw, false).0.concat();
    // After an open quote, readline hands over the text past it, and closes the quote itself.
    let quoted = kept_raw
        .chars()
        .last()
        .is_some_and(|q| matches!(q, '\'' | '"') && raw.matches(q).count() % 2 == 1);
    let words = words.into_iter().map(OsString::from).collect();
    complete(exec, cwd, words, index, limit)
        .into_iter()
        .filter_map(|c| {
            let rest = c.strip_prefix(&kept)?;
            Some(if quoted {
                rest.to_owned()
            } else {
                escaped(rest)
            })
        })
        .collect()
}

/// `text` backslash-escaped for bash to read back as typed; a finished word, anything but a
/// directory, gets a space.
fn escaped(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if " \t\n\"'\\$`!&;|<>()[]{}*?".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    if !text.ends_with('/') {
        out.push(' ');
    }
    out
}

/// The words of `text` as a POSIX shell reads them: quotes and backslashes removed, an
/// unterminated quote running to the end. Without `at_blanks`, all of `text` is one word.
/// Also where the last word starts, when `text` ends inside it.
fn split(text: &str, at_blanks: bool) -> (Vec<String>, Option<usize>) {
    let (mut words, mut word, mut start) = (Vec::new(), String::new(), None);
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            ' ' | '\t' | '\n' if at_blanks => {
                if start.take().is_some() {
                    words.push(std::mem::take(&mut word));
                }
                continue;
            }
            '\\' => word.extend(chars.next().map(|(_, c)| c)),
            '\'' => word.extend(chars.by_ref().map(|(_, c)| c).take_while(|&c| c != '\'')),
            '"' => {
                while let Some((_, c)) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next().map(|(_, c)| c) {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some(c) => word.extend(['\\', c]),
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                }
            }
            c => word.push(c),
        }
        start.get_or_insert(i);
    }
    if start.is_some() || !at_blanks {
        words.push(word);
    }
    (words, start)
}

/// A leading `~` or `~/` expanded as the shell would, for a lookup: completion sees the word
/// as typed.
fn home(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{home}{rest}")
        }
        _ => path.to_owned(),
    }
}

/// The command line being completed, for completers that need more than their own word.
struct Line {
    /// `None` when the seam cannot be set up, such as for a `NIX_SSHOPTS` that does not split.
    exec: Option<Arc<dyn Exec>>,
    cwd: PathBuf,
    words: Vec<OsString>,
    limit: Duration,
}

#[derive(Debug, Clone, Copy)]
enum Value {
    /// `--on`: names and, for a hive, `@tags`.
    On,
    /// `--base`: local branches and tags.
    Base,
    /// `scry flake REF`: directories, or the names after `REF#`.
    Reference,
}

/// The clap definition with the dynamic completers attached.
fn command(line: &Arc<Line>) -> clap::Command {
    let add = |value| {
        let line = Arc::clone(line);
        ArgValueCompleter::new(move |current: &OsStr| line.complete(value, current))
    };
    let source = |c: clap::Command| {
        c.mut_arg("on", |a| a.add(add(Value::On)))
            .mut_arg("base", |a| a.add(add(Value::Base)))
    };
    Cli::command()
        .mut_subcommand("colmena", source)
        .mut_subcommand("flake", |c| {
            source(c).mut_arg("reference", |a| a.add(add(Value::Reference)))
        })
        .mut_subcommand("paths", |c| {
            c.mut_arg("old", |a| a.value_hint(ValueHint::AnyPath))
                .mut_arg("new", |a| a.value_hint(ValueHint::AnyPath))
        })
}

impl Line {
    /// `current` is the word, or for `--on` the last item of its comma list.
    fn complete(&self, value: Value, current: &OsStr) -> Vec<CompletionCandidate> {
        let current = current.to_string_lossy();
        if matches!(value, Value::Reference) && !current.contains('#') {
            return PathCompleter::dir()
                .current_dir(&self.cwd)
                .complete(OsStr::new(&*current));
        }
        let Some(exec) = &self.exec else {
            return Vec::new();
        };
        // The shell waits on this: one thread is enough.
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return Vec::new();
        };
        // Dropping a lookup that ran out of time kills its command.
        let mut values = runtime
            .block_on(async {
                let values =
                    tokio::time::timeout(self.limit, self.values(&**exec, value, &current));
                let values = values.await;
                exec.shutdown().await;
                values
            })
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        values.retain(|v| v.starts_with(&*current));
        values.sort();
        values.dedup();
        values.into_iter().map(CompletionCandidate::new).collect()
    }

    async fn values(&self, exec: &dyn Exec, value: Value, current: &str) -> Result<Vec<String>> {
        Ok(match value {
            Value::Base => {
                // The checkout of a local flake, as the run resolves it.
                let dir = match self.flake() {
                    Some(reference) if reference.starts_with(['.', '/']) => {
                        self.cwd.join(reference)
                    }
                    _ => self.cwd.clone(),
                };
                let refs = git(
                    &dir,
                    [
                        "for-each-ref",
                        "--format=%(refname:short)",
                        "refs/heads",
                        "refs/tags",
                    ],
                );
                let stdout = success("git", exec.run(&Machine::Local, refs).await?)?;
                String::from_utf8_lossy(&stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            }
            Value::Reference => {
                let reference = current.split('#').next().unwrap_or_default();
                let entries = flake::list(exec, &home(reference)).await?;
                entries
                    .into_iter()
                    .map(|e| format!("{reference}#{}", e.name))
                    .collect()
            }
            Value::On => match self.flake() {
                Some(reference) => flake::list(exec, &reference)
                    .await?
                    .into_iter()
                    .map(|e| e.name)
                    .collect(),
                None => colmena::nodes(exec, &self.cwd)
                    .await?
                    .into_iter()
                    .flat_map(|node| {
                        let tags = node.tags.into_iter().map(|t| format!("@{t}"));
                        std::iter::once(node.name).chain(tags)
                    })
                    .collect(),
            },
        })
    }

    /// The flake reference typed on the line without its `#name`, `.` when none is; `None`
    /// for `scry colmena`. The line is parsed leniently: it is still being typed.
    fn flake(&self) -> Option<String> {
        let matches = Cli::command()
            .ignore_errors(true)
            .try_get_matches_from(&self.words)
            .ok()?;
        let (name, matches) = matches.subcommand()?;
        (name == "flake").then(|| {
            let reference = matches.get_one::<String>("reference").map_or(".", |r| r);
            home(reference.split('#').next().unwrap_or("."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::{Fake, exit, ok};

    fn complete(exec: Fake, cwd: &Path, line: &str) -> Vec<String> {
        let mut words: Vec<&str> = line.split(' ').collect();
        if line.ends_with(' ') {
            words.pop();
            words.push("");
        }
        let index = words.len() - 1;
        candidates(Arc::new(exec), cwd, &words, index)
    }

    fn fixture(line: &str) -> Vec<String> {
        complete(crate::fixture::demo::fake(), Path::new("/fleet"), line)
    }

    #[test]
    fn subcommands_flags_and_possible_values() {
        let subcommands = fixture("scry ");
        for name in ["colmena", "flake", "paths", "completions"] {
            assert!(subcommands.iter().any(|s| s == name), "{subcommands:?}");
        }
        assert_eq!(fixture("scry colmena --o"), ["--on"]);
        assert_eq!(fixture("scry flake --format j"), ["json"]);
        assert_eq!(fixture("scry completions f"), ["fish"]);
    }

    #[test]
    fn colmena_names_and_tags() {
        assert_eq!(
            fixture("scry colmena --on "),
            [
                "@db", "@eu", "@mail", "@web", "db-a", "mail-a", "web-a", "web-b", "web-c"
            ]
        );
        assert_eq!(fixture("scry colmena --on @e"), ["@eu"]);
        // A comma list completes its last item.
        assert_eq!(
            fixture("scry colmena --base main --on db-a,web-"),
            ["db-a,web-a", "db-a,web-b", "db-a,web-c"]
        );
    }

    #[test]
    fn flake_names_after_the_reference() {
        assert_eq!(fixture("scry flake .#w"), [".#web-a", ".#web-b", ".#web-c"]);
        assert_eq!(fixture("scry flake --on m"), ["mail-a"]);
        assert!(fixture("scry flake --on @").is_empty());
    }

    #[test]
    fn on_uses_the_reference_typed_on_the_line() {
        // Each flake has one configuration, named after it.
        let flakes = Fake::new().rule(|_, c| {
            let installable = c.args.get(5)?;
            let (reference, set) = installable.split_once('#')?;
            Some(match set {
                "nixosConfigurations" => ok(serde_json::json!([reference]).to_string()),
                _ => exit(1, "error: flake does not provide attribute\n"),
            })
        });
        let cwd = Path::new("/fleet");
        assert_eq!(
            complete(flakes, cwd, "scry flake ./other --on "),
            ["./other"]
        );
        let home = std::env::var("HOME").unwrap();
        let flakes = Fake::new().rule(|_, c| {
            let installable = c.args.get(5)?;
            let (reference, set) = installable.split_once('#')?;
            Some(match set {
                "nixosConfigurations" => ok(serde_json::json!([reference]).to_string()),
                _ => exit(1, "error: flake does not provide attribute\n"),
            })
        });
        assert_eq!(
            complete(flakes, cwd, "scry flake ~/fleet --on "),
            [format!("{home}/fleet")]
        );
    }

    #[test]
    fn base_lists_branches_and_tags() {
        assert_eq!(
            fixture("scry flake --base "),
            ["main", "release-26.05", "v0.1.0"]
        );
        assert_eq!(fixture("scry colmena --base r"), ["release-26.05"]);
    }

    #[test]
    fn failures_complete_nothing() {
        let failing = || Fake::new().rule(|_, _| Some(exit(1, "error: boom\n")));
        let cwd = Path::new("/fleet");
        assert!(complete(failing(), cwd, "scry colmena --on ").is_empty());
        assert!(complete(failing(), cwd, "scry flake .#").is_empty());
        assert!(complete(failing(), cwd, "scry flake --base ").is_empty());
        let missing = Fake::new().rule(|_, _| Some(exit(127, "")));
        assert!(complete(missing, cwd, "scry colmena --on w").is_empty());
    }

    #[test]
    fn paths_and_flake_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("hosts")).unwrap();
        std::fs::write(dir.path().join("hive.nix"), "").unwrap();
        let none = || Fake::new();
        assert_eq!(
            complete(none(), dir.path(), "scry paths h"),
            ["hive.nix", "hosts/"]
        );
        assert_eq!(complete(none(), dir.path(), "scry flake h"), ["hosts/"]);
    }

    #[test]
    fn words_split_as_the_shell_reads_them() {
        let words = |text| split(text, true);
        assert_eq!(words(""), (vec![], None));
        assert_eq!(
            words("scry flake "),
            (vec!["scry".into(), "flake".into()], None)
        );
        assert_eq!(
            words("a 'b c' \"d\\\"e\" f\\ g"),
            (
                vec!["a".into(), "b c".into(), "d\"e".into(), "f g".into()],
                Some(15)
            )
        );
        // Still being typed.
        assert_eq!(words("x 'un"), (vec!["x".into(), "un".into()], Some(2)));
        // zsh hands words over as typed.
        assert_eq!(split("./good\\#web-", false).0, ["./good#web-"]);
        assert_eq!(split("", false).0, [""]);
    }

    #[test]
    fn bash_reads_candidates_back_as_typed() {
        assert_eq!(escaped("my dir/"), r"my\ dir/");
        assert_eq!(escaped("it's.nix"), r"it\'s.nix ");
        assert_eq!(escaped(".#web-a"), ".#web-a ");
        assert_eq!(escaped("a\"$b"), r#"a\"\$b "#);
    }

    #[test]
    fn bash_gets_the_part_after_its_word_breaks() {
        let bash = |line: &str, word: &str| {
            let exec: Arc<dyn Exec> = Arc::new(crate::fixture::demo::fake());
            super::bash(Some(exec), Path::new("/fleet"), line, word, LIMIT)
        };
        assert_eq!(bash("scry colmena --on @w", "w"), ["web "]);
        assert_eq!(bash("scry colmena --on web-a,@e", "e"), ["eu "]);
        assert_eq!(bash("scry colmena --on=db", "db"), ["db-a "]);
        assert_eq!(
            bash("scry flake github:o/r#web-", "o/r#web-"),
            ["o/r#web-a ", "o/r#web-b ", "o/r#web-c "]
        );
        // Readline hands over the word past an open quote.
        assert_eq!(bash("scry flake '.#m", ".#m"), [".#mail-a"]);
        assert_eq!(bash("scry flake ~/x#d", "~/x#d"), ["~/x#db-a "]);
        assert_eq!(bash("scry flake --base m", "m"), ["main "]);
        assert_eq!(bash("A=1 B= scry flake --base m", "m"), ["main "]);
        assert!(bash("scry colmena --on ", "").contains(&"@db ".to_owned()));
    }

    #[test]
    fn a_slow_lookup_completes_nothing() {
        let hung = Fake::new().hang(|_, _| true);
        let since = std::time::Instant::now();
        let words = ["scry", "flake", ".#"].map(OsString::from).to_vec();
        let limit = Duration::from_millis(50);
        assert!(super::complete(Some(Arc::new(hung)), Path::new("/"), words, 2, limit).is_empty());
        assert!(since.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn base_lists_the_revisions_of_the_flake_on_the_line() {
        let git = || {
            Fake::new().rule(|_, c| {
                let main = if c.args[1] == "/fleet/./sub" {
                    "sub-branch\n"
                } else {
                    "main\n"
                };
                (c.program == "git").then(|| ok(main))
            })
        };
        let cwd = Path::new("/fleet");
        assert_eq!(
            complete(git(), cwd, "scry flake ./sub --base "),
            ["sub-branch"]
        );
        assert_eq!(
            complete(git(), cwd, "scry flake github:o/r --base "),
            ["main"]
        );
        assert_eq!(complete(git(), cwd, "scry colmena --base "), ["main"]);
    }

    #[test]
    fn registration_scripts_call_scry_from_path() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let mut script = Vec::new();
            registration(shell, &mut script).unwrap();
            let script = String::from_utf8(script).unwrap();
            assert!(script.contains("COMPLETE"), "{script}");
            assert!(script.contains("scry"), "{script}");
        }
    }
}
