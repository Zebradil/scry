//! What the sources share: the evaluated toplevel, the selected configuration the run builds a
//! job from, and the wording of a failed evaluation.

use std::path::Path;

use anyhow::{Context, Result};

use crate::exec::{Exec, ExecError, Output};
use crate::report::ClosureKind;
use crate::transport::machine::Machine;
use crate::transport::script::Script;

/// `scry colmena`: two-step `colmena eval`, targets from deployment options (spec 4.2).
pub mod colmena;
/// `scry flake`: listing, default selection for this machine, `REF#name`, targets (spec 4.2).
pub mod flake;
/// `scry paths`: two toplevels on one machine (spec 4.2).
pub mod paths;
/// `--on` patterns: whole-name globs and `@tag`s, unmatched patterns as usage errors (spec 4.1).
pub mod select;

/// A configuration's toplevel as evaluated, before any build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluated {
    pub out: String,
    pub drv: String,
}

/// The Nix function both sources apply to a toplevel. A list, since `builtins.toJSON` turns
/// an attribute set with an `outPath` into that path alone.
pub(crate) const OUT_AND_DRV: &str = "[ t.outPath t.drvPath ]";

/// The upward search colmena and Nix make for a hive or a flake: from the directory `$1` up to
/// `$2` (`/` for no bound), the first directory holding a file named in `$3…`, earlier names
/// first. Prints that file's path relative to `$2`, with a leading `/`; exits 1 when there is
/// none or `$1` does not exist.
pub const LOCATE: Script = Script::new(
    r#"d=$(cd -P "$1" 2>/dev/null && pwd -P) || exit 1
top=$(cd -P "$2" 2>/dev/null && pwd -P) || exit 1
if [ "$top" = / ]; then top=; fi
shift 2
while :; do
  for f in "$@"; do
    if [ -f "$d/$f" ]; then printf "%s" "${d#"$top"}/$f"; exit 0; fi
  done
  case $d in "$top"/*) d=${d%/*};; *) exit 1;; esac
done"#,
);

/// [`LOCATE`] on this machine: the file found, relative to `top` with a leading `/`.
pub async fn locate(
    exec: &dyn Exec,
    dir: &Path,
    top: &Path,
    files: &[&str],
) -> Result<Option<String>> {
    let (dir, top) = (dir.to_string_lossy(), top.to_string_lossy());
    let args = [dir.as_ref(), top.as_ref()]
        .into_iter()
        .chain(files.iter().copied());
    let output = exec.run(&Machine::Local, LOCATE.command(args)).await?;
    Ok(output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

impl Evaluated {
    /// Reads the JSON of [`OUT_AND_DRV`].
    pub(crate) fn parse(stdout: &[u8]) -> Result<Self> {
        let (out, drv): (String, String) =
            serde_json::from_slice(stdout).context("reading the evaluated toplevel")?;
        Ok(Self { out, drv })
    }
}

/// A selected configuration in deployed mode: where its sides live (spec 4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub name: String,
    /// The closure kind the source expects, which decides the current system (spec 4.3).
    pub kind: ClosureKind,
    /// The old side's machine: its current system lives there.
    pub old: Machine,
    /// Where the build step leaves the new side.
    pub new: Machine,
    /// Why the configuration cannot be compared before any evaluation, such as a target
    /// that is not `[user@]host`. It fails that configuration only (spec 6.3).
    pub error: Option<String>,
}

impl Selected {
    /// Both sides on this machine: revision mode, where no target is contacted (spec 4.3).
    pub fn local(name: &str, kind: ClosureKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            old: Machine::Local,
            new: Machine::Local,
            error: None,
        }
    }
}

/// `output`'s stdout, or `program`'s failure in the wording of spec 9.
pub(crate) fn success(program: &str, output: Output) -> Result<Vec<u8>, ExecError> {
    if output.success() {
        return Ok(output.stdout);
    }
    Err(ExecError::Failed {
        program: program.to_owned(),
        code: output.code,
        line: error_line(&output.stderr),
    })
}

/// The line of a failed evaluation worth showing: Nix's first `error: …` with a message (with
/// a trace, Nix opens with a bare `error:` and states the message under the trace), else what
/// [`crate::fetch::first_line`] picks.
fn error_line(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    stderr
        .lines()
        .map(str::trim)
        .find(|l| {
            l.strip_prefix("error:")
                .is_some_and(|rest| !rest.is_empty())
        })
        .map_or_else(|| crate::fetch::first_line(&stderr), str::to_owned)
}

/// `s` as a Nix string literal, for a name inside an attribute path. `${` would interpolate,
/// so `$` is escaped too.
pub(crate) fn nix_string(s: &str) -> String {
    let mut quoted = String::with_capacity(s.len() + 2);
    quoted.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn locate_searches_upwards_within_its_bound() {
        let exec = crate::exec::process::Process::from_env().unwrap();
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        std::fs::create_dir_all(checkout.join("hosts/web-c")).unwrap();
        std::fs::write(root.path().join("flake.nix"), "").unwrap();
        let find = |dir: &str, top: &Path, files: &'static [&'static str]| {
            let (exec, dir, top) = (&exec, checkout.join(dir), top.to_owned());
            async move { locate(exec, &dir, &top, files).await.unwrap() }
        };
        // Above the bound does not count; without one it does.
        assert_eq!(find("hosts/web-c", &checkout, &["flake.nix"]).await, None);
        let unbounded = find("hosts/web-c", Path::new("/"), &["flake.nix"]).await;
        assert!(unbounded.unwrap().ends_with("/flake.nix"));
        // A directory that exists only at head finds the flake above it.
        std::fs::write(checkout.join("flake.nix"), "").unwrap();
        assert_eq!(
            find("hosts/web-c", &checkout, &["flake.nix"])
                .await
                .as_deref(),
            Some("/flake.nix")
        );
        std::fs::write(checkout.join("hosts/hive.nix"), "").unwrap();
        std::fs::write(checkout.join("hosts/flake.nix"), "").unwrap();
        assert_eq!(
            find("hosts/web-c", &checkout, &["flake.nix", "hive.nix"])
                .await
                .as_deref(),
            Some("/hosts/flake.nix")
        );
        assert_eq!(
            find("hosts/web-c", &checkout, &["hive.nix"])
                .await
                .as_deref(),
            Some("/hosts/hive.nix")
        );
        // Bounded at itself: only that directory, which must exist.
        let hosts = checkout.join("hosts");
        assert_eq!(
            find("hosts", &hosts, &["hive.nix"]).await.as_deref(),
            Some("/hive.nix")
        );
        assert_eq!(
            find("hosts/web-c", &checkout.join("hosts/web-c"), &["hive.nix"]).await,
            None
        );
        assert_eq!(
            find("missing", &checkout.join("missing"), &["flake.nix"]).await,
            None
        );
    }

    #[test]
    fn nix_strings() {
        assert_eq!(nix_string("web-a"), r#""web-a""#);
        assert_eq!(nix_string("alice@laptop"), r#""alice@laptop""#);
        assert_eq!(nix_string(r#"a"b\c${d}"#), r#""a\"b\\c\${d}""#);
        assert_eq!(nix_string("a\nb"), r#""a\nb""#);
    }

    #[test]
    fn evaluation_errors_name_the_message_under_the_trace() {
        let traced = b"error:\n       \xe2\x80\xa6 while calling the 'toJSON' builtin\n\n       error: attribute 'nope' missing\n       at \xc2\xabstring\xc2\xbb:1:552:\nERROR Operation failed\n";
        assert_eq!(error_line(traced), "error: attribute 'nope' missing");
        let flat =
            b"warning: Git tree is dirty\nerror: flake 'path:/x' does not provide attribute 'y'\n";
        assert_eq!(
            error_line(flat),
            "error: flake 'path:/x' does not provide attribute 'y'"
        );
        assert_eq!(error_line(b"fatal: bad revision\n"), "fatal: bad revision");
    }

    #[test]
    fn evaluated_reads_a_pair() {
        let parsed = Evaluated::parse(b"[\"/nix/store/a-x\",\"/nix/store/b-x.drv\"]\n").unwrap();
        assert_eq!(parsed.out, "/nix/store/a-x");
        assert_eq!(parsed.drv, "/nix/store/b-x.drv");
        assert!(Evaluated::parse(b"\"/nix/store/a-x\"").is_err());
    }
}
