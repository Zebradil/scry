# Acceptance checks over SSH against a colmena hive

Spec 11.1 checks 1, 2, 4, 7, 8 and 10, run from the Mac of ADR 0070 against a flake colmena hive
of x86_64-linux NixOS nodes on a LAN, with the user's approval and present for every SSH key use.
The run plan of ADR 0070 needed two changes:

- Each node keeps only its current system generation, so `scry paths` between generations on a
  node was impossible. Checks 1 and 2 used deployed mode instead, on a scratch copy of the hive
  with `deployment.buildOnTarget = true` on two agent nodes, checked out one and five nixpkgs
  bumps behind what the nodes run. The new side was built on the node and compared with its
  current system. The direction is reversed (the new side is older), which does not matter for
  what is compared.
- Revision mode contacts no machine and builds both sides where scry runs (4.3), so check 4 with
  builds needs scry on a Linux machine; it was attempted on one of the hive's nodes.

Nothing was deployed, switched or activated. The nodes received only the store paths of the
builds; the Linux node also held a scratch directory, removed afterwards.

Versions: Nix 2.34.9 on the nodes, colmena 0.5.0, OpenSSH 10.3p1 on the Mac. scry was the release
build of this branch.

## Check 1: one nixpkgs bump

An agent node, nixpkgs 2026-10-31 against 2026-11-01. With the build on the node: 33 s. Without
the build (`--no-build`, the toplevel already on the node), hive evaluation included: 5 s, within
the target. The changed files under `etc` are exactly the 10 that `diff -rq` of the two `etc`
trees reports on the node. No package version changed in that bump; 16 changed paths in all.

## Check 2: five nixpkgs bumps

The same node, nixpkgs 2026-10-16 against 2026-11-01. With the build on the node: 74 s; without:
7 s, within the 10 s target. 75 package changes and 672 changed paths, 628 of them under `etc`:
the same set `diff -rq` reports, except that `diff` names a directory present on one side only
(`etc/systemd/system/<unit>.d`) where scry lists the file inside it. No argv failure.

## Check 10: connection sharing

Counted from `Accepted publickey` lines in the node's sshd journal during each run:

- Default: one handshake per run. No local master process and no socket directory afterwards.
- A user `ControlMaster auto`/`ControlPath`/`ControlPersist 60` for the node, from an ssh config
  passed with `NIX_SSHOPTS="-F <file>"` whose first block sets them and which then includes the
  user's real config (the real `~/.ssh/config` was not edited): scry found the user's
  `ControlPath` with `ssh -G`, added no multiplexing options of its own, sent no `-O exit`, and
  reused the master opened before the run: no handshake during the run, and the master still
  running afterwards.
- `NIX_SSHOPTS="-o ControlMaster=no"`: 7 handshakes, one per command.

## Check 7: one of several target builds fails

The scratch hive with `buildOnTarget` on two agent nodes and a derivation that always fails on
one of them; `scry colmena --on <both>`. colmena 0.5.0 built the other node, and scry compared
it normally; the failing node reads `<toplevel> is not in the store on <target>; the build did
not produce it`. Exit 1. No change to the build step is needed.

## Check 8: fish login shell and a macOS target

- No account on the nodes has fish as its login shell, and none was created: not verified here;
  see ADR 0072.
- A macOS target reached non-interactively: this Mac over `localhost`, `scry paths --target
  <user>@localhost` between two nix-darwin generations. The report equals that of a local run
  apart from `location`; 1 s. The first attempt hit a stale `localhost` host key, and the error
  read `ssh <target>: @@@@…`, the first line of ssh's warning box. scry now reports ssh's last
  stderr line that is not a warning (spec 9 amended), here `Host key verification failed.`. The
  run used `StrictHostKeyChecking=accept-new` with a scratch known-hosts file.

## Check 4: revision mode with builds, and Ctrl-C

scry built on the Linux node with `nix build` in 112–156 s. `scry colmena --on <node> --base
<rev one bump back>` then failed in hive evaluation: the node's TLS handshakes failed, to its own
LAN substituter and to github.com (a flake input tarball), while cache.nixos.org worked. That is
the node's network, not scry. After each failed run, and after a SIGINT run that failed the same
way, the base worktree was gone, no temporary directory or process was left. The build part and
Ctrl-C during a build stayed unverified on Linux here; ADR 0070 covers the evaluation in a
worktree and Ctrl-C cleanup on the Mac, and ADR 0073 covers both on a Linux host.

## Still unverified

- Check 4 with builds and Ctrl-C during a build, on a Linux machine with working network: see
  ADR 0073.
- Check 8 with fish as the login shell: see ADR 0072.
- The 5 s and 10 s targets with the handshake inside the measurement: the timed runs above reused
  no master, but approvals were quick; a slow approval adds to the first run only.
