# Muster

Native workspace for AI coding agents: real splits, real keybindings, agent status at a glance, local and SSH agents
side by side in one window, on daemon-owned sessions that outlive the app.

Pre-alpha; nothing works yet. Start with `docs/origin.md` for why this exists and `docs/architecture.md` for the
shape.

## Desiderata

- **Agent states are the point.** Every pane shows working / blocked / done / idle at a glance, for local and remote
  agents alike. Lose this and the tool has no reason to exist.
- **Attention routing.** Glanceable states are the floor, not the ceiling: when an agent needs you, the notification
  says why, and one keystroke lands you on the pane that asked.
- **Native feel.** Real GPU-rendered splits and the platform's own keybindings. No prefix key, no chord-scavenging
  around a host terminal, no virtual splits composed into one grid.
- **Fast is a feature.** Input-to-glyph latency indistinguishable from plain Ghostty. Work is budgeted by frequency
  times cardinality - per byte, per event, per render, across panes and daemons - and profiled at 1 and 15 panes.
- **Local and remote in one window.** Harnesses on this machine and on an SSH devenv, rendered and driven identically.
- **Sessions outlive the app, and their shape outlives the daemon.** Backend daemons own the PTYs, so quitting
  Muster, dropping the VPN or closing the lid costs nothing: agents keep working and every pane comes back. Below
  that line the guarantee weakens honestly rather than silently - a daemon restart returns the pane tree and each
  pane's directory but not the processes, and a reboot is the same case with the daemon to start first. What can be
  written down is written down; a live process cannot be, and Muster does not pretend otherwise
  (`docs/architecture.md`, durability).
- **View = f(daemon state).** The app owns no truth. Layout, agent state, and scrollback live in the backend; the app
  renders them and forwards intent.
- **Swappable organs, pragmatically.** The session backend ([herdr](https://github.com/herdrdev/herdr) today) and the
  renderer ([libghostty](https://github.com/ghostty-org/ghostty) today) sit behind narrow seams we own: the core
  speaks Muster's own vocabulary, each dependency lives in one adapter, and the contract corpus is the executable
  definition of what a replacement - wholesale, or a fork - must provide. We embrace a dependency where it
  simplifies; we never let one own our contract - and the surface a person or an agent drives is part of that
  contract. Where Muster acts on the answer, the way to ask is Muster's, never the dependency's.
- **Green suite means it works.** Muster is built largely by AI agents, so the suite carries the confidence an
  author's memory cannot. What makes that achievable here: a thin shell over a thick headless core, so no logic hides
  in the untestable layer; a real, version-pinned herdr behind the backend seam rather than a stand-in, because a
  stand-in is Muster's own guess at a daemon and a wrong guess passes; oracles recorded from reality - terminal grids
  via libghostty-vt, intent on the wire - never pixels or internals; deterministic, and offline in the sense that
  nothing reaches the network.
- **Every run explains itself.** Muster is several processes, often on several machines, and a symptom in one usually
  has its cause in another: a window that ignores the keyboard is a bridge that never started, or one that started and
  could not dial back. Each run leaves a single machine-readable timeline spanning all of them, so a bug report is a
  file an agent can read rather than a session someone has to re-stage. Terminals carry secrets, so what you typed is
  never in it unless you ask for it.
- **Cross-platform stays open.** macOS ships first. The shell layer is thin and per-OS; nothing outside it may assume
  an OS, and the core is portable by construction rather than by intention - it is a different language from the
  shell, so an OS type cannot leak into it by accident. Inside that layer the most native answer wins: portability
  constrains the core, and is never a reason to make Muster feel less like the machine it is running on. Both chosen
  organs already run on Linux and Windows.
- **AI-native surface.** Configuration is files. Every action runs through one shared path exposed to GUI, CLI, and
  API alike - parity by construction, not by discipline - so an agent can drive Muster as readily as a person can,
  through Muster's own surface and in Muster's own vocabulary.
- **Harness-agnostic.** Strive to support many harnesses.

## Non-goals

Muster imposes no workflow: it provides panes, states, sessions, and a scriptable surface, and has no opinion about
how you run your agents.

- Not a terminal emulator - libghostty is.
- Not a multiplexer or session daemon - herdr is.
- Not an agent framework - Muster runs whatever agents you already run.

## Shape

    native shell (macOS first)        thin, per-OS, mostly dumb - Swift + AppKit today
      ├─ renderer seam → libghostty   real splits, GPU, VT fidelity
      └─ core seam     → one symbol   protobuf over a C ABI; events, never bytes
           portable core (Rust)       mirror, keymap, dispatch, attention, config
             └─ backend seam → herdr  JSON socket + ANSI pane streams
                  ├─ one bridge per pane      unwraps frames onto a surface's PTY
                  ├─ daemon on this machine   local agents
                  └─ daemon on devenv (SSH)   remote agents

## Configuring

Everything Muster owns lives in `~/.muster`, and `MUSTER_HOME` moves the lot. Two files in
it, both optional, both TOML. `~/.muster/config.toml` is yours to write, and it is the only
file Muster reads: `[[daemon]]` blocks name the machines a window attaches to, `[keymap]`
rebinds any of Muster's own actions, `[font]`, `[colors]` and `[cursor]` decide what the
window looks like, and the rest decides what a keystroke becomes on its way to a pane.

One directory rather than a file in each of the XDG trees, because Muster's surface is meant
to be discovered rather than taught - an agent that can list one directory needs no
documentation to find the whole of it. `XDG_CONFIG_HOME` and its family still decide where
herdr listens; they no longer move anything of Muster's, and they no longer decide what
Muster's own daemon reads.

The rule, so that the next setting has an obvious home: **a setting is Muster's when Muster
acts on the answer or hands it on**, including the ones it only translates onward - for
libghostty, and now for herdr. It is the daemon's when it is about the daemon's own interface,
which Muster never shows you. And within the file, a table when a subject has several answers,
a root key when it has one.

```toml
option_as_alt = "left"         # never (the default) | always | left | right
resize_step = "20c"            # per resize chord: cells (c) or points (px). Omit for the
                               # daemon's own step. The unit is required.
scroll_multiplier = 1.5        # scales what the trackpad or wheel reported
pane_padding = 2               # points between a pane's text and its edges; 0 fits the most rows
scrollback_bytes = 50000000    # history a pane keeps; omit for the daemon's own answer

[shell]
command = "/opt/homebrew/bin/fish"  # omit for whatever this machine thinks your shell is
mode = "login"                 # auto (the default) | login | non_login

[keymap]
split_right = "cmd+d"          # the default; Ghostty's, wherever Ghostty has one
split_left = "cmd+opt+d"       # ships unbound, as it does in Ghostty
zoom = "cmd+shift+return"
close_pane = ""                # unbound - the action stays, the shortcut goes

[text]
"shift+enter" = "\n"           # this chord sends these bytes, whatever the encoder would say

[font]
family = "Fira Code"           # omit for whatever monospace this machine would have picked
size = 13

[colors]
background = "#282c34"
foreground = "#ffffff"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"        # the character under the cursor
selection_background = "#414868"
selection_foreground = "#c0caf5"
divider = "#4a4a4a"            # the line between two regions; omit for the platform's
palette = [                    # the ANSI sixteen, all of them or none
  "#000000", "#cc0000", "#4e9a06", "#c4a000",
  "#3465a4", "#75507b", "#06989a", "#d3d7cf",
  "#555753", "#ef2929", "#8ae234", "#fce94f",
  "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
]

[cursor]
style = "block"                # block | bar | underline | hollow
blink = true                   # omit to let the program in the pane decide
```

`[keymap]` is partial, so a file that names one action rebinds one action. Chords are
modifiers and a key, in any order and any case, spelled the way you would say them: `cmd`,
`opt`, `ctrl`, `shift`, and `left`, `return`, `f5`, `[`. The actions are `new_tab`,
`next_tab`, `previous_tab`, `split_*` for each direction, `close_pane`, `next_pane`,
`previous_pane`, `focus_*` and `resize_*` for each direction, `focus_pane_1` to
`focus_pane_9`, `rename_pane`, `rename_tab`, `find`, `find_next`, `find_previous`, `zoom`,
`increase_font_size`, `decrease_font_size`, `reset_font_size`, `toggle_sidebar`,
`reload_config`, and `show_shortcuts`. On macOS these become menu items, which is where the
platform dispatches a key equivalent from - so a rebound action moves in the menu too, and
System Settings can move it again.

Three of them ship with no chord at all. Ghostty has `split_left` and `split_up` as actions and
binds neither, so Muster does the same rather than inventing a shortcut for them - they are in
the menu, one click away and one `[keymap]` line from a chord. Which of the four sides costs
herdr one request and which costs two is not something you can tell from here, and that is the
point: herdr splits rightward and downward only, so the other two are a split and a rearrange,
and Muster takes the arrangement from the daemon's own answer rather than letting you watch the
pane move.

`rename_tab` is the third, and that one is Muster's own call rather than Ghostty's: you name a
tab once and a pane several times an hour, so the chord goes to `rename_pane` (`cmd+shift+n`)
and the tab keeps a menu item.

**`cmd+1` to `cmd+9` go to a numbered agent.** The number is drawn on its row in the agent
list, counting down the whole list across every machine, so `cmd+3` is the third row whichever
daemon holds it. Panes rather than tabs, because the unit here is an agent and an agent is a
pane: attention routing promises that when one needs you, a keystroke lands you on the pane
that asked, and only a click used to do that.

It reaches the tabs too. Going to a pane brings its tab on screen, so a number gets you into
any tab through any pane in it - which is why nine chords are enough for both and why nothing
else is numbered. Past nine the numbers run out, and the tenth pane is reached by `next_pane`,
by a direction, or by clicking its row.

The number is a position, so it moves when a pane above it opens or closes. That is the cost
of numbering the thing that churns, and it is the right way round: the order is yours to
arrange, and a number that stayed put when you moved its row would be fighting you.

The two ways of moving are still different axes. `next_pane` and the four directions reach
every pane the window is **showing**; `next_tab` and `previous_tab` walk the tabs behind
those, including ones no region has on screen.

These used to be `focus_tab_1` to `focus_tab_9`, and the old names are gone rather than kept
as aliases. A `[keymap]` naming one is refused, and the whole file with it, which is what a
config carried over from before should get - silently binding `cmd+3` to something other than
what it used to reach is the one outcome worse than the refusal. Tab captions lose their
numbers in the same change: two numberings in one list is worse than either, and a tab you
have not named is now captioned `Tab 2` rather than carrying a chord's number.

**Two things cannot hold one chord, and the file is refused rather than one of them losing.**
Three ways that happens: two `[keymap]` actions on the same chord, a `[keymap]` action on a
chord `[text]` also sends bytes for, and a `[keymap]` action on one of the five chords every
pane uses for line editing - `cmd+left` and `cmd+right` for the ends of a line,
`cmd+backspace` to delete to the start of it, and `opt+left` and `opt+right` for word motion.
All three resolve the same way if allowed, and the refusal exists because of how: on macOS an
action is a menu item, and the menu is offered a key equivalent before the keystroke reaches
the window at all. So the shortcut wins every time and whatever it took stops working
silently - `opt+left` rebound to `focus_left` ends word motion in every shell in the window,
with nothing on screen connecting the two. None of the shipped defaults collide, so this only
ever answers a chord you chose.

**`find` searches a pane's history, and says how far back it got.** `cmd+f` opens a bar over
the pane with the keyboard, `cmd+g` and `cmd+shift+g` walk the matches, and landing on one
scrolls the pane to it and marks it. What it searches is not what is on screen: a pane's
scrollback belongs to the daemon holding it, so the match count is Muster's answer about the
daemon's history rather than the renderer's about the visible grid. herdr hands over at most
its last thousand rows and offers no way to ask for more, so a pane longer than that is
searched in part - and the bar says so, beside the counter, exactly when it is true. A find
that quietly covered a fifth of a pane and answered "no results" would be worse than not
having one.

**A row in the agent list says two things, and you write the first one.** Underneath is what
the agent calls itself - Claude sets its terminal title to what it is working on, so the row
reads `chasing a flaky test` while it does - and on top is what to call the pane, which starts
out as its directory and the harness in it. `rename_pane` replaces that with anything you like,
emoji included: `🔥 payments spike`. Double-clicking a row asks the same thing, which is worth
knowing because the rows most worth naming are the ones no split is showing.

**Drag a row and the pane moves with it.** Dropping one agent's row on another exchanges the
two, so the list you arrange is the window you get - and the numbers move with them, since
`cmd+3` is whatever the third row now holds. Drop a row on one in a different tab and the agent
joins that tab, landing directly behind the row you dropped it on.

An exchange rather than an insertion, because an arrangement has no "between": two panes side
by side can trade places, and there is no other reading of dragging one onto the other. Nothing
is stored to make this work - the daemon rearranges its own tree and the list is a view of it,
so the order survives quitting Muster the way the panes themselves do.

A drop onto another machine's row is refused, and the cursor says so while you hover. A pane is
a process its daemon owns, so moving one across machines would mean killing it here and
starting a different one there.

The two lines age differently, and that is the point of having both. A name is written down by
the daemon, so it survives quitting Muster and survives the daemon restarting; a title belongs
to the program, so a restart loses it until the agent sets one again. Naming something never
costs you the second line. Naming it nothing - an empty field - gives you the directory back.

A second line is drawn only for a pane with an agent in it, and only when the title says
something the first line does not. A plain shell sets a title too, usually the directory you
are already reading, and fifteen rows of that would be thirty lines saying fifteen things.

`option_as_alt` is the one that decides whether `opt+t` reaches an agent. macOS treats option
as a composing key, so by default it produces `†` and a program waiting for `alt+t` never
hears it. Naming a side keeps accented characters on the other hand. `[text]` is the escape
hatch beneath all of that: a chord bound there sends exactly those bytes and no encoder is
consulted. It is keyed by chord where `[keymap]` is keyed by action, because an action has
one chord and text has no name to key on.

The other three root keys are small answers a terminal is expected to let you change, each
one line because each is one value. `resize_step` is how far a resize chord moves a divider;
omit it and the daemon decides, which is what a chord meant before the key existed.
`scroll_multiplier` scales whatever your trackpad or wheel reported, so `1` is the device's
own answer and `0.5` is half of it - a multiplier rather than a line count, because how big
one notch is belongs to the device. `pane_padding` is the space between a pane's text and its
edges, one number for both axes; `0` is what fits the most rows into a window of fifteen
agents.

**`resize_step` takes a unit, and it is required**: `"20c"` is twenty cells, `"150px"` is a
hundred and fifty points. Two units because neither one is right for everybody. A cell is
about 8 by 17 points, so one number in cells moves a divider roughly twice as far up and down
as it does side to side, and four symmetric chords that travel visibly different distances is
not what a hand expects. Cells keep their own advantage: they survive a font size change,
where a distance in points does not, and `cmd+=` is a thing people press. Requiring the suffix
on both is what makes having two safe - a bare `20` meaning cells beside a suffixed `"150px"`
is a form you have to know rather than read. `c` rather than `cells` follows kitty, which
spells this same ambiguity that way.

Two consequences worth stating rather than leaving you to find. The bare `resize_step = 2`
that Muster used to take no longer parses, and the refusal hands you back both spellings of
the number you already chose. And Ghostty's `cmd+shift+h=resize_split:left,150` becomes
`resize_step` here rather than a chord that carries its own argument, because on macOS an
action is a menu item and a menu item has one key equivalent - so a chord cannot hold a value.
Ghostty's `150` is pixels; write `"150px"` for the same distance, and `"150c"` will move a
hundred and fifty *cells*.

**A distance is exact against one divider and short against a nested one.** What the daemon
moves is a divider's share of what it divides, so Muster turns your distance into a share of
the region the chord happened in - exact when that region holds one divider on the axis you
are resizing along, and less than you asked for when the divider you are moving splits only
part of it. The alternative is Muster keeping its own copy of the pane tree to work out which
divider a direction refers to, which is a large thing to carry for a number that is already
close, so this is a known limit rather than an oversight.

`pane_padding` stays a bare number of points, which is a decision rather than an oversight: a
unit is worth its cost only where two of them are genuinely plausible, and nobody wants
padding measured in cells.

`[font]`, `[colors]` and `[cursor]` are the window's appearance, and every one of them is
optional. **Leave a value out and you get the renderer's own default, not one Muster
invented** - the vocabulary names what you may change and nothing else, because Muster has no
opinion about which monospace font your machine has and a default palette written into Muster
would be a transcription of somebody else's. `palette` is the sixteen ANSI colours, all of
them or none: a partial one leaves the rest as the renderer's and produces a scheme nobody
designed. `divider` sits with the pane colours even though Muster rather than the renderer
paints it, because you pick colours all at once and which piece of code holds the brush is
not something you should have to know.

**A `family` this machine does not have is reported rather than ignored.** Leaving `family` out
asks for the renderer's own font and is the design; naming one that is not installed is a
different thing, and it used to look identical - a family name is a string, so `Fira Cod` and
`Fira Code` both paint on a machine with neither. It now appears at the foot of the agent list,
naming the font and saying that panes are using the renderer's default instead. A family that
*is* installed but is not monospaced is reported the same way and for the same reason: the
columns stop lining up, which reads as a Muster bug rather than a font one. Both are warnings
rather than refusals - a font is wrong only on the machine that lacks it, so refusing the file
would mean one config could not be shared between a laptop and a devenv.

Muster reads no file belonging to another application. It used to: fonts and colours came
from a Ghostty config if you had one, which is why the whole of `[colors]` is new rather than
a rename. If you configured Muster's appearance through Ghostty, that stops working and this
is where it moves to. `docs/architecture.md` says what the loan cost and why it went.

`[shell]` and `scrollback_bytes` are the two Muster does not act on at all. What a pane runs
and how much of it you can scroll back through belong to the daemon that makes the pane - so
Muster translates them into a file of its own and hands that to the daemon, exactly as it does
`[font]` and `[colors]` for the renderer. Before this you had to learn that herdr existed and
find its config file, and a `default_shell` set for your own terminal quietly decided what
every Muster pane ran.

`scrollback_bytes` is bytes because that is what the buffer is measured in; a line has no
fixed size, so a count of them would be a number that did not mean what it said. Zero is a
real answer - a pane that keeps only what is on screen. What is deliberately *not* offered is
update checking: Muster ships one herdr, pinned by version and checksum, and turns its update
checks off. A daemon that could be told to go and fetch a different version of itself would
make "this was tested against the daemon it ships with" mean nothing.

**Saving the file is enough.** Muster watches it and reads it again, and `cmd+shift+,` or
Reload Configuration asks for the same thing when you would rather say so yourself - the
watcher dispatches that action rather than being a second way in. Colours, fonts, the cursor,
the keymap, `[text]`, `option_as_alt`, `resize_step` and `scroll_multiplier` all take effect
where they are, including in panes that were already open; `pane_padding` reaches panes opened
afterwards, because that is as far as the renderer takes it. `[shell]` and `scrollback_bytes`
reach panes opened afterwards too, and for the same shape of reason: the daemon takes both when
it builds a pane, so a pane you are already typing in keeps what it was made with.

The exception is `[[daemon]]`. Which machines a window is attached to is a question about live
sessions rather than about settings, and answering it on a save would move panes somebody is
working in - so a change there is read, noticed, and reported as still wanting a relaunch. A
file that will not parse changes nothing at all, which means an editor that saves halfway
through a thought cannot leave you running half a config.

**A refused file says so at the foot of the agent list**, in the words the refusal itself used,
naming the value and what to write instead. The list opens itself if you had it closed, and
closes again when the last problem clears - a window too narrow for the list at all puts
`· 1 problem` in its title instead. Waving the box away leaves a count rather than silence:
what is outstanding is a fact about your file and not a message you have read, so it goes when
the file parses and not when you dismiss it. That disappearance is how you know a save was
accepted.

Opening a list you closed is the one liberty Muster takes with your window, and this is what it
buys. Before it, a refused config went to the run log and nowhere else: you could break your
keymap at six in the evening, work all night on default bindings, and never be told - not when
it broke, and not when a later save fixed it.

**A pane that never becomes typeable appears in the same place, and that one is not your
fault.** Your keystrokes reach a pane through a bridge Muster starts and waits to hear from,
and until it does the pane renders, paints, and throws away everything you type. Five seconds
of that is a problem naming the pane and where to look, so you are told rather than left to
discover it by typing into something that stopped listening. It clears itself if the bridge
turns up late, and it goes with the pane if you close it.

`~/.muster/state/` is Muster's to write, and holds three files nobody should edit. `window.toml`
is rewritten whenever the window settles: which tabs it was showing, in what order, at what
widths, and under `[window]` whether the agent list was open, how far the text was sized from
what the config file asked for, and how big the window itself was. Delete it and the next launch
opens fresh. Nothing about a session is in it - what a tab holds is the daemon's answer, asked
again on every launch.

**A window opens at the size and position it was left, and full-screen if that is how you left
it.** The rectangle is written down as the window settles rather than at quit, because quitting
is not how this is usually lost: a crash, a reboot or a stray `kill` costs the same thing, and
the tabs already survive all three. The four numbers are always the size the window goes back to
on the way out of full-screen rather than the display it filled, so leaving full-screen returns
you to the window you had.

The display it was measured on may be smaller now, or unplugged, or arranged somewhere else, so
the rectangle is checked the way a saved tab is. A window whose title bar still lands on some
screen opens exactly where it was, including one deliberately dragged half off the side. One
whose title bar does not - a window saved on a desk monitor and reopened on a laptop alone - is
brought onto the screen it has most in common with, clamped to fit and centred. Nothing on
screen gets you out of a window you cannot grab, which is why that case is worth the move.

Text size is the one appearance setting that is also an action. `cmd+=`, `cmd+-` and `cmd+0`
size every pane in the window at once - not one pane, because a grid you read at a glance is
harder to read with ragged cell sizes - and the size you land on is remembered under `[window]`
and comes back on the next launch. `cmd+0` is the way back to whatever `[font] size` says.

`libghostty.conf` is `[font]`, `[colors]` and `[cursor]` restated in the renderer's own
format, because libghostty has no way to be handed a value except as a file. Rewritten every
launch, so editing it changes nothing - but reading it answers "what did Muster actually tell
the renderer", which is the first question when a colour does not take.

`herdr.toml` is the same arrangement for the daemon: `[shell]` and `scrollback_bytes` in
herdr's own format, plus the update checks Muster turns off, handed over by name so it moves
which file that daemon reads without moving the socket it listens on. Reading it answers "what
did Muster actually tell the daemon", which is the first question when a pane opens the wrong
shell. It is written even when you have configured nothing, because the update checks are
Muster's answer rather than yours - and your own `~/.config/herdr/config.toml` is untouched,
still read by your own herdr, and handed back to every pane Muster opens so that `herdr` typed
inside one reads what it always did.

## Driving Muster

Everything a chord does, `muster` does. `muster window` says what a window is showing and what
every agent in it is doing, `muster pane new` makes a pane, and `muster pane send` types into one
by name - all of it the same requests the keyboard sends, because there is one path through the
core and this is the other door into it.

    muster window
    muster pane new --down --run claude --name "🤖 A"
    muster pane send --pane p1w3r07bsd "read AGENTS.md and wait" --enter
    muster tab focus t1w3r07bsd
    muster window --json

**Tabs are named too, and for a narrower reason than panes.** `t1w3r07bsd`, minted by Muster and
unique across every machine a window shows, so `muster tab focus` and `muster tab rename` reach one
without saying which daemon holds it. What a tab does not get is a name in any pane's environment:
nothing has to tell a tab which tab it is, so there is no `$MUSTER_TAB`, and a script that means the
tab it is sitting in reads that out of `muster window` - where every pane says which tab holds it.

`muster docs` is the reference and it ships inside the binary, so it describes the version you are
running. `muster --help` has the grammar, `muster completions zsh` writes a completion script.

Every pane Muster makes can drive the window it is drawn in without being set up first: the
command is on its `PATH` from `~/.muster/bin`, `$MUSTER_PANE` says which pane it is, and
`$MUSTER_SOCKET` says which window to tell. So `muster pane new` inside a pane splits that pane,
and an agent told "split two panes below you and start an agent in each" can do it. Add
`~/.muster/bin` to your own `PATH` for terminals outside Muster; it holds a link to the command
belonging to the running app, refreshed at every launch.

Muster imposes no workflow. These are primitives, and `extras/skill/SKILL.md` is a Claude Code
skill that points an agent at them and nothing more.

## Building

`./dev` is the only supported way to build, test, and lint. With no flags it takes the full gate, and
`.github/workflows/gate.yml` runs that one command on every push and pull request - so a contributor's green and a
merge gate's green cannot drift apart. Flags narrow it and cluster: `./dev -t` tests, `./dev -tl` tests and lints,
`./dev -h` lists them all.

**A narrowed flag still takes what it cannot run without**, so `./dev -t` on a checkout nothing has been built in
fetches libghostty and generates the seam's types before running anything. Both are near-free once they are there -
a stamp read and four path checks - and the alternative was worse than slow: the seam's Swift types are generated
during a build and committed nowhere, so a suite that skipped it ran the shell against whatever was generated last
and went green while the schema said something else.

`./dev --bundle` assembles `.build/muster.app` around the built binary - a thing you can double-click, keep in the
Dock, or hand to somebody, with the pinned herdr and both dylibs inside it. Out of the gate because nothing in the
gate needs one, and it is also the only way to meet the descriptor ceiling launchd imposes on a GUI-launched process.

`./dev --contract` is the exception that stays out of the gate. It launches the real app against a real herdr and
reads its run log to see what connected, so it needs a daemon on PATH and a logged-in GUI session - neither of which
the default suite is allowed to require.

`./dev --ssh` is the remote tier, and sits out of the gate for the same reason: it starts the devenv container and
proves a forwarded socket is a socket - a real ssh master to a real machine running a real daemon - which needs
docker rather than a GUI session.

`./dev --perf` and `./dev --latency` are the other two out-of-gate tiers: the first measures the per-unit budgets
against a checked-in baseline and fails on regression, the second times input-to-glyph against a real daemon, at one
pane and at a full window of fifteen with fourteen of them printing. A functional green is never a performance
claim, so neither runs by default.

Two toolchains, one door: the gate builds, tests and lints the Rust core and the Swift shell together, and a suite
that discovers zero tests fails in either language rather than reporting green.

Requires a Swift 6.2 toolchain, [Rad](https://github.com/amterp/rad), and Zig 0.16 on your PATH; a missing `rad`
shows up as `env: rad: No such file or directory`. Rust installs itself - `rust-toolchain.toml` pins the version and
rustup fetches it on the first `cargo` call, the same way `deps/ghostty.pin` decides which libghostty gets built.
libclang, which the libghostty-vt bindings are generated with, comes from the Xcode command line tools.

`deps/rad.pin` names the Rad the gate is written against, and is the one pin `./dev` cannot act on - it is already
running under Rad by the time it could look. CI installs exactly that build; your own `rad` is free to be any
version, and one too old to parse `./dev` says which line it could not read.

herdr is not something you install. Muster ships one: `deps/herdr.pin` names a release and a checksum per platform,
`./dev` downloads that binary into `deps/herdr/<version>/` once and verifies it, and every place that needs a daemon
gets that one - the tests spawn it, a build stages it beside the app, and `./dev --bundle` puts it inside
`muster.app`, where the app starts it on a socket of its own. Deliberately **not** your PATH, and deliberately not
the socket your own herdr uses: a Muster talking to a daemon its corpus was never recorded against is a window whose
every behaviour is unverified. So the herdr you run for your own work stays whatever version you want, a suite that
passed did so against the daemon the corpus was recorded with, and the app never meets either.
`MUSTER_HERDR=/path/to/herdr` overrides it for anyone
bisecting herdr itself, and the run says so when it does. A daemon whose wire schema differs from
`corpus/herdr-<version>/api-schema.json` fails the run with the command that shows what moved, rather than
surfacing later as a confusing test failure. The download is the one step that touches the network; everything
after it is offline.

The seam's types are generated from `proto/muster.proto` on both sides and committed on neither, so a checkout
cannot hold a shell and a core that disagree. Neither generator is a thing you install: Rust compiles the schema
with a Rust library, and swift-protobuf vendors protoc's own source, so `./dev` builds it. `./dev --proto`
regenerates on demand; a normal build does it only when the schema's hash changes.

## Repo conventions

- `docs/`: `origin.md` is the founding story, frozen as history; `architecture.md`, `testing.md`, and `glossary.md`
  are living doctrine; `docs/mip/` holds MIPs, the rare large decisions; `docs/observations/` records what a
  dependency was measured doing, one file per version, each claim citing raw transcripts in `corpus/`. Routine
  rationale lives in commit messages; open questions live in the kan board's `uncommitted` column. `docs/cli/` is
  the reference `muster docs` ships inside the CLI binary, so a file there is prose the gate checks is reachable.
- `extras/` holds things that are Muster-adjacent rather than Muster: today one Claude Code skill pointing an agent
  at `muster docs`.
- `corpus/`: what the code is judged against, in no language. `conformance/` holds the cases that define the core's
  behavior, `snapshots/` the rendered oracles too broad to be cases, and the rest raw transcripts recorded from a
  real dependency. The gate fails if a file here is checked in and never run.
- `crates/` is the portable core (Rust), `Sources/` the macOS shell (Swift), and `proto/muster.proto` plus
  `include/muster.h` are the seam between them. Both languages are built, tested and linted by `./dev`.
- Work is tracked on the in-repo [kan](https://github.com/amterp/kan) board (`.kan/`, `kan list`). Agents keep it
  current: move cards as work starts and finishes, and add cards for work discovered along the way. If finishing work
  empties the `next` column, refill it from the backlog by priority and natural sequencing, so there is always a
  scoped next thing to pick up.

## Contributing

- Keep ./docs and this README.md up to date with changes. Don't inflate them needlessly, be judicious.
- Use Conventional Commits for commits.
