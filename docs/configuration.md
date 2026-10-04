# Configuring Muster

Everything Muster owns lives in `~/.muster`, and `MUSTER_HOME` moves the lot. Two files in
it, both optional, both TOML. `~/.muster/config.toml` is yours to write, and it is the only
file Muster reads: `[[daemon]]` blocks name the machines a window attaches to, `[keymap]`
rebinds any of Muster's own actions, `[font]`, `[colors]` and `[cursor]` decide what the
window looks like, `[notifications]` decides which agents interrupt you, and the rest decides
what a keystroke becomes on its way to a pane.

One directory rather than a file in each of the XDG trees, because Muster's surface is meant
to be discovered rather than taught - an agent that can list one directory needs no
documentation to find the whole of it. `XDG_CONFIG_HOME` and its family move nothing of
Muster's.

The run log is the one exception: it goes to `~/Library/Logs/muster/`, where macOS keeps app
logs, until `MUSTER_HOME` is set, which moves it to `logs/` in that home with everything else.

```toml
option_as_alt = "left"         # never (the default) | always | left | right
resize_step = "20c"            # per resize chord: cells (c) or points (px). Omit for the
                               # daemon's own step. The unit is required.
scroll_multiplier = 1.5        # scales what the trackpad or wheel reported
hide_pointer_while_typing = true  # until the mouse moves; off by default, as in Ghostty
clipboard_write = "deny"       # allow (the default) | deny: may a program set the clipboard
name_sessions = false          # the default true: naming a pane names its agent's session
human_name = "Alex"            # what messages call you beside @human; omit for your login name
pane_padding = 2               # points between a pane's text and its edges; 0 fits the most rows
scrollback_bytes = 50000000    # history a pane keeps; omit for the daemon's own answer

[[daemon]]
id = "devenv"                  # Muster's name for the machine, in the agent list and the logs
host = "devenv"                # an ssh destination; omit for a daemon on this machine
color = "#4a90d9"              # its swatch in the agent list; omit for one drawn from its id

[shell]
command = "/opt/homebrew/bin/fish"  # omit for whatever this machine thinks your shell is
mode = "login"                 # auto (the default) | login | non_login
ssh_terminfo = true            # the default: ssh gives a host the terminal's terminfo entry
ssh_env = true                 # the default: ssh tells a host the terminal's name and colors
sudo = false                   # the default: sudo keeps the terminal's terminfo; needs SETENV

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
agent_working = "#7aa2f7"      # the six agent states, on a pane's edge and its row's dot
agent_blocked = "#ff9e64"      # each optional on its own; omit for the one Muster ships
agent_waiting = "#7c7fd8"      # idle, and waiting on work it started itself
agent_done = "#9ece6a"
agent_idle = "#565f89"
agent_unknown = "#3b4261"
context_empty = "#565f89"      # the ring beside a row, with none of the context used
context_full = "#f7768e"       # and with all of it; the ring moves between the two as it fills
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
programs = true                # a program in a pane asking to notify you (OSC 9, OSC 777)
messages = true                # a message for you from an agent (`muster docs msg`)
done = true                    # an agent that finished while nobody was looking
muted = false                  # silences all four, without forgetting which you wanted
```

`[keymap]` is partial, so a file that names one action rebinds one action. Chords are
modifiers and a key, in any order and any case, spelled the way you would say them: `cmd`,
`opt`, `ctrl`, `shift`, and `left`, `return`, `f5`, `[`. The actions are `new_window`, `reopen_window`, `close_window`, `new_tab`,
`next_tab`, `previous_tab`, `split_*` for each direction, `close_pane`, `next_pane`,
`previous_pane`, `focus_*` and `resize_*` for each direction, `numbered_chord_1` to
`numbered_chord_9`, `focus_asking`, `focus_back`, `focus_forward`, `rename_pane`, `rename_tab`, `close_tab`, `move_pane_to_new_tab`, `find`,
`find_next`, `find_previous`, `zoom`, `equalize_panes`, `scroll_to_top`, `scroll_to_bottom`, `scroll_page_up`,
`scroll_page_down`, `jump_to_previous_prompt`, `jump_to_next_prompt`, `select_all`,
`clear_screen`, `reset_terminal`, `increase_font_size`, `decrease_font_size`, `reset_font_size`, `toggle_sidebar`,
`reload_config`, `show_shortcuts`, and `quit_and_close_sessions`. On macOS these become menu items, which is where the
platform dispatches a key equivalent from - so a rebound action moves in the menu too, and
System Settings can move it again.

**Ghostty's own binding actions are here, on Ghostty's macOS chords.** Scrolling to the top
or bottom (`cmd+home`, `cmd+end`) and by a page (`cmd+pageup`, `cmd+pagedown`), jumping to the
prompt above or below (`cmd+shift+up`, `cmd+shift+down`, which needs Ghostty's shell integration
to know where prompts are) and `select_all` (`cmd+a`) happen in the pane's surface, which holds
its history. `clear_screen` (`cmd+k`) and `reset_terminal` (unbound, as in Ghostty) are carried
out by the pane's daemon, which holds its terminal: clear_screen drops the history and has a
shell at its prompt draw it again, and on the alternate screen it does what Ghostty does - hands
the key to the program. `equalize_panes` (`cmd+ctrl+=`) is Ghostty's `equalize_splits`: every pane
in the tab comes out the same size, which is what `muster pane resize --equalize` does. Ghostty's
`text:`, `csi:` and `esc:` are `[text]` here. The rest of
Ghostty's actions are about its own windows, tabs and clipboard, and are not offered; a
`[keymap]` line naming one is refused, and names Muster's equivalent where there is one.

**Both renames have a chord.** `rename_pane` is `cmd+shift+n` and `rename_tab` is
`cmd+shift+r`, R for rename. Neither is Ghostty's, and Ghostty's macOS chords leave both alone.
`rename_tab` used to ship unbound, on the theory that a tab is named once and a pane several
times an hour; tabs turned out to be renamed often enough to want one.

Seven of them ship with no chord at all. Ghostty has `split_left` and `split_up` as actions and
binds neither, so Muster does the same rather than inventing a shortcut for them - they are in
the menu, one click away and one `[keymap]` line from a chord.

`move_pane_to_new_tab` is the third, and that one is Muster's own call rather than Ghostty's: it
is the newest of them, and a chord invented for an action nobody has asked to reach by keyboard is a
chord taken away from whatever wants it later. It takes the pane the keyboard is on out of its
split and gives it a tab of its own, in one request - the CLI's `pane move --new-tab` is the
same act with a name for the tab.

`close_tab` is the fourth, and that one is unbound because of what it does rather than because
nobody has asked for it: it ends every pane in the tab, and a chord that destroys several panes
is one somebody reaches by accident. `cmd+w` stays on `close_pane`, where the damage is one
pane and the muscle memory is everybody's.

`reopen_window` is the fifth. `cmd+shift+t` is what a browser puts on reopening a *tab*, and
Muster's tabs belong to the daemon and were never gone - so binding it to a window would make
one keystroke mean two things across two apps. The menu carries it, and `muster window reopen`
is the same act from a script.

`quit_and_close_sessions` is the sixth, and it is the only one unbound for safety rather
than for parity. Quitting Muster leaves every session running - that is the whole promise, and `cmd+q`
does it - and this is the other answer, for when you are finished for the day and want the
agents to stop too. It asks first, naming every machine and the directories its panes are in,
because it is the one thing in Muster that ends somebody's work. Bind it if you want to, and
know that everything else here is undone by doing it again and this is not.

`reset_terminal` is the seventh, as it is in Ghostty: it throws away the pane's screen and modes,
and a chord for that is one somebody finds by losing their screen.

**The mouse's back and forward buttons go back to the pane you were on, and forward again.**
Every pane the keyboard lands on is recorded, however it got there, so a step back can change
tabs or machines, and a pane that has closed is stepped over. The buttons work wherever the
pointer is in the window, and they go to Muster even over a program that uses the mouse.
`focus_back` and `focus_forward` are the same walk on `cmd+opt+[` and `cmd+opt+]`, beside
`cmd+[` for panes and `cmd+shift+[` for tabs.

**`cmd+1` to `cmd+9` go to a tab, and the press after one goes to a pane inside it.** `cmd+2`
moves you to the second tab the moment you press it. Keep `cmd` down and press `3`, and you land
on that tab's third pane. A tab's number is its place in the window's tab order, counted across
every machine, and a pane's is its place inside its tab.

**Let go of the modifier to stop at the tab.** Release `cmd` after `cmd+2` and the sequence is
over, so `cmd+2`, let go, `cmd+3` is two tab jumps rather than a tab and a pane. The sequence
lasts exactly as long as your thumb is down, so you cannot be left in a mode you have forgotten
about. Whichever modifier the nine chords are bound with is the one that ends it, so rebinding
them to `ctrl+1` moves that too.

Everything else that ends a sequence still ends it - a keystroke into a pane, another chord, a
click, `Escape`. Nothing times out. And a tab holding a single pane does not start one at all:
`cmd+2` onto it lands on the only pane it has and stops there, because there is nothing inside
to choose between.

**A window holding one tab numbers its panes.** Naming the only tab there is spends a press on
nothing, so with one tab `cmd+2` reaches the second pane in one press - with one tab, a pane's
place in the window and its place inside the tab are the same number. The moment a second tab
appears anywhere in the window - you make one, or you attach a machine that brings its own -
`cmd+2` means the second tab again, and every pane row in the agent list grows a second digit as
it happens. That the chord changes meaning under you is the real cost of this, and it is why
the chords are drawn beside the rows at all: what reaches an agent is something you read rather
than remember.

**Every row in the agent list shows the whole chord that reaches it.** A pane in the second tab
reads `2 1`, and pressing those two is how you get there - including when you are looking at
another tab entirely, which is the case this list exists for. The pane digit is the one that
varies down a group, so it is drawn brighter and the tab digit in front of it quieter. A row
nothing reaches shows nothing: a tenth pane in a tab has no second press, and its tab's digit
alone would be a keystroke that lands on that tab's first pane instead. A tab holding one pane
is the other way round - reaching the tab is the whole chord, so its pane reads `2` and `2 1`
would be the first tab.

Nothing in that moves as you type. Press `cmd+2` and the digits stay exactly where they are;
what changes is that the second digit down that tab's panes turns the accent colour, because
those are the keystrokes your hand can make while the modifier is still down. Every other row
stays grey, including the tab captions - so `cmd+2` momentarily not meaning "the second tab" is
said by where the colour is rather than by taking a digit away.

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

The numbers are positions, so they move when a tab or pane before them opens or closes. That is
the cost of numbering things that churn, and it is the right way round: the order is yours to
arrange, and a number that stayed put when you moved its row would be fighting you. Past nine
they run out, and a tenth tab, or a tenth pane in a tab, is reached by `next_tab`, `next_pane`,
a direction, or clicking its row.

The two ways of moving are still different axes. `next_pane` and the four directions reach
every pane the window is **showing**; `next_tab` and `previous_tab` walk the tabs behind
those, including ones no region has on screen.

The nine actions are `numbered_chord_1` to `numbered_chord_9` in `[keymap]`, and Tab or Pane 1
to 9 in the Tab menu. `muster focus --place 3` is not one of them:
it goes to the pane `muster window` prints at place 3, counted down the whole window, because a
script reading that number has to be able to hand it back.

**`numbered_chords` is gone.** It chose between this and an older scheme, where `cmd+3` was the
third pane down the whole agent list. A file still saying `numbered_chords = "tab_then_pane"`
loads, because that is what Muster now does, and the line can be deleted. One saying `"panes"`
is refused, and the whole file with it: silently turning `cmd+3` from the third pane into the
third tab is the one outcome worse than the refusal. The actions' older names, `focus_pane_1` to
`focus_pane_9` and before them `focus_tab_1` to `focus_tab_9`, are refused too, and the refusal
gives the new name, so `focus_pane_3` is one rename away from `numbered_chord_3`.

**Two things cannot hold one chord, and the file is refused rather than one of them losing.**
Three ways that happens: two `[keymap]` actions on the same chord, a `[keymap]` action on a
chord `[text]` also sends bytes for, and a `[keymap]` action on one of the five chords every
pane uses for line editing - `cmd+left` and `cmd+right` for the ends of a line,
`cmd+backspace` to delete to the start of it, and `opt+left` and `opt+right` for word motion.
All three resolve the same way if allowed, and the refusal exists because of how: on macOS an
action is a menu item, and the menu is offered a key equivalent before the keystroke reaches
the window at all. So the shortcut wins every time and whatever it took stops working
silently - `opt+left` rebound to `focus_left` ends word motion in every shell in the window,
with nothing on screen connecting the two.

**A default gives its chord up to yours.** That refusal is about chords the file chose. When
the file gives a default's chord to `[text]` or to another action, and does not name the
default's action itself, the default gives the chord up: the action keeps its menu item without
a shortcut, and the run log says so as `config.keymap.default_given_up`. So a file that sent
bytes on `cmd+k` before `clear_screen` shipped there goes on sending them, where refusing it
would throw away everything else in the file for a collision nobody in it made. A `[keymap]`
line gives the action a chord back.

**`find` is Ghostty's search, over the pane's whole history.** `cmd+f` opens a bar over the
pane with the keyboard, `cmd+g` and `cmd+shift+g` walk the matches, and landing on one scrolls
the pane to it and marks it. It searches the screen that is showing, as Ghostty does: a pane
whose program has taken the whole terminal - an agent harness, an editor, anything on the
alternate screen - is searched as that one screen, and its history is searchable again once
the program gives the terminal back.

**`cmd`-click a link in a pane to open it**, whether it is an address on the screen or an OSC 8
hyperlink a program printed. Web and mail links open on this Mac, from a devenv pane too. A path
opens only from a pane on this Mac, since from a devenv it names a file over there; and a file
that would run code when opened, or a link hiding invisible characters, is refused with a beep.
A hyperlink to another app's scheme, such as `vscode://`, asks first, because the screen shows
its text rather than where it goes. The rules are Ghostty's. The pointer turns into a hand over
a link a click would open, and the link it is over shows at the pane's bottom left, so a
hyperlink's target can be read before it is opened.

**A row in the agent list says two things, and you write the first one.** Underneath is what
the agent calls itself - Claude sets its terminal title to what it is working on, so the row
reads `chasing a flaky test` while it does - and on top is what to call the pane, which starts
out as its directory and the harness in it. `rename_pane` replaces that with anything you like,
emoji included: `🔥 payments spike`. Double-clicking a row asks the same thing, which is worth
knowing because the rows most worth naming are the ones no split is showing.

**Drag a row and the pane moves with it.** Dropping one agent's row on another exchanges the
two, so the list you arrange is the window you get - and the numbers move with them, since a
chord reaches whatever sits at its place now. Drop a row on one in a different tab and the agent
joins that tab, landing directly behind the row you dropped it on.

**Grab a pane by its handle and drop it on another pane's edge.** The handle is at the top
middle of every pane, a small ellipsis that shows when the pointer is near the top, and it is
Ghostty's, in the same place and the same size. Drag it over another pane and the half nearest
the pointer lights up: drop there and the pane goes to that side, the two sharing the space
half each - which is how a side by side pair becomes one above the other. The handle is the
only way to start one, so a drag anywhere else in a pane is still a selection, or the program's
when it uses the mouse. A pane dropped on a row or a tab caption in the agent list does what a
row does there, and a row dragged out of the list onto a pane's edge goes to that side too.

A pane on another machine lights nothing, because a pane cannot change machines; drop it on that
tab's caption instead and it joins the tab as its own machine's part. Dropping on itself, outside
the window, on a divider or on empty space does nothing, and `Escape` cancels. Ghostty opens a new
window for a pane dropped outside one; Muster does not yet.

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

The other root keys are small answers a terminal is expected to let you change, each one line
because each is one value. `resize_step` is how far a resize chord moves a divider; omit it
and the daemon decides, which is what a chord meant before the key existed.
`scroll_multiplier` scales whatever your trackpad or wheel reported, so `1` is the device's
own answer and `0.5` is half of it - a multiplier rather than a line count, because how big
one notch is belongs to the device. It scales both what the pane scrolls and what a program
that asked for the wheel is sent, so a notch moves `less` as far as it moves the scrollback.
`pane_padding` is the space between a pane's text and its edges, one number for both axes; `0`
is what fits the most rows into a window of fifteen agents. `clipboard_write` decides whether
a program may set your clipboard, which is how `tmux`, `vim` and an agent over ssh copy (OSC
52); `deny` drops those writes. Reading the clipboard is never offered to a program, since
that would hand any process in a pane whatever you copied last.
`hide_pointer_while_typing` hides the pointer as soon as you type into a pane and shows it again
when the mouse moves. It is Ghostty's `mouse-hide-while-typing` under Muster's word for the
mouse's arrow, since `[cursor]` is the text cursor, and it is off unless you turn it on, as it
is in Ghostty.

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
designed. `divider`, `focus_ring`, the `agent_*` and the two `context_*`
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

`context_empty` and `context_full` are the two ends of the small ring beside an agent's row, which
fills as the agent's context does. Its colour moves from one end to the other as it fills, slowly at
first and most of the way by 80%, so a glance says how close the agent is to compacting rather than
only whether it has passed a line. What ships is the grey of the row's other quiet marks and the
platform's red - red because no agent state is red, so a full ring is never read as an agent
waiting on you.

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

`programs` is a program in a pane asking to notify you, with OSC 9 or OSC 777, as a build or
a test runner can. Its banner carries the program's own words, since it has said why it wants
you, and it stands until you look at the pane. Notifying again before then raises nothing more,
so a program that notifies in a loop costs one banner. It is on because a program that asks has
decided it is worth it; `programs = false` is for a tool that decides that too often.

`messages` is a message for you from an agent: posted to `@human`, or to a group whose policy
rings you (`muster docs msg`). Its banner names the group and who wrote, one per group, replaced
by each new message, and choosing it opens the group's transcript. It is on because an agent
addressing you has chosen to interrupt you. `messages = false` takes the banner away and leaves
the group asking, so ⌘⇧A still reaches it.

A pane running an agent Muster recognizes never raises one, whatever this says. Every pane tells
its programs it is Ghostty, and agents that see that notify at the moments their state already
asks about: Claude Code writes "Claude is waiting for your input" once it has sat idle for a
minute, and "Claude needs your permission to use ..." at every permission prompt. Codex can
write one too, as its `notification_method` setting allows. So `done` and `blocked` ask for an
agent, once each, and `done = false` means no banner when an agent finishes. Once the agent
leaves the pane, what its shell runs notifies again.

A bell is never a banner, whatever this says: shells ring for a completion that found nothing,
so a bell marks the pane's row until you look, and bounces the Dock once if Muster is behind
another app.

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

`name_sessions` decides whether naming a pane - the chord, the menu, `muster pane rename` or `pane
new --name` - also names the session of the agent running in it, which Muster does by typing the
harness's own rename, `/rename <name>`, at the agent's prompt once it is idle (`muster docs
harnesses`). It is on because a pane and its session going by one name is the point of naming
either; `false` keeps the pane's name in Muster, for anyone who would rather nothing were typed
into their agents unasked. It stops only that direction: a session renamed in its harness still
renames the pane, since nothing is typed for that. The daemon is the one that types, so it is
handed the answer as a setting, as `scrollback_bytes` is below.

`human_name` is what messages between agents call you where they show you: `muster msg who`, and
the framing of `read` and `log`, which a transcript is. It goes beside your address rather than
in place of it - `Alex (@human)` - because an agent reading the transcript has to know what to
put in `--to`, and the address stays `@human`. Omit it and you are called by your login name.
It is one line of at most 64 bytes. The daemon answers the messaging commands, so it is handed
the name as a setting too, every daemon this window attaches included, since the human a devenv
shows is you.

`[shell]` and `scrollback_bytes` are the two Muster does not act on at all. What a pane runs
and how much of it you can scroll back through belong to the daemon that makes the pane - so
Muster hands them to the daemon as settings over its own connection, as it hands `[font]` and
`[colors]` to the renderer, and sends them again whenever the file changes or the daemon
reconnects. No daemon reads a config file of its own. When Muster ran herdr, you had to learn
that herdr existed and find its config file, and a `default_shell` set for your own terminal
quietly decided what every Muster pane ran.

`ssh_terminfo`, `ssh_env` and `sudo` are Ghostty's shell integration features of the same
names, which every pane's shell gets, and they answer one problem: a program on another machine,
or running as root, that has never heard of `xterm-ghostty`. `ssh_terminfo` has a pane's `ssh`
install the entry in the host's `~/.terminfo` the first time it reaches that host, over a
connection of its own, and remember the host; `ssh_env` sends `TERM` and the terminal's name and
colors along. Both are on, where Ghostty ships them off, because a Muster pane on a devenv is the
case Muster exists for, and a host without the entry draws a broken screen. They leave alone any
ssh that opens no terminal on the host, such as `ssh -N` for a tunnel or `ssh -G`. `sudo` wraps
`sudo` to keep `TERMINFO`, pointed at your `~/.terminfo`, where Muster then puts the entry, so a
root shell finds it. It is off because keeping `TERMINFO` needs a sudoers rule that allows it -
`SETENV`, or `ALL` - and under a rule that names its commands without that, sudo refuses the
command outright. Turn it on where your sudoers rules allow it. With it off, `sudo vim` in a local
pane cannot find `xterm-ghostty` and draws as a terminal it does not know, as in Ghostty with the
feature off: sudo drops the variable that points at Muster's copy of the entry, and macOS's own
database has none.

Your own `~/.terminfo` wins over the entry Muster carries, because the terminal database is read
from there first. On a machine where an older Ghostty or Muster installed `xterm-ghostty`, a pane
uses that one.

`scrollback_bytes` is bytes because that is what the buffer is measured in; a line has no
fixed size, so a count of them would be a number that did not mean what it said. Zero is a
real answer - a pane that keeps only what is on screen.

**Detection rules travel with the app, and `~/.muster/agent-detection/` is where you correct
them yourself.** A manifest is data describing how somebody else's agent looks on screen, and
those agents change on their own schedule. When Muster ran herdr, Claude Code moved its busy
spinner from a Braille character to a half-circle while the one rule that could produce
`working` still matched Braille, and eleven agent transitions over a day on two machines never
once said `working`. So a fix to a manifest has to reach a daemon that is already running. The
daemon's manifests are built in, the app sends its own when it connects, and a file here beats
both; nothing is fetched from the network. A file is one agent's manifest in herdr's format,
named for the agent - `claude.toml` replaces the built-in claude, and a new name adds an agent
Muster never knew. A file named for a different agent than its `id`, or needing a newer
detection engine than the daemon has, is ignored and the daemon's log says why. The daemon reads
the directory when it starts, looks at it every couple of seconds while it runs, and reads it again
when the app sends manifests on connecting, so a file saved here applies within a few seconds and
ends nothing; only panes running an agent whose manifest changed start their detection over, so
neither an edit nor a reconnect makes a working agent flash idle.

**`MUSTER_AGENT` names the agent a process is, whatever its executable is called.** Set it in
the environment a wrapper script starts its agent with - `MUSTER_AGENT=claude` - and the daemon
reads it off that process and matches it against the manifests' names. macOS does not show the
environment of its own binaries (`/bin/sh`, `/bin/sleep`), so the variable has to be on the agent
process itself rather than on a system shell that runs it.

**Both work on a devenv too**, where the daemon reads that machine's own
`~/.muster/agent-detection/`. A `[[daemon]]` with a `host` starts the daemon of this Muster's
version over there, `~/.muster/daemon/<version>/muster-daemon` with its data beside it, or reuses
one still running from last time, agents and all. One left running by an older Muster is asked to
hand its panes to the new daemon, which keeps every agent running; the same happens on this
machine. Older means a lower version number, and 0.10.1 is the first release whose daemon is
muster-daemon, so the first handoff between releases is from 0.10.1 to 0.11.0. The first
time, or when that directory holds another build, Muster copies its own daemon there over the same
ssh connection; nothing is downloaded. Linux on x86_64 or aarch64 and macOS on Apple silicon are the
machines it carries a daemon for, and the run log says so for any other. Naming a `socket` in a
`[[daemon]]` block still attaches whatever is listening at it, on either machine, and never asks it
to hand over - that is how you ask for somebody else's daemon on purpose.

**A machine that is slow or away does not hold the window closed.** Muster waits a second for the
daemons the file names, opens the window without any that have not answered by then, and their
panes arrive when they do. A daemon that cannot be reached is tried again, on the same backoff as a
dropped connection, for as long as Muster runs; the window says which one is missing and why from
the first failed attempt, and takes that back when it answers, so there is no need to relaunch.
The exception is a daemon that can never start, such as one whose binary is not there: another
attempt would fail the same way, so it is reported once, as an error naming what to change, and
not tried again until Muster is relaunched.

**Each machine has a color, and its rows in the agent list carry a swatch of it** while the
window is attached to more than one, so a laptop pane and a devenv pane can be told apart
without reading their labels. The color is drawn from the machine's `id`, so the same machine
has the same color in every window and after every launch, from six chosen to stay clear of the
agent-state colors. Six go round quickly, and two machines can land on one: a `color` in either
one's `[[daemon]]` block settles it. Hovering a row names the machine too. A machine's heading,
drawn when it is unreachable or holds no panes, carries the same swatch.

**Saving the file is enough.** Muster watches it and reads it again, and `cmd+shift+,` or
Reload Configuration asks for the same thing when you would rather say so yourself - the
watcher dispatches that action rather than being a second way in. Colours, fonts, the cursor,
the keymap, `[text]`, `option_as_alt`, `resize_step`, `scroll_multiplier`, `hide_pointer_while_typing`, `clipboard_write`,
`[notifications]`, `name_sessions`, `human_name` and a `[[daemon]]` block's `color` all take effect where they are, including in panes that were already open; `pane_padding`
reaches panes opened afterwards, because that is as far as the renderer takes it. `[shell]` and `scrollback_bytes`
reach panes opened afterwards too, and for the same shape of reason: the daemon takes both when
it builds a pane, so a pane you are already typing in keeps what it was made with.

`[[daemon]]` is half an exception. A block added is attached as soon as the file is saved, and
that machine's panes arrive in the window. A block whose machine never attached - still being
tried, or given up on - has nothing to keep, so correcting its `host`, `socket` or `ssh_options`
attaches it from the corrected one at once. A block taken out, or changed in any of those once its
machine is attached, is not acted on until Muster is relaunched: the agents on that machine keep
running whatever the file says, and detaching it would take their tabs out of the window, which
reads as agents gone. So it stays attached, and a warning at the foot of the agent list says it is
waiting for a relaunch. A file that will not parse changes nothing at all, which means an editor that saves halfway
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

`~/.muster/state/` is Muster's to write, and nothing in it should be edited by hand. Arrangements
live under `<install>/windows/` - `release/windows/` for the app you installed - one file per
window rather than one for the machine, and each is rewritten
whenever that window settles: which tabs it was showing, in what order, at what widths; under
`[window]`, whether the agent list was open and how wide, and how big the window itself was; and
one `[[pane]]` row for each pane whose text somebody sized. Delete the
directory and the next launch opens fresh. Nothing about a session is in any of them - what a tab
holds is the daemon's answer, asked again on every launch.

**Drag the agent list's edge to make it wider or narrower**, anywhere from 140 to 480 points.
The width is the window's, written down with the rest of `[window]`, so it comes back on the next
launch and a new window starts at 200. A window too small to give the list its width gives it half
the window instead, and a window narrower than 400 points still puts the list away altogether.

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

The daemon has no such file: what it is told arrives over its connection, and nothing is
downloaded, so Muster keeps no cache directory either.
