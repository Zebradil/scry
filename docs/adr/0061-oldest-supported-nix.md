# Oldest supported Nix: 2.13 on this machine, 2.4 on targets (acceptance check 5)

## The calls

Every `nix` invocation in `src/`:

| Call | Where it runs | When |
|---|---|---|
| `nix --extra-experimental-features nix-command path-info --json -r <toplevel>` | each side's machine, targets included | every comparison |
| `nix --extra-experimental-features 'nix-command flakes' eval --json --no-write-lock-file <ref>#<set> --apply <fn>` | this machine | `scry flake`, completion |
| `nix --extra-experimental-features 'nix-command flakes' flake metadata --no-update-lock-file <dir>` | this machine | `scry colmena` with a flake hive |
| `nix --extra-experimental-features nix-command build --keep-going --out-link <link> <drv>^out…` | this machine | `scry flake` and revision mode, without `--no-build` |

colmena runs its own `nix` calls; its version requirement is colmena's.

## Result

**Tested**, on aarch64-darwin, each client against the same Determinate Nix 2.35.3 daemon, with
a flake without inputs holding one `nixosConfigurations` entry built by `/bin/sh`:

| Nix | `eval` (both forms) | `flake metadata` | `build <drv>^out` | `path-info --json -r` |
|---|---|---|---|---|
| 2.4 | ok | ok | `is not a valid URL` | ok (array shape) |
| 2.8.1 | ok | ok | `is not a valid URL` | ok (array shape) |
| 2.9.2, 2.10.3, 2.11.1, 2.12.1 | ok | ok | `could not find a flake.nix file` | ok (2.12.1 checked) |
| 2.13.6 | ok | ok | ok | ok |
| 2.31.5, 2.35.3 | ok | ok | ok | ok (object shape) |

Binaries came from nixos-22.05 (2.4, 2.8), nixos-22.11 (2.9 to 2.11), nixos-23.05 (2.12,
2.13) and this flake's nixpkgs (2.31). The pinned nixpkgs has nothing older than 2.31.

So this machine needs **Nix 2.13** when scry builds (`scry flake`, revision mode); without the
build step, and for `scry colmena` in deployed mode, 2.4 suffices. A target needs **Nix 2.4**:
it only runs `path-info`, and `--extra-experimental-features` is unknown before 2.4. Older Nix
on a target prints `warning: unknown setting` lines for settings newer than it; the fetch script
reads stderr only on a non-zero exit.

**Derived, not tested**: that 2.4 also works as a daemon on a target (only the client was
older here); behaviour on Linux; revision mode on a flake inside a git worktree, where older Nix
had trouble with a `.git` file (acceptance check 9). The release notes were not consulted for
the `^` change; the boundary comes from the runs above.
