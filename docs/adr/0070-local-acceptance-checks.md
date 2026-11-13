# Acceptance checks 3, 4, 6 and 9 on one Mac, without SSH

Spec 11.1 asks for each acceptance check to be run on real machines and recorded. This records
the ones that need no SSH, run on one machine: a nix-darwin Mac with home-manager (Apple M5 Pro,
15 cores, macOS 27.0.1). No command reached any host, this one included. Builds were local and
aarch64-darwin only; nothing was deployed or activated.

Versions: Determinate Nix 3.24.1 (Nix 2.35.3), colmena 0.5.0 (nixpkgs), git 2.56.0,
OpenSSH 10.3p1, `/bin/sh` bash 3.2.57, `/usr/bin/tar` bsdtar 3.5.3. scry was the release build
of the commit that adds this ADR.

## Check 3: nix-darwin and home-manager on macOS

**What ran.** `scry paths OLD NEW`, text and JSON, between generations already on the machine:
nix-darwin system profiles and home-manager profiles. Each pair's direct file changes in the
JSON (`via` empty, small rebuilds left out) were compared with a baseline: `find -L <tree> -type
f -exec cksum` over the followed trees of spec 5 on both sides, plus the toplevel's regular
files, minus the marker files. Apple's `diff -rq` (FreeBSD-based) was the first baseline, but it
stops with `Directory loop detected` inside home-manager's `home-files` and was dropped. Packages
were compared with `nix store diff-closures` by name.

| Pair | Kind | Time (JSON) | File changes (real / hash-only / version-only) | Baseline files = scry | Package changes |
|---|---|---|---|---|---|
| one nixpkgs bump, launchd plist changed | nix-darwin | 0.63 s | 9 (0 / 6 / 3) | 3 = 3 | 9 |
| one nixpkgs bump, `etc` content unchanged | nix-darwin | 0.61 s | 9 (1 / 6 / 2) | 2 = 2 | 1 |
| config change, same nixpkgs | nix-darwin | 0.48 s | 3 (0 / 2 / 1) | 3 = 3 | 1 |
| config change, same nixpkgs | nix-darwin | 0.52 s | 4 (1 / 3 / 0) | 3 = 3 | 0 |
| seven nixpkgs bumps | nix-darwin | 0.83 s | 75 (6 / 19 / 50) | 10 = 10 | 47 |
| first to last generation | nix-darwin | 0.81 s | 67 (25 / 2 / 40) | 18 = 18 | 576 |
| dotfile change | home-manager | 0.79 s | 4 (1 / 1 / 2) | 2 = 2 | 2 |
| 121 package upgrades | home-manager | 1.47 s | 153 (4 / 35 / 114) | 26 = 26 | 139 |
| 25 generations | home-manager | 1.51 s | 695 (562 / 32 / 101) | 572 = 572 | 248 |

File changes include followed references and small rebuilds; the baseline column counts only
paths in the trees. Closures: 244 paths (nix-darwin), 898 (home-manager).

**Results.**

- launchd plists show: under `Library/LaunchDaemons` and `Library/LaunchAgents` (nix-darwin) and
  `LaunchAgents` (home-manager), with followed references from a plist into the config it runs.
  Dotfiles under `home-files` show.
- The changed-file set equals the baseline on every pair, both ways. One pair lists a file inside
  an added directory where the baseline names the directory; same change.
- Packages match `diff-closures` in substance. Differences are naming only: `diff-closures`
  splits some names differently (`am-alerts` as `am`), lists the toplevel (`darwin-system`), and
  reports `brew 7.0.4 -> 7.0.11` where scry has `changed`, because the closure holds a versionless
  `brew` path beside the versioned one: two versions on a side are `changed` by spec 7.2.
- Boot: absent for nix-darwin and home-manager (`boot: null`, no section in text).
- Round trips per run, both sides local: 1 probe, 2 listing, 1 content, 1 or 2 reference.
- **Only BSD tools.** With `SCRY_DEBUG=1`, the only local programs are `sh` scripts running
  `find`, `ls`, `cksum`, `cat`, `tar`, `gzip` and `nix`. Five pairs rerun with
  `PATH=/usr/bin:/bin:/nix/var/nix/profiles/default/bin` (Apple `/bin/sh`, `find`, `ls`,
  `cksum`, `tar`, `gzip`) gave byte-identical JSON to the runs with the GNU tools of the user's
  profile first on `PATH`, in about the same time (0.67 to 1.62 s).
- Timings are 0.5 to 1.6 s per configuration against the 5 s and 10 s targets of checks 1 and
  2, but all local: no SSH round trip is in them.

**Deployed flake mode.** No flake for this Mac's own nix-darwin configuration was found outside
repositories that must not be used here, so `scry flake` in deployed mode ran on a synthetic
flake instead (the one of check 9): a `darwinConfigurations` entry named after the short
hostname and a `homeConfigurations` entry named `$USER`, each a tiny aarch64-darwin derivation
with the marker file, an `etc` or `home-files` file and a plist. Default selection picked exactly
those two; both were local, with the real `/run/current-system` and home-manager profile as old
sides. `--no-build` failed the unbuilt one with
`<path> is not in the store on this machine; run without --no-build` and compared the other;
with the build step, both compared (2.3 s including the build).

**Gap found and closed: three more home-manager entries are followed.** Current home-manager
generations hold three top-level links that spec 5 did not follow: `LaunchAgentDomains` (a
directory of per-agent launchd domain files), `LaunchAgentRestartTriggers` (a directory), and
`putter.json` (a link to one store file, so not a toplevel regular file either). They are now in
the home-manager allow-list.

- Why: they are small configuration outputs of the generation, not package trees, and the
  allow-list exists to keep package trees out. A missed change to an agent's launchd domain is
  the missing content spec 5 calls cheap to fix.
- A followed entry that is a link to a single file needs no code of its own: the walk follows
  the link and lists the file as a root, and its content is compared like any other file.
- Real data: `LaunchAgentDomains` moved to new store paths between generations here, but its
  files kept their content, so it shows no change, which the `find -L` baseline confirms.
  `putter.json` names the `home-files` store path and shows in every pair: hash-only for one
  bump and the dotfile change, real across 25 generations. File counts went up by one (153 to
  154, 695 to 696, 4 to 5); run times stayed within 0.7 to 1.7 s.

## Check 6: a target that hangs

**What ran.** `scry flake` on the synthetic flake with three configurations: two local ones and
a NixOS one with `--target hang=hang.example`, with
`NIX_SSHOPTS="-o 'ProxyCommand=sleep 600'"`. ssh then waits on `sleep` for the server's banner
and never resolves or contacts the host, and no key is offered.

**Results.**

- `--timeout 5`: the remote configuration reads `timeout after 5 s`, the two local ones finish,
  exit 1, 5.5 s in total.
- Default timeout: `timeout after 60 s`, the local one done, exit 2 under `--exit-code`, 60.5 s
  in total.
- Ctrl-C (`kill -INT`) while the remote fetch hangs: `scry: interrupted`, exit 1.
- After each run: no `ssh` or `sleep 600` process, and the `/tmp/scry-*` socket directory is
  gone. No `-O exit` ran, as ADR 0010 expects when no master socket exists.

## Check 9: revision mode on a flake in a subdirectory

**What ran.** A git repository in a temporary directory with the flake in `sub/`, two commits
and an uncommitted change, `scry flake ./sub --base HEAD~1` from the repository root.

**Results.**

- The base worktree is created with `git worktree add --detach` under the run's temporary
  directory, the base flake is evaluated as `<worktree>/sub`, and the worktree is removed at the
  end. Head includes the uncommitted change. All three configurations compared with the expected
  one-line diffs; 4.5 s including two builds.
- Ctrl-C during the build step (a derivation sleeping 40 s): `scry: interrupted`, exit 1; the
  worktree entry, the worktree directory and the run's temporary directory are gone, and no
  `nix build` or builder process is left.
- Nix 2.35.3 (Determinate 3.24.1) evaluates a flake inside a git worktree, where `.git` is a
  file, without trouble. Older versions are untested.

## Check 4, the part without SSH: colmena evaluation in a revision worktree

**What ran.** A local clone of a real flake hive (five x86_64-linux NixOS nodes) and
`scry colmena --base HEAD~1 --no-build`, with colmena 0.5.0 from nixpkgs first on `PATH`, and a
`ssh` shim first on `PATH` that logs and fails any call.

**Results.**

- No SSH is needed: revision mode evaluates both revisions locally. The shim logged no call.
- Both `colmena eval` steps ran in the head checkout and in the base worktree (10.1 s for five
  nodes, two revisions). Every node then failed with
  `<toplevel> is not in the store on this machine; run without --no-build`, as expected: their
  x86_64-linux toplevels were never built here, and this Mac has no Linux builder.
- Ctrl-C at 2 s (during step 1 at base) and at 5 s (during step 2): `scry: interrupted`, exit 1,
  worktree and temporary directory gone, no `colmena` or `nix eval` process left.
- The concurrent evaluations wait on one another while Nix fetches the hive (`waiting for another
  Nix process to finish fetching input`); harmless.

## Still unverified

- Check 3 against this Mac's real nix-darwin flake in deployed mode, and over SSH to a macOS
  target (check 8).
- Check 4 with the build step: the toplevels need an x86_64-linux builder, or a run on a Linux
  machine.
- Checks 1, 2, 7, 8 and 10, and the 5 s and 10 s targets over a real connection: see ADR 0071.

## Run plan for the SSH checks

Against a colmena hive of x86_64-linux NixOS nodes, from a Mac without a Linux builder. Each new
SSH connection needs a hardware-backed approval (Touch ID); scry opens one connection per target
per run (ADR 0010), and `ssh -G` and `ssh -O exit` need none. Counts below are per step and
assume no approval is remembered between connections.

| Step | Check | Command (sketch) | Approvals |
|---|---|---|---|
| 1 | — | `ssh node 'ls -l /nix/var/nix/profiles; cat /nix/var/nix/profiles/system-*-link/nixos-version'` to pick generation pairs | 1 per node |
| 2 | 1 | `scry paths --target node system-A-link system-B-link`, A to B one nixpkgs bump, text and JSON, timed | 1 |
| 3 | 1 | Baseline on the node: `diff -ruN` of the two `etc` trees, file count | 1 |
| 4 | 2 | `scry paths --target node` across about five bumps, timed; baseline as step 3 | 2 |
| 5 | 10 | Step 2 again with `SCRY_DEBUG=1`, then count `Accepted publickey` in the node's sshd journal for the run's minute; locally no `ssh -M` process and no `/tmp/scry-*` left | 1 + 1 for the journal |
| 6 | 10 | User `ControlMaster auto` / `ControlPath` / `ControlPersist` for the node in `~/.ssh/config` (by the user); run step 2; the user's master survives, scry opens none | 1 |
| 7 | 10 | `NIX_SSHOPTS="-o ControlMaster=no"`, step 2 again: one connection per command | about 6 (probe, 2 listings, content, up to 2 reference levels) |
| 8 | 8 | `scry paths --target user@node` for an account whose login shell is fish (set by the user if none exists); a macOS target by the same command against a Mac with Remote Login | 1 each |
| 9 | 7 | Copy of the hive with `deployment.buildOnTarget = true` on two nodes and a broken derivation on one; `scry colmena --on node1,node2`. The spec allows target builds only through `buildOnTarget`, and nothing else builds x86_64-linux here | colmena's own connections, unmultiplexed: about 3 per node (unverified), plus 1 per node for scry |
| 10 | 4 | `scry colmena --base REV` with the build step needs an x86_64-linux builder: run it on a Linux machine with the hive checked out, in one interactive session (tmux), Ctrl-C included | 1 for the session |

To keep approval time out of the 5 s and 10 s measurements, time a second run while a master
from the first is alive (within `ControlPersist=60`) or after step 6, and record both. Steps 2 to
5 use generations already on the node and build nothing.
