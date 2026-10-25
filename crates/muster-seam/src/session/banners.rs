//! The banners for messages to the human, as the app keeps them (mip/0009-group-conversation.md,
//! section 5): the core's model, fed by the daemons' notices, filled by fetching each message from
//! its group's log as the human, and told what the human did - a reply posted as the human, a
//! dismissal, a reply being written.
//!
//! Banners are about the whole app, like the attention they come from: one stack, which the shell
//! draws in the window in front.

use std::sync::Mutex;

use muster_core::attention::GroupKey;
use muster_core::banners::Message;
use muster_core::composition::DaemonId;
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::intent::Refusal;
use muster_core::posting::{self, Logged, Reach, Reached, Sent};
use muster_daemon_proto::msg_answer::{self, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};

use super::{SESSION, ask_as_human_for};
use crate::proto::{Event, Response, event};
use crate::{convert, ffi};

/// Fetches the message numbered `seq` from `group`'s log for its banner, off the caller's thread:
/// the caller is a daemon's notice thread, which must not wait on that daemon's answer.
pub(super) fn fetch(group: &GroupKey, seq: u64) {
    let spawned = std::thread::Builder::new().name("muster-banner-fetch".into()).spawn({
        let group = group.clone();
        move || {
            let fetched = fetch_message(&group, seq);
            if let Err(why) = &fetched {
                log::warn(
                    "banner.fetch_failed",
                    fields! {
                        "daemon" => group.daemon.to_string(),
                        "group" => group.group.clone(),
                        "seq" => seq,
                        "error" => why.clone(),
                        "impact" => "the banner says how many messages wait and from whom, but \
                                     not what the newest says, and takes no reply",
                        "check" => "whether the daemon answers `muster msg log` for that group; \
                                    opening the conversation shows the message",
                    },
                );
            }
            let changed = poison::lock(&SESSION, "session").banners.fetched(&group, seq, fetched);
            if changed {
                announce();
            }
        }
    });
    if let Err(error) = spawned {
        let why = format!("no thread could be started to fetch it ({error})");
        let changed = poison::lock(&SESSION, "session").banners.fetched(group, seq, Err(why));
        if changed {
            announce();
        }
    }
}

fn fetch_message(group: &GroupKey, seq: u64) -> Result<Message, String> {
    let log = msg_request::Log {
        group: group.group.clone(),
        since: seq.saturating_sub(1),
        ..Default::default()
    };
    let answer = ask_as_human_for(&group.daemon, Asked::Log(log))
        .map_err(|refusal| refusal.detail().to_string())?;
    let entry = entries(answer)
        .find(|entry| entry.seq == seq)
        .ok_or_else(|| format!("the group's log holds no message #{seq}"))?;
    let at_ms = entry.at_ms;
    match entry.what {
        Some(What::Message(message)) => {
            Ok(Message::new(seq, message.author, message.to, message.urgent, at_ms, &message.body))
        }
        _ => Err(format!("entry #{seq} in the group's log is not a message")),
    }
}

fn entries(answer: muster_daemon_proto::MsgAnswer) -> impl Iterator<Item = msg_answer::Entry> {
    let groups = match answer.answer {
        Some(msg_answer::Answer::Entries(entries)) => entries.groups,
        _ => Vec::new(),
    };
    groups.into_iter().flat_map(|group| group.entries)
}

/// Replies to the message the banner for `group` shows, as the human, to its author.
pub(crate) fn reply(daemon: &str, group: &str, seq: u64, body: &str) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let to = poison::lock(&SESSION, "session").banners.reply(&key, seq, body);
    let to = match to {
        Ok(to) => to,
        Err(why) => return Response::failure(why),
    };
    announce();
    let sent = post_as_human(&key, &to, body, false, seq);
    log::info(
        "banner.replied",
        fields! {
            "daemon" => daemon,
            "group" => group,
            "seq" => seq,
            "outcome" => match &sent {
                Sent::Pending { .. } => "pending",
                Sent::Posted { .. } => "posted",
                Sent::Refused { .. } => "refused",
                Sent::Unknown { .. } => "unknown",
            },
        },
    );
    let response = match &sent {
        Sent::Posted { said, .. } => Response::noted(said.clone()),
        Sent::Refused { why, .. } => Response::failure(why.clone()),
        Sent::Unknown { why, .. } => Response::unanswered(why.clone()),
        Sent::Pending { .. } => Response::ok(),
    };
    poison::lock(&SESSION, "session").banners.replied(&key, seq, sent);
    announce();
    response
}

/// Posts `body` to `to` in `group` as the human, and says what came of it.
///
/// An unanswered post may have been kept, and a post carries no key the daemon could refuse a
/// second copy by, so it is never sent again. The group's log is asked once instead, for the
/// human's entry after `after` with these recipients and this body.
pub(crate) fn post_as_human(
    group: &GroupKey,
    to: &[String],
    body: &str,
    urgent: bool,
    after: u64,
) -> Sent {
    // Spelled with the rest defaulted, so a field the daemon's protocol gains does not break it.
    #[allow(clippy::needless_update, reason = "fields the daemon's protocol may add")]
    let post = msg_request::Post {
        group: Some(group.group.clone()),
        to: to.to_vec(),
        body: body.to_string(),
        urgent,
        ..Default::default()
    };
    match ask_as_human_for(&group.daemon, Asked::Post(post)) {
        Ok(answer) => match answer.answer {
            Some(msg_answer::Answer::Posted(posted)) => {
                let reached: Vec<Reached> = posted.reached.iter().map(reached).collect();
                Sent::Posted {
                    seq: posted.seq,
                    said: posting::said(&reached),
                    heard: posting::heard(&reached),
                }
            }
            // A daemon that keeps a post answers it as posted; one that says nothing more was
            // still done, and only whom it reached is unknown.
            _ => Sent::Posted {
                seq: 0,
                said: "kept; the daemon did not say whom it woke".to_string(),
                heard: true,
            },
        },
        Err(Refusal::Unanswered { detail, .. }) => match settled(group, to, body, after) {
            Some(seq) => Sent::Posted {
                seq,
                said: "kept, though the daemon never answered; whom it woke is not known"
                    .to_string(),
                heard: true,
            },
            None => Sent::Unknown {
                body: body.to_string(),
                why: format!(
                    "{detail}. It may have been posted; it was not sent again, since a second \
                     send could post it twice. Open the conversation to see whether it landed."
                ),
            },
        },
        Err(refusal) => Sent::Refused { body: body.to_string(), why: refusal.detail().to_string() },
    }
}

/// The number of the human's entry that shows an unanswered post was kept, read once from the
/// group's log.
fn settled(group: &GroupKey, to: &[String], body: &str, after: u64) -> Option<u64> {
    let log = msg_request::Log { group: group.group.clone(), since: after, ..Default::default() };
    let answer = ask_as_human_for(&group.daemon, Asked::Log(log)).ok()?;
    let messages: Vec<(u64, msg_answer::Message)> = entries(answer)
        .filter_map(|entry| match entry.what {
            Some(What::Message(message)) => Some((entry.seq, message)),
            _ => None,
        })
        .collect();
    posting::settles(
        messages.iter().map(|(seq, message)| Logged {
            seq: *seq,
            by_human: posting::is_human(&message.author),
            to: &message.to,
            body: &message.body,
        }),
        after,
        to,
        body,
    )
}

fn reached(reached: &msg_answer::Reached) -> Reached {
    use msg_answer::Reach as Wire;
    let reach = match reached.reach() {
        Wire::Woken => Reach::Woken,
        Wire::AlreadyWoken => Reach::AlreadyWoken,
        // A reach this build is too old to know is one it cannot claim anybody heard.
        Wire::Waiting | Wire::Unspecified => Reach::Waiting,
        Wire::Gone => Reach::Gone,
        Wire::Deferred => Reach::Deferred,
        Wire::NoAgent => Reach::NoAgent,
        Wire::NoDoorbell => Reach::NoDoorbell,
        Wire::Paused => Reach::Paused,
        Wire::Unreachable => Reach::Unreachable,
    };
    Reached { name: reached.name.clone(), reach }
}

/// Takes down the banner showing `seq`, without reading its group.
pub(crate) fn dismiss(daemon: &str, group: &str, seq: u64) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let dismissed = poison::lock(&SESSION, "session").banners.dismiss(&key, seq);
    match dismissed {
        Ok(()) => {
            announce();
            Response::ok()
        }
        Err(why) => Response::failure(why),
    }
}

/// Says whether the human is writing a reply in the banner showing `seq`.
pub(crate) fn hold(daemon: &str, group: &str, seq: u64, held: bool) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let changed = poison::lock(&SESSION, "session").banners.hold(&key, seq, held);
    match changed {
        Ok(changed) => {
            if changed {
                announce();
            }
            Response::ok()
        }
        Err(why) => Response::failure(why),
    }
}

/// Tells the shell the banners, if they changed since it was last told.
///
/// Settled and sent under one lock of its own, as the human's groups are: the handler's thread,
/// fetch threads and daemon threads all call this, and one that settled first and sent second
/// would leave the shell drawing an older stack.
pub(crate) fn announce() {
    static TELLING: Mutex<()> = Mutex::new(());
    let _telling = poison::lock(&TELLING, "banners");
    let (changed, raised) = {
        let mut session = poison::lock(&SESSION, "session");
        let changed = convert::banners(&session.banners.stack());
        if changed == session.banners_sent {
            return;
        }
        let raised: Vec<(String, String, u64)> = changed
            .shown
            .iter()
            .filter(|banner| {
                !session.banners_sent.shown.iter().any(|was| {
                    (&was.daemon_id, &was.group, was.seq)
                        == (&banner.daemon_id, &banner.group, banner.seq)
                })
            })
            .map(|banner| (banner.daemon_id.clone(), banner.group.clone(), banner.seq))
            .collect();
        session.banners_sent.clone_from(&changed);
        (changed, raised)
    };
    for (daemon, group, seq) in raised {
        log::info("banner.shown", fields! { "daemon" => daemon, "group" => group, "seq" => seq });
    }
    ffi::emit(&Event::new(event::Payload::BannersChanged(changed)));
}
