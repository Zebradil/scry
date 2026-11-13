# Architecture

How the code is cut. Behaviour is in [`spec.md`](spec.md), terms in [`CONTEXT.md`](../CONTEXT.md),
decisions in [`adr/`](adr/). Read all of this file before starting a milestone.

## Module map

Every module is declared in `src/lib.rs` with its responsibility and owning milestone. A
milestone edits only its own modules (and their files under `src/<module>/`), declares further
submodules in its own module file, and never edits `lib.rs`.

| Module | Responsibility | Milestone | I/O |
|---|---|---|---|
| `cli` | clap types: subcommands, flags, defaults, `--target` validation | M0 | — |
| `exit` | `Outcome` to exit code (4.7), `UsageError`, `ignore_broken_pipe` | M0 | — |
| `report` | serde report types, the JSON schema (8.1, 8.2) | M0 | — |
| `store::{name, version, pathinfo}` | store names, `parseDrvName`, `compareVersions`, path-info JSON | M1 | pure |
| `size` | sizes and deltas as text | M1 | pure |
| `packages` | closure comparison into `report::Packages` | M1 | pure |
| `tree` | listing parsing, level walk state, followed trees and exclusions per kind | M2 | pure |
| `files::{compare, churn, diff, highlight, refs}` | file statuses, churn, unified diff, word highlight, reference pairing | M2 | pure |
| `boot` | Boot section | M2 | pure |
| `render::{text, json}` | text and JSON output, colour choice | M3 | writes |
| `shared` | shared changes, `(also in N)` | M3 | pure |
| `exec` | the seam: `Exec`, `Command`, `Output`, `ExecError` (M0); `process` real, `fake` (M4) | M0/M4 | seam |
| `transport::machine` | `Machine` (local or remote), `Remote`, `Target` | M0 | — |
| `transport::{ssh, script}` | ssh argv, connection sharing, remote script quoting and `PATH` | M4 | pure |
| `fetch::{probe, listing, content, references}` | round trips per side's machine (6.2) | M5 | seam |
| `source::{select, colmena, flake, paths}` | selection, evaluation, targets (4.1, 4.2) | M6 | seam |
| `build_step` | `colmena build` / `nix build`, out-links (4.4) | M6 | seam |
| `revision` | base worktree for `--base` (4.5) | M6 | seam |
| `run` | entry point `run::main` (M0 stub), tasks, timeouts, Ctrl-C, cleanup, output | M6 | seam |
| `tui` | ratatui viewer, live status (8.4) | M7 | terminal |
| `fixture` | synthetic fleet on the fake seam (11) | M8 | — |
| `completions` | dynamic completion (10) | M8 | seam |

`src/main.rs` only calls `scry::run::main()`, so tests drive the library.

## Data flow

```text
cli::Cli
  └─ run::main ─ source::{colmena|flake|paths}   evaluate names, targets, toplevels (≤ --eval-jobs)
                   └─ source::select              --on patterns
       ├─ revision                                base worktree, evaluated the same way
       ├─ build_step                              one build, stderr inherited (4.4)
       ├─ per configuration, ≤ --jobs, --timeout around the fetch phase:
       │    fetch::probe → fetch::listing → fetch::content → fetch::references   (seam, per machine)
       │    store, packages, tree, files::*, boot                                (pure)
       │    → report::Configuration ─────────────► event channel ─► tui (live)
       ├─ shared over all configurations → report::Report
       ├─ render::text | render::json | tui
       └─ exit::Outcome → exit code
```

Sides are compared where they are; nothing is copied (4.3). Where both sides are on one
machine, their commands share round trips (6.1).

## The command seam

`exec::Exec` (ADR 0005) is the only way to run a program:

- `run(&Machine, Command) -> Result<Output, ExecError>`. `Command` is program, args, stdin bytes
  and `Stderr::{Capture, Inherit}`. `Output` is exit code (`None` on a signal), stdout, stderr.
- A non-zero exit is `Ok`; callers that need success build `ExecError::Failed` from the first
  stderr line. `Err` is a missing program, `ssh` exit 255, or broken I/O, worded as in spec 9.
- On a remote machine the argv is single-quoted word by word into one `ssh` command line. Scripts
  are `sh -c <script>` built with `transport::script`; data goes on stdin.
- Dropping the future kills the child. `shutdown()` runs once at the end, also after Ctrl-C.
- The run holds an `Arc<dyn Exec>`: real in production, fake in tests and the fixture mode.

## Concurrency

tokio, multi-threaded (ADR 0001). One task per configuration under a `--jobs` semaphore;
evaluations under an `--eval-jobs` semaphore; at most 4 commands per SSH connection inside the
real seam. `tokio::time::timeout` bounds a configuration's fetch phase; Ctrl-C drops the run;
both kill children by drop. Then `Exec::shutdown`, worktree and temporary directory removal,
and the exit code with unfinished configurations counted as failed.

Live status is a `tokio::sync::mpsc::unbounded_channel::<report::Configuration>`: each
configuration is sent once, when final. The TUI is given the report header (a `Report` with empty
`configurations` and `shared`), the selected names in name order, and the receiver; rows without
a received configuration show `fetching…`. `tui::run` blocks on the caller's thread (the run
picks which) until the user quits, drains the receiver with `try_recv`, and returns whether
every configuration had finished, with the configurations received. Text and JSON wait for
the full `Report`.

## Errors

ADR 0004. `exit::UsageError` exits 2; any other error reaching `run::main` prints
`scry: <message>` and exits as a failure; an error inside one configuration becomes
`Status::Error` with `format!("{err:#}")`, the chain joined by `": "`. Typed errors only where a
caller branches (`ExecError`, `UsageError`); anyhow with `.context()` everywhere else.
`SCRY_DEBUG` (non-empty) logs every command's argv and stderr to stderr.

Stdout: renderers write to a `&mut impl Write` and return `io::Result`; the run wraps the result in
`exit::ignore_broken_pipe` so `scry … | head` ends quietly with the run's exit code. Never use
`print!`/`println!` for output: they panic on a closed pipe.

## Report

`report` is the contract between producers (M1, M2, M5, M6) and consumers (M3, M7). Enums carry
the presence rules of 8.1: `Status::Done(Sections)` holds the sections, `Error`/`Timeout` the
message, `Mode::Revision` holds `base`/`head`. Fields beyond the spec: ADR 0006. Renderers read
nothing else (ADR 0003).

## Testing

- **Pure modules**: table-driven unit tests next to the code (`#[cfg(test)] mod tests`), covering
  the cases spec 11 lists.
- **Pipeline tests** in `tests/`: build the run on `exec::fake`, feed canned command output
  (path-info JSON, listings, tar streams, evaluations), assert on rendered text and JSON with
  `insta` snapshots (`cargo insta review` in the dev shell). Assert on output, never on which
  command ran.
- **Remote scripts**: every script is checked for no backslash and no quote inside a quoted word
  (6.1); `ssh` argument order is checked on the built argv.
- **TUI**: logic tested through its state, without a terminal.
- **Binary**: `tests/cli.rs` runs the built binary for exit codes and `--version`.
- **Fixtures** are synthetic and hand-written: `web-a`, `db-a`, `root@web-a`,
  `https://cache.example`, store hashes from Nix's base32 alphabet. No captured production data,
  no real hostnames, keys or URLs.

## Build and checks

```sh
nix develop -c cargo test
nix develop -c cargo clippy --all-targets -- -D warnings
nix fmt                     # treefmt: nixfmt + rustfmt, config in treefmt.toml
nix build && nix flake check
```

`build.rs` stamps the revision for `--version`: `SCRY_GIT_REV` from the flake (no `.git` in the
sandbox), else `git`. The flake's package installs shell completions from M8 on (marked spot in
`flake.nix`). A flake only sees git-tracked files: `git add` new files before `nix build`.
Add a dependency in the milestone that first uses it.
