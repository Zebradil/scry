# The README and docs demo. `nix run .#demo` records it from the repository root; fixture mode
# keeps the output the same on every machine.

Output docs/public/demo.gif

Require scry

Set Shell bash
Set FontSize 16
Set Width 1400
Set Height 820
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 50ms

Env SCRY_FIXTURE "1"
Env PS1 "$ "

Hide
Type "cd $(mktemp -d) && source <(scry completions bash) && clear" Enter
Show

Type "scry colmena --on "
Sleep 400ms
Tab@200ms 2
Sleep 2s
Type "'*'"
Sleep 500ms
Enter
Sleep 3s

Type "3"
Sleep 1.5s
Tab
Sleep 500ms
Type@350ms "JJJJJJJJJ"
Sleep 1s
Enter
Sleep 2.5s
Type "/"
Sleep 300ms
Type "app.json"
Sleep 600ms
Enter
Sleep 1.5s
Type "g"
Sleep 300ms
Type@400ms "JJJ"
Sleep 500ms
Enter
Sleep 3.5s
Type "q"
Sleep 1s
