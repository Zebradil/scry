# Acceptance check 4 with builds and Ctrl-C during a build

Spec 11.1 check 4, the part ADRs 0070 and 0071 left open: revision mode on a colmena hive with
the build step, and Ctrl-C during a build, on an x86_64-linux machine. Run on `daddy`, the
x86_64-linux NixOS host of ADR 0072, as `suok`, driven from the Mac over one SSH connection with
the user's approval. Its outbound network worked (github.com and cache.nixos.org answered), unlike
the node ADR 0071 tried.

Versions: Nix 2.35.6, colmena 0.5.0 and git 2.56.0 from `daddy`'s nixpkgs, tmux 3.8. scry was
commit 9300ffc, `nix build github:Zebradil/scry/<rev>`, fetched from the Zebradil cache in 2 s.
The host has 16 cores and 31 GiB of memory; `max-jobs = 16`.

The hive is a fresh clone of `znix` checked out detached at 6fa5e91 (a flake.lock update that
moves nixpkgs from a078812 to d357a2b) with `--base HEAD~1`, so the base is 7a980ee. `daddy`
itself is not a node at that revision; revision mode contacts no machine (4.3), so any
x86_64-linux node serves. `d1` and `junior` were used.

## Build both revisions and compare

`scry colmena --on d1 --base HEAD~1`, from the checkout, stdout and stderr to files:

- Cold: exit 0 in 73 s. Two builds: 100 derivations built and 142 paths fetched (298.4 MiB) for
  the first side, 21 built and 3 fetched for the second; the base toplevel
  `nixos-system-d1-26.11.20261031.a078812` among them.
- The report: `d1 · nixos (7a980ee -> working tree)`, system `26.11.20261031.a078812 ->
  26.11.20261101.d357a2b`, 19 packages rebuilt with the same version (+134.0 KiB), seven changed
  files: `boot.json`, `etc/issue`, `etc/nix/registry.json`, `etc/os-release`, the two
  `generate-shutdown-ramfs.service` entries that reach `etc-os-release`, and
  `nixos-version/bin/nixos-version`. What a nixpkgs bump with no package version change should
  touch.
- Warm, both toplevels in the store: exit 0 in 11 s, the same report byte for byte.
- After both runs: no `/tmp/scry-*`, no entry in `git worktree list` or `.git/worktrees`, no GC
  root under `/tmp/scry-` (`nix-store --gc --print-roots`), the checkout clean.

## Ctrl-C during a build

scry ran in a tmux pane; a script waited for the build step (a `nix build … --out-link` child,
or a `building '` line on stderr), waited 5 s more, and sent `C-c` with `tmux send-keys`, so the
pane's foreground process group got `SIGINT` as from a real terminal. `--on junior`, whose
toplevels were not built yet:

- Build step seen at 11 s, Ctrl-C at 16 s. At that moment scry's tree held
  `nix --extra-experimental-features nix-command build --keep-going --out-link
  /tmp/scry-W47Lx4/new <junior toplevel>.drv^out` in scry's process group, and a `nixbld1`
  builder (`default-builder.sh`) under the daemon.
- scry printed `scry: interrupted` and exited 1 within the same second, stdout empty.
- Afterwards: `/tmp/scry-W47Lx4` gone along with the base worktree inside it, no entry in
  `git worktree list` or `.git/worktrees`, no `scry`, `nix build`, `nix eval` or `colmena`
  process, and no process of a `nixbld` user.

Spec 11.1 check 4 now holds on Linux and on the Mac: evaluation in a temporary worktree (ADR
0070), both builds and the comparison, and cleanup after Ctrl-C during evaluation (ADR 0070) and
during a build (here).

## Not covered

- Ctrl-C while `git worktree add` runs: the window is short and was not hit. A worktree locked
  there may survive `remove --force` (handover, plausible, unverified).
- `daddy` keeps `~/scry-check4` (the clone, scry and tool out-links) until the user removes it.
