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
//!
//! An event a request caused carries the request's correlation id to the connection that sent
//! it, and to nobody else. The cause is passed in by whoever publishes, from what it holds under
//! the same lock as the event: a cause kept here would be read by a publisher under another
//! lock, at the same moment, and given to the wrong event.

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
    held: Option<Vec<Held>>,
}

/// An event held unnumbered, with the correlation id it carries to this subscription.
#[derive(Debug)]
struct Held {
    event: api::event::Event,
    correlation: String,
}

/// The request whose events are being published: on which connection, and its correlation id.
#[derive(Debug, Clone)]
pub(crate) struct Cause {
    pub(crate) connection: u64,
    pub(crate) correlation: String,
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
        for held in subscription.held.take().unwrap_or_default() {
            delivered = delivered
                && deliver(&open.outbox, subscription, &source, held.event, held.correlation);
        }
        open.ended |= !delivered;
    }

    /// Queues `answer` on `connection`, as of the last event its subscription has been sent.
    /// Called after the request's events were published, so they reach the client first.
    pub(crate) fn answer(&self, connection: u64, mut answer: api::Answer) {
        let mut connections = self.connections();
        let Some(open) = connections.find(connection) else { return };
        let at = open
            .subscription
            .as_ref()
            .map(|subscription| Position { epoch: subscription.epoch, number: subscription.last })
            .unwrap_or_default();
        answer.epoch = at.epoch;
        answer.through = at.number;
        if !open.outbox.push_at(frame(api::daemon_message::Message::Answer(answer)), at) {
            open.ended = true;
        }
    }

    /// Sends what `event` builds to every subscription that has joined `topic`, with `cause`'s
    /// correlation id to the connection that sent it. Called holding the lock `topic`'s events
    /// are published under; `event` is built only when somebody hears it.
    pub(crate) fn publish(
        &self,
        topic: api::Topic,
        cause: Option<&Cause>,
        event: impl FnOnce() -> api::event::Event,
    ) {
        self.send(Some(topic), cause, event);
    }

    /// Tells every subscription this daemon's state now. Called with the session locked.
    pub(crate) fn source_changed(&self, state: api::SourceState) {
        let status = api::SourceStatus { source: Some(self.source()), state: state.into() };
        self.send(None, None, || {
            api::event::Event::SourceChanged(api::SourceChanged { status: Some(status) })
        });
    }

    /// Delivers to subscriptions that joined `topic`, or to all of them when it is `None`.
    fn send(
        &self,
        topic: Option<api::Topic>,
        cause: Option<&Cause>,
        event: impl FnOnce() -> api::event::Event,
    ) {
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
            let correlation = cause
                .filter(|cause| cause.connection == open.outbox.id)
                .map(|cause| cause.correlation.clone())
                .unwrap_or_default();
            match subscription.held.as_mut() {
                Some(held) => held.push(Held { event, correlation }),
                None => {
                    open.ended |= !deliver(&open.outbox, subscription, &source, event, correlation);
                }
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
    correlation: String,
) -> bool {
    subscription.last += 1;
    let numbered = api::Event {
        epoch: subscription.epoch,
        number: subscription.last,
        source: Some(source.clone()),
        correlation,
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

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::*;

    fn hub() -> Arc<Api> {
        Api::new(api::Machine {
            source: Some(api::Source { daemon: "d1".to_string(), instance: 1 }),
            ..api::Machine::default()
        })
    }

    /// A connection on `hub`, and the client's end of it.
    fn connect(hub: &Api) -> (u64, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let outbox = Outbox::open(&ours).unwrap();
        let id = outbox.id;
        hub.open(outbox);
        theirs.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        (id, theirs)
    }

    fn subscribe(hub: &Api, connection: u64) -> Joining {
        let joining = hub.begin(connection).unwrap();
        hub.join(&joining, api::Topic::Panes);
        joining
    }

    fn next(client: &mut UnixStream) -> api::daemon_message::Message {
        let frame = muster_frame::read_frame_or_end(client, 1 << 24).unwrap().unwrap();
        api::DaemonMessage::decode(frame.as_slice()).unwrap().message.unwrap()
    }

    fn next_event(client: &mut UnixStream) -> api::Event {
        match next(client) {
            api::daemon_message::Message::Event(event) => event,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    fn changed() -> api::event::Event {
        api::event::Event::SourceChanged(api::SourceChanged::default())
    }

    #[test]
    fn an_event_carries_its_cause_to_the_connection_that_asked_and_to_nobody_else() {
        let hub = hub();
        let (asker, mut asking) = connect(&hub);
        let (other, mut watching) = connect(&hub);
        for connection in [asker, other] {
            let joining = subscribe(&hub, connection);
            hub.go_live(&joining, String::new(), api::Snapshot::default());
        }
        let _ = (next(&mut asking), next(&mut watching));

        let cause = Cause { connection: asker, correlation: "c1".to_string() };
        hub.publish(api::Topic::Panes, Some(&cause), changed);
        hub.publish(api::Topic::Panes, None, changed);

        assert_eq!(next_event(&mut asking).correlation, "c1");
        assert_eq!(next_event(&mut asking).correlation, "");
        assert_eq!(next_event(&mut watching).correlation, "");
        assert_eq!(next_event(&mut watching).correlation, "");
    }

    #[test]
    fn an_event_held_while_a_subscription_joins_keeps_its_cause() {
        let hub = hub();
        let (asker, mut asking) = connect(&hub);
        let joining = subscribe(&hub, asker);
        let cause = Cause { connection: asker, correlation: "c1".to_string() };
        hub.publish(api::Topic::Panes, Some(&cause), changed);
        hub.go_live(&joining, "s1".to_string(), api::Snapshot::default());

        assert!(matches!(next(&mut asking), api::daemon_message::Message::Answer(_)));
        let event = next_event(&mut asking);
        assert_eq!((event.number, event.correlation.as_str()), (1, "c1"));
    }
}
