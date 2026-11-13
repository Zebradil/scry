# Roadmap

Implementation plan for [`spec.md`](spec.md). This file is the hand-over point: whoever picks the work
up reads this file, [`handover.md`](handover.md), `CONTEXT.md` and `docs/architecture.md`, then continues with the first milestone
that is not done.

## How the work runs

- An orchestrating agent runs one subagent per milestone, each in its own worktree under
  `.worktrees/` on a branch `wt/<yymmdd>-<HHMMSS>-<slug>`. Milestones without a dependency between
  them run in parallel.
- A milestone is done when `nix develop -c cargo test` and
  `nix develop -c cargo clippy --all-targets -- -D warnings` pass, a review subagent found no open
  defect against the spec, and the branch is merged into `main`. Merging into `main` needs no approval.
- Decisions taken while implementing go to `docs/adr/` as numbered ADRs (`NNNN-slug.md`).
- The user is asked only for real blockers: an ambiguity in the spec that changes behaviour, or a
  missing resource (a target machine, credentials).

## Milestones

| # | Scope | Spec | Depends on | Status |
|---|---|---|---|---|
| M0 | Scaffold: cargo crate, flake (package, clippy check, dev shell, treefmt), clap CLI with every flag, exit codes, `--version`, module skeleton, report types, `docs/architecture.md` | 4, 8.1, 11, 13 | — | done |
| M1 | Packages core: store names, `parseDrvName`, `compareVersions` with the git-revision rule, path-info JSON, sizes, package comparison | 7.1, 7.2 | M0 | done |
| M2 | Files core: listing parsing, level walk state, file statuses, binary detection, churn, unified diff, word highlight, reference pairing; Boot comparison | 5, 6.2, 7.3–7.5 | M0 | done |
| M3 | Report rendering: JSON, text, shared changes, colour; snapshot tests | 8.1–8.3, 8.5 | M1, M2 | done |
| M4 | Command seam (real and fake), ssh invocation, connection sharing, remote script wrapping, per-connection limit, kill on timeout | 6.1, 9, 11 | M0 | done |
| M5 | Fetch pipeline: probe, level listing, content fetch, references and small rebuilds; pipeline tests | 6.2, 6.3, 7.4 | M1, M2, M4 | done |
| M6 | Sources and run: selection, colmena, flake, paths, build step, revision mode, parallelism, cleanup, exit codes | 4.1–4.8, 6.3, 9 | M3, M5 | done |
| M7 | TUI: state, rendering, keys, filter, tree view, wrap, live status, OSC 52 | 8.4 | M3 | done |
| M8 | Fixture run mode, dynamic shell completions, README | 10, 11, 13 | M6, M7 | done |
| M9 | Acceptance checks on real machines, recorded as ADRs | 11.1 | all | done |

## Follow-ups (not in this plan)

- GitHub repository, CI building every package and dev shell, binary cache, release-please.
- Acceptance check 4 with builds and Ctrl-C during a build, on a Linux machine with working
  network; check 8 with fish as the login shell (ADR 0071).

## Log

- 2026-11-12: plan set. Project is personal; spec adjusted to drop employer assumptions.
- 2026-11-12: M0 merged (ADRs 0001–0006, `docs/architecture.md`). `flake.lock` pins an August
  nixpkgs-unstable: the current one has no cached rustfmt for aarch64-darwin and builds LLVM from
  source. Do not run `nix flake update` until the cache catches up.
- 2026-11-12: `store::name::{store_name, parse_drv_name}` written on main ahead of wave 2, so M1
  and M2 share one split. Wave 2 (M1, M2, M4) started in parallel. Wave-2 agents number new ADRs
  from 0010 (M4) to avoid collisions; renumber on merge if needed.
- 2026-11-12: M1, M4, M2 merged after review rounds. M4's review corrected the spec: Nix splits
  `NIX_SSHOPTS` shell-style. M2's review moved links to `ls -ldn` and added newline detection to
  the listing script. `docs/handover.md` written: process, prompt shapes and notes per milestone.
- 2026-11-13: wave 3 (M3, M5) started.
- 2026-11-13: M5 and M7 merged after review rounds (ADRs 0030, 0031, 0040). M6 implemented and
  reviewed; review fixes and TUI wiring pending (ADRs 0050, 0051). Commits waited for the user's
  1Password approval from 05:20 to 08:30.
- 2026-11-13: M6 merged after three review rounds; the TUI is wired into the run. M8 implemented
  (ADRs 0060, 0061), in review.
- 2026-11-13: M8 merged after three review rounds (bash completion needed its own registration).
  M9 local checks pass (ADR 0070); SSH checks running against the user's hive.
- 2026-11-13: M9 merged: acceptance checks recorded in ADRs 0070 and 0071. Two fixes came out of
  them: home-manager follows three more entries, and ssh errors show ssh's last stderr line.
