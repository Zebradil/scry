# Remote command line and the quoting check

Spec 6.1: the login shell on a target may be bash, zsh or fish, so it must only see
single-quoted words, and no quoted word may hold a backslash or a single quote. Every remote
script extends `PATH` first, and a test enforces the rule for every remote script.

Every remote command, script or not, is sent as

```text
sh -c 'PATH="$PATH:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:$HOME/.nix-profile/bin"; export PATH; exec "$@"' sh '<program>' '<arg>'…
```

built by `transport::script::command_line`. A script is the argv `sh -c <script> sh <args…>`
(`Script::command`), wrapped like any other, so one code path covers `nix path-info …` and
scripts, and `PATH` holds for both. The cost is one extra `exec` of `sh` on the target, no extra
round trip. Commands on this machine run their argv with scry's own `PATH`.

The rule is checked in three places, so no list of scripts has to be kept in step:

- **Compile time**: `Script::new` is a `const fn` that panics on a backslash or single quote. A
  script declared as a `const` is checked by the compiler; declaring it is registering it.
- **Command build**: `command_line` refuses any other word, such as a path argument, and the
  real seam returns `ExecError::Io` (`InvalidInput`) without running anything.
- **Pipeline tests**: the fake seam builds the command line for every remote command and panics
  if it is refused, so each test that runs a script on a target checks it.

Checked by hand with the command line above run by `bash` 5.3, `zsh` 5.9 and `fish` 4.9 as the
login shell: arguments with spaces, `$HOME`, double quotes, `*`, `;`, braces, `~` and the empty
string arrive verbatim, and `$HOME` in `PATH` is expanded by `sh`.

## Exit 127

`sh` exits 127 when it cannot find a program, also for `exec "$@"`. The seam maps it to
`` `<cmd>` not found on <machine> `` (spec 9), naming the program from the shell's message
(`sh: 1: nix: not found`, `sh: line 1: nix: command not found`). This holds on this machine
too, where the same scripts run without `ssh`. A 127 without such a line, such as `xargs`
failing to run its utility, or any 127 under `Stderr::Inherit`, stays an ordinary exit (ADR
0005); a caller that needs success words it `` `<program>` failed (exit 127): <first line> ``.
