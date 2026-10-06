//! `muster msg`: agents posting to each other through the daemon on this machine (MIP-4).
//!
//! These verbs talk to muster-daemon directly rather than to a window, because messaging has to
//! work with no window open - a Claude session in a plain terminal takes part the same way as
//! one in a pane. Their names come from `muster_daemon_proto::messaging`, which the daemon's
//! wakes and refusals read too, so a rename is one edit there.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use muster_daemon_proto::connection;
use muster_daemon_proto::messaging::{
    self as spelling, GROUP, GROUPS, JOIN, LEAVE, LOG, OPEN, PAUSE, POST, READ, RESUME, WAIT, WHO,
};
use muster_daemon_proto::msg_answer::{self, Answer, Until, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::{self as proto, ConnectionKind, request::Service};

use crate::args::{Failure, TextSource};
use crate::daemon::MayStart;
use crate::environment;
use crate::{Trouble, daemon};

/// Claude Code's inbox socket, which it exports to every command a session runs.
pub const CLAUDE_INBOX: &str = "CLAUDE_CODE_MESSAGING_SOCKET";
pub use crate::daemon::SOCKET as DAEMON_SOCKET;

/// What `muster msg --help` says before the verbs: the protocol an agent follows.
pub const PROTOCOL: &str = "\
Agents post messages to groups through the muster-daemon on this machine, and are woken when a \
message arrives for them - nobody waits in a loop.

Join a group, post, and end your turn. When a message arrives for you, you are told in one line \
how many wait and from whom, and the command that reads them: run `muster msg read`. Reading \
moves your place, so a message is read once.

A post is refused while you have unread messages in that group: read, then post again. This \
keeps you from answering a conversation you have not seen. The human is not held to it.

An unaddressed post wakes every member of its group but you; `--to NAME` wakes only NAME, \
though every member can still read it. With no --group, a post goes to the one group you share \
with the people you address, or to a new group of exactly you and them.

An agent in a pane is woken once it is idle. `--urgent` reaches it while it works, typed into \
its prompt for the turn it is running: keep it for what should change what the agent is doing \
now, since it interrupts.

Who you are: `--as NAME` if given, else the Claude Code session you run in (from \
$CLAUDE_CODE_MESSAGING_SOCKET), else the agent in the pane you run in (from $MUSTER_PANE), else \
you are the human. A session that never joined under a \
name is named after its working directory.

A message to @human notifies the person at the Muster window, and choosing the notification \
opens the group's transcript: `muster msg log --group G --follow`. On a machine that window \
reaches over ssh, @human is the same person, whose messages are kept where the app runs.

A Claude Code session started with --dangerously-skip-permissions holds a wake for approval \
unless it was also started with --settings '{\"crossSessionInbound\":\"accept\"}'.

A group convened with a policy decides whom an unaddressed post wakes, whom you may address, \
who may post urgently, and who may add or remove members, you included; a refusal says what it \
allows. `muster msg groups` \
shows each group's policy.

Do not run `muster msg wait` in the foreground: it blocks until a message arrives, which is the \
loop this exists to remove. It is for hooks and scripts.

`muster docs msg` is the full reference.";

/// Who is asking, when it is not the session this runs in.
#[derive(Debug, Args)]
pub struct Identity {
    /// Act as this participant rather than the session this runs in
    #[arg(long = "as", value_name = "NAME", global = true)]
    pub as_name: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Verb {
    /// Take part: register under a name, and join a group, creating it if it does not exist
    #[command(name = JOIN)]
    Join {
        /// The name to take part under; a gone participant's name is taken over with its place
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// The group to join
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// Your hooks fetch your messages (a PostToolUse read and a Stop `wait --due`), so
        /// nothing is typed into your pane while they run
        #[arg(long)]
        pull: bool,
    },

    /// Leave a group, or with no --group stop taking part at all
    #[command(name = LEAVE)]
    Leave {
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
    },

    /// Who takes part, and whether each is still there
    #[command(name = WHO)]
    Who {
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
    },

    /// Post a message, waking whom it is for
    #[command(name = POST)]
    Post {
        /// The group to post to; without it, the one group you share with --to, or a new one
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// Whom the message is for, comma-separated; everyone else in the group can still read it
        #[arg(long, value_name = "NAMES", value_delimiter = ',')]
        to: Vec<String>,
        /// Post this file's contents
        #[arg(long, value_name = "PATH", conflicts_with = "text")]
        file: Option<String>,
        /// Reach agents in panes while they work, not once they are idle
        #[arg(long)]
        urgent: bool,
        /// The message, or `-` to read it from stdin
        #[arg(value_name = "TEXT", num_args = 0.., trailing_var_arg = true)]
        text: Vec<String>,
    },

    /// Print your unread messages and move your place past them
    #[command(name = READ)]
    Read {
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// Print nothing when nothing is unread, for a hook to run after every tool call
        #[arg(long)]
        if_unread: bool,
    },

    /// Print a group's transcript, moving nothing
    #[command(name = LOG)]
    Log {
        #[arg(long, value_name = "GROUP")]
        group: String,
        /// Only entries after this number
        #[arg(long, value_name = "N", default_value_t = 0)]
        since: u64,
        /// Keep printing entries as they land, until interrupted; with --json, one line each
        #[arg(long)]
        follow: bool,
    },

    /// Block until a message that would wake you is unread, then print what a wake would say
    #[command(name = WAIT)]
    Wait {
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// Give up after this many seconds, exiting 5
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u32>,
        /// Only for a wake you are due - once per batch, and once more "still unread" - as a
        /// Stop hook waits; marks you as fetching with hooks
        #[arg(long)]
        due: bool,
    },

    /// Every group, with its members and whether it is paused
    #[command(name = GROUPS)]
    Groups,

    /// Make a group with a policy, change its policy, add and remove its members, or delete it
    #[command(name = GROUP, subcommand)]
    Group(GroupVerb),

    /// Pause a group: its posts are kept and wake nobody but the human until it is resumed
    #[command(name = PAUSE)]
    Pause {
        #[arg(value_name = "GROUP")]
        group: String,
    },

    /// Resume a paused group, waking each member once for what it has unread
    #[command(name = RESUME)]
    Resume {
        #[arg(value_name = "GROUP")]
        group: String,
    },

    /// Go to a group's transcript in the window, as choosing its banner does: the pane
    /// following it, or a new tab. Asks the window, not the daemon
    #[command(name = OPEN)]
    Open {
        #[arg(long, value_name = "GROUP")]
        group: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum GroupVerb {
    /// Make a group, and join it as its first member
    New {
        #[arg(value_name = "GROUP")]
        group: String,
        /// A policy file (TOML: ring, allow, membership, urgent, paused); the default lets anyone do anything
        #[arg(long, value_name = "PATH")]
        policy: Option<String>,
    },
    /// Replace a group's ring, allow, membership and urgent with a file's; `pause` and `resume` change
    /// whether it is paused
    Set {
        #[arg(value_name = "GROUP")]
        group: String,
        #[arg(long, value_name = "PATH")]
        policy: String,
    },
    /// Add members: participants' names, or panes'
    Add {
        #[arg(value_name = "GROUP")]
        group: String,
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,
    },
    /// Remove members
    Remove {
        #[arg(value_name = "GROUP")]
        group: String,
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,
    },
    /// Delete a group and its log, letting every member go; unread messages go with it
    Delete {
        #[arg(value_name = "GROUP")]
        group: String,
    },
}

/// A `msg` request as the command line gives it, before anything is read from disk.
#[derive(Debug)]
pub struct Messaging {
    pub request: proto::MsgRequest,
    /// Where a post's body comes from when it is not on the command line.
    pub body_from: Option<TextSource>,
    /// The policy file a group is made with or set to, read when the request is sent.
    pub policy_from: Option<String>,
    pub if_unread: bool,
    /// A log to keep printing as it grows, rather than answer once.
    pub follow: bool,
}

/// Turns a command line into a request, reading nothing: the caller's identity is what its
/// environment names, and its inbox's inode is looked up when the request is sent.
pub fn parse(
    verb: &Verb,
    identity: &Identity,
    environment: &BTreeMap<String, String>,
    here: Option<&Path>,
) -> Result<Messaging, Failure> {
    let caller = msg_request::Caller {
        as_name: identity.as_name.clone(),
        inbox: environment
            .get(CLAUDE_INBOX)
            .filter(|socket| !socket.is_empty())
            .map(|socket| msg_request::Inbox { socket: socket.clone(), inode: 0 }),
        pane: environment.get(environment::PANE_NAME).filter(|pane| !pane.is_empty()).cloned(),
        directory: here.map(|here| here.display().to_string()),
    };
    let mut body_from = None;
    let mut policy_from = None;
    let mut if_unread = false;
    let mut follow = false;
    let asked = match verb {
        Verb::Join { name, group, pull } => {
            Asked::Join(msg_request::Join { name: name.clone(), group: group.clone(), pull: *pull })
        }
        Verb::Leave { group } => Asked::Leave(msg_request::Leave { group: group.clone() }),
        Verb::Who { group } => Asked::Who(msg_request::Who { group: group.clone() }),
        Verb::Post { group, to, file, urgent, text } => {
            let body = if let Some(file) = file {
                body_from = Some(TextSource::File(crate::args::file_to_read(file, here)?));
                String::new()
            } else if text == &["-"] {
                body_from = Some(TextSource::Stdin);
                String::new()
            } else if text.is_empty() {
                return Err(Failure::Refused(format!(
                    "`{}` needs a message: as text, as `-` to read stdin, or with --file.",
                    spelling::command(POST, "")
                )));
            } else {
                text.join(" ")
            };
            Asked::Post(msg_request::Post {
                group: group.clone(),
                to: to.clone(),
                body,
                urgent: *urgent,
            })
        }
        Verb::Read { group, if_unread: quiet } => {
            if_unread = *quiet;
            Asked::Read(msg_request::Read { group: group.clone() })
        }
        Verb::Log { group, since, follow: following } => {
            follow = *following;
            Asked::Log(msg_request::Log { group: group.clone(), since: *since, follow: false })
        }
        Verb::Wait { group, timeout, due } => Asked::Wait(msg_request::Wait {
            group: group.clone(),
            timeout_ms: timeout.map(|seconds| seconds.saturating_mul(1000)),
            due: *due,
        }),
        Verb::Groups => Asked::Groups(msg_request::Groups {}),
        Verb::Group(GroupVerb::New { group, policy }) => {
            if let Some(policy) = policy {
                policy_from = Some(crate::args::file_to_read(policy, here)?);
            }
            Asked::GroupNew(msg_request::GroupNew { group: group.clone(), policy: None })
        }
        Verb::Group(GroupVerb::Set { group, policy }) => {
            policy_from = Some(crate::args::file_to_read(policy, here)?);
            Asked::GroupSet(msg_request::GroupSet { group: group.clone(), policy: None })
        }
        Verb::Group(GroupVerb::Add { group, names }) => {
            Asked::GroupMembers(msg_request::GroupMembers {
                group: group.clone(),
                add: names.clone(),
                remove: Vec::new(),
            })
        }
        Verb::Group(GroupVerb::Remove { group, names }) => {
            Asked::GroupMembers(msg_request::GroupMembers {
                group: group.clone(),
                add: Vec::new(),
                remove: names.clone(),
            })
        }
        Verb::Group(GroupVerb::Delete { group }) => {
            Asked::GroupDelete(msg_request::GroupDelete { group: group.clone() })
        }
        Verb::Pause { group } => Asked::Pause(msg_request::Pause { group: group.clone() }),
        Verb::Resume { group } => Asked::Resume(msg_request::Resume { group: group.clone() }),
        Verb::Open { .. } => unreachable!("`msg open` asks the window, and args sends it there"),
    };
    let request = proto::MsgRequest { caller: Some(caller), request: Some(asked) };
    Ok(Messaging { request, body_from, policy_from, if_unread, follow })
}

/// Sends the request to this machine's daemon and renders its answer.
pub fn run(
    mut messaging: Messaging,
    environment: &BTreeMap<String, String>,
    input: &mut impl Read,
    json: bool,
) -> Result<String, Trouble> {
    if let Some(from) = &messaging.body_from {
        let body = read_body(from, input)?;
        if let Some(Asked::Post(post)) = messaging.request.request.as_mut() {
            post.body = body;
        }
    }
    if let Some(path) = &messaging.policy_from {
        let (policy, pauses) = read_policy(path)?;
        match messaging.request.request.as_mut() {
            Some(Asked::GroupNew(new)) => new.policy = Some(policy),
            Some(Asked::GroupSet(set)) if pauses => {
                return Err(Trouble::Refused(format!(
                    "{path} says `paused = true`, and `group set` leaves whether {} is paused as \
                     it is: pause it with `muster msg pause {}`, which holds its wakes until \
                     `resume` wakes each member for what it missed.",
                    set.group, set.group
                )));
            }
            Some(Asked::GroupSet(set)) => set.policy = Some(policy),
            _ => {}
        }
    }
    if let Some(inbox) = messaging.request.caller.as_mut().and_then(|caller| caller.inbox.as_mut())
    {
        // The inode tells this session's socket from a later one bound to the same path. A
        // socket that cannot be looked at is no inbox anyone can wake.
        match std::fs::metadata(&inbox.socket) {
            Ok(metadata) => inbox.inode = metadata.ino(),
            Err(_) => {
                if let Some(caller) = messaging.request.caller.as_mut() {
                    caller.inbox = None;
                }
            }
        }
    }
    let socket = daemon::socket_or_refusal(environment)?;
    let answer = ask(&socket, &messaging.request, Some(MayStart { environment, json }))?;
    render(&messaging.request, &answer, messaging.if_unread, json)
}

/// Prints a group's log, then each entry as it lands, until the reader goes away. Each batch is
/// flushed as it is written: the reader is a person watching a transcript, or a pipe acting on
/// each line.
pub fn follow(
    messaging: &Messaging,
    environment: &BTreeMap<String, String>,
    json: bool,
    out: &mut impl std::io::Write,
) -> Result<(), Trouble> {
    let socket = daemon::socket_or_refusal(environment)?;
    let mut request = messaging.request.clone();
    // Only the first ask may start a daemon: after that one has answered, nothing listening is
    // a handover in progress.
    let mut may_start = Some(MayStart { environment, json });
    loop {
        let answer = ask(&socket, &request, may_start.take())?;
        let Some((entries, human)) = render_entries(&answer)? else { return Ok(()) };
        let Some(Asked::Log(log)) = request.request.as_mut() else {
            unreachable!("only a log is followed")
        };
        if let Some(last) = entries.groups.iter().flat_map(|group| &group.entries).last() {
            log.since = last.seq;
        }
        log.follow = true;
        let text = if json {
            let lines: Vec<String> = entries
                .groups
                .iter()
                .flat_map(|group| group.entries.iter().map(|entry| (&group.group, entry)))
                .map(|(group, entry)| {
                    let mut value = entry_json(entry);
                    value["group"] = group.clone().into();
                    value.to_string()
                })
                .collect();
            lines.join("\n")
        } else {
            entries_text(&entries, false, false, &human)
        };
        // The reader went away - the pane closed, `| head` had enough. Nobody is left to tell.
        if !text.is_empty() && writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            return Ok(());
        }
    }
}

/// A log's entries, or why there are none.
/// A followed log's entries, with what the answer calls the human.
fn render_entries(
    answer: &proto::Answer,
) -> Result<Option<(msg_answer::Entries, String)>, Trouble> {
    let Some(proto::answer::Detail::Msg(msg)) = &answer.detail else {
        return Err(Trouble::Refused(format!(
            "the muster-daemon answered with nothing to show: {}",
            answer.reason
        )));
    };
    if answer.outcome() == proto::Outcome::Refused {
        return Err(Trouble::Refused(answer.reason.clone()));
    }
    Ok(match &msg.answer {
        Some(Answer::Entries(entries)) => Some((entries.clone(), msg.human_name.clone())),
        _ => None,
    })
}

fn read_body(from: &TextSource, input: &mut impl Read) -> Result<String, Trouble> {
    let bytes = match from {
        TextSource::File(path) => std::fs::read(path).map_err(|error| {
            Trouble::Refused(format!("could not read {path} ({error}), so nothing was posted."))
        })?,
        TextSource::Stdin => {
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes).map_err(|error| {
                Trouble::Refused(format!("could not read stdin ({error}), so nothing was posted."))
            })?;
            bytes
        }
    };
    String::from_utf8(bytes).map_err(|error| {
        Trouble::Refused(format!("the message is not UTF-8 text ({error}), so nothing was posted."))
    })
}

/// A policy file as a person writes it: any field left out keeps the default's value, which
/// lets anyone do anything (MIP-4, section 8).
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default = "everyone_and_the_human_by_author")]
    ring: BTreeMap<String, Vec<String>>,
    #[serde(default = "everyone_by_author")]
    allow: BTreeMap<String, Vec<String>>,
    #[serde(default = "everyone")]
    membership: Vec<String>,
    /// Left out, the daemon's default: anyone.
    urgent: Option<Vec<String>>,
    #[serde(default)]
    paused: bool,
}

fn everyone() -> Vec<String> {
    vec!["*".to_string()]
}

fn everyone_by_author() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([("*".to_string(), everyone())])
}

/// A ring set's `*` leaves out the human, who is rung only by name, so the default names it.
fn everyone_and_the_human_by_author() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([("*".to_string(), vec!["*".to_string(), "@human".to_string()])])
}

/// The policy a file holds, and whether it says to be paused.
fn read_policy(path: &str) -> Result<(msg_request::Policy, bool), Trouble> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Trouble::Refused(format!("could not read the policy file {path} ({error})."))
    })?;
    let file: PolicyFile = toml::from_str(&text).map_err(|error| {
        Trouble::Refused(format!(
            "{path} is not a policy: {error}\nA policy has `ring` and `allow` (tables of author \
             to names), `membership` and `urgent` (names) and `paused`; `muster docs msg` has an \
             example."
        ))
    })?;
    let names = |map: BTreeMap<String, Vec<String>>| {
        map.into_iter().map(|(author, names)| (author, msg_request::Names { names })).collect()
    };
    let policy = msg_request::Policy {
        ring: names(file.ring),
        allow: names(file.allow),
        membership: file.membership,
        urgent: file.urgent.map(|names| msg_request::Names { names }),
        paused: file.paused,
    };
    Ok((policy, file.paused))
}

/// How long to keep asking a daemon that is handing over to a new one, which is refusing
/// changes until the new one serves. A handoff takes well under a second.
const HANDOVER_PATIENCE: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(200);

/// How long a request other than a wait may take to be answered.
const PATIENCE: Duration = Duration::from_mins(1);

/// Asks, and asks again while the daemon is handing over to a new one: it refuses changes
/// until the new one serves, and ends the connections of waits in progress. With `may_start`,
/// a first ask that finds nothing listening starts this machine's daemon and asks that.
fn ask(
    socket: &Path,
    request: &proto::MsgRequest,
    may_start: Option<MayStart>,
) -> Result<proto::Answer, Trouble> {
    let mut patience = Patience::default();
    let waits = blocks(request);
    let mut answered_before = false;
    loop {
        let began = Instant::now();
        let answer = ask_once(socket, request, may_start.filter(|_| !answered_before));
        let again = match &answer {
            Ok(answer) => matches!(refusal_of(answer), "handing_over" | "ended"),
            // A wait whose daemon went away mid-wait, and the moment after, when nothing
            // listens yet: what a handoff looks like from here. A daemon never reached at all
            // is not worth waiting for.
            Err(Trouble::Unanswered(_)) => waits,
            Err(Trouble::Unreachable(_)) => waits && answered_before,
            Err(_) => false,
        };
        answered_before |= matches!(answer, Ok(_) | Err(Trouble::Unanswered(_)));
        if !again || !patience.allows_another(began, Instant::now()) {
            return answer;
        }
        std::thread::sleep(RETRY);
    }
}

/// How long asking again may go on while a daemon hands over.
#[derive(Debug, Default)]
struct Patience {
    since: Option<Instant>,
}

impl Patience {
    /// Whether to ask again after an attempt that began at `began` and ended, at `now`, in a
    /// handover.
    ///
    /// Patience runs from the first handover seen, not from the first request: an attempt that
    /// itself lasted that long was a wait that ran until a handover ended it, and the handover
    /// starts now.
    fn allows_another(&mut self, began: Instant, now: Instant) -> bool {
        if now.duration_since(began) >= HANDOVER_PATIENCE {
            self.since = None;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) < HANDOVER_PATIENCE
    }
}

/// Whether the request is answered only once something happens, so may take any time.
fn blocks(request: &proto::MsgRequest) -> bool {
    match &request.request {
        Some(Asked::Wait(_)) => true,
        Some(Asked::Log(log)) => log.follow,
        _ => false,
    }
}

/// What a request asks that a daemon of an older minor does not know, and the first minor that
/// does. An older one ignores a field it does not know and answers as if it were not set: a
/// follow answered at once asks again as fast as it can, and a `Stop` hook's `wait --due`
/// answered as a plain wait rewakes the session at the end of every turn. And it refuses a
/// request it does not know as though messaging were missing altogether.
fn needs_minor(request: &proto::MsgRequest) -> Option<(&'static str, u32)> {
    match &request.request {
        Some(Asked::Log(log)) if log.follow => Some(("follow a log", 1)),
        Some(Asked::Wait(wait)) if wait.due => Some(("answer a wait only when a wake is due", 1)),
        Some(Asked::Join(join)) if join.pull => Some(("take a participant's hooks fetching", 1)),
        Some(
            Asked::Groups(_)
            | Asked::GroupNew(_)
            | Asked::GroupSet(_)
            | Asked::GroupMembers(_)
            | Asked::Pause(_)
            | Asked::Resume(_),
        ) if !says_who_may_urge(request) => Some(("keep groups with a policy", 1)),
        Some(Asked::Post(post)) if post.urgent => Some(("post urgently", 3)),
        Some(Asked::GroupNew(_) | Asked::GroupSet(_)) => {
            Some(("keep a policy saying who may post urgently", 3))
        }
        Some(Asked::GroupDelete(_)) => Some(("delete a group", 3)),
        _ => None,
    }
}

/// The request as sent to a daemon speaking `speaks`. A policy file is the whole policy, so one
/// that leaves out `urgent` means the default, said outright to a daemon that knows the key:
/// left unsaid, a daemon keeps the list the group has, for a client that cannot see it.
fn whole(request: &proto::MsgRequest, speaks: proto::Version) -> proto::MsgRequest {
    let mut request = request.clone();
    if speaks.minor >= 3
        && let Some(Asked::GroupSet(msg_request::GroupSet { policy: Some(policy), .. })) =
            &mut request.request
        && policy.urgent.is_none()
    {
        policy.urgent = Some(msg_request::Names { names: everyone() });
    }
    request
}

/// Whether the request sets a policy that says who may post urgently.
fn says_who_may_urge(request: &proto::MsgRequest) -> bool {
    let policy = match &request.request {
        Some(Asked::GroupNew(new)) => new.policy.as_ref(),
        Some(Asked::GroupSet(set)) => set.policy.as_ref(),
        _ => None,
    };
    policy.is_some_and(|policy| policy.urgent.is_some())
}

fn ask_once(
    socket: &Path,
    request: &proto::MsgRequest,
    may_start: Option<MayStart>,
) -> Result<proto::Answer, Trouble> {
    let (mut stream, welcome) = match may_start {
        Some(may) => daemon::connect_or_start(socket, ConnectionKind::Control, may)?,
        None => daemon::connect_welcomed(socket, ConnectionKind::Control)?,
    };
    let speaks = welcome.protocol.unwrap_or_default();
    if let Some((cannot, minor)) = needs_minor(request)
        && speaks.minor < minor
    {
        return Err(Trouble::Refused(format!(
            "this machine's muster-daemon speaks protocol {speaks}, which cannot {cannot}. \
             Update Muster; a new daemon takes over from an old one when the app starts."
        )));
    }
    let waits = blocks(request);
    let _ = stream.set_read_timeout(if waits { None } else { Some(PATIENCE) });
    let request = proto::Request { id: 1, service: Some(Service::Msg(whole(request, speaks))) };
    connection::send(&mut stream, &request)
        .map_err(|error| Trouble::Unreachable(format!("{}: {error}", socket.display())))?;
    until_answer(&mut stream)
}

fn until_answer(stream: &mut UnixStream) -> Result<proto::Answer, Trouble> {
    loop {
        match connection::receive::<proto::ControlMessage>(stream) {
            Ok(Some(proto::ControlMessage {
                message: Some(proto::control_message::Message::Answer(answer)),
            })) => return Ok(answer),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                return Err(Trouble::Unanswered(
                    "the muster-daemon hung up before answering; if this was a post, `muster msg \
                     log` says whether it landed"
                        .to_string(),
                ));
            }
        }
    }
}

fn refusal_of(answer: &proto::Answer) -> &str {
    match &answer.detail {
        Some(proto::answer::Detail::Msg(msg)) => &msg.refusal,
        _ => "",
    }
}

// ---------------------------------------------------------------------------------------------
// What the answer says

fn render(
    request: &proto::MsgRequest,
    answer: &proto::Answer,
    if_unread: bool,
    json: bool,
) -> Result<String, Trouble> {
    let Some(proto::answer::Detail::Msg(msg)) = &answer.detail else {
        return Err(Trouble::Refused(if answer.outcome() == proto::Outcome::Refused {
            format!(
                "this machine's muster-daemon predates messaging, so it cannot take `{}` \
                 requests ({}). Update Muster; a new daemon takes over from an old one when the \
                 app starts.",
                spelling::NAMESPACE,
                answer.reason
            )
        } else {
            format!("the muster-daemon answered with nothing to show: {}", answer.reason)
        }));
    };
    if answer.outcome() == proto::Outcome::Refused {
        return Err(if msg.refusal == "timed_out" {
            Trouble::TimedOut(answer.reason.clone())
        } else if msg.refusal == "unanswered" {
            Trouble::Unanswered(answer.reason.clone())
        } else {
            Trouble::Refused(answer.reason.clone())
        });
    }
    let Some(said) = &msg.answer else { return Ok(String::new()) };
    Ok(match said {
        Answer::Joined(joined) => joined_text(joined, json),
        Answer::Left(left) => left_text(left, json),
        Answer::Posted(posted) => {
            let urgent = matches!(&request.request, Some(Asked::Post(post)) if post.urgent);
            return posted_text(posted, urgent, json);
        }
        Answer::Entries(entries) => {
            let reading = matches!(request.request, Some(Asked::Read(_)));
            // A hook hands this to the model after every tool call; joins and leaves alone
            // are nothing to interrupt it with.
            let any_message = entries
                .groups
                .iter()
                .flat_map(|group| &group.entries)
                .any(|entry| matches!(entry.what, Some(What::Message(_))));
            if if_unread && !any_message && !json {
                return Ok(String::new());
            }
            entries_text(entries, reading && !if_unread, json, &msg.human_name)
        }
        Answer::Members(members) => members_text(members, json, &msg.human_name),
        Answer::Changed(changed) => changed_text(request, changed, json),
        Answer::Resumed(resumed) => {
            // Resuming is heard or not by the members, not by whoever resumed it.
            let text = match posted_text(resumed, false, json) {
                Ok(text) | Err(Trouble::Unheard(text)) => text,
                Err(other) => return Err(other),
            };
            if json {
                text
            } else {
                text.replacen(
                    &format!("posted #{} to {}", resumed.seq, resumed.group),
                    &format!("resumed {}", resumed.group),
                    1,
                )
            }
        }
        Answer::Groups(groups) => groups_text(groups, json),
        Answer::Deleted(deleted) => deleted_text(deleted, json),
        Answer::Notices(notices) => {
            if json {
                let notices: Vec<_> = notices.notices.iter().map(notice_json).collect();
                serde_json::json!({ "notices": notices }).to_string()
            } else {
                let lines: Vec<String> = notices.notices.iter().map(spelling::wake_text).collect();
                lines.join("\n")
            }
        }
    })
}

fn changed_text(request: &proto::MsgRequest, changed: &msg_answer::Changed, json: bool) -> String {
    if json {
        return serde_json::json!({
            "group": changed.group,
            "seq": changed.seq,
            "added": changed.added,
            "removed": changed.removed,
        })
        .to_string();
    }
    let group = &changed.group;
    match (&request.request, changed.seq) {
        (Some(Asked::GroupNew(_)), _) => format!("made {group} and joined it"),
        (Some(Asked::GroupSet(_)), _) => format!("set {group}'s policy"),
        (Some(Asked::Pause(_)), Some(_)) => format!("paused {group}"),
        (Some(Asked::Pause(_)), None) => format!("{group} was already paused"),
        (Some(Asked::GroupMembers(_)), None) => format!("{group}'s members are unchanged"),
        _ => {
            let mut lines = Vec::new();
            if !changed.added.is_empty() {
                lines.push(format!("added {} to {group}", changed.added.join(", ")));
            }
            if !changed.removed.is_empty() {
                lines.push(format!("removed {} from {group}", changed.removed.join(", ")));
            }
            lines.join("\n")
        }
    }
}

fn deleted_text(deleted: &msg_answer::Deleted, json: bool) -> String {
    if json {
        return serde_json::json!({
            "deleted": deleted.group,
            "entries": deleted.entries,
            "let_go": deleted.let_go,
        })
        .to_string();
    }
    let entries = match deleted.entries {
        1 => "1 entry".to_string(),
        n => format!("{n} entries"),
    };
    if deleted.let_go.is_empty() {
        return format!("deleted {} ({entries})", deleted.group);
    }
    format!("deleted {} ({entries}); let go: {}", deleted.group, deleted.let_go.join(", "))
}

fn groups_text(groups: &msg_answer::Groups, json: bool) -> String {
    let policy_json = |policy: Option<&msg_request::Policy>| {
        let policy = policy.cloned().unwrap_or_default();
        let map = |map: &std::collections::HashMap<String, msg_request::Names>| {
            map.iter()
                .map(|(author, names)| (author.clone(), serde_json::json!(names.names)))
                .collect::<serde_json::Map<_, _>>()
        };
        serde_json::json!({
            "ring": map(&policy.ring),
            "allow": map(&policy.allow),
            "membership": policy.membership,
            "urgent": policy.urgent.map_or_else(|| vec!["*".to_string()], |names| names.names),
            "paused": policy.paused,
        })
    };
    if json {
        let groups: Vec<_> = groups
            .groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "name": group.name,
                    "members": group.members,
                    "policy": policy_json(group.policy.as_ref()),
                })
            })
            .collect();
        return serde_json::json!({ "groups": groups }).to_string();
    }
    if groups.groups.is_empty() {
        return "no groups".to_string();
    }
    let width = groups.groups.iter().map(|group| group.name.len()).max().unwrap_or(0);
    groups
        .groups
        .iter()
        .map(|group| {
            let paused = group.policy.as_ref().is_some_and(|policy| policy.paused);
            let line = format!("{:<width$}  {}", group.name, group.members.join(", "));
            if paused { format!("{line}  (paused)") } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn joined_text(joined: &msg_answer::Joined, json: bool) -> String {
    if json {
        return serde_json::json!({
            "name": joined.name,
            "group": joined.group,
            "created": joined.created,
            "took_over": joined.took_over,
            "groups": joined.groups,
        })
        .to_string();
    }
    let mut text = match &joined.group {
        Some(group) if joined.created => {
            format!("created {group} and joined it as {}", joined.name)
        }
        Some(group) => format!("joined {group} as {}", joined.name),
        None => format!("taking part as {}", joined.name),
    };
    let others: Vec<&str> = joined
        .groups
        .iter()
        .filter(|group| Some(*group) != joined.group.as_ref())
        .map(String::as_str)
        .collect();
    match (joined.took_over, others.is_empty()) {
        (true, true) => text.push_str(" (taken over from a session that had gone)"),
        // A name like `director` outlives the council it directed, and the session taking it
        // over would be woken by every group the gone one was in.
        (true, false) => {
            let leave = spelling::command(LEAVE, "--group <group>");
            let others = others.join(", ");
            let _ = write!(
                text,
                ", taken over from a session that had gone, with its place in {others}; `{leave}` \
                 leaves one you are done with"
            );
        }
        (false, false) => {
            let _ = write!(text, "; also in {}", others.join(", "));
        }
        (false, true) => {}
    }
    text
}

fn left_text(left: &msg_answer::Left, json: bool) -> String {
    if json {
        return serde_json::json!({ "groups": left.groups, "stopped": left.stopped }).to_string();
    }
    match (left.stopped, left.groups.is_empty()) {
        (true, true) => "stopped taking part".to_string(),
        (true, false) => format!("left {} and stopped taking part", left.groups.join(", ")),
        (false, _) => format!("left {}", left.groups.join(", ")),
    }
}

/// The human, on this machine or, as `@human@laptop`, on another.
fn is_human(name: &str) -> bool {
    name.strip_prefix(spelling::HUMAN).is_some_and(|rest| rest.is_empty() || rest.starts_with('@'))
}

/// A participant as text shows it: the human by the name the app set beside its address, so a
/// reader knows who it is and a model still knows what to put in `--to` (MIP-4, section 10).
fn shown(name: &str, human: &str) -> String {
    if human.is_empty() || !is_human(name) {
        return name.to_string();
    }
    format!("{human} ({name})")
}

/// What a post did for each participant it was for. Nobody live heard it - nobody woken, to be
/// rung, already woken, held for a resume or a link, or the human - is [`Trouble::Unheard`],
/// printed the same way.
fn posted_text(posted: &msg_answer::Posted, urgent: bool, json: bool) -> Result<String, Trouble> {
    use msg_answer::Reach;
    let named = |wanted: Reach| -> Vec<&msg_answer::Reached> {
        posted.reached.iter().filter(|reached| reached.reach() == wanted).collect()
    };
    let (woke, deferred) = (named(Reach::Woken), named(Reach::Deferred));
    let (already, waiting, gone) =
        (named(Reach::AlreadyWoken), named(Reach::Waiting), named(Reach::Gone));
    let (no_agent, no_doorbell) = (named(Reach::NoAgent), named(Reach::NoDoorbell));
    let paused = named(Reach::Paused);
    let unreachable = named(Reach::Unreachable);
    let heard = !woke.is_empty()
        || !deferred.is_empty()
        || !already.is_empty()
        || !paused.is_empty()
        || !unreachable.is_empty()
        || waiting.iter().any(|reached| is_human(&reached.name));
    let text = if json {
        let listed = |reached: &[&msg_answer::Reached]| -> Vec<String> {
            reached.iter().map(|reached| reached.name.clone()).collect()
        };
        let doing: serde_json::Map<String, serde_json::Value> = posted
            .reached
            .iter()
            .filter_map(|reached| {
                Some((reached.name.clone(), activity(reached.activity())?.into()))
            })
            .collect();
        let until: serde_json::Map<String, serde_json::Value> = deferred
            .iter()
            .map(|reached| (reached.name.clone(), until_key(reached.until()).into()))
            .collect();
        serde_json::json!({
            "group": posted.group,
            "seq": posted.seq,
            "urgent": urgent,
            "woke": listed(&woke),
            "deferred": listed(&deferred),
            "already_woken": listed(&already),
            "waiting": listed(&waiting),
            "gone": listed(&gone),
            "no_agent": listed(&no_agent),
            "no_doorbell": listed(&no_doorbell),
            "paused": listed(&paused),
            "unreachable": listed(&unreachable),
            "doing": doing,
            "until": until,
        })
        .to_string()
    } else {
        let with = |reached: &msg_answer::Reached, also: Option<&str>| {
            let said: Vec<&str> = activity(reached.activity()).into_iter().chain(also).collect();
            if said.is_empty() {
                reached.name.clone()
            } else {
                format!("{} ({})", reached.name, said.join(", "))
            }
        };
        let urgently = if urgent { ", urgent" } else { "" };
        let mut lines = vec![format!("posted #{} to {}{urgently}", posted.seq, posted.group)];
        let mut woken: Vec<String> = woke.iter().map(|reached| with(reached, None)).collect();
        woken.extend(already.iter().map(|reached| with(reached, Some("already woken"))));
        if !woken.is_empty() {
            lines.push(format!("woke: {}", woken.join(", ")));
        }
        for until in [Until::Idle, Until::Unblocked, Until::Prompt, Until::Agent] {
            let later: Vec<String> = deferred
                .iter()
                .filter(|reached| reached.until() == until)
                .map(|reached| with(reached, None))
                .collect();
            if !later.is_empty() {
                lines.push(format!("rung once {}: {}", until_text(until), later.join(", ")));
            }
        }
        if !paused.is_empty() {
            let held: Vec<String> = paused.iter().map(|reached| with(reached, None)).collect();
            lines.push(format!("held while {} is paused: {}", posted.group, held.join(", ")));
        }
        let mut not: Vec<String> = gone.iter().map(|reached| with(reached, Some("gone"))).collect();
        not.extend(no_agent.iter().map(|reached| with(reached, Some("no agent in its pane"))));
        not.extend(
            no_doorbell.iter().map(|reached| with(reached, Some("its prompt cannot be read"))),
        );
        not.extend(
            waiting
                .iter()
                .map(|reached| with(reached, Some(unless_human(reached, "sees it when it reads")))),
        );
        not.extend(unreachable.iter().map(|reached| {
            let why = "its machine cannot be reached; it sees this once it can";
            with(reached, Some(unless_human(reached, why)))
        }));
        if !not.is_empty() {
            lines.push(format!("not woken: {}", not.join(", ")));
        }
        lines.join("\n")
    };
    if heard { Ok(text) } else { Err(Trouble::Unheard(text)) }
}

/// Why `reached` was not woken: `why`, unless it is the human, who is notified when a window
/// opens - which is also what links the machines when the human is on one that cannot be reached.
fn unless_human(reached: &msg_answer::Reached, why: &'static str) -> &'static str {
    if is_human(&reached.name) { "notified when a window opens" } else { why }
}

/// What a deferred ring waits for, as the post's answer says it. A daemon from before the
/// reason was sent deferred only until its agent was idle.
fn until_text(until: Until) -> &'static str {
    match until {
        Until::Idle | Until::Unspecified => "idle",
        Until::Unblocked => "it is not blocked",
        Until::Prompt => "its prompt is empty",
        Until::Agent => "an agent is found",
    }
}

fn until_key(until: Until) -> &'static str {
    match until {
        Until::Idle | Until::Unspecified => "idle",
        Until::Unblocked => "unblocked",
        Until::Prompt => "prompt",
        Until::Agent => "agent",
    }
}

/// What an agent in a pane is doing, as its daemon's detection reads it.
fn activity(activity: msg_answer::Activity) -> Option<&'static str> {
    match activity {
        msg_answer::Activity::Unspecified => None,
        msg_answer::Activity::Working => Some("working"),
        msg_answer::Activity::Blocked => Some("blocked"),
        msg_answer::Activity::Idle => Some("idle"),
        msg_answer::Activity::Waiting => Some("waiting"),
    }
}

/// Council v1's framing, a start and an end marker per message, so a model reading several
/// knows where each begins and whose it is.
fn entries_text(
    entries: &msg_answer::Entries,
    say_when_empty: bool,
    json: bool,
    human: &str,
) -> String {
    if json {
        let groups: Vec<_> = entries
            .groups
            .iter()
            .map(|group| {
                let entries: Vec<_> = group.entries.iter().map(entry_json).collect();
                serde_json::json!({
                    "group": group.group,
                    "entries": entries,
                    "behind": group.behind,
                })
            })
            .collect();
        let mut value = serde_json::json!({ "groups": groups });
        if !human.is_empty() {
            value["human_name"] = human.into();
        }
        return value.to_string();
    }
    let mut blocks = Vec::new();
    for group in &entries.groups {
        for entry in &group.entries {
            let name = &group.group;
            let seq = entry.seq;
            blocks.push(match &entry.what {
                Some(What::Message(message)) => {
                    let to = if message.to.is_empty() {
                        String::new()
                    } else {
                        let to: Vec<String> =
                            message.to.iter().map(|name| shown(name, human)).collect();
                        format!(" -> {}", to.join(", "))
                    };
                    let urgent = if message.urgent { ", urgent" } else { "" };
                    let author = shown(&message.author, human);
                    let head = format!("--- {name} #{seq} | {author}{to}{urgent} ---");
                    format!(
                        "{head}\n{}\n--- end {name} #{seq} | {author} ---",
                        message.body.trim_end_matches('\n'),
                    )
                }
                Some(What::Created(by)) => {
                    format!("--- {name} #{seq} | created by {} ---", shown(by, human))
                }
                Some(What::Joined(who)) => {
                    format!("--- {name} #{seq} | {} joined ---", shown(who, human))
                }
                Some(What::Left(who)) => {
                    format!("--- {name} #{seq} | {} left ---", shown(who, human))
                }
                Some(What::Changed(changed)) => format!(
                    "--- {name} #{seq} | {} {} ---",
                    shown(&changed.by, human),
                    change_text(changed)
                ),
                None => continue,
            });
        }
    }
    if blocks.is_empty() && say_when_empty {
        blocks.push("nothing unread".to_string());
    }
    // A replica whose home cannot be reached may lack what was posted there since.
    for group in &entries.groups {
        if let Some(machine) = &group.behind {
            blocks.push(format!(
                "{machine} cannot be reached now, so {} may be missing messages posted since",
                group.group
            ));
        }
    }
    blocks.join("\n")
}

fn change_text(changed: &msg_answer::entry::Changed) -> &'static str {
    use msg_answer::entry::Change;
    match changed.change() {
        Change::SetPolicy => "set the policy",
        Change::Paused => "paused it",
        Change::Resumed => "resumed it",
        Change::Unspecified => "changed it",
    }
}

fn entry_json(entry: &msg_answer::Entry) -> serde_json::Value {
    let mut value = serde_json::json!({ "seq": entry.seq, "at_ms": entry.at_ms });
    match &entry.what {
        Some(What::Message(message)) => {
            value["author"] = message.author.clone().into();
            value["to"] = message.to.clone().into();
            value["body"] = message.body.clone().into();
            if message.urgent {
                value["urgent"] = true.into();
            }
        }
        Some(What::Created(by)) => value["created"] = by.clone().into(),
        Some(What::Joined(who)) => value["joined"] = who.clone().into(),
        Some(What::Left(who)) => value["left"] = who.clone().into(),
        Some(What::Changed(changed)) => {
            value["changed"] =
                serde_json::json!({ "by": changed.by, "change": change_text(changed) });
        }
        None => {}
    }
    value
}

fn members_text(members: &msg_answer::Members, json: bool, human: &str) -> String {
    let liveness = |member: &msg_answer::Member| match member.liveness() {
        msg_answer::Liveness::Alive => "alive",
        msg_answer::Liveness::Gone => "gone",
        msg_answer::Liveness::Human => "human",
        msg_answer::Liveness::Unreachable => "unreachable",
        msg_answer::Liveness::Unspecified => "unknown",
    };
    if json {
        let members: Vec<_> = members
            .members
            .iter()
            .map(|member| {
                serde_json::json!({
                    "name": member.name,
                    "liveness": liveness(member),
                    "activity": activity(member.activity()),
                    "pane": member.pane,
                    "groups": member.groups,
                })
            })
            .collect();
        let mut value = serde_json::json!({ "members": members });
        if !human.is_empty() {
            value["human_name"] = human.into();
        }
        return value.to_string();
    }
    if members.members.is_empty() {
        return "nobody".to_string();
    }
    let states: Vec<String> = members
        .members
        .iter()
        .map(|member| match activity(member.activity()) {
            Some(doing) => format!("{} ({doing})", liveness(member)),
            None => liveness(member).to_string(),
        })
        .collect();
    let names: Vec<String> =
        members.members.iter().map(|member| shown(&member.name, human)).collect();
    let width = names.iter().map(String::len).max().unwrap_or(0);
    let state_width = states.iter().map(String::len).max().unwrap_or(0);
    members
        .members
        .iter()
        .zip(&states)
        .zip(&names)
        .map(|((member, state), name)| {
            let groups = if member.groups.is_empty() {
                "in no group".to_string()
            } else {
                member.groups.join(", ")
            };
            format!("{name:<width$}  {state:<state_width$}  {groups}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn notice_json(notice: &msg_answer::Notice) -> serde_json::Value {
    serde_json::json!({
        "group": notice.group,
        "first": notice.first,
        "last": notice.last,
        "count": notice.count,
        "to_you": notice.to_you,
        "urgent": notice.urgent,
        "from": notice.from,
        "text": spelling::wake_text(notice),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A join says which groups the name is in besides the one joined, and a name taken over
    /// says so together with how to leave what came with it.
    #[test]
    fn a_join_says_what_else_the_name_is_in() {
        let joined = |took_over, groups: &[&str]| msg_answer::Joined {
            name: "director".to_string(),
            group: Some("review".to_string()),
            created: false,
            took_over,
            groups: groups.iter().map(ToString::to_string).collect(),
        };
        assert_eq!(joined_text(&joined(false, &["review"]), false), "joined review as director");
        assert_eq!(
            joined_text(&joined(false, &["alpha", "review"]), false),
            "joined review as director; also in alpha"
        );
        assert_eq!(
            joined_text(&joined(true, &["review"]), false),
            "joined review as director (taken over from a session that had gone)"
        );
        let took_over = joined_text(&joined(true, &["alpha", "demo", "review"]), false);
        assert!(
            took_over.starts_with(
                "joined review as director, taken over from a session that had gone, with its \
                 place in alpha, demo; `"
            ),
            "{took_over}"
        );
        assert!(took_over.ends_with("leave --group <group>` leaves one you are done with"));
    }

    #[test]
    fn what_protocol_1_0_does_not_know_is_asked_of_1_1_or_later_only() {
        let asked = |request| proto::MsgRequest { caller: None, request: Some(request) };
        let wait = |due| asked(Asked::Wait(msg_request::Wait { due, ..Default::default() }));
        let join = |pull| asked(Asked::Join(msg_request::Join { pull, ..Default::default() }));
        let pause = asked(Asked::Pause(msg_request::Pause { group: "g".to_string() }));
        assert_eq!(needs_minor(&wait(false)), None);
        assert_eq!(needs_minor(&join(false)), None);
        assert_eq!(needs_minor(&wait(true)).map(|(_, minor)| minor), Some(1));
        assert_eq!(needs_minor(&join(true)).map(|(_, minor)| minor), Some(1));
        assert_eq!(needs_minor(&pause).map(|(_, minor)| minor), Some(1));
    }

    /// A policy file is the whole policy: one leaving out `urgent` lets anyone post urgently
    /// again, said outright to a daemon that knows the key, which keeps the group's list when a
    /// request says nothing. An older daemon is sent the request as it was.
    #[test]
    fn a_policy_file_without_urgent_says_anyone_to_a_daemon_that_knows_the_key() {
        let set = |urgent: Option<Vec<String>>| proto::MsgRequest {
            caller: None,
            request: Some(Asked::GroupSet(msg_request::GroupSet {
                group: "g".to_string(),
                policy: Some(msg_request::Policy {
                    urgent: urgent.map(|names| msg_request::Names { names }),
                    ..msg_request::Policy::default()
                }),
            })),
        };
        let urgent = |request: proto::MsgRequest| match request.request {
            Some(Asked::GroupSet(set)) => set.policy.and_then(|policy| policy.urgent),
            _ => unreachable!(),
        };
        let speaking = |minor| proto::Version { major: 1, minor };
        let anyone = Some(msg_request::Names { names: vec!["*".to_string()] });
        assert_eq!(urgent(whole(&set(None), speaking(3))), anyone);
        assert_eq!(urgent(whole(&set(None), speaking(2))), None);
        let director = vec!["director".to_string()];
        assert_eq!(
            urgent(whole(&set(Some(director.clone())), speaking(3))),
            Some(msg_request::Names { names: director })
        );
    }

    /// A daemon before 1.3 ignores what it does not know: an urgent post would ring as an
    /// ordinary one, and a policy saying who may post urgently would let anyone.
    #[test]
    fn what_protocol_1_2_does_not_know_is_asked_of_1_3_or_later_only() {
        let asked = |request| proto::MsgRequest { caller: None, request: Some(request) };
        let post = |urgent| {
            asked(Asked::Post(msg_request::Post { urgent, ..msg_request::Post::default() }))
        };
        let new = |urgent: Option<Vec<String>>| {
            let policy = msg_request::Policy {
                urgent: urgent.map(|names| msg_request::Names { names }),
                ..msg_request::Policy::default()
            };
            asked(Asked::GroupNew(msg_request::GroupNew {
                group: "g".to_string(),
                policy: Some(policy),
            }))
        };
        assert_eq!(needs_minor(&post(false)), None);
        assert_eq!(needs_minor(&post(true)).map(|(_, minor)| minor), Some(3));
        assert_eq!(needs_minor(&new(None)).map(|(_, minor)| minor), Some(1));
        assert_eq!(needs_minor(&new(Some(vec![]))).map(|(_, minor)| minor), Some(3));
    }

    #[test]
    fn an_urgent_post_says_so_and_a_ring_at_a_dialog_says_what_it_waits_for() {
        let posted = msg_answer::Posted {
            group: "review".to_string(),
            seq: 9,
            reached: vec![msg_answer::Reached {
                name: "critic".to_string(),
                reach: msg_answer::Reach::Deferred.into(),
                activity: msg_answer::Activity::Blocked.into(),
                until: Until::Unblocked.into(),
            }],
        };
        assert_eq!(
            posted_text(&posted, true, false).unwrap(),
            "posted #9 to review, urgent\nrung once it is not blocked: critic (blocked)"
        );
        let json: serde_json::Value =
            serde_json::from_str(&posted_text(&posted, true, true).unwrap()).unwrap();
        assert_eq!(json["urgent"], true);
        assert_eq!(json["until"]["critic"], "unblocked");
    }

    /// The human is shown by the name the app set, beside the address an agent writes in `--to`,
    /// wherever a transcript names it; nobody else changes.
    #[test]
    fn the_human_is_shown_by_name_beside_its_address() {
        let message = msg_answer::Message {
            author: "@human".to_string(),
            to: vec!["builder".to_string(), "@human@lap".to_string()],
            body: "go".to_string(),
            urgent: false,
        };
        let entry = |seq, what| msg_answer::Entry { seq, at_ms: 0, what: Some(what) };
        let entries = msg_answer::Entries {
            groups: vec![msg_answer::GroupEntries {
                group: "review".to_string(),
                entries: vec![
                    entry(2, What::Joined("@human".to_string())),
                    entry(3, What::Message(message)),
                ],
                behind: None,
            }],
        };
        assert_eq!(
            entries_text(&entries, true, false, "Alex"),
            "--- review #2 | Alex (@human) joined ---\n\
             --- review #3 | Alex (@human) -> builder, Alex (@human@lap) ---\ngo\n\
             --- end review #3 | Alex (@human) ---"
        );
        assert!(
            entries_text(&entries, true, false, "").starts_with("--- review #2 | @human joined")
        );

        let member = |name: &str, liveness| msg_answer::Member {
            name: name.to_string(),
            liveness: liveness as i32,
            groups: vec!["review".to_string()],
            ..msg_answer::Member::default()
        };
        let members = msg_answer::Members {
            members: vec![
                member("@human", msg_answer::Liveness::Human),
                member("builder", msg_answer::Liveness::Gone),
            ],
        };
        assert_eq!(
            members_text(&members, false, "Alex"),
            "Alex (@human)  human  review\nbuilder        gone   review"
        );
    }

    #[test]
    fn an_urgent_message_says_so_where_it_is_read() {
        let message = msg_answer::Message {
            author: "director".to_string(),
            to: vec!["builder".to_string()],
            body: "stop".to_string(),
            urgent: true,
        };
        let entries = msg_answer::Entries {
            groups: vec![msg_answer::GroupEntries {
                group: "review".to_string(),
                entries: vec![msg_answer::Entry {
                    seq: 4,
                    at_ms: 0,
                    what: Some(What::Message(message)),
                }],
                behind: None,
            }],
        };
        assert_eq!(
            entries_text(&entries, true, false, ""),
            "--- review #4 | director -> builder, urgent ---\nstop\n--- end review #4 | director ---"
        );
    }

    #[test]
    fn a_handover_is_waited_out_for_a_while_and_no_longer() {
        let start = Instant::now();
        let mut patience = Patience::default();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert!(patience.allows_another(at(0), at(200)));
        assert!(patience.allows_another(at(400), at(600)));
        assert!(!patience.allows_another(at(10_000), at(10_200)));
    }

    /// A daemon from before messaging decodes a `msg` request as one it does not know, and
    /// refuses it with no msg answer: the caller is told to update, not shown the refusal.
    #[test]
    fn a_daemon_from_before_messaging_is_named_as_such() {
        let request = proto::MsgRequest {
            caller: None,
            request: Some(Asked::Who(msg_request::Who::default())),
        };
        let answer = proto::Answer {
            outcome: proto::Outcome::Refused.into(),
            reason: "unsupported request".to_string(),
            ..proto::Answer::default()
        };
        let Err(Trouble::Refused(said)) = render(&request, &answer, false, false) else {
            panic!("an old daemon's refusal is a refusal");
        };
        assert!(said.contains("predates messaging"), "{said}");
        assert!(said.contains("Update Muster"), "{said}");
    }

    /// A post another machine took and never answered may have landed there, so it exits as a
    /// daemon that hung up before answering does, not as a refusal: a script that posts again on
    /// a refusal would post it twice.
    #[test]
    fn a_post_the_other_machine_never_answered_is_unanswered() {
        let request = proto::MsgRequest {
            caller: None,
            request: Some(Asked::Post(msg_request::Post::default())),
        };
        let answer = proto::Answer {
            outcome: proto::Outcome::Refused.into(),
            reason: "devenv did not answer".to_string(),
            detail: Some(proto::answer::Detail::Msg(proto::MsgAnswer {
                refusal: "unanswered".to_string(),
                ..proto::MsgAnswer::default()
            })),
            ..proto::Answer::default()
        };
        let trouble = render(&request, &answer, false, false).unwrap_err();
        assert_eq!(trouble.code(), 4, "{trouble:?}");
    }

    /// A wait that ran for minutes and was then ended by a handover is asked again of the new
    /// daemon: its patience is for the handover, not for the wait before it.
    #[test]
    fn a_long_wait_ended_by_a_handover_is_asked_again() {
        let start = Instant::now();
        let mut patience = Patience::default();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert!(patience.allows_another(at(0), at(300_000)));
        assert!(patience.allows_another(at(300_200), at(300_400)));
        assert!(!patience.allows_another(at(310_400), at(310_600)));
    }

    /// A member on a machine that cannot be reached is not woken yet, and says why. It is
    /// woken when the link returns, as a paused group's members are on resume, so the post was
    /// heard: exit 6 would tell its author that no answer is coming. The human there is notified
    /// once a window opens, since a window is what links the machines.
    #[test]
    fn a_member_on_an_unreachable_machine_is_named_as_such_and_hears_it_later() {
        let unreachable = |name: &str| msg_answer::Reached {
            name: name.to_string(),
            reach: msg_answer::Reach::Unreachable.into(),
            ..msg_answer::Reached::default()
        };
        let posted = msg_answer::Posted {
            group: "review".to_string(),
            seq: 7,
            reached: vec![unreachable("critic@devenv"), unreachable("@human@lap")],
        };
        let text = posted_text(&posted, false, false).expect("heard once the link returns");
        assert_eq!(
            text,
            "posted #7 to review\nnot woken: critic@devenv (its machine cannot be reached; it \
             sees this once it can), @human@lap (notified when a window opens)"
        );
    }

    #[test]
    fn a_read_of_a_group_whose_home_cannot_be_reached_says_it_may_be_behind() {
        let entries = msg_answer::Entries {
            groups: vec![msg_answer::GroupEntries {
                group: "review@lap".to_string(),
                entries: Vec::new(),
                behind: Some("lap".to_string()),
            }],
        };
        assert_eq!(
            entries_text(&entries, true, false, ""),
            "nothing unread\nlap cannot be reached now, so review@lap may be missing messages \
             posted since"
        );
    }
}
