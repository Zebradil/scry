# Fetch round trips: one script, the first level in the probe, references one level deep

Spec 6.1 caps the round trips per configuration and machine at 2 + link depth + reference
depth, shared where both sides are on one machine. Spec 6.2 lists four steps: probe, one listing
per link level, one content stream, one reference fetch per depth. Counted naively, the listing
of the toplevel itself is one round trip more than the cap: link depth counts the chain of store
links starting at the followed tree's own link (`etc`), and the toplevel is listed before that
chain starts.

## Decisions

- **One script, `fetch::SCRIPT`, with a mode argument** (`probe`, `list`, `refs`, `content`). It
  takes `tree::LISTING` as an argument and runs it, so the listing has one definition. Paths go
  on stdin.
- **The probe lists the toplevel** (the walk's first level) in the same round trip as `cd -P`,
  the marker files, `kernel-params` and `nix path-info`. Boot links and specialisations come
  from that listing's `ls -ldn` lines. A comparison then costs per machine: probe, one round trip
  per further link level, one content stream when anything changed, one per reference depth
  level. Measured in the pipeline tests: 7 on each machine for link depth 3 and reference depth
  2, also with both sides on one machine; 1 for no changes.
- **Failures are framed, not read from stderr.** The listing's and `nix`'s stderr are captured
  inside the script and printed on stdout as `::scry failed <program> <status>` plus that
  stderr; exit 127 there is a missing program (spec 9). Otherwise `ssh` chatter (a host key
  notice, `-v`) would pass for a `find` diagnostic, and Nix 2.35's `--json` deprecation warning
  for a failure. `tar` reports its status on stderr, since stdout carries the archive; its
  leading-`/` notice is no failure. The error line prefers Nix's `error:` line and skips
  warnings.
- **Both sides on one machine** send one request per step: the probe resolves both, the level
  listing takes both walks' roots (shared roots once) and the output is split back by root; the
  content stream and the reference round trips hold both sides' paths.
- **References: one round trip per depth lists and archives.** Spec 6.2 step 4 asks to list,
  checksum and fetch in one round trip, so a reference's tree is listed one level deep. A store
  link inside it is not followed and is left out of the comparison: its content is unknown, and
  calling it dangling would be wrong. A small rebuild's `system-path/bin`, all links, then shows
  nothing instead of hundreds of dangling links.
  - Only `bin/` of a directory is ever compared, so a directory stands for its `bin/`: the
    script prints the directory's own line and lists and archives `bin/` alone. Archiving whole
    directories of up to 1 MiB cost 47 MB raw (20.6 MB gzipped) on a real NixOS bump.
  - Each root goes with what to take of it: `w` (a pair of at most 1 MiB `narSize` each: a
    directory's `bin/`, or a file), `f` (a larger pair: only a file, since a large directory is
    never compared and listing its `bin/` could cost a package's binaries), `b` (a small
    rebuild: only a directory's `bin/`). Two requests for one root take `w`.
  - Files are archived only below `--max-file-size + 1` bytes (`find -size -Nc`, POSIX), with
    one `find` per 500 roots, as the listing batches them.
  - A small rebuild is named `referrer → …` only when a depth-one pair from the main tree is the
    same pair.
- **Reference chains**: a pair already on the chain is not followed again, a small rebuild's
  own pair included (gnugrep's `egrep` script names its own package, which gave
  `egrep → egrep`), and references to either toplevel or a
  specialisation are not followed: the comparison already holds the toplevel's own files, and
  following `activate`'s `systemConfig=` would add the toplevel's `bin/` that spec 6.2 leaves
  out.
- **A toplevel is a store path**: the probe keeps a resolved toplevel only when it is
  `/nix/store/<name>`, so `scry paths /tmp …` fails as "not in the store" instead of walking
  `/tmp`, and `nix path-info` never gets a directory it could read as a flake.
- **Archive names**: GNU `tar` unescapes backslashes in `-T` names, so it fails on such a path
  or archives another file. The content step leaves paths with a backslash out, which shows
  them as content not fetched; the reference archive, which `find` names in the script, leaves
  out names with any of `[`, `\`, `]`, `^` (`! -name "*[[-^]*"`, as no script may hold a
  backslash). `COPYFILE_DISABLE=1` keeps macOS `tar` from adding `._name` entries for extended
  attributes.
- **Timeout inside the fetch**: `Job.timeout` bounds `fetch::configuration`, which returns
  `timeout after N s` with the sides and kind known by then. Each machine records its sides as
  soon as its probe returns, so a target that hangs still leaves the other side in the report.
- **Kinds**: an old toplevel without a marker is compared as the new kind; only an old marker
  naming another kind fails with `closure kinds differ` (spec 5 says "the old toplevel's marker
  names a different kind"). The `unknown` kind note of spec 5 is not a report field: a `done`
  configuration with kind `unknown` carries it, and renderers print it from that.
- **home-manager current system**: `${XDG_STATE_HOME:-$HOME/.local/state}/nix/profiles/home-manager`
  when it exists, else the per-user profile when that exists; with neither, the error names the
  XDG path.
- **Report state on failure**: a side given by path is known before its probe (path as given,
  no version); a current system only after it. Sides resolve new first, so the new side's kind
  and version are kept when the old side fails, and the new side's error wins when both fail.

## Considered options

- **A separate listing round trip for the toplevel**: one round trip over the budget on every
  comparison.
- **Following links inside reference trees** with more listing levels: more than one round trip
  per depth. `find -L` instead would follow links out of the store into the live filesystem and
  checksum every package binary a `bin/` links to.
- **Telling a `find` diagnostic from the probe's stderr as a whole**: breaks under `ssh -v` and
  on Nix 2.35.

## Consequences

- The fixture (`fixture::Fleet`) answers each mode by computing the real output format, and a
  test runs the real script on this machine's tools against the same tree and compares the
  parsed results, so the two cannot drift apart unnoticed.
- The scripts ran once against a nix-darwin and home-manager machine through the real seam,
  BSD userland: no changes in 1 round trip, every comparison in 6.
