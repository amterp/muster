# muster-daemon-data

What `muster-daemon` gives every shell it starts, beyond the binary: Ghostty's terminfo entry and
Ghostty's shell integration for bash, zsh and fish. The daemon reads this directory from beside its
own executable, or from `--data`. Copy it with the daemon when installing one somewhere.

Everything here is copied unchanged from Ghostty, at the commit Muster pins, except `bin/ghostty`,
which is Muster's. None of it is compiled into the daemon.

`bin/ghostty` is what Ghostty's shell integration calls for its `ssh-terminfo` and `ssh-env`
features, `ghostty +ssh`, which installs the terminfo entry on the host a pane sshes to. It hands
that to `muster-daemon ssh`, Muster's port of it, and passes anything else to a real `ghostty` on
the PATH if there is one. It is Muster's own file beside Ghostty's scripts, never a change to
them.

| Path | License |
|---|---|
| `terminfo/` | MIT, `LICENSE-ghostty` |
| `bin/ghostty` | Apache-2.0, Muster's own |
| `shell-integration/fish/` | MIT, `LICENSE-ghostty` |
| `shell-integration/bash/bash-preexec.sh` | MIT, as its upstream licenses it (the file carries no header): bash-preexec by Ryan Caloras, https://github.com/rcaloras/bash-preexec |
| `shell-integration/bash/ghostty.bash` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's bash integration |
| `shell-integration/zsh/.zshenv` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's zsh integration |
| `shell-integration/zsh/ghostty-integration` | GPL-3.0-or-later, `GPL-3.0.txt`; based on kitty's zsh integration |

The GPL files are their own source. Each carries its license header, and Ghostty's repository at
https://github.com/ghostty-org/ghostty holds them under `src/shell-integration/`.
