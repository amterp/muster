//! `muster msg`: agents posting to each other through the daemon on this machine (MIP-4).
//!
//! These verbs talk to muster-daemon directly rather than to a window, because messaging has to
//! work with no window open - a Claude session in a plain terminal takes part the same way as
//! one in a pane. Their names come from `muster_daemon_proto::messaging`, which the daemon's
//! wakes and refusals read too, so a rename is one edit there.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand};
use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::messaging::{self as spelling, JOIN, LEAVE, LOG, POST, READ, WAIT, WHO};
use muster_daemon_proto::msg_answer::{self, Answer, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::{self as proto, ConnectionKind, install, request::Service};

use crate::Trouble;
use crate::args::{Failure, TextSource};
use crate::environment;

/// Claude Code's inbox socket, which it exports to every command a session runs.
pub const CLAUDE_INBOX: &str = "CLAUDE_CODE_MESSAGING_SOCKET";
/// The daemon a pane runs on, which the daemon sets in every pane it starts.
pub const DAEMON_SOCKET: &str = "MUSTER_DAEMON_SOCKET";

/// What `muster msg --help` says before the verbs: the protocol an agent follows.
pub const PROTOCOL: &str = "\
Agents post messages to groups through the muster-daemon on this machine, and are woken when a \
message arrives for them - nobody waits in a loop.

Join a group, post, and end your turn. When a message arrives for you, you are told in one line \
how many wait and from whom, and the command that reads them: run `muster msg read`. Reading \
moves your place, so a message is read once.

A post is refused while you have unread messages in that group: read, then post again. This \
keeps you from answering a conversation you have not seen.

An unaddressed post wakes every member of its group but you; `--to NAME` wakes only NAME, \
though every member can still read it. With no --group, a post goes to the one group you share \
with the people you address, or to a new group of exactly you and them.

Who you are: `--as NAME` if given, else the Claude Code session you run in (from \
$CLAUDE_CODE_MESSAGING_SOCKET), else you are the human. A session that never joined under a \
name is named after its working directory.

A Claude Code session started with --dangerously-skip-permissions holds a wake for approval \
unless it was also started with --settings '{\"crossSessionInbound\":\"accept\"}'.

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
    },

    /// Block until a message that would wake you is unread, then print what a wake would say
    #[command(name = WAIT)]
    Wait {
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// Give up after this many seconds, exiting 5
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u32>,
    },
}

/// A `msg` request as the command line gives it, before anything is read from disk.
#[derive(Debug)]
pub struct Messaging {
    pub request: proto::MsgRequest,
    /// Where a post's body comes from when it is not on the command line.
    pub body_from: Option<TextSource>,
    pub if_unread: bool,
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
    let mut if_unread = false;
    let asked = match verb {
        Verb::Join { name, group } => {
            Asked::Join(msg_request::Join { name: name.clone(), group: group.clone() })
        }
        Verb::Leave { group } => Asked::Leave(msg_request::Leave { group: group.clone() }),
        Verb::Who { group } => Asked::Who(msg_request::Who { group: group.clone() }),
        Verb::Post { group, to, file, text } => {
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
            Asked::Post(msg_request::Post { group: group.clone(), to: to.clone(), body })
        }
        Verb::Read { group, if_unread: quiet } => {
            if_unread = *quiet;
            Asked::Read(msg_request::Read { group: group.clone() })
        }
        Verb::Log { group, since } => {
            Asked::Log(msg_request::Log { group: group.clone(), since: *since })
        }
        Verb::Wait { group, timeout } => Asked::Wait(msg_request::Wait {
            group: group.clone(),
            timeout_ms: timeout.map(|seconds| seconds.saturating_mul(1000)),
        }),
    };
    let request = proto::MsgRequest { caller: Some(caller), request: Some(asked) };
    Ok(Messaging { request, body_from, if_unread })
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
    let socket = daemon_socket(environment).ok_or_else(|| {
        Trouble::Unreachable(format!(
            "no muster-daemon to ask: ${DAEMON_SOCKET} is not set and neither is $HOME."
        ))
    })?;
    let answer = ask(&socket, &messaging.request)?;
    render(&messaging.request, &answer, messaging.if_unread, json)
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

/// `$MUSTER_DAEMON_SOCKET`, which names the daemon a pane runs on, else this install's daemon
/// under Muster's home.
pub fn daemon_socket(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(socket) = environment.get(DAEMON_SOCKET).filter(|socket| !socket.is_empty()) {
        return Some(PathBuf::from(socket));
    }
    environment::muster_home(environment).map(|home| install::socket_path(Path::new(&home)))
}

/// How long to keep asking a daemon that is handing over to a new one, which is refusing
/// changes until the new one serves. A handoff takes well under a second.
const HANDOVER_PATIENCE: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(200);

/// How long a request other than a wait may take to be answered.
const PATIENCE: Duration = Duration::from_mins(1);

/// Asks, and asks again while the daemon is handing over to a new one: it refuses changes
/// until the new one serves, and ends the connections of waits in progress.
fn ask(socket: &Path, request: &proto::MsgRequest) -> Result<proto::Answer, Trouble> {
    let started = std::time::Instant::now();
    let waits = matches!(request.request, Some(Asked::Wait(_)));
    let mut answered_before = false;
    loop {
        let answer = ask_once(socket, request);
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
        if !again || started.elapsed() >= HANDOVER_PATIENCE {
            return answer;
        }
        std::thread::sleep(RETRY);
    }
}

fn ask_once(socket: &Path, request: &proto::MsgRequest) -> Result<proto::Answer, Trouble> {
    let client = format!("muster {}", env!("CARGO_PKG_VERSION"));
    let (mut stream, _) = connection::connect(socket, ConnectionKind::Control, &client).map_err(
        |error| match error {
            HandshakeError::Refused(refused) => Trouble::Refused(format!(
                "the muster-daemon at {} would not talk to this muster: {}",
                socket.display(),
                refused.reason
            )),
            HandshakeError::Unreachable(why)
            | HandshakeError::Garbled(why)
            | HandshakeError::Stalled(why) => Trouble::Unreachable(format!(
                "no muster-daemon answered at {} ({why}). One runs while Muster does; set \
                     ${DAEMON_SOCKET} to reach another.",
                socket.display()
            )),
        },
    )?;
    let waits = matches!(request.request, Some(Asked::Wait(_)));
    let _ = stream.set_read_timeout(if waits { None } else { Some(PATIENCE) });
    let request = proto::Request { id: 1, service: Some(Service::Msg(request.clone())) };
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
        } else {
            Trouble::Refused(answer.reason.clone())
        });
    }
    let Some(said) = &msg.answer else { return Ok(String::new()) };
    Ok(match said {
        Answer::Joined(joined) => joined_text(joined, json),
        Answer::Left(left) => left_text(left, json),
        Answer::Posted(posted) => posted_text(posted, json),
        Answer::Entries(entries) => {
            let reading = matches!(request.request, Some(Asked::Read(_)));
            entries_text(entries, reading && !if_unread, json)
        }
        Answer::Members(members) => members_text(members, json),
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

fn joined_text(joined: &msg_answer::Joined, json: bool) -> String {
    if json {
        return serde_json::json!({
            "name": joined.name,
            "group": joined.group,
            "created": joined.created,
            "took_over": joined.took_over,
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
    if joined.took_over {
        text.push_str(" (taken over from a session that had gone)");
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

fn posted_text(posted: &msg_answer::Posted, json: bool) -> String {
    let named = |wanted: msg_answer::Reach| -> Vec<&str> {
        posted
            .reached
            .iter()
            .filter(|reached| reached.reach() == wanted)
            .map(|reached| reached.name.as_str())
            .collect()
    };
    let woke = named(msg_answer::Reach::Woken);
    let already = named(msg_answer::Reach::AlreadyWoken);
    let waiting = named(msg_answer::Reach::Waiting);
    let gone = named(msg_answer::Reach::Gone);
    if json {
        return serde_json::json!({
            "group": posted.group,
            "seq": posted.seq,
            "woke": woke,
            "already_woken": already,
            "waiting": waiting,
            "gone": gone,
        })
        .to_string();
    }
    let mut lines = vec![format!("posted #{} to {}", posted.seq, posted.group)];
    let mut woken: Vec<String> = woke.iter().map(ToString::to_string).collect();
    woken.extend(already.iter().map(|name| format!("{name} (already woken)")));
    if !woken.is_empty() {
        lines.push(format!("woke: {}", woken.join(", ")));
    }
    let mut not: Vec<String> = gone.iter().map(|name| format!("{name} (gone)")).collect();
    not.extend(waiting.iter().map(|name| format!("{name} (sees it when it reads)")));
    if !not.is_empty() {
        lines.push(format!("not woken: {}", not.join(", ")));
    }
    lines.join("\n")
}

/// Council v1's framing, a start and an end marker per message, so a model reading several
/// knows where each begins and whose it is.
fn entries_text(entries: &msg_answer::Entries, say_when_empty: bool, json: bool) -> String {
    if json {
        let groups: Vec<_> = entries
            .groups
            .iter()
            .map(|group| {
                let entries: Vec<_> = group.entries.iter().map(entry_json).collect();
                serde_json::json!({ "group": group.group, "entries": entries })
            })
            .collect();
        return serde_json::json!({ "groups": groups }).to_string();
    }
    let mut blocks = Vec::new();
    for group in &entries.groups {
        for entry in &group.entries {
            let name = &group.group;
            let seq = entry.seq;
            blocks.push(match &entry.what {
                Some(What::Message(message)) => {
                    let head = if message.to.is_empty() {
                        format!("--- {name} #{seq} | {} ---", message.author)
                    } else {
                        format!(
                            "--- {name} #{seq} | {} -> {} ---",
                            message.author,
                            message.to.join(", ")
                        )
                    };
                    format!(
                        "{head}\n{}\n--- end {name} #{seq} | {} ---",
                        message.body.trim_end_matches('\n'),
                        message.author
                    )
                }
                Some(What::Created(by)) => format!("--- {name} #{seq} | created by {by} ---"),
                Some(What::Joined(who)) => format!("--- {name} #{seq} | {who} joined ---"),
                Some(What::Left(who)) => format!("--- {name} #{seq} | {who} left ---"),
                None => continue,
            });
        }
    }
    if blocks.is_empty() && say_when_empty {
        return "nothing unread".to_string();
    }
    blocks.join("\n")
}

fn entry_json(entry: &msg_answer::Entry) -> serde_json::Value {
    let mut value = serde_json::json!({ "seq": entry.seq, "at_ms": entry.at_ms });
    match &entry.what {
        Some(What::Message(message)) => {
            value["author"] = message.author.clone().into();
            value["to"] = message.to.clone().into();
            value["body"] = message.body.clone().into();
        }
        Some(What::Created(by)) => value["created"] = by.clone().into(),
        Some(What::Joined(who)) => value["joined"] = who.clone().into(),
        Some(What::Left(who)) => value["left"] = who.clone().into(),
        None => {}
    }
    value
}

fn members_text(members: &msg_answer::Members, json: bool) -> String {
    let liveness = |member: &msg_answer::Member| match member.liveness() {
        msg_answer::Liveness::Alive => "alive",
        msg_answer::Liveness::Gone => "gone",
        msg_answer::Liveness::Human => "human",
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
                    "groups": member.groups,
                })
            })
            .collect();
        return serde_json::json!({ "members": members }).to_string();
    }
    if members.members.is_empty() {
        return "nobody".to_string();
    }
    let width = members.members.iter().map(|member| member.name.len()).max().unwrap_or(0);
    members
        .members
        .iter()
        .map(|member| {
            let groups = if member.groups.is_empty() {
                "in no group".to_string()
            } else {
                member.groups.join(", ")
            };
            format!("{:<width$}  {:<6}  {groups}", member.name, liveness(member))
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
        "from": notice.from,
        "text": spelling::wake_text(notice),
    })
}
