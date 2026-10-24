---
mip: 9
title: A group's conversation and a banner, drawn by Muster for the human
status: Draft
kind: Product
created: 2026-10-24
decided:
supersedes:
superseded-by:
related: 1, 2, 4, 6
---

# MIP-9: A group's conversation and a banner, drawn by Muster for the human

## Summary

The human reads and answers a group's messages in two views Muster draws itself. The
**conversation** shows one group's messages in place of the window's panes, laid out so the
messages meant for the human stand out, with a message box that posts as `@human`. The **banner**
shows a message that needs the human while they work in the window, and takes a short reply
without leaving the pane. Neither is a terminal, and neither is the operating system's
notification. The transcript pane, `muster msg log --follow`, stays for terminals and scripts.

Both views are models the core builds and the shell draws, as the agent list is. The core decides
what a conversation holds: which messages fold away, where the unread ones begin, whom a reply
goes to, and what a send came to. It parses Markdown, and it counts a window showing a
conversation as showing none of the window's panes. Only the drawing, key capture and the
operating system's notification are per platform: the macOS shell draws the views in SwiftUI
inside its AppKit window, and a shell on Linux or Windows would draw the same events in its own
toolkit.

It lands in three stages: the banner, then the conversation, then answers an agent offers with a
question. A queue of everything asking for the human ("Needs you") and a conversation placed in a
tab's tree as a pane are left for later, and nothing here may rule them out.

## Decisions for amterp

amterp approved the design on 2026-10-24 (kan a_2die8LU6o, whose comments hold the mockups'
rounds and an adversarial review of them). These were settled then, and are recorded here so the
MIP stands on its own:

- **Shown in place of the panes**, not over them in a drawer and not in a pane of its own.
- **Muster draws its own banner.** Nothing is answered through the operating system's
  notification: "the more vertically integrated we are, the better an experience we can deliver
  with Muster, as we learned when we got rid of herdr." While Muster is not in front, the
  notification stays, as a pointer back to Muster only.
- **Each window's message box is its own.** Typing in one window never appears in another, and a
  post shows in every window showing the group.
- **A message that starts with @ and a member's name addresses that member.** An @ later in a
  message only completes the name.
- **Picking an offered answer fills the reply rather than sending it**, so a note can go with it.
- **Drafts are kept**, so leaving a conversation loses nothing.
- **The words**: group, conversation, message box, transcript (the pane), banner, and "Needs you"
  for the queue, which the code and the CLI call the asking list. Nothing is called chat, inbox or
  drawer: a participant's `inbox` is already the socket it is woken on.

amterp asked the author to settle what did not need him. Two of those choices are hard to undo,
so they are flagged here for a veto rather than asked:

**A. `--option` spells an offered answer** (section 6): `muster msg post --to @human --option
"Run it now" --option "Skip it"`. Once skills and agents' habits spell it, renaming it breaks
them. `--answer` reads as the agent answering, and `--choice` says the same as `--option` in more
letters.

**B. The conversation replaces the transcript pane as where choosing the banner, the sidebar row,
⌘⇧A or `muster msg open` takes the human.** `muster msg open` then answers with a window rather
than a pane (section 8), which changes what a script reading its answer gets.

## Context / Motivation

MIP-4 built messaging between agents and gave the human the plainest view of it (its first
decision for amterp, options (a) and (b)): a tab running `muster msg log --group G --follow`,
which choosing the group's notification or its sidebar row opens, and `muster msg post` from some
shell to answer. It was recommended as a first step, to be used for a round before deciding more.

The round has been used. amterp, on 2026-10-07: following a group is "a very plain text file.
It's really hard to parse ... There's no way for me to kind of like talk in the group and have
that be broadcast through muster." And on 2026-10-14: "It opens up like a new pane and there's
just plain text, it's hard to parse."

What a terminal makes of a group's log:

- Every entry looks alike. A message between two agents, a question to the human, an urgent one
  and a join are all lines between `---` markers, so the human reads the whole log to find the
  two that were for them.
- The terminal wraps long lines at its width, mid-word, and Markdown arrives as its source.
- Answering takes another shell, the group's name, the right `--to`, and quoting.
- Going there opens a tab, so the human leaves the panes they were in and finds their way back.

A terminal can do better than plain lines, with color, boxes and bold, the way Claude Code draws
its own screen. It cannot draw proportional text, Markdown as Markdown, a text field with the
platform's input methods and spell checking, or a selection of one message to copy. Those need a
view the shell draws, the way it already draws the sidebar beside the terminals.

MIP-4 declined option (c) of that decision, a web view as council v1 had, because "the web view
was one more window". This MIP keeps that lesson: both views are in the window the human is
already in.

## Decision

### 1. The conversation

A group's conversation is shown in place of the window's panes. Choosing the group's row in the
sidebar, its banner or its notification from the operating system shows it, and so do ⌘⇧A, the
chord to whatever asks for the human most, when that is the group, and `muster msg open --group
G`. The tab the window was showing stays as it was underneath. Esc, choosing a tab or a pane, a
numbered chord, or a member's Go to pane returns to the panes, with the keyboard back where it
was or on whatever was chosen.

The conversation has four parts:

- **A header**: the group, the machine that keeps it and how many members it has, with buttons to
  pause or resume it and to open its transcript in a new tab.
- **The thread**, oldest at the top, opened scrolled to the first message the human has not read.
- **A members column**: each member with its agent state, what it is doing, whether messages wait
  on it unread and why (the reason `muster msg who` already gives, such as "it is at a dialog,
  waiting for somebody to answer it"), and Go to pane.
- **The message box**, at the foot of the thread (section 3).

The thread is built so the human finds what was for them without reading the rest:

- **Messages to the human stand out**: tinted, marked "to you", and marked urgent when posted
  urgently.
- **A run of consecutive messages that woke nobody but agents folds into one line**: "6 messages
  between agents, none to you", with their time span, opened in place by choosing it. A message
  that woke the human through its group's policy, without naming them, does not fold.
- **A line marks where unread messages begin**, at the first message that waited for the human
  when the conversation opened (the `first` of the notice the daemon sends the window about the
  group), and stays until the conversation closes.
- **Joins, leaves, pauses and policy changes are one-line rows**, consecutive ones merged: "otter
  made the group; heron, lynx and you joined".
- **Messages from `@muster` are rows of their own**, with no Reply, since `@muster` is never
  addressed.
- **Markdown is drawn**: paragraphs, lists, emphasis, inline code, code blocks and links. A body
  over 40 lines shows its first 40 and "Show all", since a body runs to 1 MiB (`LARGEST_BODY`).
- **Times are 24-hour with seconds**, under dividers dated yyyy-mm-dd, and hovering one shows its
  milliseconds and UTC offset.
- **Each message has Reply**, which addresses the message box to its author, and **Go to pane**
  for an author in a pane.

A group kept on another machine says so in the header. While that machine cannot be reached, the
conversation says the group may be missing what was posted there since, as `muster msg log` does,
and a post is refused at once, naming the machine (MIP-4, section 11). A paused group says it is
paused, and that a post there wakes nobody until it resumes.

While a conversation is shown, the sidebar marks the rows of its members' panes, so the window
says which panes are in the group.

### 2. What the human reads

Showing a conversation is the human reading the group, as going to its transcript is now (MIP-4,
section 10): the core tells the group's daemon, the daemon moves the human's place to the end of
the group, and what waited stops asking. It reads when the transcript would: when the
conversation is in the window that is in front and focused. A second window showing the same
group while a different window is in front reads nothing, and reads once it comes to the front.
While the conversation is in the front, focused window, a message is read as it arrives.

Every window can show any group's conversation, two windows showing the same one included, and
each shows every message as it lands.

### 3. The message box

The message box posts as `@human`, with the request `muster msg post` sends from a shell.

**Recipients.** It opens addressed to whoever last wrote to the human in that group, when that
author can be addressed: not `@muster`, and still a member. Otherwise it opens addressed to
nobody, and says that the group's policy decides whom the post wakes. Reply on a message changes
the recipients to its author. A message that starts with @ and a member's name addresses that
member instead, completed from the group's members; several at the start address several, and
Backspace straight after one turns it back into text. An @ later in a message completes the name
and stays text. Only the message box reads a leading @. The post it sends names its recipients,
as `--to` does, and the daemon never reads an @ in a body.

**Urgent** is a toggle. A group whose policy does not let the human post urgently refuses the
post, and the message box says so (section 4). MIP-4's stale-context guard never holds the
human's post (MIP-4, section 4), so no post is refused for messages the human has not read.

**Send** is ⌘↩ on macOS, a binding in the core's keymap like every other.

**A draft is kept per window and group**, whole: the text, the recipients, Urgent and a picked
answer (section 6). The core writes it beside the window's saved arrangement as it changes,
debounced, so it survives leaving the conversation, quitting Muster and an upgrade. A post that is
kept clears it; a refused or unknown one (section 4) leaves it. A message arriving never changes
a draft's recipients.

### 4. What a send comes to

A post has four outcomes, and the message box shows each:

- **Pending**: sent, with no answer yet. The text stays, marked as sending.
- **Posted**: the daemon kept it. The post appears in the thread, and under it whom it woke, from
  the answer's `reached`: "woke otter", or that nobody live heard it, which is the CLI's exit 6.
- **Refused**: the daemon said no, in its own words, which say what would let the post through: a
  group kept on a machine that cannot be reached, an addressee that left, a policy that refuses
  the human urgent posts. The text stays.
- **Unknown**: the daemon took the request and never answered, which is the seam's `Unanswered`
  and the CLI's exit 4. The post may have been kept. The text stays, marked as perhaps sent, and
  is never sent again on its own, because a post carries no key the daemon could use to refuse a
  second copy. The core settles it from the group's log: an entry by the human with the same
  recipients and body, after the last entry the human had seen, means the post was kept, and the
  draft clears. A banner, which follows no log, asks the log for that once.

### 5. The banner

While Muster is in front, a message that wakes the human raises a banner Muster draws at the top
right of the window's pane area, unless that group's conversation is already on screen in that
window. The banner shows the group, the author, the urgent mark, the message's first lines with
their Markdown drawn, a reply field addressed to the author, and Open conversation. Offered
answers appear as buttons (section 6).

- **One banner per group**, replaced by its next message. Banners from several groups stack,
  newest on top, at most three, and the rest fold into a count that opens the oldest.
- **It stays until answered, dismissed or read.** A banner whose group the human read somewhere
  else, in another window or from a shell, comes down, as the notification does now when the
  daemon says nothing waits.
- **A banner holds the message it shows**: the group, the message's number and its author. A
  reply goes to that author, whatever has arrived since.
- **Answering from a banner does not read the group.** Reading moves the human's place in a group
  to its end (`MsgRequest.Read` takes only the group), so reading on a reply to otter's banner
  would also read heron's earlier question, which the human never saw. Dismissing does not read
  either. Only the conversation reads.
- **A message from `@muster` takes no reply.** Its banner shows what Muster said and Open
  conversation.
- **A send from a banner has the four outcomes of section 4.** Refused or unknown, the banner
  stays with the reply in it and says what happened.

While Muster is not in front, a message that wakes the human raises the operating system's
notification, as now: who wrote and how many wait, and choosing it brings Muster forward to the
conversation. It takes no reply.

The banner's text needs no change to the daemon. The notice a window is sent carries a message's
own words only for a lone message from `@muster` (`Notice.said`), but it always carries the
number of the newest waiting message (`Notice.last`), so the core asks the group's log for that
entry, as the human, and builds the banner from it.

### 6. Answers an agent offers

A post may carry answers for the human to pick from, which makes it a question:

    muster msg post --to @human --option "Run it now" --option "Skip it this round" \
      "Can you run ./dev --contract on 9b2f3d1? It needs a GUI session."

A post carries at most six, each at most 120 bytes. `muster msg read` and `log` print them after
the body as a numbered list, so an agent or a person in a terminal sees them too. The
conversation and the banner draw them as buttons under the message.

Picking one fills the reply rather than sending it: the message box shows the picked answer
beside its text, the human may add a note, and Send posts both. Writing without picking is an
ordinary reply. Digits never pick an answer inside the message box, since a reply may start with
"1."; Tab reaches the buttons.

The reply carries the pick twice. Its body starts with `Answer: Run it now`, so an agent reads it
with no new parsing, and the post carries a field naming the question's number and the answer, so
the conversation marks the question answered and takes its buttons away. A question takes one
answer. Whether an agent may ask for several, as Claude Code's multi-select does, waits until an
agent needs it.

Answers are a field on a message, so they need a daemon protocol minor of their own. A post with
answers to a group kept by an older daemon is refused, as an urgent post to a daemon before 1.3
is, rather than kept with its answers silently dropped.

### 7. Where the work is done

The conversation and the banner are core state and core decisions, built where the agent list
is. The shell draws them and says what the person did.

**The conversation is part of the window's view.** The core's view of a window gains the group
whose conversation it shows. While one is shown, the view shows no panes, so the set of panes on
screen that `View::showing` holds, which `Attention::seen` consults, is empty for that window.
Covered panes then count as not seen with no new rule: an agent that finishes under a
conversation is announced and keeps its `done`, and a covered transcript pane reads nothing. Had
the shell hidden the panes on its own, the core would still count them seen: the finish would go
unannounced, and the transcript would read messages nobody saw. Reading follows section 2's
condition, which widens `looking_at_transcript` in `crates/muster-seam/src/session.rs` to cover
a conversation.

**The core keeps one copy of each open group's log**, shared by every window showing it. It
fetches the log once with `Log`, as the human, and follows it from its last entry as `muster msg
log --follow` does, so two windows on one group cost one fetch and one follow. A long group's
history is fetched newest first, a page at a time as the thread scrolls up, which needs `Log` to
take a count of newest entries: a daemon protocol minor, in stage 2.

**The core builds the thread**: rows for dividers, joins, folds and messages, each message's
Markdown parsed once into blocks, the unread line, the recipients a reply opens with, each
member's state and unread reason, and where a send stands. It parses Markdown with a Rust library
(pulldown-cmark) into a small tree of blocks and runs of text, which crosses the seam as protobuf
and which the macOS shell turns into SwiftUI text.

**The seam gains** requests to show and leave a conversation, open a fold, post as the human, save
a draft and pick an answer, each answered as every request is, `Unanswered` included; and events
carrying a window's conversation and its banners, coalesced like the view's. The CLI and agents
reach every one of them (section 8).

**The macOS shell's part** is SwiftUI views hosted in the AppKit window, as the find bar already
is; capturing the keys the core's keymap binds, such as send and leave; leaving an input method's
composition alone, so Esc while composing ends the composition and not the conversation; and the
operating system's notification while Muster is not in front.

**Cost.** Messages arrive at the speed people and agents write, so the budget is per message, not
per byte: a body is parsed once when it arrives, a thread's rows are rebuilt for new entries only,
and a window with no conversation open and no banner up does no conversation work at all.

### 8. The CLI and agents

Parity follows from the seam:

- `muster msg open --group G` shows the conversation in a window and answers with the window,
  where today it answers with the transcript's pane.
- `muster window --json` says which group's conversation each window shows, so an agent can see
  that the human is reading.
- `muster msg post` from a shell is still how the human posts outside a window, and its request is
  the one the message box sends.
- `muster msg log --group G --follow` is still the transcript, and the conversation's header opens
  one in a new tab.

### 9. Stages

1. **The banner**, with what it needs underneath: posting as the human from the window, the four
   outcomes, Markdown in the core, and the banner's model and events. No daemon change.
2. **The conversation**, with the view's new state, the shared log, drafts, the members column,
   the sidebar marking a group's members while its conversation is shown, and `Log`'s count of
   newest entries.
3. **Offered answers**: the answers field on a message, `--option`, and both views drawing them.

Each stage is tested as the rest of Muster is (`docs/testing.md`). The thread's rules are
conformance cases in the core, with no daemon: folds, the unread line, the recipients a reply
opens with, @ completion and Markdown blocks. The seam's behavior runs headless against a real
muster-daemon: a pane finishing under a conversation is announced, a covered transcript reads
nothing, a banner reply leaves the human's place where it was, two windows keep two drafts, and
each send outcome. The drawn views are checked in the contract tier, which has a GUI session, and
so is Esc during an input method's composition.

## Rationale

**Built in the core, so another platform's shell only draws.** Muster's core is portable by
construction (MIP-1): the rules live in Rust, and the shell is thin and per platform. A
conversation whose folding, unread line, recipients and Markdown lived in Swift would be the first
feature whose behavior a Linux shell had to write again, and the first whose rules the core's
suite could not see. Built in the core, the macOS shell's work is SwiftUI and key capture, and a
GTK or Windows shell's would be its own toolkit's views over the same events. Nothing in the model
names a macOS type: Markdown crosses the seam as a neutral tree, not as an `NSAttributedString`.

**In place of the panes**, because amterp chose it over the drawer on 2026-10-24 as the simplest
first shape, and because it gives the view a clear meaning in the core. A drawer over part of the
panes left open which covered panes counted as seen. A view that replaces the panes answers it.

**Muster's own banner**, because amterp wants Muster vertically integrated, and because the
operating system's banner limits the design: macOS's text reply (`UNTextInputNotificationAction`)
takes a placeholder but no prefilled text, so an offered answer could not fill it.

**A draft per window**, because amterp does not want typing in one window to appear in another.
It is kept with the window's saved arrangement because a draft is something the person wrote,
which that arrangement exists to keep ("intent rather than observation",
`crates/muster-core/src/composition/saved.rs`), and not on the daemon, which every window shares.

**Four send outcomes**, because each simpler scheme loses something: counting every failure as
unsent sends a reply twice, and counting every finished request as delivered hides a reply the
daemon refused.

## Alternatives Considered

**A richer terminal view in a pane.** Color, folds drawn in box characters, and a compose line
read by a program in the pane. Portable at once, with no shell work. Rejected for what a terminal
grid cannot draw (proportional text, Markdown, a native text field, a selection of one message),
and because it is a pane, which amterp ruled out: it "doesn't open a new pane ... I think that's
just crazy."

**A drawer over the right side of the panes** (the mockups' first recommendation). It keeps the
panes in sight and does not reflow them. amterp chose the conversation in place of the panes as
simpler, and the drawer left seen-ness unclear for the panes it half covered.

**Answering through the operating system's notification.** The cheapest to build, and it works
while Muster is in the background. Rejected by amterp for vertical integration, and macOS's text
reply cannot hold a picked answer.

**A web view in the window.** One UI stack for every platform. Rejected because it brings a second
UI stack into a native shell, and a page inside an app does not feel like the app, which is the
native feel Muster exists for.

**Markdown parsed in Swift.** Less code on macOS, since SwiftUI draws `AttributedString(markdown:)`
directly. Rejected for putting rules the core's suite cannot test into one platform's shell, where
the next platform's shell would write them again.

**Drafts on the daemon.** They would survive the app losing its saved state, and need nothing kept
in the app. Rejected because every window would share them, which amterp does not want.

**The conversation as a pane in a tab's tree**, beside terminal panes and arranged with them. Not
rejected but deferred (Future Directions), because the daemon's tree holds only panes with
terminals behind them today.

## Consequences & Trade-offs

- **MIP-4's section 10 changes in one place**: the human reads a group in its conversation, and
  the transcript pane becomes the terminal's view of it. MIP-4 is a Draft, so it is corrected when
  stage 2 lands, along with `docs/architecture.md`'s attention routing and `docs/cli/msg.md`.
- **`muster msg open` answers with a window, not a pane.** A script that took the pane from its
  answer asks `muster window` instead.
- **The shell gains more SwiftUI.** It hosts SwiftUI in AppKit in one place today; the banner and
  the conversation make that the shell's usual way of drawing anything that is not a terminal.
- **The core gains a Markdown parser**, a dependency the gate's license check then covers.
- **Two daemon protocol minors**, one in stage 2 for paged `Log` and one in stage 3 for answers. A
  window that meets an older daemon loads a group's whole log at once and draws no answers.
- **Muster draws over its panes.** The banner sits at the top right of the pane area and covers
  what is under it while it stands.

## Future Directions

- **Needs you**: one list of everything asking for the human (kan a_2naAbSaGZ), built from the
  core's existing asking list, which holds blocked, stuck, finished and notified panes as well as
  messages, with each group's ask opened out into the messages that wait. It needs reading and
  answering told apart: today opening a group clears its ask whether or not anything was answered.
- **A conversation as a pane in a tab's tree**, beside terminal panes (kan a_2naAbjFtA). The
  conversation's model names its group and nothing about where it is drawn, so the shell's side
  is small; the daemon's tree would have to hold a pane with no terminal behind it, a protocol
  change.
- **Muster's banner for panes**: drawing the blocked and finished banners in the window too,
  while Muster is in front.

## Open Questions

- **Whether 40 lines is the right cut for a long body, and how long a folded run may grow before
  the thread pages it.** The first real groups will answer this better than a guess.
- **Whether a stack of three banners is right**, or one banner with a count serves better when
  several groups talk at once.

## References

- MIP-1 (the portable core and the seam), MIP-2 (the window's view), MIP-4 (messaging, the human
  and the transcript), MIP-6 (several windows of one process).
- kan a_2die8LU6o and its children: the mockups' rounds, amterp's decisions, and the review that
  shaped sections 2, 4 and 5.

---

## History
- 2026-10-24 Draft
