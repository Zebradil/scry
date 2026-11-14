# Acceptance check 8 with fish as the login shell

Spec 11.1 check 8, the half ADR 0071 left open: a target whose login shell is fish runs the remote
scripts and finds `nix`. Run from the Mac of ADR 0070 against `daddy`, an x86_64-linux NixOS
host of the user's homelab, with the user's approval and present for every SSH key use.

The target account is `fishtest`, added to `daddy` for this check (znix PR #247): a normal user
without sudo, login shell `/run/current-system/sw/bin/fish`. `suok` on the same host, login
shell bash, is the control.

Versions: fish 4.9.3, Nix 2.35.6 on `daddy`, OpenSSH 10.3p1 on the Mac. scry was the release
build (`nix build .#default`) of commit 83fb41e.

## Run

`scry paths --target <user>@192.168.0.101 --format json <old> <new>`, once per account, with the
two most recent system generations of `daddy` (nixpkgs 2026-11-01 against 2026-11-12):

- `fishtest`: exit 0 in 2.5 s, stderr empty.
- `suok`: exit 0 in 2.0 s, stderr empty.

The two reports are equal apart from the target name in `name`, `target` and `location`: 81
package changes, 1220 changed files, kernel `linux-6.18.60` to `linux-6.18.63`. Every remote
script ran under fish without a quoting or parse error, and `nix` was found.

## What this does and does not show

- sshd runs the command as `fish -c '<command line>'`; `status is-login` was false there, so
  fish read its non-login startup files. ADR 0011 checked the command line under fish by hand;
  this checks it through a real sshd, with fish's startup on NixOS.
- On NixOS, fish's non-login startup already puts `/run/current-system/sw/bin` on `PATH`
  (`command -v nix` in a plain `ssh fishtest@daddy` gave `/run/current-system/sw/bin/nix`). So
  the `PATH` that `sh -c` extends (6.1) was not needed to find `nix` here. A target where only
  the extension finds `nix` (Nix on a non-NixOS Linux, or macOS, where ADR 0071 covers the
  non-interactive half) was not tried with fish.

The deploy that added the account is the new side of the comparison (system generation 3); the
old side is the generation it replaced. The account stays until the user removes it.
