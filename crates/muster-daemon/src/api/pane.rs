//! `pane.new`: a pane made for a program, under an operation id (`crate::operations`).
//!
//! The pane is composed as `muster pane new` composes one with no window
//! (`muster_daemon_proto::making`) and started through the session's own create, so the API and
//! the CLI make the same pane from the same ask. What the operation came to depends on which side
//! of the command's exec a failure fell: before it nothing ran, and the operation is forgotten so
//! that the client may ask again; after it the command may have done something, and the
//! operation is remembered as uncertain so that no retry runs it a second time.

use std::convert::Infallible;
use std::path::Path;
use std::time::Duration;

use muster_api as api;
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::names::{Mint, Minter};
use muster_daemon_proto::{self as proto, making};
use prost::Message;

use super::{Caller, refused};
use crate::api::Cause;
use crate::operations::{self, Begun};
use crate::persist::Made;
use crate::session::{Handled, Session, Shared, Starting};

/// How long a debug build holds a started pane before placing it, when set, in milliseconds: the
/// one way a test gets a handoff between a command starting and its pane being placed. A shipped
/// daemon never reads it.
const STARTED_DELAY_SAID: &str = "MUSTER_DAEMON_API_STARTED_DELAY";

pub(super) fn new(shared: &Shared, caller: &Caller, correlation: String, asked: &api::NewPane) {
    let operation = Operation {
        client: &caller.client,
        id: &asked.operation,
        cause: Cause { connection: caller.connection, correlation },
    };
    let answer = made(shared, caller, &operation, asked).unwrap_or_else(|answer| *answer);
    // Under a hold of the lock after the one the pane was placed under, as a control connection
    // answers: the work letting go of that one left on the pane's terminal is done, so the answer
    // means the pane is there.
    let _session = shared.lock();
    shared.api.answer(caller.connection, answer);
}

/// Makes the pane `asked` for, answering that it did, or answers why it did not.
fn made(
    shared: &Shared,
    caller: &Caller,
    operation: &Operation<'_>,
    asked: &api::NewPane,
) -> Result<api::Answer, Box<api::Answer>> {
    checked(shared, operation, asked)?;
    let mut session = shared.lock();
    begun(&mut session, operation, asked)?;
    let (mut starting, pane) = match starting(&mut session, caller, operation, asked) {
        Ok(starting) => starting,
        Err(refusal) => {
            operation.forget(&mut session);
            return Err(refusal);
        }
    };
    drop(session);

    // Outside the lock, as a control connection starts a pane: starting waits for the process
    // to change directory and exec.
    let started = starting.start();
    let ran = started.is_ok();
    if ran && let Some(delay) = started_delay() {
        std::thread::sleep(delay);
    }
    let mut session = shared.lock();
    session.cause = Some(operation.cause.clone());
    let reply = session.started(*starting, started);
    session.cause = None;
    let correlation = operation.cause.correlation.clone();
    let answer = if reply.outcome == proto::Outcome::Done {
        let callsign = match reply.detail.as_deref() {
            Some(proto::answer::Detail::Created(created)) => created.callsign.clone(),
            _ => String::new(),
        };
        let made = Made { pane, callsign };
        session.pane_operations.done(operation.client, operation.id, made.clone());
        made_answer(correlation, api::Outcome::Done, made)
    } else if ran {
        session.pane_operations.uncertain(operation.client, operation.id);
        unknown(
            correlation,
            &format!(
                "the pane's command started, and then the pane was lost: {}. Whether the command \
                 did anything is unknown; asking again with this operation id never runs it \
                 again, and a new id does",
                reply.reason
            ),
        )
    } else {
        operation.forget(&mut session);
        refused(correlation, api::Refusal::Unspecified, reply.reason)
    };
    session.state_changed();
    Ok(answer)
}

/// Refuses what no daemon state could make right.
fn checked(
    shared: &Shared,
    operation: &Operation<'_>,
    asked: &api::NewPane,
) -> Result<(), Box<api::Answer>> {
    let refuse = |refusal, reason| Err(Box::new(operation.refused(refusal, reason)));
    if asked.operation.is_empty() {
        return refuse(
            api::Refusal::Invalid,
            "a new pane needs an operation id, which makes asking again safe; nothing was made"
                .to_string(),
        );
    }
    let this = shared.api.source().daemon;
    if !asked.machine.is_empty() && asked.machine != this {
        return refuse(
            api::Refusal::NotFound,
            format!(
                "this daemon is {this} and answers for no machine {}; nothing was made",
                asked.machine
            ),
        );
    }
    if !asked.directory.is_empty() && !Path::new(&asked.directory).is_absolute() {
        return refuse(
            api::Refusal::Invalid,
            format!(
                "the directory {:?} is not an absolute path, and a daemon has no directory of \
                 the client's to resolve it against; nothing was made",
                asked.directory
            ),
        );
    }
    Ok(())
}

/// Begins the operation, or answers a repeat of it.
fn begun(
    session: &mut Session,
    operation: &Operation<'_>,
    asked: &api::NewPane,
) -> Result<(), Box<api::Answer>> {
    if let Some(why) = session.unavailable() {
        let reason = format!("{why}; nothing was made");
        return Err(Box::new(operation.refused(api::Refusal::Unavailable, reason)));
    }
    let asked_for = operations::fingerprint(&asked.encode_to_vec());
    let now = operations::now_ms();
    let correlation = operation.cause.correlation.clone();
    Err(Box::new(match session.pane_operations.begin(operation.client, operation.id, asked_for, now) {
        Begun::New => return Ok(()),
        Begun::Done(made) => made_answer(correlation, api::Outcome::AlreadySo, made),
        Begun::UnderWay => unknown(
            correlation,
            "a request with this operation id is still being carried out; ask again with the \
             same id for what it came to",
        ),
        Begun::Uncertain => unknown(
            correlation,
            "a request with this operation id started its pane's command, and the pane was then \
             lost, so whether the command did anything is unknown. Asking again with this id \
             never runs it again; a new id does",
        ),
        Begun::Reused => operation.refused(
            api::Refusal::OperationReused,
            format!(
                "the operation id {:?} was used for a different request in the last day; nothing \
                 was made",
                asked.operation
            ),
        ),
        Begun::Full => {
            log::warn(
                "daemon.api.operations_full",
                fields! {
                    "client" => operation.client,
                    "held" => operations::CAPACITY,
                    "impact" => "programs cannot make panes through the API until the oldest \
                                 operation is a day old",
                    "check" => "which program is making panes in such numbers",
                },
            );
            operation.refused(
                api::Refusal::Unavailable,
                format!(
                    "this daemon remembers {} operations from the last day, as many as it holds, \
                     and forgets none early; ask again later. Nothing was made",
                    operations::CAPACITY
                ),
            )
        }
    }))
}

/// The pane's create, checked and its names reserved by the session's own create, with the
/// pane's id: or why it was refused, before anything ran.
fn starting(
    session: &mut Session,
    caller: &Caller,
    operation: &Operation<'_>,
    asked: &api::NewPane,
) -> Result<(Box<Starting>, String), Box<api::Answer>> {
    let mut drawn = Minter::new(Mint::Drawn);
    let minted = making::Minted { pane: drawn.pane().to_string(), tab: drawn.tab().to_string() };
    let held = |pane: &str| Ok::<_, Infallible>(session.held(pane));
    let create = match making::create(composed(asked), held, minted) {
        Ok(create) => create,
        Err(making::Unmade::NotHeld(pane)) => {
            let reason = format!("this daemon holds no pane {pane}; nothing was made");
            return Err(Box::new(operation.refused(api::Refusal::NotFound, reason)));
        }
        Err(making::Unmade::Lookup(never)) => match never {},
    };
    let pane = create.pane.clone();
    let service = proto::request::Service::Pane(proto::PaneRequest {
        request: Some(proto::pane_request::Request::Create(create)),
    });
    session.cause = Some(operation.cause.clone());
    let handled = session.handle(service, &caller.outbox);
    session.cause = None;
    match handled {
        Handled::Start(starting) => Ok((starting, pane)),
        Handled::Reply(reply) => {
            let refusal = match reply.outcome {
                proto::Outcome::NotThere => api::Refusal::NotFound,
                _ => api::Refusal::Unspecified,
            };
            Err(Box::new(operation.refused(refusal, reply.reason)))
        }
        other => unreachable!("a create is started or refused, never {other:?}"),
    }
}

/// The operation a request carries out, and who asked for it.
struct Operation<'a> {
    client: &'a str,
    id: &'a str,
    cause: Cause,
}

impl Operation<'_> {
    /// Nothing was carried out, so the client may ask again under the same id.
    fn forget(&self, session: &mut Session) {
        session.pane_operations.forget(self.client, self.id);
    }

    fn refused(&self, refusal: api::Refusal, reason: String) -> api::Answer {
        refused(self.cause.correlation.clone(), refusal, reason)
    }
}

/// What `muster pane new` would ask for this pane. A helper starts from its maker, in its
/// directory and hidden in the tab its helpers share, and is quiet unless asked otherwise, as
/// `--helper` makes one.
fn composed(asked: &api::NewPane) -> making::NewPane {
    let said = |text: &str| (!text.is_empty()).then(|| text.to_string());
    let maker = said(&asked.helper_of);
    let helper = maker.is_some();
    making::NewPane {
        beside: maker.clone(),
        hidden: helper || asked.hidden,
        quiet: asked.quiet.unwrap_or(helper),
        cwd: said(&asked.directory),
        run: said(&asked.run),
        name: said(&asked.name),
        maker,
        ..making::NewPane::default()
    }
}

fn made_answer(correlation: String, outcome: api::Outcome, made: Made) -> api::Answer {
    api::Answer {
        correlation,
        outcome: outcome.into(),
        detail: Some(api::answer::Detail::Made(api::Made {
            pane: made.pane,
            callsign: made.callsign,
        })),
        ..api::Answer::default()
    }
}

fn unknown(correlation: String, reason: &str) -> api::Answer {
    api::Answer {
        correlation,
        outcome: api::Outcome::Unknown.into(),
        reason: reason.to_string(),
        ..api::Answer::default()
    }
}

fn started_delay() -> Option<Duration> {
    let said = cfg!(debug_assertions).then(|| std::env::var(STARTED_DELAY_SAID).ok()).flatten()?;
    said.parse().ok().map(Duration::from_millis)
}
