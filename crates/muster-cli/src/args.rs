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

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use muster_proto::{
    ClosePane, FocusPane, FocusTab, ReadWindow, RenamePane, RenameTab, Request, SendToPane,
    SplitPane, ZoomPane, request,
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
    /// Boxed because a `Request` is two orders of magnitude the size of the other variant, and
    /// every invocation would otherwise carry room for the largest message Muster has.
    Send(Box<Request>),
    Print(String),
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
possible. It does not move the keyboard: making a pane is not the same act as looking at one, and \
an agent opening three panes should not drag somebody's cursor through all three. Ask with \
--focus.
";

const EXAMPLES: &str = "\
Examples:
  muster window --json
  muster pane new --down --run claude --name '🤖 A'
  muster pane send --pane p1w3r07bsd 'read AGENTS.md and wait' --enter
  muster focus p1w3r07bsd

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
    Window,

    /// Make a pane, name one, type into one, or close one
    Pane {
        #[command(subcommand)]
        doing: Doing,
    },

    /// Go to a tab, or name one
    Tab {
        #[command(subcommand)]
        doing: WithTab,
    },

    /// Put the window's keyboard on a pane
    Focus {
        /// The pane to go to, or the one this is running in
        #[arg(value_name = "REF")]
        pane: Option<String>,
    },

    /// Fill the region with one pane, or put the others back
    Zoom {
        /// The pane to fill with, or the one this is running in
        #[arg(value_name = "REF")]
        pane: Option<String>,
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
        #[arg(long, value_name = "REF")]
        pane: Option<String>,

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

    /// Close a pane, which ends what is running in it
    Close {
        /// The pane to close, or the one this is running in - which ends this command's own shell
        #[arg(long, value_name = "REF")]
        pane: Option<String>,
    },
}

/// What `muster tab` can do.
///
/// Two verbs rather than the pane's four, because the other two do not exist for a tab: a tab
/// closes when its last pane does, and nothing types into a tab.
#[derive(Debug, Subcommand)]
enum WithTab {
    /// Bring a tab on screen and put the window's keyboard in it
    Focus {
        /// The tab to go to
        //
        // A `String` rather than an `Option<String>`, which is what makes clap demand one. A pane
        // command with no ref means the pane it is running in, and there is no such answer for a
        // tab: nothing tells a pane which tab it is in, and "the tab the keyboard is already in"
        // is not somewhere to ask to go.
        #[arg(value_name = "REF")]
        tab: String,
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
        What::Window => send(request::Payload::ReadWindow(ReadWindow {})),
        What::Pane { doing } => pane(doing, environment),
        What::Tab { doing } => tab(doing),
        What::Focus { pane } => {
            // The only command where an empty pane is not an answer: everywhere else the core
            // reads it as "the pane the keyboard is on", and asking to focus the focused pane is
            // not something to ask for. So it is refused here, where the reason is known.
            let named = pane.clone().or_else(|| running_in(environment)).ok_or_else(|| {
                Failure::Refused(format!(
                    "`muster focus` needs a pane, and ${} is not set - so this is not running \
                     inside a pane Muster made. Name one: `muster focus p1w3r07bsd`. `muster \
                     window` lists them.",
                    environment::PANE_NAME
                ))
            })?;
            send(request::Payload::FocusPane(FocusPane { pane_id: named, ..FocusPane::default() }))
        }
        What::Zoom { pane } => send(request::Payload::ZoomPane(ZoomPane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ZoomPane::default()
        })),
        What::Docs { topic } => Asking::Print(documentation(topic.as_deref())?),
        What::Completions { shell } => Asking::Print(completions(*shell)),
    };

    Ok(Invocation { asking, json: cli.json, socket: cli.socket })
}

fn pane(doing: &Doing, environment: &BTreeMap<String, String>) -> Asking {
    match doing {
        Doing::New { left, right: _, up, down, pane, cwd, run, name, focus } => {
            // The same four words the schema uses, and the same words a `muster window --json`
            // answer carries, rather than English ones like `below`: a caller that reads a side out
            // should be able to send it straight back. `--right` is not read, because it is what
            // happens anyway - and right rather than any other side because it is where ⌘D splits
            // to, and a CLI whose default matched no chord would make the two disagree about what
            // "a split" means. clap holds the four in one group, so two at once is refused before
            // this is reached.
            let side = if *left {
                "left"
            } else if *up {
                "up"
            } else if *down {
                "down"
            } else {
                "right"
            };
            send(request::Payload::SplitPane(SplitPane {
                pane_id: pane_ref(pane.as_ref(), environment),
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
        Doing::Close { pane } => send(request::Payload::ClosePane(ClosePane {
            pane_id: pane_ref(pane.as_ref(), environment),
            ..ClosePane::default()
        })),
    }
}

/// No environment, unlike [`pane`] beside it.
///
/// A tab name is not in any pane's environment - nothing has to tell a tab which tab it is - so
/// there is nothing here to fall back to. `focus` demands one; `rename` leaves the field empty,
/// which the schema already reads as the tab the keyboard's pane is in.
fn tab(doing: &WithTab) -> Asking {
    match doing {
        WithTab::Focus { tab } => send(request::Payload::FocusTab(FocusTab {
            tab_id: tab.clone(),
            ..FocusTab::default()
        })),
        WithTab::Rename { tab, name } => send(request::Payload::RenameTab(RenameTab {
            tab_id: tab.clone().unwrap_or_default(),
            name: name.join(" "),
            ..RenameTab::default()
        })),
    }
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
