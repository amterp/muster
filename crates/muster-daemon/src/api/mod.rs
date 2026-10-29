//! The API for other programs (MIP-10): a connection's handshake, and the requests it serves.
//! `proto/muster_api.proto`'s header is the API's documentation; `hub.rs` keeps every
//! connection's subscription, and `topics.rs` what each topic holds.

mod acting;
mod hub;
pub(crate) mod identity;
mod messages;
mod pane;
pub(crate) mod panes;
mod posting;
mod reading;
mod topics;

pub(crate) use hub::{Api, Cause};
pub(crate) use messages::Published;
pub(crate) use topics::attention_event;

use std::collections::BTreeSet;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use muster_api as api;
use muster_api::connection;
use muster_api::version::{API, serves};
use muster_core::diagnostics::log;
use muster_core::fields;
use prost::Message;

use crate::control::{Budget, Farewell, Outbox};
use crate::session::Shared;

/// Every request this daemon serves, by the name a client tests for in Welcome's features.
const REQUESTS: [&str; 10] = [
    "api.read_pane",
    "api.subscribe",
    "msg.ack",
    "msg.history",
    "msg.post",
    "pane.close",
    "pane.compact",
    "pane.new",
    "pane.restart",
    "pane.send",
];

/// Every topic a subscription may name.
const TOPICS: [api::Topic; 4] =
    [api::Topic::Panes, api::Topic::Machines, api::Topic::Messages, api::Topic::Attention];

/// What a client may have waiting before it is dropped. Twice the largest message, so that one
/// snapshot alone never trips it; the schema's header promises these numbers.
const BUDGET: Budget = Budget { messages: 4096, bytes: 2 * connection::LARGEST_MESSAGE as usize };

/// How long a client that fell behind has to read again before it is hung up on without being
/// told why. The schema's header promises it.
const FAREWELL_PATIENCE: Duration = Duration::from_secs(5);

/// The byte budget a debug build uses instead of [`BUDGET`]'s, when set: the one way a test
/// gets a client past its budget without queueing 32 MiB. A shipped daemon never reads it.
pub(crate) const BUDGET_SAID: &str = "MUSTER_DAEMON_API_BUDGET";

/// What this daemon serves, as Welcome and each machine in a snapshot say it.
pub(crate) fn features() -> api::Features {
    api::Features {
        requests: REQUESTS.iter().map(ToString::to_string).collect(),
        topics: TOPICS.iter().map(|topic| (*topic).into()).collect(),
        options: Vec::new(),
    }
}

/// Serves an API connection whose Hello, `hello`, has been read and not yet answered.
pub(crate) fn serve(mut stream: UnixStream, hello: &[u8], shared: &Arc<Shared>) {
    let hello = match api::Hello::decode(hello) {
        Ok(hello) => hello,
        Err(error) => {
            log::warn(
                "daemon.api.no_hello",
                fields! {
                    "error" => error,
                    "impact" => "the connection was closed; whatever dialed was not answered",
                    "check" => "whether the client builds its Hello from muster_api.proto",
                },
            );
            return;
        }
    };
    let client = hello.client.unwrap_or_default();
    let name = client.name.clone();
    let client = format!("{} {}", client.name, client.version).trim().to_string();
    let judged = hello
        .api
        .ok_or_else(|| "the hello names no API version".to_string())
        .and_then(|theirs| serves(&theirs));
    let answer = match judged {
        Ok(()) => api::hello_answer::Answer::Welcome(api::Welcome {
            api: Some(API),
            machine: Some(shared.api.machine()),
        }),
        Err(reason) => {
            log::warn(
                "daemon.api.refused",
                fields! {
                    "client" => client,
                    "reason" => reason,
                    "impact" => "that program cannot use this daemon's API",
                    "check" => "which API version the program was built against",
                },
            );
            api::hello_answer::Answer::Refused(api::Refused { reason, api: Some(API) })
        }
    };
    let welcomed = matches!(answer, api::hello_answer::Answer::Welcome(_));
    let answer = api::HelloAnswer { answer: Some(answer) };
    if connection::send(&mut stream, &answer).is_err() || !welcomed {
        return;
    }
    let _ = stream.set_read_timeout(None);
    let outbox = match Outbox::open_with(&stream, budget(), Some(farewell())) {
        Ok(outbox) => outbox,
        Err(error) => {
            log::error(
                "daemon.api.not_served",
                fields! {
                    "client" => client,
                    "error" => error,
                    "impact" => "this program's connection is closed unanswered",
                    "check" => "whether the daemon is out of threads or descriptors",
                },
            );
            return;
        }
    };
    let id = outbox.id;
    shared.api.open(outbox.clone());
    log::info("daemon.api.opened", fields! { "connection" => id, "client" => client });
    let caller = Caller { connection: id, client: name, outbox: outbox.clone() };
    loop {
        match connection::receive::<api::Request>(&mut stream) {
            Ok(Some(request)) => handle(shared, &caller, request),
            Ok(None) => break,
            Err(error) => {
                log::warn(
                    "daemon.api.unreadable",
                    fields! {
                        "connection" => id,
                        "error" => error,
                        "impact" => "the connection is closed; its program has to reconnect",
                        "check" => "whether the program frames its requests as muster_api.proto \
                                    says",
                    },
                );
                break;
            }
        }
    }
    shared.api.close(id);
    // A program that sent its requests and closed its end still reads their answers.
    outbox.flush(FAREWELL_PATIENCE);
    let _ = stream.shutdown(Shutdown::Both);
    log::info("daemon.api.closed", fields! { "connection" => id });
}

/// The connection a request came in on.
struct Caller {
    connection: u64,
    /// The client's name from its Hello, which scopes its operation ids.
    client: String,
    outbox: Outbox,
}

fn handle(shared: &Arc<Shared>, caller: &Caller, request: api::Request) {
    use api::request::Request as Namespace;
    let correlation = request.correlation;
    match request.request {
        Some(Namespace::Api(api::ApiRequest {
            request: Some(api::api_request::Request::Subscribe(asked)),
        })) => {
            subscribe(shared, caller.connection, correlation, &asked);
        }
        Some(Namespace::Pane(api::PaneRequest { request: Some(asked) })) => {
            use api::pane_request::Request as Pane;
            match asked {
                Pane::New(asked) => pane::new(shared, caller, correlation, &asked),
                Pane::Send(asked) => acting::send(shared, caller, correlation, &asked),
                Pane::Close(asked) => acting::close(shared, caller, correlation, &asked),
                Pane::Compact(asked) => acting::compact(shared, caller, correlation, &asked),
                Pane::Restart(asked) => acting::restart(shared, caller, correlation, &asked),
            }
        }
        Some(Namespace::Msg(api::MsgRequest {
            request: Some(api::msg_request::Request::History(asked)),
        })) => {
            let answer = match shared.messages().history(&asked) {
                Ok(page) => api::Answer {
                    correlation,
                    outcome: api::Outcome::Done.into(),
                    detail: Some(api::answer::Detail::Page(page)),
                    ..api::Answer::default()
                },
                Err(reason) => refused(correlation, api::Refusal::NotFound, reason),
            };
            shared.api.answer(caller.connection, answer);
        }
        Some(Namespace::Msg(api::MsgRequest {
            request: Some(api::msg_request::Request::Post(asked)),
        })) => posting::post(shared, caller, correlation, &asked),
        Some(Namespace::Msg(api::MsgRequest {
            request: Some(api::msg_request::Request::Ack(asked)),
        })) => posting::ack(shared, caller, correlation, &asked),
        Some(Namespace::Api(api::ApiRequest {
            request: Some(api::api_request::Request::ReadPane(asked)),
        })) => read_pane(shared, caller.connection, correlation, &asked),
        _ => shared.api.answer(
            caller.connection,
            refused(
                correlation,
                api::Refusal::Unsupported,
                "this daemon serves no such request; its Welcome lists the ones it does",
            ),
        ),
    }
}

/// An answer saying a request was refused, for `reason`.
fn refused(correlation: String, refusal: api::Refusal, reason: impl Into<String>) -> api::Answer {
    api::Answer {
        correlation,
        outcome: api::Outcome::Refused.into(),
        refusal: refusal.into(),
        reason: reason.into(),
        ..api::Answer::default()
    }
}

fn subscribe(shared: &Shared, connection: u64, correlation: String, asked: &api::Subscribe) {
    let topics = match topics_of(asked) {
        Ok(topics) => topics,
        Err(reason) => {
            let answer = refused(correlation, api::Refusal::Unsupported, reason);
            return shared.api.answer(connection, answer);
        }
    };
    let mut snapshot = api::Snapshot {
        topics: topics.iter().map(|topic| (*topic).into()).collect(),
        ..api::Snapshot::default()
    };
    let joining = {
        let session = shared.lock();
        let state = if session.restoring() {
            api::SourceState::Restoring
        } else {
            api::SourceState::Current
        };
        snapshot.sources =
            vec![api::SourceStatus { source: Some(shared.api.source()), state: state.into() }];
        let joining = shared.api.begin(connection);
        if let Some(joining) = &joining {
            topics::join_under_session(&session, shared, joining, &topics, &mut snapshot);
        }
        joining
    };
    let Some(joining) = joining else { return };
    topics::join_under_messages(shared, &joining, &topics, &mut snapshot);
    topics::join_unlocked(shared, &joining, &topics, &mut snapshot);
    shared.api.go_live(&joining, correlation, snapshot);
}

/// Reads the page outside the session's lock, as the control connection's `pane.read` does: a
/// page of 4 MiB takes a while to format, and nothing else should wait on it.
fn read_pane(shared: &Shared, connection: u64, correlation: String, asked: &api::ReadPane) {
    let reading = shared.lock().reading(&reading::asked(asked));
    let answer = match reading {
        None => refused(
            correlation,
            api::Refusal::NotFound,
            format!("no pane {} on this daemon", asked.pane),
        ),
        Some(reading) => match reading.page() {
            Some(text) => api::Answer {
                correlation,
                outcome: api::Outcome::Done.into(),
                detail: Some(api::answer::Detail::PaneText(reading::page(text))),
                ..api::Answer::default()
            },
            None => refused(
                correlation,
                api::Refusal::Unspecified,
                format!(
                    "no turn has started in pane {} since this daemon began watching it, so there \
                     is no last turn to read; read its newest rows instead",
                    asked.pane
                ),
            ),
        },
    };
    shared.api.answer(connection, answer);
}

/// The topics a subscribe names, once each, or why it is refused.
fn topics_of(asked: &api::Subscribe) -> Result<BTreeSet<api::Topic>, String> {
    asked
        .topics
        .iter()
        .map(|&value| {
            api::Topic::try_from(value).ok().filter(|topic| TOPICS.contains(topic)).ok_or_else(
                || {
                    format!(
                        "this daemon serves no topic {value}; its Welcome lists the topics it \
                         does, and nothing was subscribed"
                    )
                },
            )
        })
        .collect()
}

fn budget() -> Budget {
    let said =
        cfg!(debug_assertions).then(|| std::env::var(BUDGET_SAID).ok()?.parse().ok()).flatten();
    Budget { bytes: said.unwrap_or(BUDGET.bytes), ..BUDGET }
}

/// What a connection that fell behind is told, and the daemon's log with it.
fn farewell() -> Farewell {
    let budget = budget();
    Farewell {
        words: Box::new(move |at| {
            log::warn(
                "daemon.api.fell_behind",
                fields! {
                    "epoch" => at.epoch,
                    "reached" => at.number,
                    "budget" => budget,
                    "impact" => "the program is told so and hung up on; it subscribes again and \
                                 starts from a new snapshot",
                    "check" => "whether the program stopped reading its connection",
                },
            );
            let detail = format!(
                "this program stopped reading, and more than {budget} waited for it; subscribe \
                 again"
            );
            hub::frame(hub::ended(api::ended::Reason::FellBehind, at, detail)).to_vec()
        }),
        patience: FAREWELL_PATIENCE,
    }
}

/// The API of the daemon serving `socket`, in its run `instance`.
pub(crate) fn of_daemon(socket: &Path, instance: u64) -> Arc<Api> {
    Api::new(api::Machine {
        source: Some(api::Source { daemon: identity::of(socket), instance }),
        name: crate::effects::host_name(),
        version: crate::server::said_version(),
        features: Some(features()),
    })
}
