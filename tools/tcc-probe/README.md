# tcc-probe

Asks macOS who it holds responsible for a pane's program, so `docs/observations/macos-26.4.1.md`
can rest on a recording rather than on a reading of how TCC is said to work.

```
./probe
```

One run takes about 35 seconds - most of it deliberate waiting, because the question is what
changes when the app quits and every part has to still be alive on the far side of that. The
transcript lands in `corpus/macos-26.4.1/responsible-process.txt`.

## What it builds

A throwaway `Probe.app`, and nothing of Muster's. One binary plays three parts - an app, a
daemon it spawns that outlives it, and a pane's program under the daemon - which is Muster's
arrangement with none of Muster's code in it. A probe that drove the real app would be
measuring three dependencies at once, and re-signing Muster is itself one of the things being
explained.

It is launched with `open` rather than run from this shell. Responsibility is assigned when a
process starts, and a binary started from a terminal has no bundled ancestor to be charged to -
every line then says "itself", which looks like a finding and is an artifact of how it was run.

## The trap it was nearly caught by

`responsibility_get_pid_responsible_for_pid` is what TCC itself consults, and unentitled it
answers questions about *other* processes by handing the pid straight back. That is
indistinguishable from "this process is its own responsible process", so an early version of
this measurement concluded exactly the wrong thing about a real running Muster. Every line
here is a process asking about itself, which is the case that always answers truthfully.

## Re-run it on a macOS upgrade

The behaviour recorded here is undocumented and Apple owes nobody its stability. What the
transcript costs to regenerate is one command, and what a stale claim in
`docs/observations/` costs is a design decision taken on a fact that stopped being true.
