# Resized on any size change

Spec 7.2 called a same-version rebuild resized only when its size moved by at least 1 MiB and at
least 10 % of its old size; spec 12 listed that threshold as unmeasured judgement. On a real
deployment it hid the one package the user was looking for: an in-house service, rebuilt from new
source without a version bump, moved by +8.0 KiB and sat in the collapsed `11 rebuilt` line between
ten packages that moved by 0 B.

A package is now resized on any size change, and rebuilt only when its size is the same to the
byte. Store hashes all have one length, so a rebuild that only swapped hashes in its references
keeps its `narSize` exactly. A size change means new content, or a reference whose store name
changed length (a dependency going from `x.9` to `x.10`).

Measured on one darwin machine's generations with `scry paths --format json`:

| Diff | Rebuilt before | Of those, size changed |
|---|---|---|
| nix-darwin 67 -> 68 | 3 | 1 |
| nix-darwin 68 -> 69, nixpkgs bump | 23 | 8 |
| home-manager 99 -> 100 | 34 | 6 |

About a quarter to a third of rebuilds move to the list: enough to stay readable, not a flood.

## Considered options

- **A lower threshold** (an absolute or relative floor): any floor above a few bytes hides a small
  script or service change like the one above, and it stays a guess.
- **Keeping the threshold and listing size-changed rebuilds first inside the collapsed group**: the
  package is still hidden until the reader opens the group, which is the problem reported.

## Consequences

- Generated NixOS paths without a version (`etc`, `system-path`, `system-units`) show as `[S]` rows
  whenever their content changes; the Files section shows what changed.
- The rebuilt group's label and items no longer show a size delta, since it is always 0. JSON keeps
  `rebuilt.size_delta` and each item's `size_delta`, always 0, so schema version 1 stays valid.
- Not measured: a full stdenv mass rebuild, where compiler output may change the size of many
  packages at once.
