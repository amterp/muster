//! `pane.send`, `pane.close`, `pane.compact` and `pane.restart`: acting on a pane through the
//! daemon's own handling of the request `muster pane` sends with no window open, so the API and
//! the CLI cannot do it differently.

use muster_api as api;
use muster_core::pane_text::{self, Unconfirmed};
use muster_daemon_proto as proto;
use proto::pane_request;

use super::{Caller, refused};
use crate::api::Cause;
use crate::session::{Handled, Reply, Shared};

pub(super) fn send(shared: &Shared, caller: &Caller, correlation: String, asked: &api::SendToPane) {
    let answer = sent(shared, correlation, asked);
    answer_after(shared, caller, answer);
}

pub(super) fn close(shared: &Shared, caller: &Caller, correlation: String, asked: &api::ClosePane) {
    let close = pane_request::Close { pane: asked.pane.clone() };
    let answer = match handled(shared, caller, &correlation, pane_request::Request::Close(close)) {
        Ok(handled) => answered(correlation, &replied(handled)),
        Err(unavailable) => *unavailable,
    };
    answer_after(shared, caller, answer);
}

pub(super) fn compact(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: &api::CompactAgent,
) {
    let said = |text: &str| (!text.is_empty()).then(|| text.to_string());
    let compact = pane_request::Compact {
        pane: asked.pane.clone(),
        focus: said(&asked.focus),
        then: said(&asked.then),
    };
    let answer = if let Err(refusal) = crate::session::compact_lines(&compact) {
        refused(correlation, api::Refusal::Invalid, refusal.reason)
    } else {
        match handled(shared, caller, &correlation, pane_request::Request::Compact(compact)) {
            Ok(handled) => answered(correlation, &replied(handled)),
            Err(unavailable) => *unavailable,
        }
    };
    answer_after(shared, caller, answer);
}

pub(super) fn restart(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: &api::RestartAgent,
) {
    let fault =
        asked.env.iter().find_map(|(name, value)| crate::restart::variable_fault(name, value));
    if let Some(fault) = fault {
        let answer =
            refused(correlation, api::Refusal::Invalid, format!("{fault}; nothing was ended"));
        return answer_after(shared, caller, answer);
    }
    let restart = pane_request::Restart {
        pane: asked.pane.clone(),
        env: asked.env.clone().into_iter().collect(),
    };
    let answer =
        match handled(shared, caller, &correlation, pane_request::Request::Restart(restart)) {
            Err(unavailable) => *unavailable,
            Ok(Handled::Reply(reply)) => answered(correlation, &reply),
            // On this connection's thread, with the session let go: a restart waits seconds on its
            // agent to end, and the connection carries out its requests one at a time.
            Ok(Handled::Restart(restarting)) => {
                let (reply, changed) = restarting.carried_out();
                restarted(correlation, reply, changed)
            }
            Ok(other) => unreachable!("a restart is carried out or refused, never {other:?}"),
        };
    answer_after(shared, caller, answer);
}

/// Types the send, confirming it when asked, by the sequence `muster pane send` follows with no
/// window: the reads and the typing are this daemon's own.
fn sent(shared: &Shared, correlation: String, asked: &api::SendToPane) -> api::Answer {
    let pane = asked.pane.as_str();
    if asked.text.is_empty() && asked.keys.is_empty() && !asked.enter {
        let reason = "a send needs text, keys or Return; nothing was sent";
        return refused(correlation, api::Refusal::Invalid, reason);
    }
    let send = proto::input_event::Send {
        text: asked.text.clone(),
        keys: asked.keys.clone(),
        enter: asked.enter,
    };
    let input = match crate::input::sent_input(&send) {
        Ok(input) => input,
        Err(refusal) => {
            let reason = format!("{refusal}; nothing was sent");
            return refused(correlation, api::Refusal::Invalid, reason);
        }
    };
    if shared.lock().pane_io(pane).is_none() {
        let reason = format!("no pane {pane} on this daemon; nothing was sent");
        return refused(correlation, api::Refusal::NotFound, reason);
    }
    let read_page = |read: pane_request::Read| {
        let reading = shared.lock().reading(&read)?;
        reading.page()
    };
    let prompt = || {
        let read = pane_request::Read {
            pane: pane.to_string(),
            last: 1,
            prompt: true,
            ..Default::default()
        };
        read_page(read)?.prompt
    };
    // The pane's last `rows` rows, or as many as a page holds for zero, as the CLI reads them.
    let read = |rows: u32| {
        let last = if rows == 0 { u32::MAX } else { rows };
        let read = pane_request::Read { pane: pane.to_string(), last, ..Default::default() };
        let text = read_page(read).ok_or_else(|| format!("pane {pane} closed"))?.text;
        Ok(pane_text::PaneText { text, ..Default::default() }.tail(rows).text)
    };
    let typed = || {
        let io = shared
            .lock()
            .pane_io(pane)
            .ok_or_else(|| format!("pane {pane} closed; nothing was sent"))?;
        if io.queue(input) {
            return Ok(());
        }
        Err(if io.discards().is_restarting() {
            format!(
                "pane {pane}'s agent is restarting, so nothing was typed; send it again once it \
                 is back"
            )
        } else {
            format!(
                "pane {pane}'s program has stopped reading its input and more waits for it than \
                 the daemon holds, so nothing was typed"
            )
        })
    };
    let outcome =
        pane_text::send(pane, &asked.text, asked.append, asked.confirm, prompt, read, typed);
    sent_answer(correlation, outcome)
}

fn sent_answer(correlation: String, outcome: Result<(), Unconfirmed>) -> api::Answer {
    match outcome {
        Ok(()) => done(correlation),
        Err(Unconfirmed::Refused(reason)) => {
            refused(correlation, api::Refusal::Unspecified, reason)
        }
        Err(Unconfirmed::Unknown(reason)) => unknown(correlation, reason),
    }
}

/// Handles `request` as a control connection's would be, under the session's lock, with the
/// events it causes carrying `correlation` to the caller's subscription; or refuses it as
/// UNAVAILABLE while the daemon stops or hands over, as `pane.new` is, so the client asks again
/// shortly rather than giving up.
fn handled(
    shared: &Shared,
    caller: &Caller,
    correlation: &str,
    request: pane_request::Request,
) -> Result<Handled, Box<api::Answer>> {
    let service = proto::request::Service::Pane(proto::PaneRequest { request: Some(request) });
    let mut session = shared.lock();
    if let Some(why) = session.unavailable() {
        let reason = format!("{why}; nothing was done, and asking again shortly is safe");
        return Err(Box::new(refused(correlation.to_string(), api::Refusal::Unavailable, reason)));
    }
    session.cause =
        Some(Cause { connection: caller.connection, correlation: correlation.to_string() });
    let handled = crate::control::handle(&mut session, service, None, &caller.outbox, shared);
    session.cause = None;
    Ok(handled)
}

fn replied(handled: Handled) -> Reply {
    match handled {
        Handled::Reply(reply) => reply,
        other => unreachable!("a close or compaction is answered at once, never {other:?}"),
    }
}

/// The daemon's reply as the API says it.
fn answered(correlation: String, reply: &Reply) -> api::Answer {
    match reply.outcome {
        proto::Outcome::Done => done(correlation),
        proto::Outcome::AlreadySo => api::Answer {
            correlation,
            outcome: api::Outcome::AlreadySo.into(),
            ..api::Answer::default()
        },
        proto::Outcome::NotThere => {
            refused(correlation, api::Refusal::NotFound, reply.reason.clone())
        }
        proto::Outcome::Refused | proto::Outcome::Unspecified => {
            refused(correlation, api::Refusal::Unspecified, reply.reason.clone())
        }
    }
}

/// What a restart carried out came to. One that stopped short after it signalled or ended the
/// agent changed the pane without doing what was asked, which is neither done nor refused.
fn restarted(correlation: String, reply: Reply, changed: bool) -> api::Answer {
    match (reply.outcome, reply.detail.map(|detail| *detail)) {
        (proto::Outcome::Done, Some(proto::answer::Detail::Restarted(restarted))) => api::Answer {
            correlation,
            outcome: api::Outcome::Done.into(),
            detail: Some(api::answer::Detail::Restarted(api::RestartedAgent {
                program: restarted.program,
                killed: restarted.killed,
                typed: restarted.typed,
            })),
            ..api::Answer::default()
        },
        _ if changed => unknown(correlation, reply.reason),
        _ => refused(correlation, api::Refusal::Unspecified, reply.reason),
    }
}

fn done(correlation: String) -> api::Answer {
    api::Answer { correlation, outcome: api::Outcome::Done.into(), ..api::Answer::default() }
}

fn unknown(correlation: String, reason: String) -> api::Answer {
    api::Answer {
        correlation,
        outcome: api::Outcome::Unknown.into(),
        reason,
        ..api::Answer::default()
    }
}

/// Answers under a hold of the session's lock after the one the request was carried out under,
/// as a control connection answers: letting go of that one finishes the work it left on panes'
/// terminals, so the answer means the request has taken effect.
fn answer_after(shared: &Shared, caller: &Caller, answer: api::Answer) {
    let _session = shared.lock();
    shared.api.answer(caller.connection, answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(outcome: proto::Outcome) -> Reply {
        Reply { outcome, reason: "why".to_string(), detail: None }
    }

    fn said(answer: &api::Answer) -> (api::Outcome, api::Refusal) {
        (answer.outcome(), answer.refusal())
    }

    #[test]
    fn a_reply_reads_as_the_outcome_a_client_acts_on() {
        use api::{Outcome, Refusal};
        let read = |outcome| said(&answered("c".to_string(), &reply(outcome)));
        assert_eq!(read(proto::Outcome::Done), (Outcome::Done, Refusal::Unspecified));
        assert_eq!(read(proto::Outcome::AlreadySo), (Outcome::AlreadySo, Refusal::Unspecified));
        assert_eq!(read(proto::Outcome::NotThere), (Outcome::Refused, Refusal::NotFound));
        assert_eq!(read(proto::Outcome::Refused), (Outcome::Refused, Refusal::Unspecified));
    }

    /// A restart refused before it ended anything is refused; one that ended the agent and could
    /// not bring it back is unknown, since refused would say nothing happened.
    #[test]
    fn a_restart_that_stopped_short_is_refused_only_when_it_changed_nothing() {
        let refused = || reply(proto::Outcome::Refused);
        let untouched = restarted("c".to_string(), refused(), false);
        assert_eq!(untouched.outcome(), api::Outcome::Refused);
        let ended = restarted("c".to_string(), refused(), true);
        assert_eq!((ended.outcome(), ended.reason.as_str()), (api::Outcome::Unknown, "why"));
    }

    #[test]
    fn a_send_reads_as_what_its_confirmation_saw() {
        let read = |outcome| sent_answer("c".to_string(), outcome).outcome();
        assert_eq!(read(Ok(())), api::Outcome::Done);
        assert_eq!(read(Err(Unconfirmed::Refused("no".to_string()))), api::Outcome::Refused);
        assert_eq!(read(Err(Unconfirmed::Unknown("maybe".to_string()))), api::Outcome::Unknown);
    }
}
