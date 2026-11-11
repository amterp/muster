//! Muster's notices (`@muster`) that go in a group kept on another machine: a stall seen here of a
//! member in a group kept there, or a helper here telling its maker in the group the two share
//! there. Only a group's home appends to it, so the home is asked to post the notice
//! ([`muster_msg::Call::Muster`]), and this machine's members are reached from the answer.
//!
//! Also the notices for the person about an agent, which go in their group with it wherever that
//! is kept.

use std::sync::Arc;

use muster_msg::{Away, Settled};

use super::presence::Panes;
use super::{Delivered, activities_of, delivering, now_ms, peer, ring, told, words};
use crate::session::Shared;

/// The protocol minor from which a group's home posts Muster's notice for a member on another
/// machine.
const MUSTER_SINCE: u32 = 8;

/// Has `away`'s machine post the notice it carries, on a thread of its own: the call may wait
/// seconds on the link, and the doorbell's thread rings everybody else's panes meanwhile. Logs
/// `told` with the notice's `bytes` once it is posted, and calls `untold` with why when it is not.
pub(super) fn post_at_home(
    shared: &Arc<Shared>,
    away: Away,
    told_event: &'static str,
    bytes: usize,
    untold: impl Fn(&str) + Send + Sync + 'static,
) {
    let machine = away.machine.clone();
    let Some(link) = shared.peers.to(&machine) else {
        untold(&format!("no link to {machine}, where the group is kept, is up"));
        return;
    };
    if link.speaks.minor < MUSTER_SINCE {
        let speaks = format!("{}.{}", link.speaks.major, link.speaks.minor);
        untold(&format!(
            "{machine}'s muster-daemon speaks protocol {speaks}, before a group's home posted \
             Muster's notice for a member elsewhere (1.{MUSTER_SINCE})"
        ));
        return;
    }
    let shared = Arc::clone(shared);
    let untold = Arc::new(untold);
    let untold_here = Arc::clone(&untold);
    let started = std::thread::Builder::new().name("muster-notice".to_string()).spawn(move || {
        let (settle, holding) = peer::call_away(&shared, &away);
        let rang = ring(&shared, holding);
        match settle.result {
            Ok(Settled::Posted(posted)) => {
                let activities = activities_of(&shared, &posted, &Panes::of(&shared));
                told(told_event, Some(bytes), &Delivered { posted, rang, activities });
            }
            Ok(other) => untold(&format!("{machine} answered {other:?}")),
            Err(refusal) => untold(&words(&refusal)),
        }
    });
    if let Err(error) = started {
        untold_here(&format!("no thread could be started to ask the group's home: {error}"));
    }
}

/// Tells the human Muster's notice about the agent in `pane`, which `called` is what it is called,
/// in the group the two share or one made for them: posted where that group is kept, here or on
/// another machine. Logs `told_event` once it is posted, and calls `untold` with why when it is not.
pub(super) fn tell_the_person(
    shared: &Arc<Shared>,
    panes: &Panes,
    pane: &str,
    called: Option<&str>,
    body: &str,
    told_event: &'static str,
    untold: impl Fn(&str) + Send + Sync + 'static,
) {
    let human = muster_msg::HUMAN;
    let routed = shared.messages().service.route_tell_as_muster(human, pane, called, body, panes);
    match routed {
        Ok(Some(away)) => return post_at_home(shared, away, told_event, body.len(), untold),
        Ok(None) => {}
        Err(refusal) => return untold(&words(&refusal)),
    }
    let delivered = delivering(shared, panes, |service| {
        service.tell_as_muster(human, pane, called, body, panes, now_ms())
    });
    match delivered {
        Ok(delivered) => {
            told(told_event, Some(body.len()), &delivered);
        }
        Err(reply) => untold(&reply.reason),
    }
}
