# The command seam runs an argv on a machine

Spec 11 requires one interface for every external command, with a fake for tests and for the
fixture run mode. The seam is `exec::Exec::run(&Machine, Command) -> Output`: an argv, stdin
bytes and a stderr mode, run on `Machine::Local` or `Machine::Remote`.

The real implementation (M4) owns everything about how a command reaches a machine: spawning,
`ssh` argument order, connection sharing, the 4-command limit per connection, quoting the argv
into one remote command line, and `ssh -O exit` plus socket cleanup in `shutdown`. Its pure
pieces (argv building, quoting) live in `transport::ssh` and `transport::script` with unit tests.

## Considered options

- A seam at the local process level, with `ssh` wrapping above it. Every pipeline test and the
  fixture fleet would then answer `ssh -G`, multiplexed `ssh … -- sh -c '…'` and `ssh -O exit`
  command lines and pick the target and script back out of them. At the machine level the fake
  answers "this script on `root@web-a`" directly, and the ssh layer is tested where it is built.

## Consequences

- A non-zero exit is an `Ok(Output)`; callers decide. `Err(ExecError)` means missing program,
  failed `ssh` (exit 255) or broken I/O.
- Dropping the returned future kills the child; timeouts and Ctrl-C rely on that (ADR 0001).
- `Stderr::Inherit` exists for the build step only; its output is not captured.
- No working-directory field: `git -C`, `colmena -f` and flake paths cover every planned call.
  Add one only when a command cannot take a path.
