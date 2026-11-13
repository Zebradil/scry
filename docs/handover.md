# Hand-over

State of the implementation and how to continue it in a new session. Read with
[`roadmap.md`](roadmap.md) (milestones and status), [`architecture.md`](architecture.md) (module
ownership, seam, concurrency, errors, testing) and [`../CONTEXT.md`](../CONTEXT.md) (terms). The
spec is [`spec.md`](spec.md).

## Where things stand

- Done and merged into `main`: M0, M1, M2, M3, M4, M5, M7. Every milestone passed a review
  subagent, and the review findings were fixed before the merge. Commits are signed through
  1Password and need the user's in-person approval; when the user is away a commit fails with
  "1Password: failed to fill whole buffer". Never bypass signing: keep working uncommitted and
  commit once the user is back.
- **M6** (sources and run), worktree `.worktrees/wt-261113-052109-m6-run`, branch
  `wt/261113-052109-m6-run`: implementation committed (`9d671ce`), `main` merged in (`9ecd5f3`).
  **Done**: three review rounds, all findings fixed, TUI wired into `run` (viewer on
  `spawn_blocking` after evaluation and build; a quit flag for SIGINT/SIGTERM/SIGHUP; a `viewer`
  field on `Env` for tests), gate passed (187 + 3 + 14 + 18 tests). The fixes are **uncommitted**
  in that worktree because signing failed (user away). Backup:
  `<session scratchpad>/m6-final.patch` (binary diff against `9ecd5f3`). When signing works:
  commit them there as `fix(run): M6 review findings and TUI wiring`, then squash-merge the branch
  as `feat: sources and run (M6)`, remove the worktree and branch.
- **M8** (fixture mode, completions, README), worktree
  `.worktrees/wt-261113-105208-m8-fixture-completions`, branched from the M6 branch head with
  M6's uncommitted fixes **staged** as its base; M8's own changes are **unstaged** and untracked
  (backups: `<session scratchpad>/m8-unstaged.patch`, `m8-untracked.tar`, `m8-untracked.txt`).
  **Implemented, not reviewed yet.** Implementer's report:
  - Fixture mode: a non-empty `SCRY_FIXTURE` swaps the real seam for the fake one
    (`SCRY_FIXTURE=1 scry colmena|flake [--base main] [--format text|json]`), with a stderr notice;
    five configurations (unchanged; two sharing churn with binary, Boot and specialisation
    changes; one absent at base in revision mode; one failing evaluation). ADR 0060.
  - Completion: `clap_complete` dynamic engine; `--on`, `--base`, `REF#` completers attached at
    runtime, reusing `colmena::nodes`, `flake::list` and `git` through the seam; silent on error;
    tested on the fake. Bash checked by hand, zsh not.
  - Oldest Nix: 2.13 where scry builds (`drv^out`), 2.4 otherwise and on targets; tested 2.4–2.13
    and 2.31 clients against a 2.35 daemon, darwin only. ADR 0061.
  - Gate: test, clippy, fmt, `nix fmt`, `nix build path:$PWD`, completions in `result/share`,
    `nix flake check path:$PWD`, fixture text and JSON, TUI under tmux: all pass.
  - Open: the TUI exited at once under `script -q /dev/null` (M6's pty checks with `script`
    worked); README's install URL is a placeholder until the GitHub follow-up.
  Next: review M8 (opus, high), with these points to check: the `script` exit, `SCRY_FIXTURE` as
  an env var in the release binary (spec 11 says "build option or hidden flag"), completion
  silence and latency, zsh and fish registration, the README against the code. After M6 is
  committed: in the M8 worktree, `git reset --soft <M6 fix commit>` (the staged content equals
  it), then M8's changes are the only diff; fix, commit, merge.
- After M8: M9 (needs a NixOS SSH target and a colmena hive from the user).
- ADRs: 0001–0006 (M0), 0010–0012 (M4), 0020 (M3), 0030–0031 (M5), 0040 (M7), 0050–0051 (M6),
  0060–0061 (M8). The
  next ADR takes the next free number after the highest.
- No git remote. GitHub, CI and the binary cache are a follow-up (roadmap).

## How each milestone runs

The orchestrating session keeps its own context small: it never reads implementation code in full,
only reports, diff stats and test results.

1. Create a worktree from `main`, as the user's global instructions describe:
   `branch="wt/$(date +%y%m%d-%H%M%S)-<slug>"; git worktree add -b "$branch" ".worktrees/${branch//\//-}" main`.
2. Spawn one implementer subagent per milestone: `model: opus`; `effort: xhigh` for subtle
   milestones (M5, M6, M7), `high` otherwise (M3, M8). The user asked for quality over cost and
   allowed tuning the effort. The prompt carries:
   - the worktree path and branch, "do not touch main or other worktrees";
   - Conventional Commits, with the two trailer lines the session's system reminder gives;
   - what to read: `CONTEXT.md`, `docs/architecture.md`, `docs/adr/*`, `src/report.rs`, and the
     spec sections by number (`rg -n '^#' docs/spec.md` finds them), never the whole spec unless
     needed;
   - toolchain: `nix develop -c cargo …` (the `cargo` on PATH outside the dev shell is a nightly);
     `rg`/`fd`, GNU `sed`, `nix shell nixpkgs#<pkg>`, never brew;
   - module ownership: edit only the milestone's modules; the smallest change to an M0 interface,
     reported, when truly needed;
   - the scope as a numbered list, with the hand-over notes below for that milestone;
   - done-when: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `nix fmt` without a diff,
     all committed;
   - the final message: a word limit (200–250), no code; public API, deviations, spec ambiguities
     with section numbers, done-when results.
3. Gate: in the worktree, run `nix develop -c cargo test` and the clippy command; look at
   `git diff --stat main...HEAD`.
4. Spawn a review subagent (`model: opus`, `effort: high`, `xhigh` for subtle milestones). It
   reads the same docs and spec sections, checks a numbered list of the spec's rules for the area,
   may write throwaway probes under the session scratchpad but never edits the repository, never
   connects to another host, and reports at most 400–500 words: findings ranked by severity, each
   with `file:line`, a concrete failing input → wrong output, a one-line fix, and **confirmed** or
   **plausible**. Reviews so far found real defects every time; do not skip them.
5. Send the findings back to the implementer with `SendMessage` (it keeps its context, which is
   cheaper than a new agent). Accept a deviation when it is argued from the spec or an ADR.
6. Squash-merge into `main` (`git merge --squash <branch>` and one `feat: … (Mn)` commit), resolve
   `Cargo.toml`/`Cargo.lock` conflicts by hand, rerun the tests on `main`, remove the worktree and
   the branch, and mark the milestone done in `roadmap.md`. Merging needs no approval from the user.
7. Ask the user only for real blockers.

## Notes for the next milestones

Collected from the reports and reviews so far; pass the relevant ones to each implementer.

### M3: rendering (spec 8.1–8.3, 8.5)

- `old_versions`/`new_versions` hold the full version sets in byte order; the renderer shows the
  set difference (7.2). When one side's difference is empty (`{1.0} -> {1.0, 2.0}`), 7.2 is silent:
  show the full sets on both sides.
- Lines carry `hash_only` (ADR 0006): such a pair is dimmed without bold. M2 also marks a missing
  newline at end of file on `Line`; print `\ No newline at end of file` after that line.
- Shared changes (8.2) compare hunks after `files::churn::normalize_hashes`.
- Fields beyond the spec are in ADR 0006, among them an optional `target` used in the header when
  `old` is `null`, and the JSON of shared Boot items.
- The TUI's detail pane shows an entry "as in 8.3" and `Y` copies the text output, so build the
  text renderer on a small line or styled-span model that M7 can reuse instead of duplicating
  alignment and labels.

### M5: fetch pipeline (spec 6.2, 6.3, 7.4)

- Inputs from M1: `store::pathinfo::parse` (three JSON shapes; Nix 2.35 prints a deprecation
  warning on stderr for `--json`, which must not count as a failure), and
  `packages::compare(old, new, toplevels) -> Comparison { packages, small_rebuilds }`.
- Inputs from M2: `tree::Walk::{toplevel, reference}` with `next_roots()`, `add_listing(bytes)`
  (once per chunk) and `tree()`; the listing script is a `Script` constant in `tree`, owned by M2;
  run it on absolute roots, skip roots that do not exist, never call `find` without roots, chunk
  below the argument limit. Then `files::compare::{to_fetch, compare, sort}` (call `sort` again
  after adding reference results), `files::refs`, and `boot::{Probe, compare}`.
- Inputs from M4: `exec::Exec`, `transport::script::Script` (refuses backslashes and single quotes
  at compile time), `exec::fake::Fake` (rules, computed answers from stdin, panics on an unmatched
  command, checks the quoting of every remote command). Pipeline tests assert on output, never on
  which command ran.
- Where both sides are on one machine, their commands share round trips (6.1). Respect the round
  trip budget of 6.1.

### M6 review findings (fix before merging M6)

Send these to an implementer (effort xhigh) working in the M6 worktree; it commits
each step if signing works, else leaves the changes uncommitted:

1. High, confirmed: `src/source/flake.rs:70-73`: `nix eval` writes `flake.lock` into the user's
   repository (and `git add -N` it) when the lock is missing or outdated, in deployed and revision
   mode, even with `--no-build` (4.4: nothing written outside the run's temp dir). Fix: pass
   `--no-write-lock-file` on every `nix` invocation that takes a flake reference (eval and build);
   check whether `colmena eval`/`build` on a flake hive can write the lock too, and record it.
2. Medium, confirmed: `src/run.rs:466-471` (flake) and `src/run.rs:309-314` (colmena): when the
   base revision has no flake or hive at that path (added after `REV`), every configuration fails
   with an error that leaks the temp path; 4.5 says configurations missing at base are
   `absent-at-base`, not failures. Fix: a base path without `flake.nix`/hive is an empty listing.
3. Low, plausible: `src/revision.rs:133-140`: Ctrl-C during `git worktree add` leaves the entry
   locked ("initializing"); `remove --force` and `prune` refuse a locked worktree. Fix:
   `git worktree remove --force --force`.
4. Tests (`tests/run.rs`): `--show-prefix` is always empty (line 229), so subdirectory flakes are
   untested (add a `sub/` case); the `--target` "not selected" warning on stderr is unchecked;
   fake rules assert on argv (lines 151, 184) against the "assert on output" rule: let rules
   return `None` instead.

Review evidence: synthetic git flake in a subdirectory with names `a.b`, `q"x`, `d$x`,
`user@host` (quoting correct), local `paths` generations, SIGINT during build and `nix eval`
(no child left, worktree and temp dir removed), usage errors, `--target` warning, broken pipe,
`--exit-code`. Not checked: real colmena, `SCRY_DEBUG`, `--jobs` bounds under load.

### M6: sources and run (spec 4.1–4.8, 6.3, 9)

- Children run in their own process groups (ADR 0012), so the terminal's Ctrl-C does not reach
  them: handle it with `tokio::signal::ctrl_c`, drop the run, then `Exec::shutdown`. Build children
  (`Stderr::Inherit`) stay in scry's foreground group.
- `exec::process::Process::from_env()` returns a `Result`: a bad `NIX_SSHOPTS` is fatal.
- The fake has no "hang" answer yet; M6 may add one to `exec::fake` for timeout tests.
- A fatal error under `--exit-code` exits 2 (M0 decision). Live events are finished
  `report::Configuration`s on an unbounded channel (architecture).
- `colmena` is not installed on this machine; `nix shell nixpkgs#colmena` provides it.

### M7: TUI (spec 8.4)

Implemented (see above). What M6 calls, from the author's report:

- `tui::run(header, names, events, options) -> Finished`: blocking, on the calling thread (M6
  picks the thread; `docs/architecture.md` says "its own thread": align the doc when merging);
  restores the terminal on every exit path, including panics.
- `tui::Options { expand_churn, color }` (`color` from `render::color_enabled`).
- `tui::Finished { all_finished, configurations }` (name order); pass `!all_finished` to
  `exit::Outcome::of_report` so quitting early counts unfinished configurations as failed.
- ADR 0040 records its readings: no reverse video (8.5 forbids backgrounds), so a `>` cursor
  column and a thick border on the focused pane; the KiB in the copy notice is the OSC 52 payload
  size; `nothing to copy yet` only for a configuration still fetching; `g`/`G` act on the detail
  pane; `l` from the list only focuses the detail pane.
- Unverified by its author: terminal restore after a real panic, OSC 52 outside tmux, colours by
  eye; the panic hook stays installed after `run` returns (harmless).

### M8 and M9

- M8: fixture run mode on the fake seam, dynamic completions, README; the flake has a marked spot
  for installing completions.
- M9 needs a NixOS target over SSH and a colmena hive from the user. Unverified so far, and only
  checkable there: ADR 0010's single-handshake and master clean-up claims (acceptance check 10),
  GNU `ls`/`find` output on a real Linux target, fish as login shell. This Mac (nix-darwin plus
  home-manager) covers acceptance check 3 locally.

## Usage limits

The user wants a hand-over written before the usage limit cuts the session off, not after: a
resumed session starts with cold caches. Claude Code's status line saves the limits to
`~/.config/personal-claude/state/rate-limits.json`
(`{t, rate_limits: {five_hour: {used_percentage, resets_at}, seven_day: …}}`, `resets_at` in Unix
seconds; it is `null` until the first status line refresh with data). Read it with
`jq -c '{age_s: (now - .t | floor), rate_limits}' ~/.config/personal-claude/state/rate-limits.json`
before starting any subagent and after every merge. From 80 % of the five-hour window: start no new
subagent, let running ones finish, write their results into this file instead of starting fix
rounds, then schedule a one-shot wake-up (`CronCreate`) a few minutes after `resets_at` and continue in the same session when it fires. This file stays the fallback if the session dies.

## Cautions

- Do not run `nix flake update`: the current nixpkgs-unstable has no cached rustfmt for
  aarch64-darwin and builds LLVM from source (roadmap log).
- Never commit real store contents or hostnames; fixtures are synthetic (`web-a`,
  `cache.example`).
- Never read a subagent's output file; wait for its notification.

## Starting the new session

Paste into a new Claude Code session in the main checkout:

> Continue implementing scry. Read `docs/handover.md` and follow it: orchestrate subagents per
> milestone, starting with the in-flight branches listed there. Call me in only for real blockers.
