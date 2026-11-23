# scry specification

Status: settled 2026-11-12. Terms follow [`GLOSSARY.md`](../GLOSSARY.md). Record decisions you make while
implementing as ADRs in [`docs/adr/`](adr/), with measurements where a decision rests on one.

This spec sets behaviour and hard constraints. It leaves internal structure open: module layout,
types, crates beyond the ones named, and the threading model are the implementer's choice. Where it
says *must*, it is a requirement; *should* is a strong default that an ADR may override with evidence.
Sample outputs are normative for content, tags and order, not for exact spacing.

## 1. Problem

Before a change to a Nix fleet lands, an operator wants to know what it does to each machine. Deploy
tools show what will be *built*. `nix store diff-closures` and `nvd` show which *package versions*
change. Neither shows the change that usually matters.

A real generation bump on a server changed a trusted public key in `nix.conf`, added an SSH key under
`authorized_keys.d`, bumped flake inputs in `registry.json` and moved a systemd `ExecStart` to a new
store path. `nix store diff-closures` reported two size deltas and nothing else. `nix-diff` showed every
derivation attribute and buried the changed config line under hundreds of lines of hash noise.

A nixpkgs bump is the opposite problem: hundreds of files change only because store hashes changed, and
the handful of real edits drown in them.

## 2. Solution

`scry` is one binary. For every selected configuration it finds the old and the new toplevel, reads
metadata of both closures wherever each lives, and computes three sections of change:

- **Packages**: which packages were added, removed, upgraded or downgraded, and what the closure size
  did. Rebuilds with the same version are collapsed.
- **Files**: a line diff of every changed config file, with store-hash churn collapsed and the files
  behind changed store references followed.
- **Boot**: kernel, initrd, kernel modules, kernel parameters, specialisations.

It answers two questions with the same core:

- **Deployed mode**: what will this deploy change on each machine? Old is what the target runs now.
- **Revision mode**: what does this change (a branch, a PR) do to every configuration? Old is the same
  configuration built from a git revision. No machine is contacted.

Results go to a TUI with a configuration list and an expandable detail pane when stdout is a terminal,
to plain text otherwise, or to JSON on request. Changes that appear identically in several
configurations are grouped, so a fleet-wide bump reads once.

## 3. User stories

Operator, deployed mode:

1. As an operator, I want to run one command in my colmena repository and see what each node will
   change, so that I review before deploying.
2. As an operator, I want the build step run for me by default, with its output visible, so that one
   command covers build and review.
3. As an operator, I want to skip the build when the closures already exist, so that a second look is
   fast.
4. As an operator, I want to select configurations with one syntax in every source (names, globs,
   colmena tags), so that I learn it once.
5. As an operator, I want each configuration fetched in parallel with a visible status, so that one slow
   or unreachable machine does not block the others.
6. As an operator, I want an unreachable or failing configuration reported inline with its error, and
   a timeout on hung machines, so that the rest of the review still happens.
7. As an operator, I want the tool to reach machines exactly like `ssh` does, honouring `~/.ssh/config`,
   my agent and `NIX_SSHOPTS`.
8. As an operator, I want a preview that changes nothing on any machine: no copies, no GC roots, no
   writes.

Reviewer, revision mode:

9. As a reviewer, I want `--base main` to show what my working tree changes in every configuration
   compared to `main`, without contacting any machine, so that I can review a PR's effect.
10. As a CI author, I want JSON output and an exit code that says whether anything changed, so that a
    pipeline can comment on a PR or gate on it.

Content, all modes:

11. As a user, I want added, removed, upgraded and downgraded packages with old and new versions side by
    side, so that version bumps are obvious.
12. As a user, I want packages that are merely rebuilt collapsed into one line with their size delta, so
    that a nixpkgs bump does not hide real changes.
13. As a user, I want the closure size and its delta per configuration, so that I notice bloat.
14. As a user, I want every changed config file with a line diff, so that config changes, new keys and
    unit edits are visible.
15. As a user, I want file changes that differ only by store hashes or store path versions collapsed
    into groups I can expand, so that churn does not drown real edits and nothing is hidden for good.
16. As a user, I want the files behind a changed store reference shown (a script called from a unit's
    `ExecStart`, a config file referenced from a plist), so that a change two hops away is visible.
17. As a user, I want changed words highlighted inside long lines, so that a one-token change in a
    single-line JSON file is findable.
18. As a user, I want kernel, initrd, kernel-module and kernel-parameter changes and specialisations
    added or removed, so that a reboot-requiring change stands out.
19. As a user, I want changes shared by several configurations shown once with the list of
    configurations, so that a fleet-wide change reads once.
20. As a nix-darwin user, I want launchd agents and daemons included, and as a home-manager user, my
    dotfiles, so that both get more than package versions.
21. As a flake user, I want `scry flake` with no arguments to pick the configurations that belong to
    this machine (system and home), so that one command covers my laptop.
22. As a user, I want to compare two arbitrary toplevels on one machine, such as an older generation
    against `/run/booted-system`, without any deploy tool.

Interface:

23. As a user, I want a TUI with vim-style keys and a visible key hint line, a filter, folding, and a
    tree view of file paths.
24. As a user, I want to copy what the detail pane shows, or a configuration's whole report, to the
    clipboard, also when the tool runs over SSH.
25. As a user, I want readable colours on dark and light terminals (foreground only, 16 colours), and
    `NO_COLOR` and `--color` respected.
26. As a user, I want plain text when stdout is not a terminal, with the same content.
27. As a user, I want shell completion for subcommands, flags, configuration and node names, tags and
    git revisions.
28. As a macOS user, I want the tool to build and run on my machine.

Maintainer:

29. As a maintainer, I want every external command routed through one seam that tests replace, so that
    the whole pipeline is testable without machines.
30. As a maintainer, I want a way to run the full UI on synthetic fixtures, so that demos and manual UI
    checks need no fleet.
31. As a maintainer, I want the tool packaged as a flake, so that other repositories consume it as an
    input and CI pushes it to a binary cache.
32. As a maintainer, I want no environment-specific names in code, docs or fixtures, so that
    publishing the repository is a licence decision only.

## 4. Command line

```text
scry colmena [--on SEL] [--base REV | --build-on-target] [--no-build] [COMMON]
scry flake [REF] [--on SEL] [--target NAME=TARGET]... [--base REV] [--no-build] [COMMON]
scry paths [--target TARGET] OLD NEW [COMMON]
scry completions SHELL

COMMON: [--format auto|text|json] [--color auto|always|never] [--expand-churn]
        [--word-highlight dim|background|reverse|underline|bold] [--exit-code]
        [--jobs N] [--eval-jobs N] [--timeout SECONDS] [--max-file-size BYTES]
```

### 4.1 Selection (`--on SEL`)

One syntax in every source. `SEL` is a comma-separated list of patterns; a configuration is selected
when any pattern matches.

- A pattern matches the **whole** configuration name: `web` does not match `web-1`.
- `*` matches any run of characters, `?` one character.
- `@tag` matches colmena nodes whose `deployment.tags` contain `tag`. Globs apply to tags too
  (`@db-*`). In `scry flake`, an `@` pattern is a usage error.
- A pattern that matches nothing is a usage error naming it (exit 2). This catches typos.

### 4.2 Sources

**`scry colmena`** reads the hive in the current directory. Without `--on`, every node is selected.

- Evaluation runs in two steps, both with `colmena eval -E`, without building:
  1. One cheap call returns, per node, `deployment.targetHost`, `deployment.targetUser`,
     `deployment.targetPort`, `deployment.sshOptions`, `deployment.tags` and
     `deployment.buildOnTarget` (absent means false). Selection, including `@tag`, runs on this result.
  2. The toplevel's `outPath` and `drvPath` for the selected nodes only, up to `--eval-jobs`
     evaluations at once. One NixOS evaluation takes about one core and 1 GB, so evaluating a whole
     hive in one process risks running out of memory.
- A node's target is `targetUser@targetHost`, or `targetHost` without a user. `targetPort` becomes
  `-p <port>`, and `sshOptions` are passed to `ssh` after `NIX_SSHOPTS`. A null `targetHost` means the
  node is this machine.
- `colmena --version` is checked first. If it fails: `scry: colmena not found on PATH`.

**`scry flake [REF]`** reads a flake; `REF` defaults to `.`.

- Listing: `nix eval --json --apply builtins.attrNames` on `nixosConfigurations`,
  `darwinConfigurations` and `homeConfigurations` lists names without evaluating configurations. A
  missing attribute set is skipped.
- Default selection in deployed mode, without `--on` and without `#`: the configurations of this
  machine, by the rules `nh` and `home-manager` use. That is `<host>` for NixOS and nix-darwin, and
  `$USER@<host>`, else `$USER`, for home-manager, where `<host>` is the short hostname (`hostname -s`)
  or, on macOS, also `scutil --get LocalHostName`; either one matching is enough. A bare `$USER` entry
  is not selected when `$USER@<host>` exists. A default selection that matches nothing is a usage
  error that lists the available names and suggests `--on`.
- Default selection in revision mode: every configuration of the flake, since no machine is involved
  and "this machine" would match nothing in CI.
- `--on` replaces the default selection.
- `REF#name` selects exactly one configuration. A name present in several attribute sets is a usage
  error that lists them. An empty name after `#` is a usage error.
- Evaluation: only selected configurations are evaluated: `outPath` and `drvPath` of
  `config.system.build.toplevel` (NixOS, darwin) or `activationPackage` (home), in one `nix eval` per
  configuration, up to `--eval-jobs` at once. Names that are not plain identifiers are quoted in the
  attribute path.
- Target: a configuration that matches this machine is local. For home-manager that needs both the
  user part (when present) to equal `$USER` and the host part to match, so `alice@laptop` is remote
  when `$USER` is `bob`. Otherwise `--target NAME=TARGET` sets its target, else its name is the
  target. A `user@host` name maps to that target, and a bare name resolves through `~/.ssh/config`. A `--target` entry for a configuration that is not selected gets
  a warning on stderr.

**`scry paths [--target TARGET] OLD NEW`** compares two toplevels that both live on `TARGET`, or on this
machine without `--target`. `OLD` and `NEW` may be symlinks such as `/run/current-system`,
`/run/booted-system` or `/nix/var/nix/profiles/system-42-link`; both are resolved on that machine.
There is no build step, and `--base` does not apply. The configuration's name is `TARGET`, or
`localhost`.

### 4.3 Where each side lives

| Mode | Source and node | Old side | New side |
|---|---|---|---|
| Deployed | colmena, `buildOnTarget = true` or `--build-on-target` | target, current system | target, after `colmena build` |
| Deployed | colmena, `buildOnTarget = false` | target, current system | this machine, after `colmena build` |
| Deployed | colmena, no `targetHost` | this machine, current system | this machine |
| Deployed | flake, local configuration | this machine, current system | this machine, after `nix build` |
| Deployed | flake, remote configuration | target, current system | this machine, after `nix build` |
| Revision | any | this machine, built from base | this machine, built from head |
| — | paths | `TARGET` or this machine | same machine as old |

The tool never copies a closure. Sides are compared where they are (section 6).

The current system per closure kind is the default old side in deployed mode:

| Closure kind | Current system |
|---|---|
| NixOS, nix-darwin | `/run/current-system` |
| home-manager | `$XDG_STATE_HOME/nix/profiles/home-manager` (default `~/.local/state`), else `/nix/var/nix/profiles/per-user/$USER/home-manager` |
| unknown | `/run/current-system` |

The current system is resolved on the old side's machine, with that machine's environment.

### 4.4 Build step

Without `--no-build`, the tool builds the new side, and in revision mode the base side too, before the
UI opens. The build output passes through to stderr, so stdout stays clean for text and JSON.
Evaluation before it captures its output and can take seconds, so the tool announces each wait on
stderr: `scry: evaluating the hive` (colmena step 1) or `scry: listing the configurations of <ref>`
(flake), then `scry: evaluating N nodes` or `N configurations`, with ` at head and base` in revision
mode.

- colmena, deployed mode: one `colmena build --on <names>` for the selected nodes. It honours
  `buildOnTarget`, so for those nodes colmena copies derivations to the target and builds there. That
  write is the deploy tool's build, asked for by the user, and the only write to a target a run can
  cause. `--build-on-target` passes colmena's own `--build-on-target`, which treats every selected
  node as `buildOnTarget`, and scry reads every new side on its target to match. It suits a machine
  whose local Linux builds differ from the targets' own, such as a Mac whose case-insensitive store
  leaks `~nix~case~hack~` names into symlink targets. A node without `targetHost` then fails in
  colmena, as it does with `buildOnTarget`. The flag conflicts with `--base`, which contacts no
  target.
- flake, deployed mode: one `nix build --keep-going` over all selected toplevels.
- Revision mode, both sources: one `nix build --keep-going` per revision over the selected toplevels'
  `drvPath`s (`<drv>^out`). Everything builds on this machine and ignores `buildOnTarget`, because no
  target may be contacted. Remote builders and substituters come from the local Nix configuration.
- Local build results get out-links in the run's temporary directory, so a concurrent garbage
  collection cannot remove them mid-run. They are removed at exit. Closures on targets get no GC
  root: a target collected mid-run fails that configuration with the missing-path error.
- A failed build does not abort the run. Each configuration whose toplevel is missing afterwards fails
  on its own with the error in section 9. Whether `colmena build` keeps going after one node fails is
  unverified (acceptance check 7).

With `--no-build`, nothing is built, and the run writes nothing to any machine except its own
temporary directory. A side whose toplevel is missing fails as in section 9.

### 4.5 Revision mode (`--base REV`)

- `REV` is anything `git rev-parse` accepts. The source must be a local path inside a git checkout,
  otherwise it is a usage error (exit 2): `scry flake github:owner/repo --base main` is rejected.
- A flake or hive in a subdirectory of the checkout is found at the same relative path inside the
  base worktree.
- Head is the working directory as it is, including uncommitted changes. Nix sees tracked files only.
- Base is a temporary `git worktree add --detach` at `REV`, placed outside the checkout. It is removed
  when the run ends, including on error and on Ctrl-C.
- The configuration list comes from head. For colmena, base is evaluated with the same `colmena eval -E`
  inside the base worktree. For flakes, it is evaluated with `nix eval` on the worktree path.
- A selected configuration missing at base gets status `absent at base`, with no sections. It is not a
  failure.
- A base that fails to evaluate or build fails only the configurations it affects.

### 4.6 Common flags

| Flag | Default | Meaning |
|---|---|---|
| `--format` | `auto` | `auto`: TUI when stdout is a terminal, text otherwise. `text`, `json`: force. |
| `--color` | `auto` | `auto`: colour when stdout is a terminal and `NO_COLOR` is unset or empty. |
| `--expand-churn` | off | Show hash-only and version-only groups expanded (text) or open (TUI). |
| `--word-highlight` | `dim` | How changed words stand out in a paired hunk line (7.3). Text and TUI only. |
| `--exit-code` | off | Exit codes follow `diff(1)`: 1 for changes, 2 for trouble (4.7). |
| `--jobs` | 16 | Configurations fetched at once. Unmeasured default. |
| `--eval-jobs` | CPU count | Concurrent `nix eval` / `colmena eval` calls. One NixOS evaluation takes about one core and 1 GB. |
| `--timeout` | 60 | Seconds per configuration for the fetch phase (section 6). Builds and evaluations have no timeout. Unmeasured default. |
| `--max-file-size` | 1048576 | Files larger than this on either side are reported without content. |

`scry --version` prints the version and the git revision it was built from, with `-dirty` when built
from a dirty tree. Inside `nix build` there is no `.git`, so the flake passes `self.shortRev` or
`self.dirtyShortRev` to the build through an environment variable. Outside Nix, the build reads git.

### 4.7 Exit codes

A configuration **failed** when it errored, timed out, or was still unfinished when the user quit the
TUI. A configuration **has changes** when old ≠ new, which includes `absent-at-base`.

| Outcome | Default | With `--exit-code` |
|---|---|---|
| All compared, no changes | 0 | 0 |
| All compared, some changes | 0 | 1 |
| At least one failed | 1 | 2 |
| Usage error (bad flags, unknown selection pattern, `--base` outside a git checkout) | 2 | 2 |

With `--exit-code` the codes follow `diff(1)`: `0` same, `1` different, `2` trouble.

A broken pipe on stdout ends the run quietly with the exit code it would have had.

### 4.8 Environment

- `NO_COLOR`: a non-empty value disables colour under `--color auto`.
- `NIX_SSHOPTS`: split shell-style, as Nix does (spaces and tabs separate words; single quotes,
  double quotes and backslashes group them), and passed to every `ssh` invocation. A value that does
  not split, such as one with an unterminated quote, is a fatal error naming `NIX_SSHOPTS`.
- `USER`, and the short hostname: flake default selection.
- On the old side's machine: `HOME`, `USER`, `XDG_STATE_HOME` for the home-manager current system.

## 5. Closure kinds

The kind is detected from the new toplevel, on its machine, in the same round trip as the other
toplevel probes (section 6.2):

| Marker file | Kind | Followed trees | Boot section |
|---|---|---|---|
| `nixos-version` | NixOS | `etc` | yes |
| `darwin-version` | nix-darwin | `etc`, `Library/LaunchAgents`, `Library/LaunchDaemons`, `user/Library/LaunchAgents` | no |
| `hm-version` | home-manager | `home-files`, `LaunchAgents`, `LaunchAgentDomains`, `LaunchAgentRestartTriggers`, `putter.json` | no |
| none of them | unknown | none | no |

- An unknown kind still gets Packages and the toplevel's regular files, plus a visible note
  `unknown closure kind: no followed trees, no Boot section`.
- If the old toplevel's marker names a different kind, that configuration fails with
  `closure kinds differ: old is <kind>, new is <kind>`.
- The content of the marker file, trimmed, is the configuration's **system version**, shown in the
  header as `old -> new`. `nixos-version` holds `26.05.20261102.7dcdd87`, and `darwin-version` holds
  `26.11.ff0e21a`, both seen on real toplevels.
- Toplevel files shown elsewhere are excluded from Files: the marker files, `darwin-version.json`
  and `kernel-params`.
- A followed entry may be a link to a single store file (`putter.json`); its content is compared like
  any other file.
- Followed trees are an allow-list: package trees such as `sw`, `home-path`, `Applications` and
  `Library/Fonts` are never followed. A new symlink to a package tree must not pull a whole package
  into the comparison. An allow-list fails towards missing content, which is visible and cheap to fix.
- Excluded inside followed trees: `etc/profiles/per-user` (NixOS, nix-darwin). Exclusions are paths
  relative to the toplevel and match only at that exact path.

## 6. Fetching

### 6.1 Hard constraints

- **Transport**: commands run on a target through the `ssh` binary, never through an in-process SSH
  library and never through `nix --store ssh://` or `ssh-ng://`. Measured on a NixOS server with a
  709-path closure: `ssh host nix path-info --json -r` took 1.4 s, and the same through
  `--store ssh-ng://host` took 24 s. Invocation:
  `ssh -o BatchMode=yes <NIX_SSHOPTS…> <colmena sshOptions…> <multiplexing options> [-p <port>] <target> -- <cmd>`.
  For options given on the command line, ssh keeps the first value it sees. So the user's
  `NIX_SSHOPTS` and the hive's `sshOptions` beat the tool's own defaults.
- **One connection per target**: all commands to one target share one SSH connection for the run.
  - Why: each round trip would otherwise pay a full handshake, while one over an open connection
    took 0.13 s, measured. Hardware security keys and agent-less passphrase keys would also prompt
    on every connection.
  - Respect existing setups: when `ssh -G <target>` (local, no connection) shows a `controlpath`
    other than `none`, the user's own multiplexing is used as is.
  - Otherwise pass `-o ControlMaster=auto -o ControlPath=<dir>/<n> -o ControlPersist=60`, with `<n>`
    one name per connection: `%C` would give two connections that differ only in options, such as an
    identity file, the same socket. `<dir>` is a private (mode 0700) per-run directory with a short path such as `/tmp/scry-XXXXXX`, because
    macOS rejects socket paths of 104 bytes or more (hit while measuring). At exit, the tool sends
    `ssh -O exit` to each master it opened and removes the directory. `ControlPersist=60` bounds a
    master left behind by a hard kill.
  - Concurrency: at most 4 commands at once over one connection. OpenSSH's server allows 10
    sessions per connection by default (`MaxSessions`), and the design needs 2, one per side.
  - Opt-out without a flag: `NIX_SSHOPTS="-o ControlMaster=no"` wins by the first-value rule.
  - Any process of the same user can use an open socket, as with the user's own multiplexing. The
    private directory keeps it to that user.
- **Login shell independence**: the remote login shell may be bash, zsh or fish. Every remote command is
  one `sh -c '<script>'` with any arguments single-quoted, so the login shell only ever parses
  single-quoted words. Inside single quotes, fish treats `\\` and `\'` as escapes and POSIX shells do
  not, so the command line sent to `ssh` must contain no backslash and no quote inside a quoted word. A
  test enforces this for every remote script. Data such as path lists goes over stdin, never on the
  command line.
- **PATH**: a non-interactive SSH session may lack the Nix profile on `PATH`, for example a multi-user
  install on macOS. Every remote script appends `/run/current-system/sw/bin`,
  `/nix/var/nix/profiles/default/bin` and `$HOME/.nix-profile/bin` to `PATH` before running
  anything.
- **Allowed commands on a side's machine**: `nix`, `sh` and its builtins, `echo`, `cat`, `ls`,
  `find`, `xargs`, `cksum`, `tar`, and `gzip` when present (probe for it, fall back to
  uncompressed). Use only POSIX options, plus `tar -c -h -f - -T -`, which GNU tar 1.35 and
  bsdtar 3.5.3 both accept (checked). No GNU-only options of other tools, since macOS ships BSD
  userland. Resolve a toplevel with `cd -P <path> && pwd -P`, not `readlink -f`.
- **Nix flags**: every `nix` invocation passes `--extra-experimental-features nix-command`, so machines
  without it enabled work. Invocations that take a flake reference (local evaluation and builds) pass
  `--extra-experimental-features 'nix-command flakes'`.
- **Read-only**: nothing is written on any target: no copy, no GC root, no profile change, no temporary
  file. Temporary files on this machine go to the system temporary directory and are removed.
- **Locality**: a side on this machine runs the same commands without `ssh`.
- **Round trips**: per configuration and per machine that holds a side, at most
  **2 + link depth + reference depth** round trips (section 6.2). Link depth is the longest chain of
  store symlinks under a followed tree, 5 on a measured NixOS `etc`. The count is independent of the
  number of files. Where both sides are on one machine, their commands share round trips.
- **argv limits**: lists of paths go over stdin or in chunks, never as one unbounded argument list.
  A five-bump span had 845 changed files, and a single argument list failed on it.
- **Process count**: a remote command must not start one process per file. Measured on 1688 files:
  a per-file shell loop took 15 s, and one `tar` stream took 0.36 s.

### 6.2 Round trips per side's machine

1. **Probe**: resolve the toplevel (`cd -P … && pwd -P`). Read the marker files. For NixOS: the
   `kernel`, `initrd` and `kernel-modules` link targets (`ls -ld`), `kernel-params`, and the entries
   under `specialisation` with their targets. Resolve the current system when this side is the default
   old side. Run
   `nix path-info --json -r <toplevel>` for the closure with `narSize` per path. A toplevel that is
   missing from the store fails as in section 9. The probe of the new side yields the kind, which
   decides the trees for step 2. Where the old side's machine differs, its probe runs in parallel.
2. **Tree listing, one round trip per link level, never following links**: the first level's only root
   is the toplevel. `find` without `-L` descends real directories only, so links such as `sw` or
   `etc` are listed, not entered. Each level sends its roots over stdin to one script of
   the shape
   `LC_ALL=C; export LC_ALL; nl="<newline>"; set --; while IFS= read -r r; do if [ -e "$r" ] || [ -h "$r" ]; then set -- "$@" "$r"; fi; done; find "$@" -name "*$nl*" -exec echo newline: {} ";" -o -type l -exec ls -ldn {} + -o -type f -exec cksum {} + -o -type d -print`,
   where `<newline>` is a literal newline. `-a` binds tighter than `-o`, so no escaped parentheses
   are needed, and the script needs no nested quotes (see the login-shell rule). Roots that do not
   exist are skipped, since `find` fails on them; their links are dangling. `ls -ldn` prints numeric
   ids, so an owner or group name with a space cannot shift the fields. `ls` prints a newline in a
   name as `?`, so the first alternative reports such names (section 9). Roots are chunked below the
   machine's argument limit, inside the script. Locally:
   - a symlink at or below a followed tree whose target is in `/nix/store` is replaced by that target,
     which becomes a root of the next level unless it was already listed. Store links elsewhere (`sw`,
     `Applications`, `Library/Fonts`, `etc/profiles/per-user`) are neither followed nor compared;
   - a symlink whose target is outside `/nix/store` is an **external link**: recorded with its target
     text and never followed, so the result depends only on the closure and not on the live
     filesystem (measured on NixOS: `mtab -> /proc/mounts`, a masked unit `-> /dev/null`,
     `fonts.conf -> /etc/fonts/fonts.conf`);
   - a store target missing from the listing is a dangling link;
   - the dereferenced tree is rebuilt as relative path to (type, checksum, size, or link text).

   Exclusions apply to relative paths. For the toplevel itself, only its own regular files count, not
   the followed trees, `specialisation`, or package links such as `sw`.
3. **Content fetch**: compare the trees locally. For every path whose checksum, size or type differs
   and whose size is at most `--max-file-size` on both sides, fetch both versions' contents in one
   stream per machine (`tar -chf - -T -` reading paths from stdin, piped through `gzip -1` when
   available).
4. **Reference fetch**, once per reference depth level (section 7.4): list, checksum and fetch the
   followed references and small-rebuild `bin/` directories in one round trip of the same shape.

A non-zero exit or a stderr diagnostic from `find` fails the configuration with its first stderr line.

Measured on a NixOS server over a VPN, `etc` tree:

| Case | Listing | Changed files | Content fetch |
|---|---|---|---|
| Non-following level listing, one side | 5 levels, 5 round trips, 1.3 s, 1 MiB, 5709 files, 3 external links | — | — |
| `find -L … cksum`, config change, same nixpkgs | 0.13 s per side | 15 | not measured |
| `find -L … cksum`, one nixpkgs bump | 0.13 s warm, 0.83 s cold, per side | 58 | not measured |
| `find -L … cksum`, five nixpkgs bumps | 0.38 s for both sides | 845 | 0.36 s raw (8.6 MB), 0.18 s with `gzip -1` (1.9 MB) |

`find -L` follows external links into the live filesystem, so it is a baseline only: it gives the
changed-file counts the level listing must reproduce, minus external links. A walk with
`nix store ls -R --json` instead took 20.8 s on the one-bump case: about 395 `nix` processes and
4.9 MiB of listings, because `etc` links into whole package trees. It also reports false positives that
only content can settle, such as mode files inside a rebuilt `etc` root.

### 6.3 Parallelism and failure isolation

- Configurations are fetched in parallel, up to `--jobs` at once, each fully independent.
- Every per-configuration step fails only that configuration: evaluation, build presence, probe,
  listing, fetch, parse. The others are still reported.
- `--timeout` bounds the fetch phase of one configuration, from its first probe to its last fetch.
  When it expires, the child processes are killed and the status is `timeout after N s`.
- Ctrl-C (or quitting the TUI) kills all child processes, removes temporary worktrees and files, and
  exits with the code from 4.7.

## 7. Sections

### 7.1 Package names and versions

- A store path's name is the file name after the 32-character hash and its `-`. A file name whose first
  `-` is not at position 32 is a name without a hash.
- The name splits into **pname** and **version** by the `builtins.parseDrvName` rule: the version starts
  after the first `-` that is not followed by a letter. `python3-3.12.4` gives `python3` and `3.12.4`.
  `libx11-1.8` gives `libx11` and `1.8`. `hello` gives `hello` and an empty version. Files churn
  normalisation (7.3) uses the same rule.
- Output suffixes: when a version exists and ends in `-bin`, `-dev`, `-lib`, `-man`, `-doc`, `-info`,
  `-out`, `-debug`, `-static` or `-devdoc`, that suffix is dropped from the version. A versionless name
  such as `foo-dev` stays as it is.
- The toplevel itself is never a package row, and neither are the toplevels of its specialisations
  (the targets of `specialisation/*` from the probe). Its change is the header's system version
  (section 5), and specialisations are in Boot. Embedded hostnames ending in a digit (`web-1`) would
  otherwise split wrong.
- Paths ending in `.drv` are ignored.

### 7.2 Package comparison

Per side, all paths with one pname merge into one package: the set of versions, the set of paths, and
the summed `narSize`. `narSize` is the unpacked size, as `nix path-info --closure-size` and `nvd`
report it.

| Condition | Kind | Tag | Colour |
|---|---|---|---|
| Only in new | added | `[A]` | green |
| Only in old | removed | `[R]` | red |
| One version per side, new > old | upgraded | `[U]` | cyan |
| One version per side, new < old | downgraded | `[D]` | yellow |
| Versions differ but are unordered, or several versions on a side | changed | `[C]` | magenta |
| Same version set, different paths, size moved ≥ 1 MiB and ≥ 10 % of old | resized | `[S]` | default |
| Same version set, different paths otherwise | rebuilt | collapsed | default |
| Same version set, same paths | unchanged | not shown | — |

- Version order is `builtins.compareVersions`, implemented to match Nix's documented behaviour, with
  one exception for git revisions. Split both versions into fields at `.` and `-`. When a field at
  the same index is, on both sides, 7 or more lowercase hex characters with at least one letter, and
  the two fields differ, the versions are unordered (changed). Nix's own splitter breaks such a field
  into digit and letter runs, so the check runs on these fields, before Nix's split. Two versions that
  compare equal but are different strings (`1.0` and `1.00`) are changed.
- Several versions on one side show as a set difference: the old-only versions `->` the new-only
  versions, for example `{1.0, 2.0} -> {2.0, 3.0}` shows `1.0 -> 3.0`. They are always changed, never
  upgraded.
- Version text in a row: added and resized rows show the new versions, removed rows the old ones, and
  the rest show `old -> new`.
- Order: by kind in the order upgraded, downgraded, changed, added, removed, resized; by pname
  within a kind. Then one collapsed line: `N rebuilt (same version, new hash, <size delta>)`. Expanded,
  it lists each rebuilt pname with its version and size delta.
- Each row shows the pname's size delta. The section header shows
  `Packages  <old closure size> -> <new closure size> (<delta>)`.
- Sizes: under 1024 bytes as an integer with `B`. Otherwise in steps of 1024 with one decimal and
  `KiB`, `MiB`, `GiB` or `TiB`, rounded half away from zero. A value that rounds to `1024.0` moves up
  one unit (`1.0 MiB`). A delta carries `+` or `-`, and a zero delta is `0 B`.
- A section with nothing to show reads `Packages  <sizes>` followed by `no package changes`.

### 7.3 Files

**What is compared**: every path from the tree listings of section 6.2, by relative path, plus the
toplevel's regular files under their own names.

| Situation | Status | Tag | Colour |
|---|---|---|---|
| Regular file on both sides, content differs | modified | `[M]` | cyan |
| Only in new | added | `[A]` | green |
| Only in old | removed | `[R]` | red |
| External link on both sides, target text differs | link changed | `[L]` | cyan |
| Kind differs (file, directory, external link, dangling link) | type changed | `[T]` | magenta |
| Modified, fetched content is binary on either side | binary | `[B]` | magenta |

- Content is **binary** when it has a NUL byte in its first 8 KiB, or is not valid UTF-8. Binary
  content is never shown. A modified binary file is `[B] path  (binary, <old size> -> <new size>)`.
  Added or removed binary files keep `[A]`/`[R]` with `(binary, <size>)`.
- A store symlink whose target changes but whose content does not is no change: content is what
  matters.
- Files over `--max-file-size` on either side are never fetched, so their binary status is unknown:
  `[M] path  (content not fetched, <old size> -> <new size>)`.
- Added and removed text files show their whole content as one hunk.
- Order: by displayed path in byte order. Followed references (7.4) come right after their referrer.
  Small rebuilds without a referrer sort by their own `name/bin/script` path.
- Line diffs are unified, with 3 lines of context, computed locally. Hunk headers are shown dimmed as
  `@@ -a,b +c,d @@`.

**Churn**: a modified text file is classified before display.

A **store reference** in text is `/nix/store/`, 32 characters of Nix's base32 alphabet
`0123456789abcdfghijklmnpqrsvwxyz`, `-`, and a name: the longest run of Nix's store-name characters
`[A-Za-z0-9+._?=-]`. A following `/subpath` is not part of the reference. The reference is the root
store path.

1. **Hash-only**: equal once every store hash is replaced by one placeholder on both sides. Only
   hashes inside store references count, so checksums inside config files stay real changes.
2. **Version-only**: not hash-only, but equal once the version of every store reference's name is
   replaced too (split by the 7.1 rule, so `single-file-1.26.4` loses `1.26.4`), and, outside store
   references, the versions scry knows per side are masked. Such a file only follows a package
   bump the Packages section, or a version bump the header and Boot section, already report. A
   known version is masked only when both sides have one and they differ, and only where no ASCII
   letter or digit touches it, so `6.1.5` stays in `6.1.50`. The known versions are:
   - the **system version** (section 5), each side's own in its own text;
   - the **nixpkgs revision**: when the system version ends in `.<rev>` with `<rev>` at least 7 hex
     digits, as flake builds' `26.05.20261102.7dcdd87` does, any 40-hex-digit string starting with
     `<rev>`;
   - the **kernel version**: the version of the `kernel` link target's store name (7.5), so
     `linux-6.12.48` gives `6.12.48`.

   A nixpkgs bump within one release thus leaves `boot.json` (its `label` reads
   `NixOS <codename> <system version> (Linux <kernel version>)`), `etc/issue`, `etc/os-release`
   (`BUILD_ID`) and the `nixos-version` script (system version, full revision, kernel version) as
   version-only. A release bump also changes the release or codename in `etc/os-release`,
   `boot.json` and the `nixos-version` script, which then stay real changes.
3. Otherwise a real change.

Churned files are grouped: `N hash-only (store hash churn)` and
`N version-only (version bumps)`. Both are collapsed by default and opened with
`--expand-churn` or in the TUI.

**Word highlight**: inside each hunk, each run of removed lines pairs with the run of added lines right
after it, line by line. Unpaired lines are coloured whole. A paired line is split into tokens: a run of
Unicode alphanumerics or `_`, or any single other character. A token-level longest common subsequence
marks the tokens that differ; store hashes count as equal, so hash churn next to a real change is not
marked. Two marked runs separated by at most 2 characters, none of them
alphanumeric, join into one, so `"4.6.8"` against `"5.0.1"` marks `4.6.8` whole rather than three
digits. A pair whose only difference is store hashes is dimmed, without highlight.

A paired line with marked runs is styled by `--word-highlight`; the line's colour is red for removed,
green for added lines:

| Style | Rest of the line | Marked runs |
|---|---|---|
| `dim` (default) | dim, line's colour | bold, line's colour |
| `background` | line's colour | removed: bright white on red; added: black on green |
| `reverse` | line's colour | bold reverse video, line's colour |
| `underline` | line's colour | underlined, line's colour |
| `bold` | line's colour | bold, line's colour |

How visible bold is depends on the terminal's theme and font, so no single style suits every
terminal. A paired line without marked runs (the side of a pure insertion or deletion) is coloured
whole like an unpaired line.

### 7.4 Followed references and small rebuilds

So a config file referenced only from a unit or a plist still shows:

- **Followed references**: for every modified text file, churned or real, pair the store references on
  its old and new lines. Within each hunk, each run of removed lines pairs with the run of added lines
  right after it, line by line (the same pairing as word highlight), and the j-th reference on one
  line pairs with the j-th on the other. Every pair whose paths differ, after de-duplication, is
  compared:
  - as a file, when both are regular files;
  - as their `bin/` directories, when both are directories of at most 1 MiB `narSize`. Binaries are
    dropped from `bin/` results.

  Results are file changes named `referrer → name` or `referrer → name/bin/script`, classified with
  churn like any other file. `name` is the new side's store name, or the old one's when it only exists
  there. The JSON `path` is the referrer's path, and `via` holds the chain of names.
- **Small rebuilds**: every rebuilt package whose paths are each at most 1 MiB `narSize` on both sides,
  with exactly one path per side, gets its `bin/` compared the same way. It is named `name/bin/script`,
  or `referrer → …` when a modified file also points at it. These results count as depth 1.
- **Depth**: references found in followed results are followed again, up to depth 2 in total. Each depth
  level is one round trip (section 6.2 step 4). Chains read `a → b → c`.

### 7.5 Boot (NixOS only)

| Item | Comparison | Display |
|---|---|---|
| `kernel`, `initrd`, `kernel-modules` | Link target store path | `kernel: <old name> -> <new name>`, or `kernel: <name> (rebuilt)` for the same name with a new hash, or `(none)` for a missing side |
| `kernel-params` | Text with whitespace runs folded to one space, trimmed | `kernel-params: <old> -> <new>` |
| `specialisation/*` | Entry names, and target path per name | `[A] specialisation X` green, `[R] specialisation X` red, `[M] specialisation X (changed)` cyan |

The section is hidden when nothing differs.

## 8. Report and outputs

### 8.1 Report

The core produces one report per run; every output renders it. Its JSON form is the `--format json`
output, so it is a public interface:

- `schema_version: 1`. Adding a field does not bump it. Renaming, removing or changing a field's
  meaning does.
Fields and their values:

| Field | Values |
|---|---|
| `mode` | `deployed`, `revision`, `paths` |
| `base`, `head` | revision mode only, otherwise absent. `base`: `{"ref": "<as typed>", "rev": "<full sha>"}`. `head`: `{"rev": "<HEAD sha>", "dirty": true}` |
| configuration `source` | `colmena`, `flake`, `paths` |
| configuration `kind` | `nixos`, `nix-darwin`, `home-manager`, `unknown` |
| configuration `status` | `done`, `no-changes`, `error`, `timeout`, `absent-at-base` |
| side `location` | `"local"` or the target string, as in `root@web-a` |
| package change `kind` | `upgraded`, `downgraded`, `changed`, `added`, `removed`, `resized` |
| file change `status` | `modified`, `added`, `removed`, `link-changed`, `type-changed` |
| file change `class` | `real`, `hash-only`, `version-only` |
| file `type_old`, `type_new` | `file`, `directory`, `external-link`, `dangling-link`, `null` when absent |
| hunk line `op` | `" "`, `"-"`, `"+"` |

- Store paths appear in full. Sizes are integers in bytes. `spans` are UTF-8 byte ranges
  `[start, end)` of highlighted runs in `text` (7.3).
- `error` is present only with status `error` or `timeout`. `packages`, `files` and `boot` are present
  only with status `done`. `boot` is `null` for kinds other than NixOS.
- `shared` lists items of status-`done` configurations only (8.2).
- `no-changes` means old and new resolve to the same store path, in every mode.

Example, shortened (`…` marks elisions, which the real output does not contain):

```json
{
  "schema_version": 1,
  "tool_version": "1.0.0",
  "mode": "deployed",
  "configurations": [
    {
      "name": "web-a",
      "source": "colmena",
      "kind": "nixos",
      "status": "done",
      "old": { "path": "/nix/store/…-nixos-system-web-a-25.05.20250801.6027c30", "system_version": "25.05.20250801.6027c30", "location": "root@web-a" },
      "new": { "path": "/nix/store/…-nixos-system-web-a-25.11.20251001.9a8b7c6", "system_version": "25.11.20251001.9a8b7c6", "location": "local" },
      "packages": {
        "size_old": 1288490188, "size_new": 1331439861,
        "changes": [ { "kind": "upgraded", "pname": "openssl", "old_versions": ["3.0.13"], "new_versions": ["3.0.14"], "size_delta": 100 } ],
        "rebuilt": { "count": 412, "size_delta": 3250586, "items": [ { "pname": "bash", "versions": ["5.2p37"], "size_delta": 0 } ] }
      },
      "files": {
        "changes": [
          {
            "path": "etc/nix/nix.conf", "via": [], "status": "modified", "class": "real",
            "type_old": "file", "type_new": "file", "binary": false, "content_fetched": true,
            "size_old": 812, "size_new": 829, "link_old": null, "link_new": null,
            "hunks": [ { "old_start": 11, "old_lines": 3, "new_start": 11, "new_lines": 3, "lines": [
              { "op": " ", "text": "substituters = https://cache.example", "spans": [] },
              { "op": "-", "text": "trusted-public-keys = cache-a:AAAA", "spans": [] },
              { "op": "+", "text": "trusted-public-keys = cache-a:AAAA cache-b:BBBB", "spans": [[34, 47]] },
              { "op": " ", "text": "max-jobs = auto", "spans": [] }
            ] } ]
          }
        ]
      },
      "boot": {
        "kernel": { "old": "linux-6.12.40", "new": "linux-6.12.48", "rebuilt": false },
        "initrd": { "old": "initrd-linux-6.12.40", "new": "initrd-linux-6.12.48", "rebuilt": false },
        "kernel_modules": null,
        "kernel_params": null,
        "specialisations": { "added": ["debug"], "removed": [], "changed": [] }
      }
    },
    { "name": "web-b", "source": "colmena", "kind": "unknown", "status": "error", "error": "ssh root@web-b: connect to host web-b port 22: Connection refused", "old": null, "new": { "path": "/nix/store/…", "system_version": null, "location": "local" } }
  ],
  "shared": [
    { "item": { "section": "packages", "change": { "kind": "upgraded", "pname": "openssl", "old_versions": ["3.0.13"], "new_versions": ["3.0.14"] } }, "configurations": ["web-a", "web-c"] }
  ]
}
```

In `boot`, an unchanged item is `null`. A configuration that failed before its kind was known has
kind `unknown`, and a side not yet resolved is `null`.

### 8.2 Shared changes

A change is shared when it appears identically in two or more status-`done` configurations of a run:

- Package change: same kind, pname, old versions and new versions. Size deltas are not compared.
- File change of class `real`: same path, `via`, status, and hunks equal after store-hash
  normalisation. Churned files, rebuilt packages, the system version and sizes are never shared items.
- Boot item: same item and same old and new names.

Shared changes are listed once with their configurations, in the TUI's **All** entry and at the top of
text output. Per-configuration views stay complete and mark shared items with `(also in N)`, where
`N` is the number of other configurations.

### 8.3 Text output

Configurations are printed in name order, separated by one blank line, after all finish. Shared changes
come first when there are two or more configurations. Colour as in the tables above; `old`/`new` lines
and hunk headers are dimmed. Normative example (`<h>` stands for a 32-character hash):

```text
Shared across configurations
  [U] openssl  3.0.13 -> 3.0.14             web-a, web-c
  [M] etc/nix/nix.conf                       web-a, web-c

web-a · nixos (root@web-a)
  old /nix/store/<h>-nixos-system-web-a-25.05.20250801.6027c30
  new /nix/store/<h>-nixos-system-web-a-25.11.20251001.9a8b7c6
  system 25.05.20250801.6027c30 -> 25.11.20251001.9a8b7c6
Packages  1.2 GiB -> 1.3 GiB (+41.0 MiB)
  [U] openssl         +100 B     3.0.13 -> 3.0.14  (also in 1)
  [U] systemd         +1.2 MiB   257.5 -> 258.1
  [C] my-tool         +4.0 KiB   1.2.0-1a2b3c4 -> 1.2.0-9f8e7d6
  [A] new-agent       +2.0 MiB   1.4.0
  [R] old-lib         -300.0 KiB 0.9
  [S] linux-firmware  +12.0 MiB  20250801
  412 rebuilt (same version, new hash, +3.1 MiB)
Files
  [M] etc/nix/nix.conf  (also in 1)
      @@ -11,3 +11,3 @@
       substituters = https://cache.example
      -trusted-public-keys = cache-a:AAAA
      +trusted-public-keys = cache-a:AAAA cache-b:BBBB
       max-jobs = auto
  [A] etc/ssh/authorized_keys.d/deploy
      @@ -0,0 +1,1 @@
      +ssh-ed25519 AAAA… deploy@example
  [L] etc/mtab  /proc/mounts -> /proc/self/mounts
  [T] etc/systemd/system/debug-shell.service  file -> external link /dev/null
  [M] etc/systemd/system/app.service → app-start/bin/app-start
      @@ -1,4 +1,4 @@
       #!/bin/sh
       set -eu
      -exec app --workers 4
      +exec app --workers 8
  [M] etc/udev/hwdb.bin  (content not fetched, 13.5 MiB -> 13.6 MiB)
  57 hash-only (store hash churn; --expand-churn shows them)
  3 version-only (version bumps; --expand-churn shows them)
Boot
  kernel: linux-6.12.40 -> linux-6.12.48
  initrd: initrd-linux-6.12.40 -> initrd-linux-6.12.48
  kernel-modules: linux-6.12.48-modules (rebuilt)
  kernel-params: quiet loglevel=4 -> quiet loglevel=4 mitigations=auto
  [A] specialisation debug

web-b · nixos (root@web-b)
  error: ssh root@web-b: connect to host web-b port 22: Connection refused

web-c · nixos (root@web-c)
  …

web-d · nixos (root@web-d)
  no changes, already running /nix/store/<h>-nixos-system-web-d-25.11.20251001.9a8b7c6
```

The `[T]` row is a unit masked in the new generation: a store file became an external link to
`/dev/null`.

- Header: `<name> · <kind> (<location of old>)`. In revision mode the parenthesis reads
  `(<base short rev> -> working tree)`.
- Rows align: pname padded to the widest pname, delta to the widest delta, trailing spaces trimmed.
- Status lines: `error: <message chain joined by ": ">` in red, `timeout after N s` in red,
  `not present at base <rev>` dimmed. For no changes: `no changes, already running <path>` in
  deployed mode, `no changes, same toplevel <path>` in revision and paths modes, dimmed.
- An empty Files or Boot section is omitted.
- Text output keeps rebuilt packages collapsed (JSON has the items). Churn groups stay collapsed
  unless `--expand-churn` is given.

### 8.4 TUI

Built with ratatui on crossterm. It opens after the build step and before fetching, and shows each
configuration's status live.

**Layout**: a left pane lists **All** (when there are two or more configurations, with the count of
shared changes, and no index), then the configurations in name order. Each configuration row shows
its index (`1`–`9` for the first nine configurations), name and status:

| Status | Display |
|---|---|
| Fetching | `fetching…`, dimmed |
| Done | `done` in green, plus the closure size delta |
| No changes | `no changes`, dimmed |
| Failed, timed out | `error` or `timeout`, red |
| Absent at base | `absent at base`, dimmed |

Left pane width: the widest label plus room for the status, clamped to a third of the screen. The detail
pane shows the selected entry as in 8.3, with collapsible nodes. A bottom line shows key hints, or a
transient notice.

**Keys**:

| Key | Action |
|---|---|
| `j`/`k`, Down/Up | Move one row |
| `J`/`K` | Next/previous actionable row (a collapsible node or a change) in detail; same as `j`/`k` in the list |
| `g`/`G` | Top/bottom of the detail pane |
| Ctrl-D/Ctrl-U, PgDn/PgUp | Half a pane, in screen lines |
| `h` | Collapse the open node, else go to its parent node, else focus the list |
| `l` | Focus detail, then expand |
| Enter, Space | In the list: focus detail. In detail: toggle the node, or on a leaf collapse its parent and move to it |
| Tab, Shift-Tab | Switch pane |
| `1`–`9` | Select that configuration |
| `w` | Toggle line wrap (default on) |
| `t` | Toggle tree view of file paths (default on) |
| `/` | Edit the filter. Enter applies it, Esc cancels and keeps the previous filter, Backspace deletes |
| `y` | Copy the detail pane as shown (filter and open state applied, rows unwrapped) |
| `Y` | Copy the selected configuration as text output prints it, without colour |
| `q`, Ctrl-C | Quit (Ctrl-C also while editing the filter) |

- **Scrolling** keeps 5 lines between the cursor and the pane edge where possible.
- **Wrap**: long rows wrap at the pane width by display width, continuing under the row's text with
  a hanging indent. The indent is dropped when it would leave less than half the width. With wrap off,
  rows are cut at the edge.
- **Tree view** groups file paths into directories in first-seen order and merges single-child
  directory chains into one node (`systemd/system/  (2)`). Text after ` → ` never splits into
  directories.
- **Filter**: case-insensitive substring match on pnames and file paths, in every configuration and in
  All. Boot rows are always shown. The pane title shows `[filter: …]`.
- **Default open state**: rebuilt and churn groups closed (churn open with `--expand-churn`), file
  changes closed, directories open. Open state is kept per configuration.
- **Clipboard**: `y` and `Y` emit an OSC 52 escape, so it works over SSH with no clipboard tool. The
  terminal must allow OSC 52 (tmux needs `set-clipboard on`) and never confirms it. Terminals and
  tmux cap the payload and may drop a large one silently, so the notice reads
  `copied N lines (X KiB)`, plus `, may exceed the terminal's clipboard limit` above 100 KiB of
  payload. While the configuration is still fetching it reads `nothing to copy yet`. The 100 KiB
  figure is a judgement, unmeasured.
- Quitting while configurations are still fetching counts those as failed (4.7).
- Mouse support, light-theme detection and truecolour are out of scope.

### 8.5 Colour

Foreground colours from the 16-colour palette, plus bold and dim. No background colours, so output
stays readable on dark and light terminals; the exceptions are the opt-in `--word-highlight` styles
(7.3), which add background colours, reverse video and underline to marked runs. No colour, and no
attribute, when colour is disabled (4.6).

## 9. Errors

- Fatal errors (usage, a missing deploy tool, an unreadable hive or flake) print `scry: <message>` to
  stderr and exit as a failure (4.7), or with 2 for usage errors.
- A missing toplevel on a side: `<path> is not in the store on <this machine|target>`. When the run
  has a build step for that side, `; the build did not produce it` is appended, or
  `; run without --no-build` under `--no-build`. A missing current system reads
  `no current system at <path> on <machine>`.
- A missing external command on a side: `` `<cmd>` not found on <this machine|target> ``.
- An SSH failure (exit 255): `ssh <target>: <last stderr line that is not a warning>`; ssh prints its
  verdict last, after warnings and banners such as the box of a changed host key.
- Any other failed command: `` `<program>` failed (exit N): <first stderr line> ``. The full argv and
  stderr go to the log only when a debug flag or environment variable asks for it (implementer's
  choice, documented in `--help`).
- A file name containing a newline cannot be listed reliably with `cksum` or `ls`. It fails that
  configuration with an error naming the tree, rather than giving a wrong answer. A newline in a
  link's target cannot be detected with POSIX `find`; `ls` shows it as `?`.

## 10. Shell completion

`scry completions <bash|zsh|fish>` prints a registration script. The Nix package installs it for all
three. Completion is dynamic and silent on any error:

- subcommands, flags and file paths;
- `scry flake REF#…`: configuration names of the flake typed before `#`;
- `--on`: configuration names of the selected source, using the `REF` typed on the line when there is
  one; colmena node names and `@tags`; a comma list completes its last item;
- `--base`: local branches and tags.

## 11. Testing

- **Command seam**: every external command (`ssh`, `nix`, `colmena`, `git`, local `sh`) goes through
  one interface. The real implementation spawns processes; a fake returns canned stdout, stderr and
  exit status per invocation. Nothing spawns a process outside it.
- **Pipeline tests** drive the whole pipeline from canned command output (path-info JSON, tree listings,
  content streams, colmena and flake evaluations) to a report and to rendered text and JSON, with
  snapshot tests (`insta`). They assert on output, never on which command ran.
- **Unit tests**:
  - name splitting: plain semver, `unstable-YYYY-MM-DD`, git revisions, output suffixes, names with
    digits (`libx11`, `python3`), hostnames ending in digits in toplevels, versionless names;
  - `compareVersions` parity cases from the Nix manual;
  - both path-info JSON shapes Nix has emitted (an array of objects with `path`, an object keyed by
    path, `null` for an invalid path);
  - hash and version normalisation;
  - token LCS highlight, including the join of close runs;
  - selection matching (whole-string globs, tags, no match);
  - tree listing parsing (`ls -ld` and `cksum` lines), including names with spaces and ` -> ` inside a
    name;
  - reference pairing;
  - every remote script: no backslash, no nested quotes (6.1).
  - `ssh` argument order: `NIX_SSHOPTS` and `sshOptions` before the tool's own defaults (6.1).
- **TUI** logic (selection, open state, filter, scrolling, wrap) is tested through its state without a
  terminal.
- **Fixtures** are synthetic and small, hand-written, with placeholder names such as `web-a` and
  `cache.example`. No captured production data.
- **Fixture run mode**: a build option or hidden flag runs the full UI against a synthetic fleet served
  by the fake seam, for demos and manual checks.

### 11.1 Acceptance checks on real machines

Before 1.0, run and record each in an ADR:

1. One nixpkgs bump on a NixOS target over SSH: the Files section lists the same changed files as
   `diff -ruN` of the two `etc` trees, within the round-trip budget, in under 5 s per configuration.
   The 5 s figure is a target, not a measurement.
2. A five-bump span (about 850 changed files): no argv failure, under 10 s per configuration. Also a
   target.
3. nix-darwin and home-manager on macOS: launchd plists and dotfiles show; only BSD tools on the side.
4. Revision mode on a colmena hive: `colmena eval` inside a temporary worktree works, and the worktree is
   gone afterwards, also after Ctrl-C.
5. The oldest Nix version that supports every `nix` call used, documented in the README. Not verified
   yet.
6. A target that hangs: `timeout after 60 s`, while the others finish.
7. `colmena build` over several nodes where one fails: whether the others still build. If they do
   not, build per node or document the limitation. Not verified yet.
8. A target whose login shell is fish, and a macOS target reached non-interactively: remote scripts
   run and find `nix`.
9. Revision mode on a flake in a subdirectory of its repository. Older Nix versions had trouble with
   git worktrees, where `.git` is a file; record the version tested.
10. Connection sharing: one SSH handshake per target per run, also with a user `ControlPath` in
    `~/.ssh/config`; the run leaves no master process and no socket directory behind; and
    `NIX_SSHOPTS="-o ControlMaster=no"` turns sharing off.

## 12. Heuristics

Every heuristic in one place. Change a value only with a measurement recorded in an ADR.

| Heuristic | Value | Basis |
|---|---|---|
| Resized threshold | ≥ 1 MiB and ≥ 10 % of old size | judgement, unmeasured |
| Small rebuild cap | ≤ 1 MiB `narSize`, each path, both sides | judgement, unmeasured |
| Reference depth | 2 | judgement, unmeasured |
| Git-revision-like version component | ≥ 7 hex characters, at least one letter | judgement |
| Binary detection | NUL byte in the first 8 KiB | common practice |
| `--max-file-size` | 1 MiB | judgement; a real `hwdb.bin` is 13.5 MB |
| `--jobs` | 16 | unmeasured |
| `--eval-jobs` | CPU count | one NixOS evaluation takes about one core and 1 GB, observed, not benchmarked |
| `--timeout` | 60 s | unmeasured |
| Scroll margin | 5 lines | judgement |

## 13. Packaging and conventions

- Rust, stable toolchain. A plain flake with `packages.default` for `x86_64-linux`, `aarch64-linux` and
  `aarch64-darwin`, built with `rustPlatform.buildRustPackage` (which runs the tests);
  `checks.clippy` with `-D warnings`; `devShells.default`; `formatter` with nixfmt and rustfmt through
  treefmt. Shell completions are installed by the package.
- CI (`.github/workflows/ci.yml`, through `zebradil/nix-ci`) builds every package, check and dev
  shell for all three systems and pushes them to a binary cache.
- Commit messages and PR titles are Conventional Commits; squash-merge only, and release-please
  (`.github/workflows/release-please.yml`, `rust` release type) cuts versions from them. Merging its
  release PR bumps `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md` and tags `vX.Y.Z`; the release
  commit's CI run pushes its closure to the cache, so a release needs no build of its own.
- No environment-specific names (hostnames, IPs, keys, internal URLs) in code, docs or fixtures.
  Licensed `MIT OR Apache-2.0`; keep the code licence-clean, and do not link GPL crates (`deny.toml`,
  `cargo deny check licenses`).
- Comments explain the non-obvious why, never history.

## 14. Out of scope

- Derivation-level diffs (`nix-diff` style).
- Markdown output for PR comments. The JSON report makes it a renderer later.
- A build plan (what a build will compile or fetch).
- Copying closures to targets, building on demand from the UI, activating or switching.
- Comparing two sides on two different remote machines from the command line. The core does not
  assume co-location, so this is a flag later.
- Home-manager used as a NixOS or nix-darwin module: its generation is not a toplevel.
- Mouse support, light-theme detection, truecolour.
