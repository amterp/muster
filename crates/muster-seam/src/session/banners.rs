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
use muster_core::conversation::{Answering, What};
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_client::convert::messaging;
use muster_daemon_proto::msg_request::{self, Request as Asked};

use super::{SESSION, ask_as_human_for, posting};
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
    // Bounded above too, so the answer is the one entry rather than everything after it. A
    // daemon before 1.8 drops `before` and answers with the rest of the log, which `find` below
    // still reads the entry out of.
    let log = msg_request::Log {
        group: group.group.clone(),
        since: seq.saturating_sub(1),
        before: Some(seq + 1),
        ..Default::default()
    };
    let answer = ask_as_human_for(&group.daemon, Asked::Log(log))
        .map_err(|refusal| refusal.detail().to_string())?;
    let entry = messaging::log_page(&answer, &group.group)
        .entries
        .into_iter()
        .find(|entry| entry.seq == seq)
        .ok_or_else(|| format!("the group's log holds no message #{seq}"))?;
    match &entry.what {
        What::Message(message) => Ok(Message::of(seq, entry.at_ms, message)),
        _ => Err(format!("entry #{seq} in the group's log is not a message")),
    }
}

/// Replies to the message the banner for `group` shows, as the human, to its author: `note`, and
/// the answer picked in the banner, if any.
pub(crate) fn reply(daemon: &str, group: &str, seq: u64, note: &str) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let reply = poison::lock(&SESSION, "session").banners.reply(&key, seq, note);
    let reply = match reply {
        Ok(reply) => reply,
        Err(why) => return Response::failure(why),
    };
    announce();
    let answering = reply.answering.map(|(question, answer)| Answering { question, answer });
    let sent = posting::post_as_human(&key, &reply.to, &reply.body, answering.as_ref(), false, seq);
    log::info(
        "banner.replied",
        fields! {
            "daemon" => daemon,
            "group" => group,
            "seq" => seq,
            "outcome" => posting::outcome(&sent),
        },
    );
    let response = posting::response(&sent);
    poison::lock(&SESSION, "session").banners.replied(&key, seq, sent);
    announce();
    response
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

/// Picks the answer numbered `answer` (from 0) of the question the banner showing `seq` asks, or
/// with none takes the pick back. Sends nothing: the pick goes with the reply.
pub(crate) fn pick(daemon: &str, group: &str, seq: u64, answer: Option<u32>) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let answer = answer.map(|answer| answer as usize);
    let changed = poison::lock(&SESSION, "session").banners.pick(&key, seq, answer);
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
