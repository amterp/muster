# tcc-probe

Asks macOS who it holds responsible for a pane's program, so `docs/observations/macos-26.4.1.md`
can rest on a recording rather than on a reading of how TCC is said to work.

```
./probe          # the four arrangements, ~35 seconds
./probe --here   # the same two questions asked of this shell, ~5 seconds
```

Most of a full run is deliberate waiting, because the question is what changes when the app
quits and every part has to still be alive on the far side of that. The transcript lands in
`corpus/macos-26.4.1/responsible-process.txt`.

## What it builds

Four arrangements of Muster's shape, and nothing of Muster's. One binary plays every part - an
app, a daemon that outlives it, and a pane's program under the daemon - which is Muster's
arrangement with none of Muster's code in it. A probe that drove the real app would be measuring
three dependencies at once, and re-signing Muster is itself one of the things being explained.

| arrangement | the daemon is | started by |
|---|---|---|
| `child` | a bare binary | the app, with `posix_spawn` - what Muster does today |
| `bundled` | an app bundle's executable | the app, the same way |
| `opened` | the same bundle | Launch Services, `open -n -a` |
| `launchagent` | a bare binary | a per-user launchd job in `gui/<uid>` |

The three that are not `child` are the candidates on kan `a_29i4bxafd`. Each bundle carries an
identifier of its own, so no two arms share a TCC subject and a permission answered for one
cannot decide another's result.

Panes are always spawned from the bare binary, whatever the daemon is. A pane runs whatever
agent somebody runs - `claude`, `python`, a shell - and never an app bundle, so spawning each
arm's own executable would quietly give every pane an identity no real pane has.

It is launched with `open` rather than run from this shell. Responsibility is assigned when a
process starts, and a binary started from a terminal has no bundled ancestor to be charged to -
every line then says "itself", which looks like a finding and is an artifact of how it was run.

## `--here`

Asks the same two questions of whatever shell you run it in, and records
`corpus/macos-26.4.1/local-network-from-here.txt`. One real process rather than a model of one,
so it is true of where it was run and nowhere else.

It is the diagnostic the local-network finding wished existed: run in a Muster pane whose Muster
has been relaunched, it *is* the reported failure, with the control beside it.

## What the local-network line means, and what it does not

Local Network is the protected request whose denial is unrecognizable as one - it arrives as
`EHOSTUNREACH` on the multicast send rather than as a prompt or a permission error. `dns-sd`
keeps working throughout and confirms the wrong thing, because it asks mDNSResponder rather than
sending multicast itself.

That is why the unicast datagram to the default gateway sits beside it. Same process, same
instant, same LAN: if that one goes and the multicast one does not, routing is not what refused
it. Without the control a reader has only an error that names routing.

**It does not separate the arrangements, and should not be read as though it did.** Across three
runs on 2026-08-30 the same arm answered differently: everything refused on the first run, and
everything succeeded by the third. What a subject has already been granted moves between runs and
the arrangement does not, so the responsibility half is the half that carries the finding.

## Two things it leaves on the machine while it runs

**The launchagent arm installs a launchd job**, which is what that arm exists to measure. macOS
announces it - "probe-bare can run in the background" - and lists it in Login Items & Extensions.
The job is booted out and its plist deleted on the way out, including when the run fails, but the
notification has already happened. That cost is not the probe's: it is what installing a
LaunchAgent does, and it is one of the things the card was deciding about.

**The multicast datagram leaves the machine.** It is the only thing here that touches the
network.

## The trap it was nearly caught by

`responsibility_get_pid_responsible_for_pid` is what TCC itself consults, and unentitled it
answers questions about *other* processes by handing the pid straight back. That is
indistinguishable from "this process is its own responsible process", so an early version of
this measurement concluded exactly the wrong thing about a real running Muster. Every line
here is a process asking about itself, which is the case that always answers truthfully.

The `prompts-say` column is derived rather than measured: it is the responsible process's own
bundle name, which is what macOS puts in a prompt's heading. A process that is its own
responsible process and has no bundle is one nothing can put a name to, and that is the whole
difference between the two arrangements that both make attribution consistent.

## Re-run it on a macOS upgrade

The behavior recorded here is undocumented and Apple owes nobody its stability. What the
transcript costs to regenerate is one command, and what a stale claim in
`docs/observations/` costs is a design decision taken on a fact that stopped being true.
