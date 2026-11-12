# Children in their own process group

ADR 0001 cancels a command by dropping its future, and tokio's `kill_on_drop` then kills the
child. It kills only the child: `find` and `tar` under a local `sh -c`, or the `nix` processes
under `colmena`, would run on after a timeout until they next write to the closed pipe, which
for an evaluation can be minutes and a gigabyte of memory.

The real seam spawns every child with captured stderr with `process_group(0)` and, when the
future is dropped before the child was reaped, sends `SIGKILL` to the whole group. After
reaping, the group is left alone, since its id may be reused.

A child under `Stderr::Inherit`, the build step, stays in scry's process group. In a background
group, a write to the terminal under `stty tostop`, or any `tcsetattr`, stops it with `SIGTTOU`,
and since builds have no timeout, scry would hang. Dropping its future kills the child only.

## Consequences

- Children with captured stderr no longer get the terminal's `SIGINT`. Ctrl-C reaches them only
  through scry: the run must handle it with `tokio::signal::ctrl_c` for its whole length and drop
  its work (ADR 0001). If scry dies without dropping, by `SIGKILL` for example, they run on. A
  build gets the terminal's `SIGINT` itself.
- The ssh `ControlPersist` master detaches into its own session, so it outlives the command that
  started it, as intended; `Exec::shutdown` stops it.
