//! argv in, one `Request` out.
//!
//! Pure, and the reason this file exists on its own: what a command line means is the part of the
//! CLI worth pinning, and `corpus/conformance/cli.json` pins it - argv and an environment in, the
//! request it becomes or the refusal it earns. Every other part of the CLI needs a window to say
//! anything at all.
//!
//! **clap owns the syntax and this file owns the meaning.** The workspace hand-writes its config
//! parser, and this deliberately does not follow that rule: a config file is read once by Muster
//! itself and a command line is read by whoever is holding a keyboard, so the things clap does
//! that a hand-rolled parser would not - suggesting the flag you meant, generating a `--help` that
//! cannot drift from the code, emitting shell completions, handling `--` and `--flag=value` the
//! way every other command does - are the difference between a surface somebody can guess at and
//! one they have to read first. What clap cannot know stays here: which pane a command is about
//! when nothing named one, and why there might be no answer to that.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use clap::{ArgGroup, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use muster_core::input::check_sent_keys;
use muster_core::names::could_be_minted;
use muster_proto::{
    AdjustFontSize, ArrangePane, ClosePane, CloseTab, CompactPane, CreateTab, EqualizePanes,
    FocusAsking, FocusHistory, FocusPane, FocusPaneAt, FocusRelative, FocusTab, FocusTabRelative,
    HideHelpers, HidePane, MoveTab, OpenTranscript, QuietPane, ReadDaemons, ReadPane, ReadWindow,
    ReattachPane, ReloadConfig, RenamePane, RenameTab, Request, ResizePane, RestartAgent,
    SendToPane, ShowHelpers, ShowPane, SplitPane, ToggleSidebar, UpgradeMuster, WatchPanes,
    ZoomPane, request,
};

use crate::{docs, environment};

/// What one command line asked for.
#[derive(Debug)]
pub struct Invocation {
    pub asking: Asking,

    /// Answer for a program rather than for a person.
    pub json: bool,

    /// The Muster to talk to, when the caller named one: its command socket.
    pub socket: Option<String>,

    /// Ask this machine's daemon rather than any window.
    pub no_window: bool,
}

/// Either something to ask a window, or something this CLI can answer by itself.
#[derive(Debug)]
pub enum Asking {
    /// Boxed because a `Request` is two orders of magnitude the size of the other variants, and
    /// every invocation would otherwise carry room for the largest message Muster has.
    Send(Box<Request>),
    /// A send whose text is not on the command line, so this CLI reads it before dialing.
    ///
    /// The request goes out with its text empty until then. Reading is kept out of [`parse`],
    /// which is pure, so what a command line means stays pinnable without a filesystem.
    SendFrom {
        request: Box<Request>,
        from: TextSource,
    },
    /// A request answered with a stream: printed as it arrives, until it ends or `timeout` runs
    /// out. `None` waits for as long as the window keeps the watch open.
    Watch {
        request: Box<Request>,
        timeout: Option<Duration>,
    },
    /// A layout drawn again each time it changes: `watch` says when it may have, and `read` is
    /// asked again to find out whether it did.
    WatchLayout {
        watch: Box<Request>,
        read: Box<Request>,
    },
    Print(String),
    /// Every window on this machine, asked the same thing and answered together.
    Survey {
        closed: bool,
    },
    /// Another window, asked of the running app or, with none running, by starting it.
    MakeWindow(crate::opening::Onto),
    /// A closed window by name, or the one closed last, asked for the same way.
    ReopenWindow(Option<String>),
    /// A window closed by name, or the one this command is about, asked of the running app.
    CloseWindow(Option<String>),
    /// A message for this machine's daemon rather than a window.
    Message(Box<crate::messaging::Messaging>),
    /// A harness's adapter, installed on this machine through the harness's own command.
    Install {
        harness: muster_core::harnesses::Harness,
        dry_run: bool,
    },
}

/// Where the text of a `pane send` comes from when it is not on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSource {
    /// An absolute path, already resolved against the directory the command was run in.
    File(String),
    Stdin,
}

impl TextSource {
    fn described(&self) -> String {
        match self {
            TextSource::File(path) => path.clone(),
            TextSource::Stdin => "stdin".to_string(),
        }
    }
}

/// Why a command line produced no request.
#[derive(Debug)]
pub enum Failure {
    /// clap could not read it, and has already written the explanation - `--help` and `--version`
    /// arrive here too, because they end the run the same way and it renders them itself.
    Usage(Box<clap::Error>),

    /// It read, and still cannot be carried out.
    Refused(String),
}

const ABOUT: &str = "Drive a Muster window from a script or an agent.";

const NOTES: &str = "\
Drive a Muster window from a script or an agent.

A REF is a pane's name, as `muster window` prints it: p1w3r07bsd. It is Muster's own name for the \
pane and is unique across every machine the window is showing, so it needs nothing else beside \
it. Leaving it out means the pane this command is running in ($MUSTER_PANE), and failing that the \
pane the window's keyboard is on.

A tab has one too - t1w3r07bsd - and `tab` takes it wherever a pane command takes a REF. Nothing \
puts a tab's name in a pane's environment, because nothing has to tell a tab which tab it is, so \
there is no equivalent of $MUSTER_PANE: read the name out of `muster window`, where every pane \
says which tab holds it.

`pane new` prints the callsign of the pane it made, which is what makes the next line of a script \
possible, and `tab new` prints the pane it put in the tab, the same way; --json adds the id. Neither moves the keyboard: making a \
pane is not the same act as looking at one, and an agent opening three panes should not drag \
somebody's cursor through all three. Ask with --focus.

`pane move` is one verb for two outcomes, because the window works out which from where the panes \
are: onto a pane in the same tab the two trade places, onto a pane in another tab it joins that \
tab. Add --left, --right, --up or --down and it goes to that side of the pane instead, which is \
how a side by side pair becomes one above the other. Both have to be on the same machine - a \
pane is a process, and it lives where it lives.

`pane new` and `tab new` also take --daemon, which is a machine's own name as `muster window` \
prints it beside every pane: local, or whatever a [[daemon]] block in your config calls the \
machine. On its own it says where rather than what to grow from, and it is the way to reach a \
machine you have no pane on at all - a devenv the day you attach it, or one whose last pane you \
closed. A machine with nothing on screen gets a first pane rather than a refusal. Beside --pane, \
`pane new` puts the new pane on that machine in the named pane's tab: split beside that \
machine's part of the tab when it has one, and as a new part at the tab's end when it does not.
";

const EXAMPLES: &str = "\
Examples:
  muster window --json
  muster pane new --down --run claude --name '🤖 A'
  muster msg post --to p1w3r07bsd --file brief.md
  muster pane wait --pane p1w3r07bsd --until idle,blocked --timeout 600
  muster pane wait --pane p1w3r07bsd --context 80
  muster pane move --pane p1w3r0ab2n --onto p1w3r07bsd --down
  muster tab new --run claude --name '🤖 reviewer'
  muster pane new --daemon devenv --run claude
  muster focus --next

Which window: --window names it. Otherwise a command in a pane is about the window holding that
pane's tab, and one outside every pane is about the window in front. Which Muster: $MUSTER_SOCKET,
which Muster sets in every pane it makes, on an SSH machine as well as this one; otherwise the one
listening under ~/.muster/state, and a refusal rather than a guess if two installs answer.

Exit codes: 0 it happened, 1 the window refused, 2 the command line was wrong, 3 there was
no window to ask, 4 a window took it and never answered, 5 a wait ran out first. Send it again
after 3 or 5, never after 4 - the request landed, and doing it twice is on you. The exception is a
wait, which exits 4 when its pane's daemon stops answering and changes nothing if run again.
";

#[derive(Debug, Parser)]
#[command(
    name = "muster",
    version,
    about = ABOUT,
    long_about = NOTES,
    after_long_help = EXAMPLES,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    what: What,

    /// Answer as JSON rather than for a person to read
    //
    // Ordered after everything a particular command takes. Both of these are on every command, so
    // in a `muster pane new --help` they are the least interesting lines on the page - and clap
    // otherwise threads them into the middle of the flags that command is actually about.
    #[arg(long, global = true, display_order = 100)]
    json: bool,

    /// The Muster to talk to, by its command socket, instead of looking for one
    #[arg(long, global = true, value_name = "PATH", display_order = 101)]
    socket: Option<String>,

    /// Ask this machine's daemon instead of a window, as happens when none answers
    //
    // For `window`, `pane read`, `pane send`, `pane wait` and `pane compact`, the verbs a daemon
    // can answer on its own. Forcing the window needs no flag of its own: `--socket` names one,
    // and a named window that is not there is refused rather than passed over.
    #[arg(long, global = true, conflicts_with = "socket", display_order = 102)]
    no_window: bool,

    /// The window this is about, by name: window-2. For `tab move`, where the tab goes
    //
    // Every window of an app answers on one socket, so a command names its window here when
    // nothing else does: a pane's tab decides inside a pane, and outside every pane the window in
    // front would answer (mip/0006-one-process.md, Open Questions). A window the app has not got,
    // or one that is closed, is refused rather than guessed at.
    //
    // For `tab move` the window a move is about is where the tab goes, so this fills the move's
    // destination - which is what `tab move --window` has always meant.
    #[arg(
        long,
        global = true,
        value_name = "WINDOW",
        conflicts_with = "no_window",
        display_order = 103
    )]
    window: Option<String>,
}

#[derive(Debug, Subcommand)]
enum What {
    /// What the window is showing: its daemons, its tabs, its panes, and what each agent is doing
    #[command(args_conflicts_with_subcommands = true)]
    Window {
        /// Keep answering: a line per pane as it stands, then a line per change, until stopped
        //
        // A flag on the one-window read rather than a verb of its own, because it is the same
        // question asked continuously - and it is the shape an integrator waiting on several
        // agents at once wants: one connection, and one line each time any of them starts,
        // stops or goes (kan a_2M9T8O6dL).
        #[arg(long)]
        watch: bool,

        /// Draw every tab's panes where they sit, with each pane's size in cells, rather than
        /// listing them; with --json, add each tab's arrangement and each pane's place and size;
        /// with --watch, draw it again each time the arrangement changes
        //
        // A flag on the read rather than a verb of its own, because the arrangement is part of
        // what a window is showing and composes with everything `muster window` already does:
        // --json, --socket, and answering for every window at once. Off by default because the
        // sizes are a question to every daemon, where the ordinary read asks none.
        #[arg(long)]
        layout: bool,

        #[command(subcommand)]
        doing: Option<AboutWindows>,
    },

    /// Make a pane, name one, read it, type into one, wait on one, move it, resize it, reattach it,
    /// or close it
    Pane {
        #[command(subcommand)]
        doing: Doing,
    },

    /// Make a tab, go to one, or name one
    Tab {
        #[command(subcommand)]
        doing: WithTab,
    },

    /// Post messages to other agents, read theirs, and be woken when one arrives for you
    #[command(name = muster_daemon_proto::messaging::NAMESPACE, long_about = crate::messaging::PROTOCOL)]
    Msg {
        #[command(flatten)]
        identity: crate::messaging::Identity,
        #[command(subcommand)]
        verb: crate::messaging::Verb,
    },

    /// Every daemon Muster started on this machine, and whether it is still there
    //
    // Its own verb rather than a section of `muster window`, because the two answer different
    // questions about different things. `window` is about one window and everything in it;
    // this is about the machine, and the daemons worth knowing about are exactly the ones no
    // window is showing.
    Daemons,

    /// Upgrade the Muster app through Homebrew, in a tab of its own, and open it again after
    //
    // Asked of the window rather than run here, because only the app knows whether Homebrew
    // installed it and where, and a pane on a devenv means the app on this side of the ssh.
    // The window prints the pane the upgrade runs in, as `tab new` does.
    Upgrade,

    /// Put the window's keyboard on a pane, or step it somewhere
    //
    // A name, a direction, a place, the pane asking and a step through the history are five ways
    // of saying where, and clap holds them in one group so that two at once is refused before
    // this is read. None of them is the sixth way, which is the pane this is running in.
    Focus {
        /// The pane to go to, or the one this is running in
        #[arg(value_name = "REF", value_parser = reference, group = "somewhere")]
        pane: Option<String>,

        /// Go to the next pane the window is showing, wrapping at the end
        #[arg(long, group = "somewhere")]
        next: bool,
        /// Go to the previous one, wrapping at the start
        #[arg(long, group = "somewhere")]
        previous: bool,
        /// Go to the pane left of this one, if there is one
        #[arg(long, group = "somewhere")]
        left: bool,
        /// Go to the pane to the right
        #[arg(long, group = "somewhere")]
        right: bool,
        /// Go to the pane above
        #[arg(long, group = "somewhere")]
        up: bool,
        /// Go to the pane below
        #[arg(long, group = "somewhere")]
        down: bool,

        /// Go to the pane at this place in the window's pane order, as `muster window` prints it
        #[arg(long, value_name = "N", group = "somewhere")]
        place: Option<u32>,

        /// Go to the pane most urgently asking for somebody, and print it; nothing if none is
        #[arg(long, group = "somewhere")]
        asking: bool,

        /// Go back to the pane the keyboard was on before, and print it; nothing if there is none
        #[arg(long, group = "somewhere")]
        back: bool,
        /// Go forward again after going back, and print the pane; nothing if there is none
        #[arg(long, group = "somewhere")]
        forward: bool,
    },

    /// Fill the region with one pane, or put the others back
    Zoom {
        /// The pane to fill with, or the one this is running in
        #[arg(value_name = "REF", value_parser = reference)]
        pane: Option<String>,
    },

    /// Read ~/.muster/config.toml again, and apply what changed
    Reload,

    /// Show the agent list, or put it away
    Sidebar,

    /// Change the size of the text in the pane the keyboard is on
    Font {
        /// Which way
        #[arg(value_name = "CHANGE")]
        change: FontChange,
    },

    /// Install the adapter Muster ships for a harness, through the harness's own command
    //
    // A namespace of one verb so far: `install` is what a person reaches for, and a harness is
    // the thing it is about. Nothing here talks to a window or a daemon.
    Harness {
        #[command(subcommand)]
        verb: HarnessVerb,
    },

    /// Read Muster's own documentation, which ships inside this binary
    Docs {
        /// A topic, or `all` for every one of them. Omit for the list.
        topic: Option<String>,
    },

    /// Print a completion script for a shell
    Completions {
        /// The shell to write for
        shell: Shell,
    },
}

fn installing(verb: &HarnessVerb) -> Result<Asking, Failure> {
    let HarnessVerb::Install { harness, dry_run } = verb;
    let harness = crate::harness::harness(harness).map_err(Failure::Refused)?;
    Ok(Asking::Install { harness, dry_run: *dry_run })
}

#[derive(Debug, Subcommand)]
pub enum HarnessVerb {
    /// Install a harness's adapter, so its agents report their own state, context and session
    ///
    /// Runs the harness's own plugin commands against the adapters this Muster carries, and
    /// prints what no command of the harness's can do. Never edits a harness's config files.
    Install {
        /// claude-code, codex or opencode
        #[arg(value_name = "HARNESS")]
        harness: String,
        /// Print the commands instead of running them
        #[arg(long)]
        dry_run: bool,
    },
}

/// What `muster window` can do besides describe the one window this is about.
///
/// A subcommand rather than a flag, and optional, so that `muster window` keeps meaning what it
/// has always meant: the window this command is talking to. Everything here is about windows in
/// the plural, which is a different question and the only one `--socket` cannot narrow.
#[derive(Debug, Subcommand)]
enum AboutWindows {
    /// List the open windows under this MUSTER_HOME, and how many panes and tabs each holds
    //
    // "Under this MUSTER_HOME" rather than "on this machine", which is what this said and did not
    // keep: the app is looked for in one state directory, so a Muster launched with a home of its
    // own is invisible here and cannot be reached without spelling out its socket. That a
    // separate home is separate is deliberate; claiming otherwise in the help was not.
    List {
        /// List the closed windows instead, which `muster window reopen NAME` brings back
        #[arg(long)]
        closed: bool,
    },

    /// Open another window, and print its name
    //
    // Asked of the running app, which opens it beside the windows it has; with none running,
    // this starts the app (mip/0006-one-process.md).
    New {
        /// Ask this machine for the window's first tab, by its id as `muster window` prints it,
        /// rather than the first one on this machine
        #[arg(long, value_name = "ID", value_parser = reference, conflicts_with = "tab")]
        daemon: Option<String>,

        /// Open the window onto this tab instead, taking it from whichever window holds it
        #[arg(long, value_name = "TAB", value_parser = reference)]
        tab: Option<String>,
    },

    /// Bring back a closed window, the last one closed unless NAME says which, and print its name
    //
    // The same act as `new` with the arrangement chosen differently: a window somebody asked for
    // takes one nothing has ever held, and this takes the closed window's own, so it comes back
    // onto the tabs it kept.
    Reopen {
        /// The closed window, as `muster window` names it: window-2
        name: Option<String>,
    },

    /// Close a window, keeping its tabs for `muster window reopen`, and print its name
    //
    // The window closes as its close button closes it. The last window open is refused rather
    // than closed, because closing it would quit Muster (mip/0006-one-process.md, section 4).
    Close {
        /// The window, as `muster window list` names it: window-2. Omit for the window this
        /// command is about
        name: Option<String>,
    },
}

/// A state `muster pane wait --until` can name.
///
/// The words `muster window` prints, so a caller can send back what it read. A ValueEnum
/// rather than free text so that `idel` is refused by the command line instead of waiting for a
/// state nothing is ever in.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Awaited {
    Working,
    Blocked,
    Stuck,
    Waiting,
    Idle,
    Done,
    Unknown,
}

impl Awaited {
    fn wire(self) -> &'static str {
        match self {
            Awaited::Working => "working",
            Awaited::Blocked => "blocked",
            Awaited::Stuck => "stuck",
            Awaited::Waiting => "waiting",
            Awaited::Idle => "idle",
            Awaited::Done => "done",
            Awaited::Unknown => "unknown",
        }
    }
}

/// What `muster font` can ask for.
///
/// The schema's own three words rather than English ones like `bigger`, on the same rule the
/// four sides of a split follow: a caller that reads one of these out of an answer should be
/// able to send it straight back.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum FontChange {
    Larger,
    Smaller,
    Reset,
}

impl FontChange {
    fn wire(self) -> &'static str {
        match self {
            FontChange::Larger => "larger",
            FontChange::Smaller => "smaller",
            FontChange::Reset => "reset",
        }
    }
}

#[derive(Debug, Subcommand)]
enum Doing {
    /// Split a pane, and print the name of the one that appears
    New {
        /// Put it to the left
        #[arg(long, group = "side")]
        left: bool,
        /// Put it to the right, which is what a split does when nobody says
        #[arg(long, group = "side")]
        right: bool,
        /// Put it above
        #[arg(long, group = "side")]
        up: bool,
        /// Put it below
        #[arg(long, group = "side")]
        down: bool,

        /// The pane to split, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// The machine to put it on: beside --pane when one is named, or on its own for a
        /// machine you have no pane on
        #[arg(long, value_name = "ID", value_parser = reference)]
        daemon: Option<String>,

        /// Where it starts, or the directory the split came from (home if that is gone)
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,

        /// A shell line for its shell to run as it starts
        #[arg(long, value_name = "CMD")]
        run: Option<String>,

        /// What to call it
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Move the window's keyboard to it
        #[arg(long)]
        focus: bool,

        /// Raise nothing for the person about it: no notification, and nothing for the chord
        /// to whatever is asking to go to. Its state still shows
        #[arg(long)]
        quiet: bool,

        /// Make it hidden from the start, in a tab of its own that no window shows. `pane
        /// show` brings it back
        //
        // Not a split, so the sides are refused rather than ignored, and so is --focus: no
        // window shows the pane, so there is nothing for the keyboard to be on.
        #[arg(long, conflicts_with_all = ["left", "right", "up", "down", "focus"])]
        hidden: bool,

        /// Make it a helper of the pane this runs in: an agent you start for your own work,
        /// listed under you, hidden in a tab your helpers share unless --shown, and quiet unless
        /// --notify, since you are told when it needs somebody
        #[arg(long, group = "helping")]
        helper: bool,

        /// Make it a helper of this pane rather than the one this runs in, splitting from it
        /// unless --pane says otherwise
        #[arg(long, value_name = "REF", value_parser = reference, group = "helping")]
        helper_of: Option<String>,

        /// Split a helper beside the pane it came from, as any pane, instead of hiding it
        #[arg(long, requires = "helping", conflicts_with = "hidden")]
        shown: bool,

        /// Let a helper notify the person as any pane does, instead of making it quiet
        #[arg(long, requires = "helping", conflicts_with = "quiet")]
        notify: bool,
    },

    /// Call a pane something. An empty name takes the name away again
    Rename {
        /// The pane to rename, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// What to call it, joined with spaces if it arrives in pieces
        #[arg(required = true, value_name = "NAME")]
        name: Vec<String>,
    },

    /// Compact the context of a pane's agent, once it is idle at an empty prompt
    //
    // Never typed while the agent works, so an agent compacting itself is compacted once its
    // turn ends. The daemon types it, under the rules it rings an agent by.
    Compact {
        /// The pane whose agent to compact, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// What the summary should keep, joined with spaces if it arrives in pieces, where the
        /// harness takes one
        #[arg(value_name = "FOCUS")]
        focus: Vec<String>,

        /// A prompt to type once the compaction has run, so the agent carries on
        #[arg(long, value_name = "PROMPT")]
        then: Option<String>,
    },

    /// Restart a pane's agent in the session it reported, with the flags it ran with
    //
    // Asks nothing, unlike the window: an agent drives this. The daemon empties what waits for
    // the agent to read, ends it, and types the session's resume at its shell.
    Restart {
        /// The pane whose agent to restart, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// A variable to set for the resumed agent, as NAME=value; give it more than once for more
        #[arg(long, value_name = "NAME=VALUE", value_parser = variable)]
        env: Vec<(String, String)>,
    },

    /// Take a pane, or a pane's helpers, out of every window, its agent still running
    Hide {
        /// The pane to hide, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// Hide this pane's helpers instead, whose tab goes back whole; a helper moved into a
        /// tab with anything else in it stays
        #[arg(long, value_name = "REF", value_parser = reference, conflicts_with = "pane")]
        helpers_of: Option<String>,
    },

    /// Bring a hidden pane back with the hidden tab holding it, or a pane's helpers together
    #[command(group = ArgGroup::new("shown").required(true))]
    Show {
        /// The pane to bring back. `muster window` lists the hidden ones
        //
        // Required, and $MUSTER_PANE is not read: the pane this runs in is the one somebody is
        // looking at, so it is not the hidden one.
        #[arg(long, group = "shown", value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// Bring this pane's hidden helpers back instead, together in the tab they share, just
        /// after the tab holding this pane and arranged as they were
        #[arg(long, group = "shown", value_name = "REF", value_parser = reference)]
        helpers_of: Option<String>,

        /// Move the window's keyboard to it, bringing its tab on screen
        #[arg(long)]
        focus: bool,
    },

    /// Make a pane quiet, so that nothing about it is raised for the person, or with --off let
    /// it notify again
    Quiet {
        /// The pane to make quiet, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// Let it notify again
        #[arg(long)]
        off: bool,
    },

    /// Type text into a pane and press keys in it, whether or not anything is showing it
    //
    // In a fixed order - the text, each key, then Return - because clap does not say how flags
    // and the text were interleaved, and nothing measured needed a key ahead of text. A caller
    // who does sends twice.
    Send {
        /// The pane to type into, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// Press a key after the text, as a keystroke rather than pasted, so a dialog or menu
        /// reads it: down, up, esc, tab, shift+tab, a digit or letter, ctrl+c. Give it once per
        /// key
        //
        // Spelled as a keybinding in the config file is, by the same parser, so there is one
        // vocabulary for keys and its refusal already lists the names. No comma-separated form,
        // since a comma is itself a key.
        #[arg(long = "key", value_name = "KEY")]
        keys: Vec<String>,

        /// Press Return last. Whether that submits is the harness's to decide
        #[arg(long)]
        enter: bool,

        /// Exit non-zero unless the text shows on the pane more than it did before, or for keys
        /// alone, unless the pane changed
        #[arg(long)]
        confirm: bool,

        /// Type the text even when the agent's prompt already holds something, adding to it.
        /// Without it, text sent into a prompt holding anything is refused, since the next
        /// Return would submit both as one prompt
        #[arg(long)]
        append: bool,

        /// Send what this file holds instead, less its trailing newlines
        //
        // Beside the positional rather than instead of it, because the two costs are different
        // callers' (kan a_2M9T8iOgk). Text with quotes in it has to survive every shell between
        // the caller and this command, and a file crosses none of them - including the one
        // `laptop run` opens, where stdin already carries the command itself.
        #[arg(long, value_name = "PATH", conflicts_with = "text")]
        file: Option<String>,

        /// The text, joined with spaces if it arrives in pieces. `-` reads it from stdin
        #[arg(required_unless_present_any = ["file", "keys", "enter"], value_name = "TEXT")]
        text: Vec<String>,
    },

    /// Wait until a pane's agent is in a state you name, or its context is so full, and print
    /// which pane got there
    //
    // The other shape of waiting on an agent: `window --watch` hears every change, and this
    // exits once, which is what a script's next line and a background job's one notification
    // want. A condition rather than an event, so a pane already there answers at once.
    Wait {
        /// The pane to wait on. Give it more than once to wait for the first of several
        //
        // Required, unlike every other pane command, and $MUSTER_PANE is not read: the pane
        // this is running in is running this, so waiting on it has no useful answer.
        #[arg(long, value_name = "REF", value_parser = reference, required = true)]
        pane: Vec<String>,

        /// The states to wait for, comma-separated. idle is also met by done, and not by waiting
        #[arg(
            long,
            value_name = "STATE",
            required_unless_present = "context",
            value_delimiter = ','
        )]
        until: Vec<Awaited>,

        /// Also end the wait once the agent says its context is at least this many percent full
        //
        // A flag of its own rather than a word in --until: `--until context>80` is a shell
        // redirect, which would wait on `context` and write a file called 80.
        #[arg(
            long,
            value_name = "PERCENT",
            value_parser = clap::value_parser!(u8).range(1..=100)
        )]
        context: Option<u8>,

        /// Give up after this many seconds, exiting 5
        #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,
    },

    /// Print what a pane has on it, as far back as the window will go
    //
    // The half of a pane an agent could not see. `muster window` says what state every agent
    // is in and what it claims to be doing; neither of those is the output, and until now
    // there was no way to read it at all.
    Read {
        /// The pane to read, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// How many rows back to ask for. Omit for as far as the window will go
        #[arg(long, value_name = "N", conflicts_with = "turn")]
        rows: Option<u32>,

        /// Only what the pane's agent printed since it last went to work: its report, without
        /// the turn before it or the line that started it
        #[arg(long)]
        turn: bool,
    },

    /// Give a pane a bridge, for one the window has stopped drawing while its agent runs on
    //
    // The verb that was missing when a pane went dark. `close` ends the agent by design, and
    // quitting and reopening the app reattaches every pane at once. This asks for the one thing
    // a dark pane actually needs, which is a bridge - and it is the same ask the window makes
    // on its own a few seconds after nothing dials one.
    Reattach {
        /// The pane to reattach, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,
    },

    /// Close a pane, which ends what is running in it
    Close {
        /// The pane to close, or the one this is running in - which ends this command's own shell
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,
    },

    /// Put a pane where another one is, without ending what is running in either
    //
    // One verb for both outcomes, because the window has one request for them: which of the
    // two a move becomes is worked out from where the panes are, and a CLI that chose would
    // be a second place that rule lives. Named `move` rather than `arrange` because moving is
    // what somebody wants; the exchange is what they get when both panes are in one tab, and
    // the help and `muster docs agents` say so.
    #[command(group = ArgGroup::new("somewhere").required(true))]
    Move {
        /// The pane to move, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// Where to put it: beside this pane with a side, or else in the same tab the two swap
        /// and in another it lands after this one
        #[arg(long, group = "somewhere", value_name = "REF", value_parser = reference)]
        onto: Option<String>,

        /// Put it in this tab, which may be one holding panes on another machine
        //
        // The one destination that crosses machines, because a tab is Muster's grouping rather
        // than a daemon's: the pane stays on its own machine and changes which tab it is in.
        // `--onto` cannot, because two panes in one tree are one machine's.
        #[arg(long, group = "somewhere", value_name = "REF", value_parser = reference)]
        tab: Option<String>,

        /// Give it a tab of its own instead, made by the move
        #[arg(long, group = "somewhere")]
        new_tab: bool,

        /// What to call that tab, with --new-tab
        //
        // Refused against `--onto` rather than made to require `--new-tab`, which reads the
        // same way and is not: a bool flag is present in clap's sense whether or not it was
        // typed, so a requirement on one is a requirement nothing can fail. The group above
        // is required, so refusing the other destination leaves exactly this one.
        #[arg(long, conflicts_with = "onto", value_name = "NAME")]
        name: Option<String>,

        /// Put it left of the --onto pane, sharing that pane's space, in whichever tab it is in
        //
        // Refused against the other two destinations rather than made to require `--onto`, for
        // the reason `--name` is: `requires` does not fire while another member of the required
        // group above is present, and refusing those two leaves exactly `--onto`.
        #[arg(long, group = "beside", conflicts_with_all = ["tab", "new_tab"])]
        left: bool,
        /// Put it right of the --onto pane
        #[arg(long, group = "beside", conflicts_with_all = ["tab", "new_tab"])]
        right: bool,
        /// Put it above the --onto pane
        #[arg(long, group = "beside", conflicts_with_all = ["tab", "new_tab"])]
        up: bool,
        /// Put it below the --onto pane
        #[arg(long, group = "beside", conflicts_with_all = ["tab", "new_tab"])]
        down: bool,
    },

    /// Move the divider beside a pane, or even the panes around it out
    //
    // `--equalize` joins the four directions in one required group rather than standing beside
    // them, because they are five answers to one question - what should happen to this pane's
    // share - and clap refusing two of them at once is the same rule the four already had.
    //
    // A scope is refused against the four directions rather than made to require `--equalize`,
    // which reads the same way and is not: a bool flag is present in clap's sense whether or not
    // it was typed, so a requirement on one is a requirement nothing can fail. The group above is
    // required, so refusing the four leaves exactly `--equalize` - the same shape `pane move`
    // already uses for `--name`.
    #[command(group = ArgGroup::new("towards").required(true))]
    #[command(group = ArgGroup::new("within").conflicts_with_all(["left", "right", "up", "down"]))]
    Resize {
        /// Grow it leftwards
        #[arg(long, group = "towards")]
        left: bool,
        /// Grow it rightwards
        #[arg(long, group = "towards")]
        right: bool,
        /// Grow it upwards
        #[arg(long, group = "towards")]
        up: bool,
        /// Grow it downwards
        #[arg(long, group = "towards")]
        down: bool,

        /// Even out the panes around it instead, moving every divider that takes
        #[arg(long, group = "towards")]
        equalize: bool,

        /// With --equalize: only the panes beside it
        #[arg(long, group = "within")]
        row: bool,

        /// With --equalize: only the panes above and below it
        #[arg(long, group = "within")]
        column: bool,

        /// The pane to grow, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference)]
        pane: Option<String>,

        /// How far, as a share of the region between 0 and 1. Omit for the window's own step
        //
        // Refused against `--equalize` rather than ignored beside it. An equalize's shares come
        // out of how many panes hang off each divider, so a caller that named a fraction as well
        // meant one of the two and would otherwise be told neither which.
        #[arg(long, value_name = "FRACTION", conflicts_with = "equalize")]
        by: Option<f32>,
    },
}

/// What `muster tab` can do.
///
/// Far fewer verbs than a pane has, and the gaps are the point: nothing types into a tab or gives
/// one a bridge - those are things you do to the pane inside it. Moving one is between windows,
/// because a tab belongs to exactly one.
#[derive(Debug, Subcommand)]
enum WithTab {
    /// Make a tab, and print the name of the pane that appears in it
    //
    // Prints the pane rather than the tab, because the pane is what a script's next line
    // needs: naming a tab is something you do once, and sending into its pane is what comes
    // next. The tab's own name is one `muster window` away, on the row of the pane below.
    New {
        /// The pane whose machine the tab is made on, or the one this is running in
        #[arg(long, value_name = "REF", value_parser = reference, group = "somewhere")]
        pane: Option<String>,

        /// The machine to make it on, instead of naming a pane in it
        #[arg(long, value_name = "ID", value_parser = reference, group = "somewhere")]
        daemon: Option<String>,

        /// Where its pane starts, or that pane's own directory (home if that is gone)
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,

        /// A shell line for its shell to run as it starts
        #[arg(long, value_name = "CMD")]
        run: Option<String>,

        /// What to call its pane
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Bring it on screen with the window's keyboard in it; without this it is made behind
        /// the tab on screen
        #[arg(long)]
        focus: bool,
    },

    /// Bring a tab on screen and put the window's keyboard in it
    //
    // A name or a direction, and one of them is required: a pane command with no ref means the
    // pane it is running in, and there is no such answer for a tab. Nothing tells a pane which
    // tab it is in, and "the tab the keyboard is already in" is not somewhere to ask to go.
    #[command(group = ArgGroup::new("which").required(true))]
    Focus {
        /// The tab to go to
        #[arg(value_name = "REF", value_parser = reference, group = "which")]
        tab: Option<String>,

        /// Go to the next tab instead, wrapping at the end
        #[arg(long, group = "which")]
        next: bool,
        /// Go to the previous one, wrapping at the start
        #[arg(long, group = "which")]
        previous: bool,
    },

    /// Close a tab, and every pane in it
    //
    // The one verb here that ends more than it names. Beside `pane close` in `muster docs
    // limits` rather than beside the tab's other verbs, because what it costs is the same.
    Close {
        /// The tab to close, or the one the window's keyboard is in
        #[arg(long, value_name = "REF", value_parser = reference)]
        tab: Option<String>,
    },

    /// Hand a tab to another window, with every pane in it still running
    //
    // Where it goes is the global `--window`: a window's name, as `muster window` prints it, open
    // or closed. The app's pid is still taken, from when each window was a process of its own,
    // and means the window in front. Without it the tab comes here - into the window this command
    // reaches - and comes on screen.
    Move {
        /// The tab to move, or the one the window is showing. `--window` says where it goes
        #[arg(long, value_name = "REF", value_parser = reference)]
        tab: Option<String>,
    },

    /// Call a tab something. An empty name takes the name away again
    Rename {
        /// The tab to rename, or the one the window's keyboard is in
        #[arg(long, value_name = "REF", value_parser = reference)]
        tab: Option<String>,

        /// What to call it, joined with spaces if it arrives in pieces
        #[arg(required = true, value_name = "NAME")]
        name: Vec<String>,
    },
}

/// Reads a command line, or says why it cannot be one.
///
/// `here` is the directory the command was run in, which is what a relative path on it means.
/// Passed rather than read, like the environment beside it: the one place that touches the
/// process is `main`, and a corpus case can then say where a command line was typed.
pub fn parse(
    argv: &[String],
    environment: &BTreeMap<String, String>,
    here: Option<&Path>,
) -> Result<Invocation, Failure> {
    // The program name clap expects at argv[0], supplied here rather than taken from the process:
    // a caller reached through a symlink or a wrapper would otherwise see that name in its own
    // help and completions, which is a different command from the one these documents.
    let words = std::iter::once("muster".to_string()).chain(argv.iter().cloned());
    let cli = Cli::try_parse_from(words).map_err(|error| Failure::Usage(Box::new(error)))?;

    let asking = match &cli.what {
        What::Window { watch: true, layout: true, .. } => Asking::WatchLayout {
            watch: Box::new(Request::new(request::Payload::WatchPanes(WatchPanes {
                layout: true,
                ..WatchPanes::default()
            }))),
            read: Box::new(Request::new(request::Payload::ReadWindow(ReadWindow { layout: true }))),
        },
        What::Window { watch: true, .. } => Asking::Watch {
            request: Box::new(Request::new(request::Payload::WatchPanes(WatchPanes::default()))),
            timeout: None,
        },
        What::Window { doing: None, layout, .. } => {
            send(request::Payload::ReadWindow(ReadWindow { layout: *layout }))
        }
        What::Daemons => send(request::Payload::ReadDaemons(ReadDaemons {})),
        What::Upgrade => send(request::Payload::UpgradeMuster(UpgradeMuster {})),
        // The window's verb among the daemon's: a transcript is a pane, which only a window
        // can open. No daemon is named, which the core reads as this machine's: the human is
        // homed here, and a group kept elsewhere is followed here as `group@machine`.
        What::Msg { verb: crate::messaging::Verb::Open { group }, .. } => {
            send(request::Payload::OpenTranscript(OpenTranscript {
                daemon_id: String::new(),
                group: group.clone(),
            }))
        }
        What::Msg { identity, verb } => {
            Asking::Message(Box::new(crate::messaging::parse(verb, identity, environment, here)?))
        }
        // Asked of every window rather than of one, which is why it is not a `Send`: `--socket`
        // and $MUSTER_SOCKET both narrow to one window, and the question here is which there are.
        What::Window { doing: Some(AboutWindows::List { closed }), .. } => {
            Asking::Survey { closed: *closed }
        }
        What::Window { doing: Some(AboutWindows::New { daemon, tab }), .. } => {
            Asking::MakeWindow(crate::opening::Onto { daemon: daemon.clone(), tab: tab.clone() })
        }
        What::Window { doing: Some(AboutWindows::Reopen { name }), .. } => {
            Asking::ReopenWindow(name.clone())
        }
        What::Window { doing: Some(AboutWindows::Close { name }), .. } => {
            Asking::CloseWindow(closing(name.as_deref(), cli.window.as_deref())?)
        }
        What::Pane { doing } => pane(doing, environment, here)?,
        What::Tab { doing } => tab(doing, environment, here, cli.window.as_deref())?,
        What::Focus { asking: true, .. } => send(request::Payload::FocusAsking(FocusAsking {})),
        What::Focus { back: true, .. } => {
            send(request::Payload::FocusHistory(FocusHistory { forward: false }))
        }
        What::Focus { forward: true, .. } => {
            send(request::Payload::FocusHistory(FocusHistory { forward: true }))
        }
        What::Focus { pane, next, previous, left, right, up, down, place, .. } => {
            // A direction and a place are answers on their own, so they are read before the
            // pane is - and clap has already refused any two of the three together.
            let stepped = chosen(&[
                (*next, "next"),
                (*previous, "previous"),
                (*left, "left"),
                (*right, "right"),
                (*up, "up"),
                (*down, "down"),
            ]);
            focus(pane.as_deref(), stepped, *place, environment)?
        }
        What::Zoom { pane } => send(request::Payload::ZoomPane(ZoomPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ZoomPane::default()
        })),
        What::Reload => send(request::Payload::ReloadConfig(ReloadConfig {})),
        What::Sidebar => send(request::Payload::ToggleSidebar(ToggleSidebar {})),
        What::Font { change } => send(request::Payload::AdjustFontSize(AdjustFontSize {
            change: change.wire().to_string(),
        })),
        What::Harness { verb } => installing(verb)?,
        What::Docs { topic } => Asking::Print(documentation(topic.as_deref())?),
        What::Completions { shell } => Asking::Print(completions(*shell)),
    };

    let asking = match (&cli.window, &cli.what) {
        (None, _)
        | (
            Some(_),
            What::Tab { doing: WithTab::Move { .. } }
            | What::Window { doing: Some(AboutWindows::Close { .. }), .. },
        ) => asking,
        (Some(window), _) => for_window(asking, window)?,
    };
    Ok(Invocation { asking, json: cli.json, socket: cli.socket, no_window: cli.no_window })
}

/// Where `muster focus` puts the keyboard: a step, a place, or a pane, in that order.
fn focus(
    pane: Option<&str>,
    stepped: Option<&'static str>,
    place: Option<u32>,
    environment: &BTreeMap<String, String>,
) -> Result<Asking, Failure> {
    Ok(if let Some(step) = stepped {
        send(request::Payload::FocusRelative(FocusRelative { direction: step.to_string() }))
    } else if let Some(place) = place {
        send(request::Payload::FocusPaneAt(FocusPaneAt { place }))
    } else {
        // The only command where an empty pane is not an answer: everywhere else the
        // core reads it as "the pane the keyboard is on", and asking to focus the
        // focused pane is not something to ask for. So it is refused here, where the
        // reason is known.
        let named =
            pane.map(str::to_string).or_else(|| running_in(environment)).ok_or_else(|| {
                Failure::Refused(format!(
                    "`muster focus` needs a pane, a direction or a place, and ${} is not \
                     set - so this is not running inside a pane Muster made. Name one: \
                     `muster focus p1w3r07bsd`, or say `--next`. `muster window` lists \
                     them.",
                    environment::PANE_NAME
                ))
            })?;
        send(request::Payload::FocusPane(FocusPane { pane_id: named, ..FocusPane::default() }))
    })
}

/// The window `window close` closes: the one it names, which `--window` may name instead. Two
/// different names are refused rather than one picked, since either could be the one meant.
fn closing(name: Option<&str>, window: Option<&str>) -> Result<Option<String>, Failure> {
    match (name, window) {
        (Some(name), Some(window)) if name != window => Err(Failure::Refused(format!(
            "`muster window close {name}` and --window {window} name two windows, so nothing \
             was closed. Name one."
        ))),
        (name, window) => Ok(name.or(window).map(str::to_string)),
    }
}

/// The request, for one window by name.
///
/// Refused for a command that is not about one window - listing them, opening one, a message for
/// the daemon - rather than ignored: a flag that silently does nothing is one nobody finds out
/// did nothing.
fn for_window(asking: Asking, window: &str) -> Result<Asking, Failure> {
    // What each refusal says instead is what the command does without the flag. Reopening the
    // window is offered only where that is the command: open or closed, reopening it would not
    // make any of the others take --window.
    let instead = match asking {
        Asking::Send(request) => return Ok(Asking::Send(Box::new(request.for_window(window)))),
        Asking::SendFrom { request, from } => {
            return Ok(Asking::SendFrom { request: Box::new(request.for_window(window)), from });
        }
        Asking::Watch { request, timeout } => {
            return Ok(Asking::Watch { request: Box::new(request.for_window(window)), timeout });
        }
        Asking::WatchLayout { watch, read } => {
            return Ok(Asking::WatchLayout {
                watch: Box::new(watch.for_window(window)),
                read: Box::new(read.for_window(window)),
            });
        }
        Asking::ReopenWindow(_) => format!(
            "`muster window reopen` takes the window as its argument: `muster window reopen \
             {window}`"
        ),
        Asking::Message(_) => {
            "`muster msg` reaches an agent by its pane on every machine, whichever window shows it"
                .to_string()
        }
        Asking::Survey { .. } => "`muster window list` lists every window".to_string(),
        Asking::MakeWindow(_) => {
            "`muster window new` opens a window rather than acting in one".to_string()
        }
        Asking::Print(_) | Asking::CloseWindow(_) | Asking::Install { .. } => {
            "this command is not about a window".to_string()
        }
    };
    Err(Failure::Refused(format!(
        "--window {window} names the window a command is about, so nothing was done: {instead}."
    )))
}

#[expect(clippy::too_many_lines, reason = "one arm per pane verb, each a request")]
fn pane(
    doing: &Doing,
    environment: &BTreeMap<String, String>,
    here: Option<&Path>,
) -> Result<Asking, Failure> {
    Ok(match doing {
        Doing::New {
            left,
            right,
            up,
            down,
            pane,
            daemon,
            cwd,
            run,
            name,
            focus,
            quiet,
            hidden,
            helper,
            helper_of,
            shown,
            notify,
        } => {
            // The same four words the schema uses, and the same words a `muster window --json`
            // answer carries, rather than English ones like `below`: a caller that reads a side out
            // should be able to send it straight back. Saying nothing means right, because that is
            // where ⌘D splits to, and a CLI whose default matched no chord would make the two
            // disagree about what "a split" means. clap holds the four in one group, so two at
            // once is refused before this is reached.
            let sides = [*left, *right, *up, *down];
            let side = chosen_side(sides).unwrap_or("right");
            let maker = maker(*helper, helper_of.as_ref(), environment)?;
            // A helper is hidden unless it is shown, and then refuses what `--hidden` refuses.
            if maker.is_some() && !*shown && (*focus || sides.contains(&true)) {
                return Err(Failure::Refused(HIDDEN_HELPER_PLACED.to_string()));
            }
            let hidden = *hidden || (maker.is_some() && !*shown);
            let quiet = *quiet || (maker.is_some() && !*notify);
            // A helper starts from its maker, beside it when shown and in its directory when not.
            let pane = pane.as_ref().or(helper_of.as_ref());
            let (pane_id, daemon_id, new_pane_daemon_id) =
                split_target(pane, daemon.as_ref(), environment);
            send(request::Payload::SplitPane(SplitPane {
                pane_id,
                daemon_id,
                new_pane_daemon_id,
                side: side.to_string(),
                cwd: directory(cwd.as_ref(), daemon.as_ref(), here)?,
                run: run.clone().unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                take_focus: *focus,
                quiet,
                hidden,
                maker: maker.unwrap_or_default(),
                ..SplitPane::default()
            }))
        }
        Doing::Hide { helpers_of: Some(maker), .. } => {
            send(request::Payload::HideHelpers(HideHelpers {
                pane_id: maker.clone(),
                ..HideHelpers::default()
            }))
        }
        Doing::Hide { pane, helpers_of: None } => send(request::Payload::HidePane(HidePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..HidePane::default()
        })),
        Doing::Show { helpers_of: Some(maker), focus, .. } => {
            send(request::Payload::ShowHelpers(ShowHelpers {
                pane_id: maker.clone(),
                take_focus: *focus,
                ..ShowHelpers::default()
            }))
        }
        Doing::Show { pane, focus, helpers_of: None } => {
            send(request::Payload::ShowPane(ShowPane {
                pane_id: pane.clone().unwrap_or_default(),
                take_focus: *focus,
                ..ShowPane::default()
            }))
        }
        Doing::Quiet { pane, off } => send(request::Payload::QuietPane(QuietPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            quiet: Some(!*off),
            ..QuietPane::default()
        })),
        Doing::Rename { pane, name } => {
            if pane.is_none() {
                let meant = if running_in(environment).is_some() {
                    "the pane this runs in"
                } else {
                    "the pane the keyboard is on"
                };
                target_in_name(
                    Renamed { verb: "pane", flag: "--pane", noun: "a pane's id" },
                    meant,
                    name,
                )?;
            }
            send(request::Payload::RenamePane(RenamePane {
                pane_id: pane_ref(pane.as_ref(), environment),
                name: name.join(" "),
                ..RenamePane::default()
            }))
        }
        Doing::Compact { pane, focus, then } => {
            compact(pane.as_ref(), focus, then.as_deref(), environment)
        }
        Doing::Restart { pane, env } => send(request::Payload::RestartAgent(RestartAgent {
            pane_id: pane_ref(pane.as_ref(), environment),
            env: env.iter().cloned().collect(),
            ..RestartAgent::default()
        })),
        Doing::Send { pane, keys, enter, confirm, append, file, text } => {
            check_sent_keys(keys).map_err(Failure::Refused)?;
            // Only a hyphen standing alone reads stdin. One inside a sentence is text, so
            // `muster pane send a - b` still means what it says.
            let from = match file {
                Some(named) => Some(TextSource::File(file_to_read(named, here)?)),
                None if text == &["-"] => Some(TextSource::Stdin),
                None => None,
            };
            let request = Box::new(Request::new(request::Payload::SendToPane(SendToPane {
                pane_id: pane_ref(pane.as_ref(), environment),
                text: if from.is_some() { String::new() } else { text.join(" ") },
                enter: *enter,
                confirm: *confirm,
                keys: keys.clone(),
                append: *append,
                ..SendToPane::default()
            })));
            match from {
                Some(from) => Asking::SendFrom { request, from },
                None => Asking::Send(request),
            }
        }
        Doing::Wait { pane, until, context, timeout } => wait(pane, until, *context, *timeout),
        Doing::Read { pane, rows, turn } => send(request::Payload::ReadPane(ReadPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            // Zero is what the window reads as "as far as you will go", and it is also what
            // proto3 sends for an absent number - so omitting `--rows` and asking for
            // everything are the same request.
            rows: rows.unwrap_or_default(),
            turn: *turn,
            ..ReadPane::default()
        })),
        Doing::Reattach { pane } => send(request::Payload::ReattachPane(ReattachPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ReattachPane::default()
        })),
        Doing::Close { pane } => send(request::Payload::ClosePane(ClosePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ClosePane::default()
        })),
        // clap holds the three destinations in one required group, so exactly one is set here.
        Doing::Move { pane, onto, tab, new_tab, name, left, right, up, down } => {
            send(request::Payload::ArrangePane(ArrangePane {
                pane_id: pane_ref(pane.as_ref(), environment),
                onto_pane_id: onto.clone().unwrap_or_default(),
                tab_id: tab.clone().unwrap_or_default(),
                new_tab: *new_tab,
                tab_name: name.clone().unwrap_or_default(),
                // No side is the older move rather than a default one: the two panes trade
                // places in one tab, and in another the pane lands after the one named.
                side: chosen_side([*left, *right, *up, *down]).unwrap_or_default().to_string(),
                ..ArrangePane::default()
            }))
        }
        Doing::Resize { left, right, up, down, equalize, row, column, pane, by } => {
            let pane_id = pane_ref(pane.as_ref(), environment);
            if *equalize {
                return Ok(send(request::Payload::EqualizePanes(EqualizePanes {
                    pane_id,
                    // Empty is the whole tab, which is what the schema reads it as, so a caller
                    // that narrowed nothing sends nothing. clap holds the two narrower scopes in
                    // one group, so at most one is true.
                    scope: chosen(&[(*row, "row"), (*column, "column")])
                        .unwrap_or_default()
                        .to_string(),
                    ..EqualizePanes::default()
                })));
            }
            // The `towards` group is required and `--equalize` is in it and handled above, so
            // exactly one of the four is true here - which is why this refuses rather than
            // falling back on a side. A default would be a real answer to a question nobody
            // asked, and the day somebody edits that group it would resize a pane rightwards
            // and report success. Unlike `pane new`, where right *is* what a split with no side
            // means.
            let direction =
                chosen(&[(*left, "left"), (*right, "right"), (*up, "up"), (*down, "down")])
                    .ok_or_else(|| {
                        Failure::Refused(
                            "`muster pane resize` reached the request with no direction and no \
                     `--equalize`, and nothing was asked of the window. clap refuses that \
                     command line before this is reached, so this is a bug in muster's own \
                     argument definition rather than in what you typed - the `towards` group in \
                     args.rs no longer holds all four sides."
                                .to_string(),
                        )
                    })?;
            send(request::Payload::ResizePane(ResizePane {
                pane_id,
                direction: direction.to_string(),
                // Zero is what the schema reads as "the window's own step", and it is also
                // what proto3 sends for an absent float - so omitting `--by` and asking for
                // nothing are the same request, which is the answer both callers want.
                amount: by.unwrap_or_default(),
                // The four measurements stay at zero. They belong to a live surface, and a
                // caller with no surface gets the daemon's own step rather than a distance
                // guessed from a font nobody here knows.
                ..ResizePane::default()
            }))
        }
    })
}

/// A `pane compact`, about the pane named or the one this runs in.
fn compact(
    pane: Option<&String>,
    focus: &[String],
    then: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Asking {
    send(request::Payload::CompactPane(CompactPane {
        pane_id: pane_ref(pane, environment),
        focus: focus.join(" "),
        then: then.unwrap_or_default().to_string(),
        ..CompactPane::default()
    }))
}

/// A `pane wait`: a watch that ends when a named pane gets somewhere, or when the caller's
/// patience does.
fn wait(panes: &[String], until: &[Awaited], context: Option<u8>, timeout: Option<u64>) -> Asking {
    Asking::Watch {
        request: Box::new(Request::new(request::Payload::WatchPanes(WatchPanes {
            pane_ids: panes.to_vec(),
            until: until.iter().map(|state| state.wire().to_string()).collect(),
            layout: false,
            context_at_least: context.map(f32::from),
        }))),
        timeout: timeout.map(Duration::from_secs),
    }
}

/// No environment, unlike [`pane`] beside it.
///
/// A tab name is not in any pane's environment - nothing has to tell a tab which tab it is - so
/// there is nothing here to fall back to. `focus` demands one; `rename` leaves the field empty,
/// which the schema already reads as the tab the keyboard's pane is in.
fn tab(
    doing: &WithTab,
    environment: &BTreeMap<String, String>,
    here: Option<&Path>,
    window: Option<&str>,
) -> Result<Asking, Failure> {
    Ok(match doing {
        WithTab::New { pane, daemon, cwd, run, name, focus } => {
            let (pane_id, daemon_id) =
                pane_and_machine(pane.as_ref(), daemon.as_ref(), environment);
            send(request::Payload::CreateTab(CreateTab {
                pane_id,
                daemon_id,
                cwd: directory(cwd.as_ref(), daemon.as_ref(), here)?,
                run: run.clone().unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                take_focus: *focus,
                ..CreateTab::default()
            }))
        }
        WithTab::Focus { tab, next, previous } => {
            // A direction is an answer on its own; clap has already refused a name beside one,
            // and required one of the two when no name was given.
            if let Some(direction) = chosen(&[(*next, "next"), (*previous, "previous")]) {
                send(request::Payload::FocusTabRelative(FocusTabRelative {
                    direction: direction.to_string(),
                }))
            } else {
                send(request::Payload::FocusTab(FocusTab {
                    tab_id: tab.clone().unwrap_or_default(),
                    ..FocusTab::default()
                }))
            }
        }
        WithTab::Close { tab } => send(request::Payload::CloseTab(CloseTab {
            tab_id: tab.clone().unwrap_or_default(),
            ..CloseTab::default()
        })),
        WithTab::Move { tab } => send(request::Payload::MoveTab(MoveTab {
            tab_id: tab.clone().unwrap_or_default(),
            window: window.unwrap_or_default().to_string(),
        })),
        WithTab::Rename { tab, name } => {
            if tab.is_none() {
                let renamed = Renamed { verb: "tab", flag: "--tab", noun: "a tab's name" };
                target_in_name(renamed, "the tab the window's keyboard is in", name)?;
            }
            send(request::Payload::RenameTab(RenameTab {
                tab_id: tab.clone().unwrap_or_default(),
                name: name.join(" "),
                ..RenameTab::default()
            }))
        }
    })
}

/// What a rename is of, as its refusal spells it.
#[derive(Clone, Copy)]
struct Renamed {
    verb: &'static str,
    flag: &'static str,
    noun: &'static str,
}

/// Refuses a rename with no target whose new name starts with one.
///
/// NAME takes every word, so `muster tab rename t29vgmaqym CHAT NOTES` meant for that tab renames
/// whichever tab the keyboard is in - another session's, often - and says nothing (kan
/// a_2dntpGvyP). Refused rather than taken as the target: a word that only looks minted would
/// then rename something nobody meant.
fn target_in_name(renamed: Renamed, meant: &str, name: &[String]) -> Result<(), Failure> {
    let Renamed { verb, flag, noun } = renamed;
    let name = name.join(" ");
    let (first, rest) = name.trim().split_once(char::is_whitespace).unwrap_or((name.trim(), ""));
    if !could_be_minted(first, verb.chars().next().unwrap_or_default(), SystemTime::now()) {
        return Ok(());
    }
    let rest = match rest.trim() {
        "" => "NAME",
        rest => rest,
    };
    Err(Failure::Refused(format!(
        "nothing was renamed: `{first}` is {noun}, and NAME is only what to call it, so this \
         would have renamed {meant} to `{name}`. To rename {first}: `muster {verb} rename \
         {flag} {first} {rest}`"
    )))
}

/// The first of these words whose flag was given.
///
/// One helper for every direction in the surface - the four sides of a split and a resize, the
/// six steps a focus takes, the two a tab takes - because they are the schema's own words and
/// have to keep meaning the same thing wherever they are spelled. clap holds each set in one
/// group, so at most one is ever true and the order here only decides what a bug would look like.
fn chosen(among: &[(bool, &'static str)]) -> Option<&'static str> {
    among.iter().find(|(said, _)| *said).map(|(_, word)| *word)
}

/// Which of the four side flags was given - left, right, up and down, in that order - spelled
/// as the core reads a side.
fn chosen_side([left, right, up, down]: [bool; 4]) -> Option<&'static str> {
    chosen(&[(left, "left"), (right, "right"), (up, "up"), (down, "down")])
}

/// Where a pane a command makes should start, as a path the far side can act on.
///
/// The one field whose meaning depends on where the command was typed, and the one this CLI has
/// to resolve rather than carry: a relative path is relative to the caller, and the caller is
/// the only thing in this picture that knows where that is. The window is another process in
/// another directory, and the daemon is a third - so `../muster-1` sent as typed is resolved
/// against a directory nobody chose, which is a pane in a home directory and an exit code of 0
/// (kan a_2LMRCLaap). The same reason [`pane_ref`] reads `$MUSTER_PANE` here rather than leaving
/// the window to guess which pane somebody meant.
///
/// Three paths are refused rather than resolved, because for each of them every available answer
/// would be somebody's guess:
///
/// - A tilde, which the shell expands and this does not. One that arrives here arrives quoted,
///   and joining it produces a directory nobody has.
/// - A relative path beside `--daemon`, which names a machine whose filesystem this command
///   cannot see and often is not even the same operating system as.
/// - A relative path with no working directory to resolve it against, which is a shell whose own
///   directory has been deleted underneath it.
///
/// Empty stays empty. That is what the schema reads as the directory of the pane being split,
/// which is what a split with no `--cwd` means and what the chord does.
fn directory(
    named: Option<&String>,
    daemon: Option<&String>,
    here: Option<&Path>,
) -> Result<String, Failure> {
    let Some(named) = named.filter(|path| !path.is_empty()) else { return Ok(String::new()) };

    if named.starts_with('~') {
        return Err(Failure::Refused(format!(
            "`--cwd {named}` still has its tilde, so the shell did not expand it - quoting is \
             what usually does that. Muster does not expand one either, because `~` is the \
             shell's own spelling of your home directory and a command that guessed at it would \
             be guessing for whoever is on the far side. Write the path out, or leave it \
             unquoted so the shell expands it first."
        )));
    }

    let named = Path::new(named);
    if !named.is_absolute() {
        if let Some(daemon) = daemon {
            return Err(Failure::Refused(format!(
                "`muster pane new --daemon {daemon}` puts the pane on another machine, and \
                 `--cwd {}` is relative to this one - so there is nothing here to resolve it \
                 against. Say where it should start in that machine's own terms: an absolute \
                 path. `muster window` lists the panes {daemon} already holds, and each one's \
                 directory is a place a path can be written from.",
                named.display()
            )));
        }
        let Some(here) = here else {
            return Err(Failure::Refused(format!(
                "`--cwd {}` is relative and this command cannot tell what directory it is \
                 running in, so there is nothing to resolve it against. The usual cause is a \
                 shell whose working directory has been deleted or unmounted underneath it. \
                 Give an absolute path, or `cd` somewhere that exists and try again.",
                named.display()
            )));
        };
        return Ok(settled(&here.join(named)));
    }

    Ok(settled(named))
}

/// The file a `pane send --file` reads, as an absolute path.
///
/// Resolved here rather than left to the read, for the reason [`directory`] resolves `--cwd`: a
/// test says where the command was typed, and a corpus case can then pin what a relative path
/// means. Unlike `--cwd` this file is read on this machine, so the only paths refused are the
/// two nothing here can resolve.
pub(crate) fn file_to_read(named: &str, here: Option<&Path>) -> Result<String, Failure> {
    if named.starts_with('~') {
        return Err(Failure::Refused(format!(
            "`--file {named}` still has its tilde, so the shell did not expand it - quoting is \
             what usually does that, and muster does not expand one itself. Nothing was sent. \
             Write the path out, or leave it unquoted so the shell expands it first."
        )));
    }
    let named = Path::new(named);
    if named.is_absolute() {
        return Ok(settled(named));
    }
    let Some(here) = here else {
        return Err(Failure::Refused(format!(
            "`--file {}` is relative and this command cannot tell what directory it is running \
             in, so there is nothing to resolve it against, and nothing was sent. The usual \
             cause is a shell whose working directory has been deleted underneath it. Give an \
             absolute path, or `cd` somewhere that exists and try again.",
            named.display()
        )));
    };
    Ok(settled(&here.join(named)))
}

/// What a file or stdin handed over, as the text a `pane send` types.
///
/// Trailing newlines are dropped, the way `"$(cat PATH)"` drops them, so `--file brief.md` sends
/// exactly what a caller passing the file's contents as an argument sent before this existed. A
/// file ends in a newline by convention rather than because anybody meant one to reach the pane,
/// and `--enter` is how a caller asks for Return.
///
/// Pure, and public so the corpus driver applies the same rule the command does. The error is
/// the refusal, worded for whoever ran the command.
pub fn text_of(bytes: Vec<u8>, from: &TextSource) -> Result<String, String> {
    let text = String::from_utf8(bytes).map_err(|error| {
        format!(
            "{} is not UTF-8 text ({error}), so there is nothing a pane could be typed, and \
             nothing was sent. A pane send carries text; check that this is the file you meant.",
            from.described()
        )
    })?;
    Ok(text.trim_end_matches('\n').to_string())
}

/// An absolute path with its `.` and `..` worked out, without asking the filesystem.
///
/// Lexical on purpose, three times over: [`parse`] is pure and a corpus case has to mean the
/// same thing on a machine that has never had these directories; the path may be bound for
/// another machine, where nothing here could check it anyway; and a shell's own `cd ..` is
/// lexical too, so this agrees with what somebody typing it saw last.
fn settled(path: &Path) -> String {
    let mut settled = PathBuf::new();
    for part in path.components() {
        match part {
            // Popping the root leaves the root, which is what `/..` means.
            Component::ParentDir => {
                settled.pop();
            }
            Component::CurDir => {}
            named => settled.push(named),
        }
    }
    settled.to_string_lossy().into_owned()
}

/// Which pane a command is about: the one named, then the one it is running in, then the window's
/// own answer.
///
/// The last of those is the empty string, which every pane request in the schema reads as "the
/// pane this window's keyboard is on". So a `muster pane new` typed in a terminal outside Muster
/// still splits whatever somebody is looking at.
fn pane_ref(named: Option<&String>, environment: &BTreeMap<String, String>) -> String {
    named.cloned().or_else(|| running_in(environment)).unwrap_or_default()
}

/// A pane, tab, machine or addressee as typed, refusing an empty one.
///
/// Empty is what the schema reads as no answer given, so `--pane ""` would otherwise act on the
/// pane the window's keyboard is on - which may be the one somebody is typing in. Nobody types an
/// empty name on purpose: it is a shell variable that was never set, and the command it was meant
/// for is better refused than carried out somewhere else (kan a_2hIftnZi2).
pub(crate) fn reference(typed: &str) -> Result<String, String> {
    if typed.is_empty() {
        return Err("an empty name is no pane, tab, machine or participant, and nothing was \
                    sent. It is refused rather than read as the flag left out, which acts on a \
                    default nobody named - the pane this runs in, the one the keyboard is on, \
                    that pane's machine, or a whole group. An empty value is usually a shell \
                    variable that was never set. `muster window` lists every pane, tab and \
                    machine by name."
            .to_string());
    }
    Ok(typed.to_string())
}

/// `NAME=value`, for a variable a restarted agent is to start with. What makes a name is the
/// daemon's to check, which says why it refuses one.
fn variable(typed: &str) -> Result<(String, String), String> {
    match typed.split_once('=') {
        Some((name, value)) if !name.is_empty() => Ok((name.to_string(), value.to_string())),
        _ => Err(format!("`{typed}` is not NAME=value, which is how a variable is given")),
    }
}

/// Which pane and which machine a command that may name either is about.
///
/// The two are alternatives and clap has already refused both at once, so the work here is what
/// clap cannot see: a named machine takes the pane out of the request *including the one this
/// command is running in*. Saying "on that machine" from inside a pane and then quietly sending
/// `$MUSTER_PANE` would send the request straight back to the machine you were leaving, because
/// the window reads a named pane as the whole address and never looks at the machine beside it.
///
/// Which is also why a machine is worth naming at all: a pane's name is a complete address and
/// needs no machine, so the only thing `--daemon` can be for is a machine you have no pane on.
fn pane_and_machine(
    pane: Option<&String>,
    daemon: Option<&String>,
    environment: &BTreeMap<String, String>,
) -> (String, String) {
    match daemon {
        Some(daemon) => (String::new(), daemon.clone()),
        None => (pane_ref(pane, environment), String::new()),
    }
}

/// The pane a split grows from, the machine holding it, and the machine the new pane goes on.
///
/// Both named is a pane put beside one on another machine, which the schema carries in a field
/// of its own: `daemon_id` beside a pane is the machine holding that pane, and is refused when
/// it disagrees.
fn split_target(
    pane: Option<&String>,
    daemon: Option<&String>,
    environment: &BTreeMap<String, String>,
) -> (String, String, String) {
    if let (Some(_), Some(onto)) = (pane, daemon) {
        return (pane_ref(pane, environment), String::new(), onto.clone());
    }
    let (pane_id, daemon_id) = pane_and_machine(pane, daemon, environment);
    (pane_id, daemon_id, String::new())
}

/// Why a helper made hidden takes no side and no `--focus`.
const HIDDEN_HELPER_PLACED: &str = "a helper is made hidden, in a tab its maker's helpers share, \
     so no side and no --focus can say where it goes or put the keyboard on it, and nothing was \
     made. Add --shown to split it beside the pane it came from instead.";

/// The pane a new pane is a helper of: the one named, or with `--helper` the one this runs in,
/// which outside every pane there is none of.
fn maker(
    helper: bool,
    helper_of: Option<&String>,
    environment: &BTreeMap<String, String>,
) -> Result<Option<String>, Failure> {
    if let Some(maker) = helper_of {
        return Ok(Some(maker.clone()));
    }
    if !helper {
        return Ok(None);
    }
    running_in(environment).map(Some).ok_or_else(|| {
        Failure::Refused(
            "--helper makes a helper of the pane this runs in, and this is not running in a \
             pane, so nothing was made. --helper-of names the pane it is to be a helper of."
                .to_string(),
        )
    })
}

fn running_in(environment: &BTreeMap<String, String>) -> Option<String> {
    environment.get(environment::PANE_NAME).filter(|name| !name.is_empty()).cloned()
}

fn send(payload: request::Payload) -> Asking {
    Asking::Send(Box::new(Request::new(payload)))
}

/// One document, all of them, or the list of what there is.
///
/// A topic nobody has is refused here rather than left to clap, because the topics are data in
/// `docs.rs` and a clap value list would be a second copy of them to keep in agreement.
fn documentation(topic: Option<&str>) -> Result<String, Failure> {
    match topic {
        None => Ok(docs::listing()),
        Some("all") => Ok(docs::everything()),
        Some(named) => docs::topic(named)
            .map(|topic| topic.text.trim_end().to_string())
            .ok_or_else(|| Failure::Refused(docs::no_such_topic(named))),
    }
}

/// A completion script, generated from the same command definition `--help` is rendered from.
///
/// Worth having rather than a nicety: the whole vocabulary here is pane names nobody can type from
/// memory, and a shell that completes `muster focus p1w<tab>` is the difference between reading
/// `muster window` first and not having to.
fn completions(shell: Shell) -> String {
    let mut written = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "muster", &mut written);
    String::from_utf8_lossy(&written).into_owned()
}
