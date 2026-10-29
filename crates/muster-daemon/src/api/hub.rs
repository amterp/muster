//! Every API connection and its subscription, and the numbering of what each is sent.
//!
//! One lock over all of them, always taken last: code holding the session's lock or the messages
//! lock publishes here, and nothing here takes either. An event is numbered per subscription
//! under this lock, so two sources publishing at once are numbered in the order they got here.
//!
//! A subscription joins its sources one at a time without a gap. The connection begins it under
//! the session's lock, which source states change under; then, for each topic, it takes the lock
//! that topic's events are published under, fills that topic's part of the snapshot, and joins
//! the topic before letting go. An event published for a topic after it joined is newer than
//! that topic's part of the snapshot, and is held unnumbered until the snapshot's answer is
//! queued; [`Api::go_live`] then numbers what was held from 1, after the answer.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use flexid::{OsRandom, RandomSource};
use muster_api as api;
use muster_core::diagnostics::poison;
use prost::Message;

use crate::control::{Outbox, Position};

#[derive(Debug)]
pub(crate) struct Api {
    machine: api::Machine,
    connections: Mutex<Connections>,
}

#[derive(Debug, Default)]
struct Connections {
    open: Vec<Connection>,
    /// Why the daemon has ended every connection, once it has: one opened after is ended at once.
    ended: Option<api::ended::Reason>,
}

#[derive(Debug)]
struct Connection {
    outbox: Outbox,
    subscription: Option<Subscription>,
    /// Told it is over, or fallen behind: nothing more is queued for it.
    ended: bool,
}

#[derive(Debug)]
struct Subscription {
    epoch: u64,
    joined: BTreeSet<api::Topic>,
    /// The number of the last event queued.
    last: u64,
    /// Events newer than the snapshot being gathered, until its answer is queued.
    held: Option<Vec<api::event::Event>>,
}

/// A subscription being gathered on a connection, from [`Api::begin`] to [`Api::go_live`].
#[derive(Debug)]
pub(crate) struct Joining {
    connection: u64,
    epoch: u64,
}

impl Api {
    pub(crate) fn new(machine: api::Machine) -> Arc<Api> {
        Arc::new(Api { machine, connections: Mutex::default() })
    }

    /// This daemon.
    pub(crate) fn machine(&self) -> api::Machine {
        self.machine.clone()
    }

    pub(crate) fn source(&self) -> api::Source {
        self.machine.source.clone().unwrap_or_default()
    }

    fn connections(&self) -> MutexGuard<'_, Connections> {
        poison::lock(&self.connections, "daemon.api")
    }

    pub(crate) fn open(&self, outbox: Outbox) {
        let mut connections = self.connections();
        let mut connection = Connection { outbox, subscription: None, ended: false };
        if let Some(reason) = connections.ended {
            end(&mut connection, reason);
        }
        connections.open.push(connection);
    }

    pub(crate) fn close(&self, connection: u64) {
        self.connections().open.retain(|open| open.outbox.id != connection);
    }

    /// Starts a subscription on `connection` in an epoch of its own, replacing any it had. Called
    /// with the session locked, so no source's change falls between the snapshot's sources and
    /// the subscription. Nothing when the connection has been ended.
    pub(crate) fn begin(&self, connection: u64) -> Option<Joining> {
        let mut connections = self.connections();
        let open = connections.find(connection)?;
        let epoch = draw_epoch();
        open.subscription =
            Some(Subscription { epoch, joined: BTreeSet::new(), last: 0, held: Some(Vec::new()) });
        Some(Joining { connection, epoch })
    }

    /// From now on `topic`'s events reach the subscription. Called holding the lock they are
    /// published under, once the topic's part of the snapshot is filled.
    pub(crate) fn join(&self, joining: &Joining, topic: api::Topic) {
        if let Some(subscription) = self.connections().subscription(joining) {
            subscription.joined.insert(topic);
        }
    }

    /// Queues the subscription's answer with `snapshot`, then every event held while it was
    /// gathered.
    pub(crate) fn go_live(
        &self,
        joining: &Joining,
        correlation: String,
        mut snapshot: api::Snapshot,
    ) {
        let source = self.source();
        let mut connections = self.connections();
        let Some(open) = connections.find(joining.connection) else { return };
        let Some(subscription) =
            open.subscription.as_mut().filter(|subscription| subscription.epoch == joining.epoch)
        else {
            return;
        };
        snapshot.epoch = subscription.epoch;
        snapshot.through = subscription.last;
        let answer = api::Answer {
            correlation,
            outcome: api::Outcome::Done.into(),
            epoch: subscription.epoch,
            through: subscription.last,
            detail: Some(api::answer::Detail::Snapshot(snapshot)),
            ..api::Answer::default()
        };
        let at = Position { epoch: subscription.epoch, number: subscription.last };
        let mut delivered =
            open.outbox.push_at(frame(api::daemon_message::Message::Answer(answer)), at);
        for event in subscription.held.take().unwrap_or_default() {
            delivered = delivered && deliver(&open.outbox, subscription, &source, event);
        }
        open.ended |= !delivered;
    }

    /// Answers a request on `connection` that carries no snapshot.
    pub(crate) fn answer(
        &self,
        connection: u64,
        correlation: String,
        outcome: api::Outcome,
        reason: String,
    ) {
        let mut connections = self.connections();
        let Some(open) = connections.find(connection) else { return };
        let at = open
            .subscription
            .as_ref()
            .map(|subscription| Position { epoch: subscription.epoch, number: subscription.last })
            .unwrap_or_default();
        let answer = api::Answer {
            correlation,
            outcome: outcome.into(),
            reason,
            epoch: at.epoch,
            through: at.number,
            detail: None,
        };
        if !open.outbox.push_at(frame(api::daemon_message::Message::Answer(answer)), at) {
            open.ended = true;
        }
    }

    /// Sends what `event` builds to every subscription that has joined `topic`. Called holding
    /// the lock `topic`'s events are published under; `event` is built only when somebody hears
    /// it.
    pub(crate) fn publish(&self, topic: api::Topic, event: impl FnOnce() -> api::event::Event) {
        self.send(Some(topic), event);
    }

    /// Tells every subscription this daemon's state now. Called with the session locked.
    pub(crate) fn source_changed(&self, state: api::SourceState) {
        let status = api::SourceStatus { source: Some(self.source()), state: state.into() };
        self.send(None, || {
            api::event::Event::SourceChanged(api::SourceChanged { status: Some(status) })
        });
    }

    /// Delivers to subscriptions that joined `topic`, or to all of them when it is `None`.
    fn send(&self, topic: Option<api::Topic>, event: impl FnOnce() -> api::event::Event) {
        let mut connections = self.connections();
        let source = self.source();
        let mut make = Some(event);
        let mut built = None;
        for open in connections.open.iter_mut().filter(|open| !open.ended) {
            let Some(subscription) = open.subscription.as_mut() else { continue };
            if topic.is_some_and(|topic| !subscription.joined.contains(&topic)) {
                continue;
            }
            let event = built
                .get_or_insert_with(|| make.take().expect("an event is built at most once")())
                .clone();
            match subscription.held.as_mut() {
                Some(held) => held.push(event),
                None => open.ended |= !deliver(&open.outbox, subscription, &source, event),
            }
        }
    }

    /// Ends every connection, saying why, and every one opened from now.
    pub(crate) fn end_all(&self, reason: api::ended::Reason) {
        let mut connections = self.connections();
        connections.ended = Some(reason);
        for open in &mut connections.open {
            end(open, reason);
        }
    }

    /// Waits until what every connection was sent has been written, or `within` has passed.
    pub(crate) fn flush(&self, within: Duration) {
        let outboxes: Vec<Outbox> =
            self.connections().open.iter().map(|open| open.outbox.clone()).collect();
        let deadline = Instant::now() + within;
        for outbox in outboxes {
            outbox.flush(deadline.saturating_duration_since(Instant::now()));
        }
    }
}

impl Connections {
    fn find(&mut self, connection: u64) -> Option<&mut Connection> {
        self.open.iter_mut().find(|open| open.outbox.id == connection && !open.ended)
    }

    fn subscription(&mut self, joining: &Joining) -> Option<&mut Subscription> {
        self.find(joining.connection)?
            .subscription
            .as_mut()
            .filter(|subscription| subscription.epoch == joining.epoch)
    }
}

/// Numbers `event` and queues it. False when the connection fell behind.
fn deliver(
    outbox: &Outbox,
    subscription: &mut Subscription,
    source: &api::Source,
    event: api::event::Event,
) -> bool {
    subscription.last += 1;
    let numbered = api::Event {
        epoch: subscription.epoch,
        number: subscription.last,
        source: Some(source.clone()),
        event: Some(event),
    };
    let at = Position { epoch: subscription.epoch, number: subscription.last };
    outbox.push_at(frame(api::daemon_message::Message::Event(numbered)), at)
}

fn end(open: &mut Connection, reason: api::ended::Reason) {
    if open.ended {
        return;
    }
    open.ended = true;
    let at = open
        .subscription
        .take()
        .map(|subscription| Position { epoch: subscription.epoch, number: subscription.last })
        .unwrap_or_default();
    let detail = match reason {
        api::ended::Reason::Replaced => {
            "a newer daemon took over this one's panes and serves the same socket; connect to it \
             and subscribe again"
        }
        api::ended::Reason::Stopping => "the daemon is exiting",
        api::ended::Reason::FellBehind | api::ended::Reason::Unspecified => "",
    };
    open.outbox.push_at(frame(ended(reason, at, detail.to_string())), at);
}

pub(super) fn ended(
    reason: api::ended::Reason,
    at: Position,
    detail: String,
) -> api::daemon_message::Message {
    api::daemon_message::Message::Ended(api::Ended {
        reason: reason.into(),
        epoch: at.epoch,
        reached: at.number,
        detail,
    })
}

pub(super) fn frame(message: api::daemon_message::Message) -> Arc<[u8]> {
    api::DaemonMessage { message: Some(message) }.encode_to_vec().into()
}

/// An epoch no other subscription on any daemon has had: drawn, and never 0, which an answer
/// on a connection that has not subscribed carries.
fn draw_epoch() -> u64 {
    let mut bytes = [0u8; 8];
    loop {
        if OsRandom.fill_bytes(&mut bytes).is_err() {
            // The clock is what is left; two subscriptions in one nanosecond are not a case.
            return crate::instance().max(1);
        }
        let epoch = u64::from_le_bytes(bytes);
        if epoch != 0 {
            return epoch;
        }
    }
}
