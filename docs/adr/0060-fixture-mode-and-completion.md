# Fixture run mode behind an environment variable; completion on clap_complete's engine

## Fixture run mode (spec 11)

A non-empty `SCRY_FIXTURE` swaps the real seam for `fixture::demo::fake()` in
`run::exec_from_env`. Nothing else changes: the same command line, `Env`, viewer, renderers and
exit codes. scry prints one line to stderr saying that no machine is contacted. That line is the
only marker: the report has no field for it, so the JSON schema stays the one real runs emit,
and completion under `SCRY_FIXTURE` shows the fixture's candidates without any notice.

The fake answers every command `scry colmena` and `scry flake` run, in deployed and revision
mode: `colmena --version`, both `colmena eval` steps, `colmena build`, the flake listing and
evaluations, `nix build`, `hostname -s`, the hive and flake lookup, `git` for the base worktree
and for `--base` completion, and the fetch script on every machine of a fleet built from the
pipeline tests' closures. Five configurations cover the UI's cases: `db-a` without changes,
`web-a` and `web-b` with shared changes (packages, churn groups, a binary file, a Boot change, a
new specialisation), `web-b` with a file of its own when built on its target, `web-c` unreachable
in deployed mode and absent at base in revision mode, and `mail-a` failing to evaluate. A command
outside the fixture exits 127 here and 255 on a target, so it fails one configuration instead of
panicking inside the TUI. `scry paths` runs too, but only against this machine's synthetic store.

### Considered options

- **A hidden `--fixture` flag**: a global flag on the clap definition and through every test
  that builds a `Cli`, for a mode no user needs. It would also not reach completion, which runs
  before the command line is parsed.
- **A cargo feature**: the packaged binary could not run demos, and the manual check needs it.
- **The environment variable** reaches the run and the completion engine alike, so
  `SCRY_FIXTURE=1 scry colmena --on <Tab>` completes the fixture's nodes and tags. It is not in
  `--help`; the README documents it.

## Shell completion (spec 10)

`clap_complete` 4.6 with `unstable-dynamic` (MIT OR Apache-2.0, as are its new dependencies
`shlex` and `is_executable`). `run::main` first calls `completions::complete_from_env`: with
`COMPLETE=<shell>` set, scry answers the words after `--` and exits. `scry completions <shell>`
prints the registration script, calling `scry` from `PATH` rather than the store path the
package was built at:

- **zsh and fish**: clap_complete's scripts. fish hands over words unquoted; zsh hands them over
  as typed, so scry removes the quoting (`./f\#w` is `./f#w`) before the engine sees them.
- **bash**: a script of scry's own. bash splits the line at `COMP_WORDBREAKS`, which holds `@`,
  `:` and `=`, and clap_complete's script passes those pieces on as words: `--on @w` arrived as
  `--on @ w`, and `github:o/r#w`, `--on=w` and `.#alice@w` were cut apart too. scry's script
  sends `${COMP_LINE:0:COMP_POINT}` and bash's current word instead; scry splits the line as
  the shell will (quotes and backslashes removed, leading `NAME=value` words dropped),
  completes the last word, and cuts each candidate down to the part after bash's word starts,
  since bash keeps what precedes it. The rest is backslash-escaped (`my\ dir/`, `it\'s.nix`),
  except after an open quote, where readline hands over the text past the quote and closes it
  itself (`'my dir/'`). The script registers with `nospace`, and scry adds the space after
  anything but a directory, since bash 3.2 has no `compopt`; the script runs there. With no
  candidates, `bashdefault` and `default` let bash complete as it would without scry, which
  covers `$HOME/…` paths; so an `--on` that matches nothing falls back to file names in bash.
- **`~`**: a REF starting with `~` or `~/` is expanded with `$HOME` for the lookups (listing,
  `--base`), since completion sees the word as typed; candidates keep the `~`.

Subcommands, flags, possible values (`--format`, `--color`, the shell) and paths come from the
clap definition. `scry paths OLD NEW` complete any path, `scry flake REF` directories. Three
completers are attached at runtime with `mut_arg`, so `cli.rs` stays free of completion code:

- `--on`: for `colmena`, step 1 of the hive in the current directory (`colmena::nodes`):
  names and `@tags`. For `flake`, `flake::list` of the `REF` typed on the line, `.` without one.
  clap_complete splits a comma list at `value_delimiter` and completes its last item.
- `flake REF#…`: `flake::list` of the part before `#`.
- `--base`: `git for-each-ref refs/heads refs/tags` in the checkout the run would use: the
  `REF` directory for a local flake (`.` or `/` first), else the current directory.

Every command goes through the seam: the fixture's under `SCRY_FIXTURE`, else the real one with
the `SCRY_DEBUG` log off, since stderr is the user's prompt. Each completion runs on a
current-thread runtime, and a lookup that takes over 3 s is dropped, which kills its command:
`tarball+http://…#` would otherwise hang the Tab. Any error, including a seam that cannot be
built, yields no candidates.

The engine gives a completer only its own word. To find `REF`, the completer keeps the whole
line and parses it with `Cli::command().ignore_errors(true)`, which accepts the line while it is
still being typed. `completions::candidates` runs the engine on given words with a given seam;
the tests drive it on the fake, without a shell. `tests/cli.rs` sources the bash script in a
real bash (`bashInteractive` in the package's check inputs) and checks what replaces bash's
word.

### Considered options

- **Our own protocol and scripts for all three shells**: zsh and fish descriptions and path
  handling, for what clap_complete already does right there. Only bash needed its own.
- **Removing `@`, `:` and `=` from `COMP_WORDBREAKS`** in the script: it changes completion of
  every other command in the user's shell.
- **clap_complete's static scripts** (`generate`): no dynamic names, tags or revisions.

### Consequences

- `unstable-dynamic` may change its API in a minor release; `Cargo.lock` pins the version.
- `COMPLETE` is clap_complete's default variable name. A user who exports it for another
  reason would get completion output from scry.
- Completion of `--on` in a colmena repository costs one `colmena eval` per Tab, a few seconds
  in a large hive. No cache.
