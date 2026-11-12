# Hand-over

State of the implementation and how to continue it in a new session. Read with
[`roadmap.md`](roadmap.md) (milestones and status), [`architecture.md`](architecture.md) (module
ownership, seam, concurrency, errors, testing) and [`../CONTEXT.md`](../CONTEXT.md) (terms). The
spec is [`spec.md`](spec.md).

## Where things stand

- Done and merged into `main`: M0, M1, M2, M3, M4. Every milestone passed a review subagent, and
  the review findings were fixed before the merge.
- In flight when this was last updated (2026-11-13): **M5** (fetch pipeline) implemented on branch
  `wt/261113-000329-m5-fetch`, under review; **M7** (TUI) being implemented on branch
  `wt/261113-002750-m7-tui`. Both worktrees are under `.worktrees/`. A new session cannot resume
  those subagents: check each branch with the gate (step 3 below), run a fresh review subagent if
  the branch has no review fixes yet (`git log main..<branch>`), fix, then merge.
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

- The run hands over the report header, the selected names in name order and the receiver; the
  TUI runs a blocking loop on its own thread and drains with `try_recv` (architecture).
- Reuse M3's line model for the detail pane and M3's text output for `Y`.

### M8 and M9

- M8: fixture run mode on the fake seam, dynamic completions, README; the flake has a marked spot
  for installing completions.
- M9 needs a NixOS target over SSH and a colmena hive from the user. Unverified so far, and only
  checkable there: ADR 0010's single-handshake and master clean-up claims (acceptance check 10),
  GNU `ls`/`find` output on a real Linux target, fish as login shell. This Mac (nix-darwin plus
  home-manager) covers acceptance check 3 locally.

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
