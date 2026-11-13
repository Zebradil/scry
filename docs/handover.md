# Hand-over

State of the implementation and how to continue it in a new session. Read with
[`roadmap.md`](roadmap.md) (milestones and status), [`architecture.md`](architecture.md) (module
ownership, seam, concurrency, errors, testing) and [`../CONTEXT.md`](../CONTEXT.md) (terms). The
spec is [`spec.md`](spec.md).

## Where things stand

- Done and merged into `main`: M0, M1, M2, M3, M4. Every milestone passed a review subagent, and
  the review findings were fixed before the merge.
- In flight when this was last updated (2026-11-13, 01:00 CET), both under `.worktrees/`; a new
  session cannot resume the old subagents, so spawn fresh ones:
  - **M5** (fetch pipeline), branch `wt/261113-000329-m5-fetch`: implemented, gate passed (121
    tests), **reviewed**; findings below, to be fixed on the branch by an implementer (effort
    xhigh), then merged. Review evidence: 26 real toplevel pairs on this Mac (14 NixOS with 413–1088
    changed paths, 6 nix-darwin, 5 home-manager, 1 unknown) through the real seam, BSD and GNU
    tools: 0 missing, extra or wrong-status files against an independent walk and `diff -rq`;
    round trips within budget (NixOS 8 of 9); a NixOS bump took 4.4–5.6 s locally, about 3 s of it
    in reference round trips; cancellation at 7 points left no child and no temp file.
  - **M7** (TUI), branch `wt/261113-002750-m7-tui`: implemented, gate passed (159 tests, clippy
    clean), checked by its author in tmux; **not reviewed yet**. Review against spec 8.4/8.5, fix,
    merge.
- Next after M5: **M6** (sources and run). After M6 and M7: M8, then M9.
- Open item for the M5 fix round or M6: the `unknown closure kind: no followed trees, no Boot
  section` note (spec 5) has no report field and M3 does not render it; render it in
  `render::view` for a `done` configuration of kind `unknown`.
- ADRs: 0001–0006 (M0), 0010–0012 (M4), 0020 (M3), 0030 (M5); M7 numbers from 0040. The next ADR
  takes the next free number after the highest.
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

### M5 review findings (fix before merging M5)

1. Confirmed: `src/fetch/references.rs:87` starts a small-rebuild job with an empty `chain`, so a
   script naming its own package is followed again at depth 2 (32–67 junk `pkg/bin/a → pkg/bin/b`
   rows per real comparison). Fix: `chain: vec![(s.old.clone(), s.new.clone())]`; add a test.
2. Confirmed by measurement: `src/fetch.rs:150` archives whole ≤ 1 MiB directories in the
   reference round trip though only `bin/` is compared (47 MB raw, 20.6 MB gzipped on a NixOS
   bump; `bin/` only cuts 35 %), and starts one `find` per root (about 1000). Fix: use `$r/bin`
   for directory roots and batch roots like `tree::LISTING`. Acceptance check 1's 5 s target
   depends on it.
3. Confirmed: the fixture cross-check test (`src/fixture.rs:574`) fails with macOS's own tools:
   `bsdtar` adds `._name` entries for files with `com.apple.provenance` (383 of 1094 entries in a
   real stream); it passes only because the dev shell puts GNU tools first. Fix: set and export
   `COPYFILE_DISABLE=1` in the content and reference scripts; run that test with `/usr/bin` first
   on macOS.
4. Confirmed: `src/fetch.rs:123` does not check that a resolved toplevel is under `/nix/store/`:
   `scry paths /tmp X` walks `/tmp`, and `nix path-info` may treat a directory as a flake
   reference (evaluating, possibly writing to a target's store). Fix: in the probe,
   `case $t in /nix/store/?*) ;; *) <not in the store> ;; esac`, with spec 9's wording.
5. Confirmed: GNU tar 1.35 rewrites backslash escapes in `-T` names (`\\`, `\t`, `\101`): the
   configuration fails ("Cannot stat") or fetches a different file. systemd's `\x2d` is safe.
   Fix: report such paths as content not fetched (simplest and correct).
6. Plausible: GNU `ls` reads `QUOTING_STYLE` and `BLOCK_SIZE` from the environment;
   `QUOTING_STYLE=shell-escape` quotes names with spaces and fails the run with a misleading error.
   Fix: unset both in the listing script (`tree::LISTING`, next to `LC_ALL=C`).
7. Plausible: `src/fetch.rs:168` keeps probe results inside the future, so when M6 drops it on
   `--timeout` the report shows kind `unknown` and empty sides, though 8.1 expects what is known.
   Fix: let the caller pass a shared state cell, or apply the timeout inside `configuration`;
   coordinate with M6.
8. Test gaps: a small rebuild that references itself; the BSD userland path (see 3). Move the
   NixOS, nix-darwin and home-manager closure builders from `tests/fetch.rs` into `fixture`, since
   M8's fixture run mode needs them.

Decision (orchestrator, record as an ADR in the fix round): pair the j-th references of two lines
(7.4) only when their pnames are equal (`store::name::parse_drv_name` of the store names). Real
data paired different packages (an `xdg-utils` reference with `dbus` and `glib`), producing whole
`bin/` `[R]`/`[A]` noise. 7.4 states the pairing without a *must*, and this evidence justifies the
narrowing.

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
