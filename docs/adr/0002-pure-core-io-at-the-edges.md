# Pure core, I/O at the edges

Name and version parsing, path-info parsing, package comparison, tree listing parsing, file and
boot comparison, churn, diffs, highlighting and reference pairing are pure functions over
in-memory data: they take bytes or parsed values and return values, and never touch the command
seam, the filesystem, the clock or the environment. Fetch, sources, the build step, revision mode
and the run do I/O, and only through the seam (ADR 0005).

The pure modules hold most of the logic and the spec's edge cases, so they get table-driven unit
tests without fakes, and M1 and M2 build them in parallel with the seam (M4) because they need
nothing from it. The I/O modules stay thin: they build commands, hand the output to the core and
assemble report types.

Consequence: a pure function never "just checks" something on disk. When it needs more data, the
caller fetches it first, in the round trip budget of spec 6.1.
