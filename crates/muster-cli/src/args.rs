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

use clap::{ArgGroup, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use muster_proto::{
    AdjustFontSize, ArrangePane, ClosePane, CreateTab, FocusPane, FocusPaneAt, FocusRelative,
    FocusTab, FocusTabRelative, ReadPane, ReadWindow, ReloadConfig, RenamePane, RenameTab, Request,
    ResizePane, SendToPane, SplitPane, ToggleSidebar, ZoomPane, request,
};

use crate::{docs, environment};

/// What one command line asked for.
#[derive(Debug)]
pub struct Invocation {
    pub asking: Asking,

    /// Answer for a program rather than for a person.
    pub json: bool,

    /// The window to talk to, when the caller named one.
    pub socket: Option<String>,
}

/// Either something to ask a window, or something this CLI can answer by itself.
#[derive(Debug)]
pub enum Asking {
    /// Boxed because a `Request` is two orders of magnitude the size of the other variants, and
    /// every invocation would otherwise carry room for the largest message Muster has.
    Send(Box<Request>),
    Print(String),
    /// Every window on this machine, asked the same thing and answered together.
    Survey,
    /// Another window, which means another process - so this starts one rather than asking.
    MakeWindow,
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

`pane new` prints the name of the pane it made, which is what makes the next line of a script \
possible, and `tab new` prints the pane it put in the tab. Neither moves the keyboard: making a \
pane is not the same act as looking at one, and an agent opening three panes should not drag \
somebody's cursor through all three. Ask with --focus.

`pane move` is one verb for two outcomes, because the window works out which from where the panes \
are: onto a pane in the same tab the two trade places, onto a pane in another tab it joins that \
tab. Both have to be on the same machine - a pane is a process, and it lives where it lives.

`pane new` and `tab new` also take --daemon, which is a machine's own name as `muster window` \
prints it beside every pane: local, or whatever a [[daemon]] block in your config calls the \
machine. It says where rather than what to grow from, so it cannot be given beside a REF, and it \
is the way to reach a machine you have no pane on at all - a devenv the day you attach it, or one \
whose last pane you closed. A machine with nothing on screen gets a first pane rather than a \
refusal.
";

const EXAMPLES: &str = "\
Examples:
  muster window --json
  muster pane new --down --run claude --name '🤖 A'
  muster pane send --pane p1w3r07bsd 'read AGENTS.md and wait' --enter
  muster pane move --pane p1w3r0ab2n --onto p1w3r07bsd
  muster tab new --run claude --name '🤖 reviewer'
  muster pane new --daemon devenv --run claude
  muster focus --next

Which window: $MUSTER_SOCKET names it, and Muster sets that in every pane it makes on this
machine. Otherwise muster looks for a listening window under ~/.muster/state, and refuses
rather than guessing if more than one answers.

Exit codes: 0 it happened, 1 the window refused, 2 the command line was wrong, 3 there was
no window to ask.
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

    /// The window to talk to, instead of looking for one
    #[arg(long, global = true, value_name = "PATH", display_order = 101)]
    socket: Option<String>,
}

#[derive(Debug, Subcommand)]
enum What {
    /// What the window is showing: its daemons, its tabs, its panes, and what each agent is doing
    Window {
        #[command(subcommand)]
        doing: Option<AboutWindows>,
    },

    /// Make a pane, name one, read it, type into one, move it, resize it, or close it
    Pane {
        #[command(subcommand)]
        doing: Doing,
    },

    /// Make a tab, go to one, or name one
    Tab {
        #[command(subcommand)]
        doing: WithTab,
    },

    /// Put the window's keyboard on a pane, or step it somewhere
    //
    // A name, a direction and a place are three ways of saying where, and clap holds them in
    // one group so that two at once is refused before this is read. None of them is the
    // fourth way, which is the pane this is running in.
    Focus {
        /// The pane to go to, or the one this is running in
        #[arg(value_name = "REF", group = "somewhere")]
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

        /// Go to the pane at this place in the window's pane order, the number ⌘1 to ⌘9 name
        #[arg(long, value_name = "N", group = "somewhere")]
        place: Option<u32>,
    },

    /// Fill the region with one pane, or put the others back
    Zoom {
        /// The pane to fill with, or the one this is running in
        #[arg(value_name = "REF")]
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

/// What `muster window` can do besides describe the one window this is about.
///
/// A subcommand rather than a flag, and optional, so that `muster window` keeps meaning what it
/// has always meant: the window this command is talking to. Everything here is about windows in
/// the plural, which is a different question and the only one `--socket` cannot narrow.
#[derive(Debug, Subcommand)]
enum AboutWindows {
    /// List every Muster window on this machine
    List,

    /// Open another window, and print the socket that reaches it
    //
    // The one command that dials no window, because it is the one asked when there may be none.
    // A window is a process, so this starts one.
    New,
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
        #[arg(long, value_name = "REF", group = "somewhere")]
        pane: Option<String>,

        /// The machine to put it on, instead of naming one of its panes
        #[arg(long, value_name = "ID", group = "somewhere")]
        daemon: Option<String>,

        /// Where it starts, or the directory the split came from
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,

        /// A shell line to run in it, waiting for its prompt first
        #[arg(long, value_name = "CMD")]
        run: Option<String>,

        /// What to call it
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Move the window's keyboard to it
        #[arg(long)]
        focus: bool,
    },

    /// Call a pane something. An empty name takes the name away again
    Rename {
        /// The pane to rename, or the one this is running in
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

        /// What to call it, joined with spaces if it arrives in pieces
        #[arg(required = true, value_name = "NAME")]
        name: Vec<String>,
    },

    /// Type text into a pane, whether or not anything is showing it
    Send {
        /// The pane to type into, or the one this is running in
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

        /// Press Return afterwards, which is what submits it
        #[arg(long)]
        enter: bool,

        /// The text, joined with spaces if it arrives in pieces
        #[arg(required = true, value_name = "TEXT")]
        text: Vec<String>,
    },

    /// Print what a pane has on it, as far back as the window will go
    //
    // The half of a pane an agent could not see. `muster window` says what state every agent
    // is in and what it claims to be doing; neither of those is the output, and until now
    // there was no way to read it at all.
    Read {
        /// The pane to read, or the one this is running in
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

        /// How many rows back to ask for. Omit for as far as the window will go
        #[arg(long, value_name = "N")]
        rows: Option<u32>,
    },

    /// Close a pane, which ends what is running in it
    Close {
        /// The pane to close, or the one this is running in - which ends this command's own shell
        #[arg(long, value_name = "REF")]
        pane: Option<String>,
    },

    /// Put a pane where another one is, without ending what is running in either
    //
    // One verb for both outcomes, because the window has one request for them: which of the
    // two a move becomes is worked out from where the panes are, and a CLI that chose would
    // be a second place that rule lives. Named `move` rather than `arrange` because moving is
    // what somebody wants; the exchange is what they get when both panes are in one tab, and
    // the help and `muster docs agents` say so.
    Move {
        /// The pane to move, or the one this is running in
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

        /// Where to put it: in the same tab the two swap, in another it lands after this one
        #[arg(long, required = true, value_name = "REF")]
        onto: String,
    },

    /// Move the divider beside a pane, making it bigger in that direction
    #[command(group = ArgGroup::new("towards").required(true))]
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

        /// The pane to grow, or the one this is running in
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

        /// How far, as a share of the region between 0 and 1. Omit for the window's own step
        #[arg(long, value_name = "FRACTION")]
        by: Option<f32>,
    },
}

/// What `muster tab` can do.
///
/// Three verbs rather than the pane's six. There is no `close`, because a tab closes when its
/// last pane does; and nothing types into a tab, or moves one.
#[derive(Debug, Subcommand)]
enum WithTab {
    /// Make a tab, and print the name of the pane that appears in it
    //
    // Prints the pane rather than the tab, because the pane is what a script's next line
    // needs: naming a tab is something you do once, and sending into its pane is what comes
    // next. The tab's own name is one `muster window` away, on the row of the pane below.
    New {
        /// The pane whose workspace the tab joins, or the one this is running in
        #[arg(long, value_name = "REF", group = "somewhere")]
        pane: Option<String>,

        /// The machine to make it on, instead of naming a pane in it
        #[arg(long, value_name = "ID", group = "somewhere")]
        daemon: Option<String>,

        /// Where its pane starts, or that pane's own directory
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,

        /// A shell line to run in it, waiting for its prompt first
        #[arg(long, value_name = "CMD")]
        run: Option<String>,

        /// What to call its pane
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Move the window's keyboard to it. The tab comes on screen either way
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
        #[arg(value_name = "REF", group = "which")]
        tab: Option<String>,

        /// Go to the next tab instead, wrapping at the end
        #[arg(long, group = "which")]
        next: bool,
        /// Go to the previous one, wrapping at the start
        #[arg(long, group = "which")]
        previous: bool,
    },

    /// Call a tab something. An empty name takes the name away again
    Rename {
        /// The tab to rename, or the one the window's keyboard is in
        #[arg(long, value_name = "REF")]
        tab: Option<String>,

        /// What to call it, joined with spaces if it arrives in pieces
        #[arg(required = true, value_name = "NAME")]
        name: Vec<String>,
    },
}

/// Reads a command line, or says why it cannot be one.
pub fn parse(
    argv: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<Invocation, Failure> {
    // The program name clap expects at argv[0], supplied here rather than taken from the process:
    // a caller reached through a symlink or a wrapper would otherwise see that name in its own
    // help and completions, which is a different command from the one these documents.
    let words = std::iter::once("muster".to_string()).chain(argv.iter().cloned());
    let cli = Cli::try_parse_from(words).map_err(|error| Failure::Usage(Box::new(error)))?;

    let asking = match &cli.what {
        What::Window { doing: None } => send(request::Payload::ReadWindow(ReadWindow {})),
        // Asked of every window rather than of one, which is why it is not a `Send`: `--socket`
        // and $MUSTER_SOCKET both narrow to one window, and the question here is which there are.
        What::Window { doing: Some(AboutWindows::List) } => Asking::Survey,
        What::Window { doing: Some(AboutWindows::New) } => Asking::MakeWindow,
        What::Pane { doing } => pane(doing, environment),
        What::Tab { doing } => tab(doing, environment),
        What::Focus { pane, next, previous, left, right, up, down, place } => {
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
            if let Some(step) = stepped {
                send(request::Payload::FocusRelative(FocusRelative { direction: step.to_string() }))
            } else if let Some(place) = place {
                send(request::Payload::FocusPaneAt(FocusPaneAt { place: *place }))
            } else {
                // The only command where an empty pane is not an answer: everywhere else the
                // core reads it as "the pane the keyboard is on", and asking to focus the
                // focused pane is not something to ask for. So it is refused here, where the
                // reason is known.
                let named = pane.clone().or_else(|| running_in(environment)).ok_or_else(|| {
                    Failure::Refused(format!(
                        "`muster focus` needs a pane, a direction or a place, and ${} is not \
                         set - so this is not running inside a pane Muster made. Name one: \
                         `muster focus p1w3r07bsd`, or say `--next`. `muster window` lists \
                         them.",
                        environment::PANE_NAME
                    ))
                })?;
                send(request::Payload::FocusPane(FocusPane {
                    pane_id: named,
                    ..FocusPane::default()
                }))
            }
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
        What::Docs { topic } => Asking::Print(documentation(topic.as_deref())?),
        What::Completions { shell } => Asking::Print(completions(*shell)),
    };

    Ok(Invocation { asking, json: cli.json, socket: cli.socket })
}

fn pane(doing: &Doing, environment: &BTreeMap<String, String>) -> Asking {
    match doing {
        Doing::New { left, right, up, down, pane, daemon, cwd, run, name, focus } => {
            // The same four words the schema uses, and the same words a `muster window --json`
            // answer carries, rather than English ones like `below`: a caller that reads a side out
            // should be able to send it straight back. Saying nothing means right, because that is
            // where ⌘D splits to, and a CLI whose default matched no chord would make the two
            // disagree about what "a split" means. clap holds the four in one group, so two at
            // once is refused before this is reached.
            let side = chosen(&[(*left, "left"), (*right, "right"), (*up, "up"), (*down, "down")])
                .unwrap_or("right");
            let (pane_id, daemon_id) =
                pane_and_machine(pane.as_ref(), daemon.as_ref(), environment);
            send(request::Payload::SplitPane(SplitPane {
                pane_id,
                daemon_id,
                side: side.to_string(),
                cwd: cwd.clone().unwrap_or_default(),
                run: run.clone().unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                take_focus: *focus,
                ..SplitPane::default()
            }))
        }
        Doing::Rename { pane, name } => send(request::Payload::RenamePane(RenamePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            name: name.join(" "),
            ..RenamePane::default()
        })),
        Doing::Send { pane, enter, text } => send(request::Payload::SendToPane(SendToPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            text: text.join(" "),
            enter: *enter,
            ..SendToPane::default()
        })),
        Doing::Read { pane, rows } => send(request::Payload::ReadPane(ReadPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            // Zero is what the window reads as "as far as you will go", and it is also what
            // proto3 sends for an absent number - so omitting `--rows` and asking for
            // everything are the same request.
            rows: rows.unwrap_or_default(),
            ..ReadPane::default()
        })),
        Doing::Close { pane } => send(request::Payload::ClosePane(ClosePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ClosePane::default()
        })),
        Doing::Move { pane, onto } => send(request::Payload::ArrangePane(ArrangePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            onto_pane_id: onto.clone(),
            ..ArrangePane::default()
        })),
        Doing::Resize { left, right, up, down, pane, by } => {
            // clap holds the four in one required group, so exactly one is true here.
            let direction =
                chosen(&[(*left, "left"), (*right, "right"), (*up, "up"), (*down, "down")])
                    .unwrap_or("right");
            send(request::Payload::ResizePane(ResizePane {
                pane_id: pane_ref(pane.as_ref(), environment),
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
    }
}

/// No environment, unlike [`pane`] beside it.
///
/// A tab name is not in any pane's environment - nothing has to tell a tab which tab it is - so
/// there is nothing here to fall back to. `focus` demands one; `rename` leaves the field empty,
/// which the schema already reads as the tab the keyboard's pane is in.
fn tab(doing: &WithTab, environment: &BTreeMap<String, String>) -> Asking {
    match doing {
        WithTab::New { pane, daemon, cwd, run, name, focus } => {
            let (pane_id, daemon_id) =
                pane_and_machine(pane.as_ref(), daemon.as_ref(), environment);
            send(request::Payload::CreateTab(CreateTab {
                pane_id,
                daemon_id,
                cwd: cwd.clone().unwrap_or_default(),
                run: run.clone().unwrap_or_default(),
                name: name.clone().unwrap_or_default(),
                take_focus: *focus,
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
        WithTab::Rename { tab, name } => send(request::Payload::RenameTab(RenameTab {
            tab_id: tab.clone().unwrap_or_default(),
            name: name.join(" "),
            ..RenameTab::default()
        })),
    }
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

/// Which pane a command is about: the one named, then the one it is running in, then the window's
/// own answer.
///
/// The last of those is the empty string, which every pane request in the schema reads as "the
/// pane this window's keyboard is on". So a `muster pane new` typed in a terminal outside Muster
/// still splits whatever somebody is looking at.
fn pane_ref(named: Option<&String>, environment: &BTreeMap<String, String>) -> String {
    named.cloned().or_else(|| running_in(environment)).unwrap_or_default()
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

fn running_in(environment: &BTreeMap<String, String>) -> Option<String> {
    environment.get(environment::PANE_NAME).filter(|name| !name.is_empty()).cloned()
}

fn send(payload: request::Payload) -> Asking {
    Asking::Send(Box::new(Request { payload: Some(payload) }))
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
