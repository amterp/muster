# Configuring Muster

Everything Muster owns lives in `~/.muster`, and `MUSTER_HOME` moves the lot. Two files in
it, both optional, both TOML. `~/.muster/config.toml` is yours to write, and it is the only
file Muster reads: `[[daemon]]` blocks name the machines a window attaches to, `[keymap]`
rebinds any of Muster's own actions, `[font]`, `[colors]` and `[cursor]` decide what the
window looks like, `[notifications]` decides which agents interrupt you, and the rest decides
what a keystroke becomes on its way to a pane.

One directory rather than a file in each of the XDG trees, because Muster's surface is meant
to be discovered rather than taught - an agent that can list one directory needs no
documentation to find the whole of it. `XDG_CONFIG_HOME` and its family still decide where
herdr listens; they no longer move anything of Muster's, and they no longer decide what
Muster's own daemon reads.

```toml
option_as_alt = "left"         # never (the default) | always | left | right
resize_step = "20c"            # per resize chord: cells (c) or points (px). Omit for the
                               # daemon's own step. The unit is required.
scroll_multiplier = 1.5        # scales what the trackpad or wheel reported
numbered_chords = "panes"      # panes (the default) | tab_then_pane. A prototype; see below
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
bold = "#e5c07b"              # bold text; omit and it keeps whatever colour it already had
divider = "#4a4a4a"            # the line between two regions; omit for the platform's
focus_ring = "#bb9af7"         # which pane has the keyboard; omit to follow the macOS accent
agent_working = "#7aa2f7"      # the five agent states, on a pane's edge and its row's dot
agent_blocked = "#ff9e64"      # each optional on its own; omit for the one Muster ships
agent_done = "#9ece6a"
agent_idle = "#565f89"
agent_unknown = "#3b4261"
palette = [                    # the ANSI sixteen, all of them or none
  "#000000", "#cc0000", "#4e9a06", "#c4a000",
  "#3465a4", "#75507b", "#06989a", "#d3d7cf",
  "#555753", "#ef2929", "#8ae234", "#fce94f",
  "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
]

[cursor]
style = "block"                # block | bar | underline | hollow
blink = true                   # omit to let the program in the pane decide

[notifications]
blocked = true                 # an agent waiting on you
done = true                    # an agent that finished while nobody was looking
muted = false                  # silences both, without forgetting which you wanted
```

`[keymap]` is partial, so a file that names one action rebinds one action. Chords are
modifiers and a key, in any order and any case, spelled the way you would say them: `cmd`,
`opt`, `ctrl`, `shift`, and `left`, `return`, `f5`, `[`. The actions are `new_window`, `reopen_window`, `new_tab`,
`next_tab`, `previous_tab`, `split_*` for each direction, `close_pane`, `next_pane`,
`previous_pane`, `focus_*` and `resize_*` for each direction, `focus_pane_1` to
`focus_pane_9`, `rename_pane`, `rename_tab`, `close_tab`, `move_pane_to_new_tab`, `find`,
`find_next`, `find_previous`, `zoom`,
`increase_font_size`, `decrease_font_size`, `reset_font_size`, `toggle_sidebar`,
`reload_config`, `show_shortcuts`, and `quit_and_close_sessions`. On macOS these become menu items, which is where the
platform dispatches a key equivalent from - so a rebound action moves in the menu too, and
System Settings can move it again.

Seven of them ship with no chord at all. Ghostty has `split_left` and `split_up` as actions and
binds neither, so Muster does the same rather than inventing a shortcut for them - they are in
the menu, one click away and one `[keymap]` line from a chord. Which of the four sides costs
herdr one request and which costs two is not something you can tell from here, and that is the
point: herdr splits rightward and downward only, so the other two are a split and a rearrange,
and Muster takes the arrangement from the daemon's own answer rather than letting you watch the
pane move.

`rename_tab` is the third, and that one is Muster's own call rather than Ghostty's: you name a
tab once and a pane several times an hour, so the chord goes to `rename_pane` (`cmd+shift+n`)
and the tab keeps a menu item.

`move_pane_to_new_tab` is the fourth, on the same reasoning and one more of its own: it is the
newest of them, and a chord invented for an action nobody has asked to reach by keyboard is a
chord taken away from whatever wants it later. It takes the pane the keyboard is on out of its
split and gives it a tab of its own, in one request - the CLI's `pane move --new-tab` is the
same act with a name for the tab.

`close_tab` is the fifth, and that one is unbound because of what it does rather than because
nobody has asked for it: it ends every pane in the tab, and a chord that destroys several panes
is one somebody reaches by accident. `cmd+w` stays on `close_pane`, where the damage is one
pane and the muscle memory is everybody's.

`reopen_window` is the sixth. `cmd+shift+t` is what a browser puts on reopening a *tab*, and
Muster's tabs belong to the daemon and were never gone - so binding it to a window would make
one keystroke mean two things across two apps. The menu carries it, and `muster window reopen`
is the same act from a script.

`quit_and_close_sessions` is the seventh, and it is the only one unbound for safety rather
than for parity. Quitting Muster leaves every session running - that is the whole promise, and `cmd+q`
does it - and this is the other answer, for when you are finished for the day and want the
agents to stop too. It asks first, naming every machine and the directories its panes are in,
because it is the one thing in Muster that ends somebody's work. Bind it if you want to, and
know that everything else here is undone by doing it again and this is not.

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

**`numbered_chords = "tab_then_pane"` is a prototype of the other answer, and may be deleted
rather than finished.** Under it `cmd+2` goes to the second tab, and the `cmd+2` after it goes
to that tab's second pane. Everything above stays true and stays the default; this is here
because the argument above is one nobody can settle by reading it, and a day of driving the
other scheme is the only thing that will.

**Hold the modifier to reach a pane, and let go to stop at the tab.** `cmd+2` moves you to the
second tab the moment you press it. Keep `cmd` down and press `3`, and you land on that tab's
third pane. Release `cmd` instead and the sequence is over, so `cmd+2` then `cmd+3` is two tab
jumps rather than a tab and a pane. That is what makes this something other than a prefix key:
you cannot be left in a mode you have forgotten about, because the mode lasts exactly as long
as your thumb is down. Whichever modifier the nine chords are bound with is the one that ends
it, so rebinding them to `ctrl+1` moves that too.

Everything else that ends a sequence still ends it - a keystroke into a pane, another chord, a
click, `Escape`. Nothing times out. And a tab holding a single pane does not start one at all:
`cmd+2` onto it lands on the only pane it has and stops there, because there is nothing inside
to choose between.

**A window holding one tab numbers panes, the way the default does.** Naming the only tab
there is spends a press on nothing, so with one tab `cmd+2` reaches the second pane in one
press. This is not a third scheme: with one tab, a pane's place down the window and its place
inside that tab are the same number. The moment a second tab appears anywhere in the window -
you make one, or you attach a machine that brings its own - `cmd+2` means the second tab
again, and every number in the agent list moves onto the tab captions as it happens. That the
chord changes meaning under you is the real cost of this, and it is why the numbers are drawn
where they are: what `cmd+2` will do is something you read rather than remember.

While it is on, the numbers in the agent list move. At rest they sit on the tab captions, and
once a chord has named a tab they sit on that tab's panes and nowhere else - so what `cmd+2`
will reach is something you read rather than something you remember, and only one thing in the
window carries numbers at any moment. Both kinds of row keep space for a digit whether or not
they have one, so the list holds still while the numbers move around it, and the numbers you
can press next are drawn in the accent colour rather than grey.

**The panes say their own numbers too.** Hold the modifier after a first press and each pane in
the tab draws its number over itself, large and half-transparent, so you pick between the panes
by looking at them rather than by reading a list at the edge of the window. They are transparent
to the mouse: clicking the number you can see focuses the pane under it, the same as clicking
anywhere else in it. With the agent list closed they are the only indicator, which is the case
the list could never cover.

They wait about a tenth of a second first, which is what keeps a tab jump you make and finish
in one motion from flashing them on the way past.

A zoomed tab is the rough edge. It still starts a sequence when it holds several panes, and you
will see one number, because only one pane is on screen to draw one.

The nine actions do not move: `focus_pane_3` is still what `[keymap]` names and still what the
menu carries, and under this scheme it means the third numbered chord rather than the third
pane. Renaming nine actions for a prototype is the thing that would make it expensive to take
out again.

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
designed. `divider`, `focus_ring` and the five `agent_*`
sit with the pane colours even though Muster rather than the renderer paints them, because you
pick colours all at once and which piece of code holds the brush is not something you should
have to know.

**`bold` is the one appearance setting that changes how readable an agent is.** A terminal
paints bold text in whatever colour the text already had, and a harness writes `**bold**` with
nothing else distinguishing it - so a Claude pane reads flat until you give bold a colour of its
own. Omitting it is the behaviour every terminal has by default.

**The colours Muster invented are yours too, and only in the window.** `agent_working` and its
four siblings are what a pane's edge and its row's dot are painted in; `focus_ring` is the thin
inner ring saying which pane the keyboard feeds. Each is optional on its own - fixing the one row
you cannot see is not adopting a theme - and leaving one out gives you the colour Muster ships.

Leaving `focus_ring` out follows the macOS accent, which is a decision rather than a shortfall:
the accent is the platform's own answer to which thing has focus, and it already tracks a choice
you made in System Settings. The two rings are told apart by weight and a gap rather than by
hue, so whatever your accent is, focus still reads as focus.

**`muster window` keeps its own sixteen and honours none of this.** A terminal has sixteen
colours and a hex triple is not one of them, so the alternative was mapping your colour onto the
nearest slot - a judgement that would be wrong for somebody, and Muster would no longer know what
the legend was. Instead the CLI paints the default legend on everybody's machine, which for an
agent reading it is a feature. `docs/cli/window.md` says the same thing from the other side.

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

**`[notifications]` is what interrupts you, and both states are on.** `blocked` is an agent
waiting on you; `done` is an agent that finished while nobody was looking, which is what
Muster's `done` already means - the state and the notification are asking the same question.
Activating one takes you to the pane that raised it, including a pane no split is showing.

A pane you are already looking at never notifies. That is what its border is for, and a
banner about something on your screen is the fastest way to learn that banners are noise.

`muted = true` is the quiet path for somebody running fifteen agents, and it is a third key
rather than setting the other two to `false` so that going quiet for an afternoon does not
cost you the two answers underneath it. Saving the file is enough: what a mute silences comes
off your screen at the moment you write it. Switching one back on does not bring back what it
silenced, which is deliberate - a notification is about the moment an agent started waiting,
and one that has been waiting ten minutes is already on its row.

Muster asks for permission once, on the launch after you first install it, and macOS
remembers the answer - System Settings > Notifications > Muster is where to change it. A
Muster run as a bare binary out of `.build` has no bundle identifier to be granted permission
against and notifies nothing; it says so in the run log, and `./dev --bundle` is the fix.

`[shell]` and `scrollback_bytes` are the two Muster does not act on at all. What a pane runs
and how much of it you can scroll back through belong to the daemon that makes the pane - so
Muster translates them into a file of its own and hands that to the daemon, exactly as it does
`[font]` and `[colors]` for the renderer. Before this you had to learn that herdr existed and
find its config file, and a `default_shell` set for your own terminal quietly decided what
every Muster pane ran.

`scrollback_bytes` is bytes because that is what the buffer is measured in; a line has no
fixed size, so a count of them would be a number that did not mean what it said. Zero is a
real answer - a pane that keeps only what is on screen. What is deliberately *not* offered is
the daemon's version check: Muster ships one herdr, pinned by version and checksum, and turns
that check off. A daemon that could be told to go and fetch a different version of itself would
make "this was tested against the daemon it ships with" mean nothing.

**herdr's other update check, the one for its agent-detection manifests, is left on**, and the
difference between the two is worth stating because Muster used to turn both off together. A
version names the binary this project records a corpus against. A manifest is data describing
how somebody else's agent looks on screen, and those agents change on their own schedule -
Claude Code moved its busy spinner from a Braille character to a half-circle, the one rule in
herdr's bundled manifest that can produce `working` matches Braille, and so the dot could no
longer say that an agent was working at all. Eleven agent transitions over a day of real use on
two machines, and `working` was not among them. herdr had published a corrected manifest two
days before anyone noticed, and Muster's config file was what stopped it arriving.

So a daemon Muster starts fetches manifests the way herdr would on its own. The cost is that
detection rules can move under a build that was tested against different ones, and that a
daemon start reaches the network. The suite is unaffected: the daemons it runs are given their
manifests up front and check for none, so a recorded corpus is still judged against frozen
rules. A daemon that has already started keeps the manifests it loaded, so a machine picks a
new set up when its daemon next restarts rather than immediately.

**Both reach a devenv pane too, and so does the pinned daemon itself.** A `[[daemon]]` with a
`host` used to attach whatever herdr somebody had installed over there, at whatever version and
with whatever settings, so a window's two halves could disagree about a setting you wrote once.
On attach Muster now asks that machine what platform it is, downloads the release its own pin
names for it, verifies the checksum, and copies it across the connection it already has open -
to `~/.muster/herdr/<version>/herdr`, with your settings in `~/.muster/state/herdr.toml` beside
it. You install nothing over there. A machine you have attached before is quicker rather than
different: the download is kept in `~/.muster/cache`, and a daemon still running from last time
is reused, agents and all.

Downloaded here rather than over there, because the machine running Muster demonstrably has web
access and a devenv often has none. A checksum that does not match is a refusal: the whole point
of the pin is that the daemon is the one everything was tested against. And naming a `socket` in
a `[[daemon]]` block still attaches whatever is listening at it, on either machine - that is how
you ask for somebody else's daemon on purpose.

**Saving the file is enough.** Muster watches it and reads it again, and `cmd+shift+,` or
Reload Configuration asks for the same thing when you would rather say so yourself - the
watcher dispatches that action rather than being a second way in. Colours, fonts, the cursor,
the keymap, `[text]`, `option_as_alt`, `resize_step`, `scroll_multiplier`, `numbered_chords`
and `[notifications]` all take effect where they are, including in panes that were already open; `pane_padding`
reaches panes opened afterwards, because that is as far as the renderer takes it. `[shell]` and `scrollback_bytes`
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
widths, under `[window]` whether the agent list was open and how big the window itself was, and
one `[[pane]]` row for each pane whose text somebody sized. Delete it and the next launch opens
fresh. Nothing about a session is in it - what a tab holds is the daemon's answer, asked again on
every launch.

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
size the pane you are in and leave the rest alone, and the size you land on is remembered for
that pane under `[[pane]]` and comes back on the next launch. `cmd+0` is the way back to
whatever `[font] size` says, for that pane; `[font] size` itself is what moves all of them.

**One pane, because the panes are not doing the same job.** The claim is a grid you read at a
glance, and a grid with ragged cell sizes is harder to read - so this sized the whole window
for a while, on the argument that the raggedness was the cost. It is the other way round: the
grid is fifteen agents, one of which you are reading closely while the rest you are watching
for a colour, and being able to say which is worth more than the tidiness. A pane a split makes
opens at the size of the pane it was split from, so growing one and splitting it gives you two
you can read.

`libghostty.conf` is `[font]`, `[colors]` and `[cursor]` restated in the renderer's own
format, because libghostty has no way to be handed a value except as a file. Rewritten every
launch, so editing it changes nothing - but reading it answers "what did Muster actually tell
the renderer", which is the first question when a colour does not take.

`herdr.toml` is the same arrangement for the daemon: `[shell]` and `scrollback_bytes` in
herdr's own format, plus the version check Muster turns off, handed over by name so it moves
which file that daemon reads without moving the socket it listens on. Reading it answers "what
did Muster actually tell the daemon", which is the first question when a pane opens the wrong
shell. It is written even when you have configured nothing, because the version check is
Muster's answer rather than yours - and your own `~/.config/herdr/config.toml` is untouched,
still read by your own herdr, and handed back to every pane Muster opens so that `herdr` typed
inside one reads what it always did. A daemon on another machine gets the same file, written to
`~/.muster/state/herdr.toml` over there, and that machine's panes are handed that machine's own
herdr config back for the same reason.

`~/.muster/cache/` is the other directory Muster writes, and holds what it downloaded - today,
one herdr per platform you attach a remote daemon on. Delete it and the next such attach fetches
again, which costs about 18 MB and nothing else.
