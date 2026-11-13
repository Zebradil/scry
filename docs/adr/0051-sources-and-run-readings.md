# Sources, revision mode and the run: readings of the spec

Spec 4 leaves some cases open. These readings fill them; `tests/run.rs` pins them.

## Sources

- **colmena runs in a directory**: every `colmena` call is
  `sh -c 'cd "$1" && shift && exec "$@"' sh <dir> colmena …`, so colmena's own upward search for
  `flake.nix` or `hive.nix` finds the base hive from the same relative directory inside the base
  worktree (4.5). `-f` needs the file name, which only a lookup on disk would give (ADR 0002),
  and a working-directory field in the seam is not needed for one program (ADR 0005).
- **Step 1** is `builtins.mapAttrs` over `nodes` returning the six deployment options, with
  `buildOnTarget = d.buildOnTarget or false`. **Step 2** and the flake evaluation return
  `[ t.outPath t.drvPath ]`: `builtins.toJSON` turns an attribute set with an `outPath` into
  that path alone.
- **Flake evaluation** is `nix eval --json REF#<set> --apply 'c: let t = c."<name>".<toplevel>;
  in …'`: every name is quoted as a Nix string (`"`, `\` and `$` escaped). Nix's attribute paths
  in an installable take quotes but no escapes, so a name holding `"` could not be quoted there.
- **A missing attribute set** is a listing whose stderr says `does not provide attribute`; any
  other listing failure is fatal (9, an unreadable flake). A flake with no configuration in any
  set, or a hive with no node, is fatal too.
- **Hosts**: `hostname -s` must work; `scutil --get LocalHostName` may fail (unset) and is then
  ignored.
- **A bare home-manager name** (`alice`, no `@`) is local when it equals `$USER`: it is the user
  part, as in home-manager's own `homeConfigurations.${username}`. Read as a host part, the
  bare `$USER` entry that the default selection picks would be compared against a target of
  that name.
- **A local configuration wins over `--target`**: 4.2 checks "matches this machine" first.
- **`REF#name` with `--on`** is a usage error: each is a whole selection.
- **A target that is not `[user@]host`** (a hive's `targetHost` starting with `-`, a port
  above 65535, a configuration name that is no target) fails that configuration only, like any
  per-configuration step (6.3).
- **`colmena --version` failing for any reason** reads `colmena not found on PATH` (4.2).

## Revision mode

- **A local path** is a reference starting with `.` or `/`, the rule Nix uses; anything else
  (`github:`, `path:`, a registry name) is a usage error under `--base`.
- **An unknown `REV`** is a usage error, like an unknown selection pattern.
- **`dirty`** ignores untracked files: Nix sees tracked files only (4.5).
- **Absent at base** is decided by listing base the same way as head (colmena step 1, the
  three `attrNames` calls), not by reading evaluation errors, which cannot tell a missing
  configuration from a broken one. A configuration in a different attribute set at base counts
  as absent.
- **Where base's source is**: head's flake or hive is found as Nix and colmena find it, from
  the source directory upwards inside the checkout (`flake.nix`, or for colmena `flake.nix`
  before `hive.nix`), and base's is the same file at the same path relative to the checkout
  (4.5: "found at the same relative path"). So a working directory that exists only at head
  still finds a flake at the root of both. When base has no such file, base lists no
  configuration and each one is absent at base. Both searches are one small `sh` script
  (`source::LOCATE`) through the seam. A source with no flake or hive inside the checkout is a
  fatal error, as an unreadable flake or hive is without `--base` (9).
- **Base errors** read `at base <short rev>: <error>`, so a reader tells them from head's.
- **Kind** of a configuration that is absent at base or failed before fetching is `unknown`:
  no probe ran (8.1).
- The worktree is `<run temporary directory>/worktree`; out-links are `new`, `new-1`, … and
  `old`, `old-1`, … beside it. The worktree is removed with `git worktree remove --force
  --force`, which also takes the locked entry an interrupted `git worktree add` leaves; when
  that fails (an add that died before writing the worktree's `.git`), the entry is unlocked,
  the directory deleted and `git worktree prune` run.

## The run

- **The viewer** opens after evaluation and the build step, on a blocking thread, while the
  tasks fetch. Everything scry writes to stderr before then (the `--target` warning, a build
  that could not start, the build's own output) stays on the main screen and is visible again
  when the viewer quits; a fatal error is printed after the terminal is restored. Quitting
  (`q`, or Ctrl-C, which raw mode delivers as a key) drops what still fetches and cleans up
  as after Ctrl-C; unfinished configurations count as failed (4.7). A signal, or a failure on
  the run's side (a task that panicked), tells the viewer to quit through a flag its poll loop
  reads; the run waits up to 1 s for the terminal to be restored, then cleans up and ends.
  A viewer stuck in crossterm's `read` on a terminal that hung up (its poll reports the tty
  readable, and the read then blocks) is left behind: the process exits without it.
- **Signals**: `SIGTERM` and `SIGHUP` are handled as `SIGINT` (Ctrl-C): the run drops its
  work, the viewer quits and restores the terminal, the base worktree, the temporary directory
  and the SSH masters are released, and the run exits as interrupted. Spec 6.3 names only
  Ctrl-C; left at their default, the other two would end scry with the terminal raw and the
  worktree behind.
- **A task that panics** (an evaluation or a fetch) fails the run with a fatal error naming
  the panic, after the same cleanup, rather than unwinding past it.
- **`SCRY_DEBUG`** logs to stderr while the viewer is up and paints over it; redirect stderr
  (`2>log`) or use `--format text` when debugging.

- **Ctrl-C** prints `scry: interrupted` and exits as a run with unfinished configurations: 1, or
  2 with `--exit-code` (4.7). Nothing is written to stdout.
- **A failed build that could not start** (the program missing) prints one line on stderr and
  the run goes on (4.4).
- **A failed evaluation** shows Nix's first `error:` line with a message: with a trace, Nix
  opens with a bare `error:` and states the message below the trace.
