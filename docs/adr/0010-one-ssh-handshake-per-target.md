# One SSH handshake per target

Spec 6.1 asks for one shared connection per target, through `ControlMaster=auto`. That option
races: every client that starts before the master's socket exists becomes a master of its own,
with its own handshake and its own hardware-key prompt. A run starts several commands to a new
target at once whenever two configurations share a host (a NixOS and a home-manager
configuration on `web-a`), or both sides of one configuration live there.

Each `Remote` (target, port and `sshOptions`; any difference is another connection) gets one
`transport::ssh::Connection`:

- **Sharing options, found once**: `ssh -G` with `NIX_SSHOPTS`, `sshOptions` and the port, so an
  option given there counts. A `controlpath` other than `none` means the user's multiplexing,
  used as is. Otherwise `-o ControlMaster=auto -o ControlPath=<dir>/<n> -o ControlPersist=60`,
  `<n>` counting connections: with `%C`, a port given or left at its default, or two identity
  files, would map two connections to one socket, and the second's options would be ignored
  once it rode the first's master. `<dir>` is a 0700 directory `/tmp/scry-XXXXXX` created on first need (`tempfile`, mode set
  explicitly: its default follows the umask). Not `$TMPDIR`: on macOS it pushes socket paths
  towards the 104-byte limit.
- **Handshake guard**: while no master is known to listen, commands to the connection run one
  at a time. The first one opens the master; the rest find its socket. A command that fails to
  connect (exit 255), or is dropped by a timeout, leaves the next waiting one first. On the
  tool's own sharing the guard re-arms when the socket file is gone, after an idle master exits
  under `ControlPersist`; when a command connects and no socket appears, sharing is off
  (`ControlMaster=no`) and commands run together. The user's own multiplexing gets the guard
  for its first command only: its socket is theirs to manage.
- **Sessions**: a semaphore of 4 per connection, held for the whole command.
- **Stderr**: once the child is reaped, stderr gets 1 s more to reach end of file. Under
  `NIX_SSHOPTS=-v` the master keeps the first command's stderr open for its whole life, which
  would hold that command, and the guard with it, for up to 60 s. In the normal case the pipe
  holds at most one buffer when the child exits and ends at once, so nothing is cut. Stdout
  still waits for end of file: the master sends it to `/dev/null`.
- **Errors**: for exit 255, lines starting with `Warning:` (`StrictHostKeyChecking=accept-new`
  adding a host key) are skipped when picking the line of spec 9.
- **Shutdown**: `ssh <same options> -O exit <target>` for each connection on the tool's sharing
  whose socket exists, then the directory is removed (dropping the `Process` removes it too). Without a master, `-O
  exit` fails at the socket and never connects (checked with OpenSSH 10.3).

## Considered options

- **A warm-up master** (`ssh -M -N -f`, or `ssh … -- true`) before the first command: one more
  round trip per target, outside the budget of spec 6.2.
- **Waiting for the socket file** instead of for the whole first command: the path is known,
  but it needs polling and a stale-socket rule.
  This is the upgrade if serialised first commands turn out to cost time.

## Consequences

- Concurrent first commands to one target wait for the whole first command, a probe for
  example, not just its handshake. Later commands run up to 4 at once.
- `NIX_SSHOPTS="-o ControlMaster=no"` comes before the tool's options, so every command connects
  on its own, all but the first in parallel; with no socket, shutdown sends no `-O exit`.
- A stale socket file, left by a master killed hard, counts as a master; the clients then race
  to replace it.
- Not verified without a target (acceptance check 10): one handshake on a real host, also with a
  user `ControlPath`; the `ControlPersist` master detaching (OpenSSH's `daemon()`) and releasing
  the captured stdout, and stderr unless `-v`; no master left
  after shutdown. With stderr captured, ssh does not print its "Confirm user presence" notice
  for a hardware key.
