# muster-daemon-data

What `muster-daemon` gives every shell it starts, beyond the binary: Ghostty's terminfo entry and
Ghostty's shell integration for bash, zsh and fish. The daemon reads this directory from beside its
own executable, or from `--data`. Copy it with the daemon when installing one somewhere.

Everything here is copied unchanged from Ghostty, at the commit Muster pins. None of it is compiled
into the daemon.

| Path | License |
|---|---|
| `terminfo/` | MIT, `LICENSE-ghostty` |
| `shell-integration/fish/` | MIT, `LICENSE-ghostty` |
| `shell-integration/bash/bash-preexec.sh` | MIT, as its upstream licenses it (the file carries no header): bash-preexec by Ryan Caloras, https://github.com/rcaloras/bash-preexec |
| `shell-integration/bash/ghostty.bash` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's bash integration |
| `shell-integration/zsh/.zshenv` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's zsh integration |
| `shell-integration/zsh/ghostty-integration` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's zsh integration |

The GPL files are their own source. Each carries its license header, and Ghostty's repository at
https://github.com/ghostty-org/ghostty holds them under `src/shell-integration/`.
