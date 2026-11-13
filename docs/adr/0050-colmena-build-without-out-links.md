# colmena build results get no out-link

Spec 4.4 asks for out-links in the run's temporary directory for local build results, so a
concurrent garbage collection cannot remove them mid-run. `nix build --out-link` gives them for
the flake build and both revision builds. `colmena build` has no such option: its only root is
`--keep-result`, which writes `.gcroots/node-<name>` into the hive's directory, a write into the
user's repository that a preview must not make. Rooting the results with a second command after
the build (`nix build <drv>^out` or `nix-store --realise --add-root`) would build again, or
substitute, whatever colmena failed to build, and turn a failed build into a passing one.

So in colmena deployed mode the local new sides have no root. A collection that removes one
mid-run fails that configuration with the missing-path error of spec 9, as a target collected
mid-run does (4.4).

## A flake hive needs a current `flake.lock`

scry passes `--no-write-lock-file` to every `nix eval` that takes a flake reference; without
it Nix writes a missing or outdated `flake.lock` into the checkout and `git add`s it (checked
with Determinate Nix 2.35.3). colmena 0.5.0 runs `nix flake metadata --no-write-lock-file`, but
then `nix eval <flake>#colmenaHive` without the flag (`src/nix/hive/mod.rs`), so for a flake
hive whose lock is missing or outdated, `colmena eval` writes it. scry cannot pass the flag
through: `--nix-option` passes settings (`--option NAME VALUE`), and lock writing is a
command-line flag with no setting behind it.

So before any `colmena eval`, scry finds the head hive as colmena does (upwards from the
working directory, `flake.nix` before `hive.nix`) and, for a flake hive, runs
`nix flake metadata --no-update-lock-file <dir>`. It fails, writing nothing, when the lock
needs changes (Nix says `requires lock file changes`), and scry stops with a fatal error
asking for `nix flake lock`, also under `--no-build`. A `hive.nix` hive and a flake without
inputs (which gets no lock) pass. The base hive is not checked: it lives in the base worktree
inside the run's temporary directory, so a lock colmena writes there is removed with it.

## colmena's build behaviour (acceptance check 7, from source)

Read in colmena 0.5.0 (`src/nix/deployment/mod.rs`, nixpkgs), not run on a fleet:

- Nodes are evaluated in chunks (`--eval-node-limit`, default a heuristic); one `eval_selected`
  per chunk. An evaluation failure fails the whole chunk, and none of its nodes is built.
- Within a chunk every node builds in its own future (`build_and_push_node`, or
  `build_on_node` for `buildOnTarget`), joined with `join_all`, which waits for all of them. A
  failed build fails only its node; the others keep building.
- scry only passes nodes whose own `colmena eval` succeeded, so the chunk evaluation inside
  `colmena build` fails only on what changed between the two evaluations.
- A `buildOnTarget` node without a `targetHost` fails in colmena with `Unsupported`.

## Considered options

- **`--keep-result` with the hive directory restored afterwards**: still a write to the
  repository, and a crash leaves it behind.
- **Building per node** for isolation: not needed, since one node's failed build does not stop
  the others.
