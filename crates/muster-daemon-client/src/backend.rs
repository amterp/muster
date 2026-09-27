//! The core's backend, spoken to a daemon: its intents as requests, its reads as pages, and
//! its input as events on the input connection.
//!
//! Everything a request did arrives as events before its answer, applied by the follower on
//! the connection's reader, so by the time a submit returns the mirror already shows it
//! (MIP-3, section 9). What a submit returns is only what no event can say: which of the
//! things that appeared this request made.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use muster_core::Key;
use muster_core::input::{InputEvent, InputSink, NotSent};
use muster_core::intent::{BackendChannel, BackendIntent, MoveDestination, Outcome, Refusal, Side};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::names::Minter;
use muster_core::pane_text::PaneText;
use muster_daemon_proto::{
    self as proto, answer, pane_request, placement, request::Service, tab_request,
};

use crate::control::Unanswered;
use crate::convert;
use crate::follow::Connection;

/// How long a daemon has to answer a request. It answers in milliseconds; this is for a
/// machine so loaded, or a link so slow, that the answer is late rather than lost.
const PATIENCE: Duration = Duration::from_secs(10);

/// One daemon, as the core's backend.
#[derive(Debug)]
pub struct DaemonBackend {
    connection: Arc<Connection>,
    mirror: Arc<Mutex<Mirror>>,
    minter: Arc<Mutex<Minter>>,
    /// Added to every pane this makes: `MUSTER_SOCKET`, which names the window, and nothing a
    /// daemon could know for itself.
    environment: BTreeMap<String, String>,
    description: String,
}

impl DaemonBackend {
    pub fn new(
        connection: Arc<Connection>,
        mirror: Arc<Mutex<Mirror>>,
        minter: Arc<Mutex<Minter>>,
        environment: BTreeMap<String, String>,
        description: String,
    ) -> DaemonBackend {
        DaemonBackend { connection, mirror, minter, environment, description }
    }

    fn ask(&self, service: Service) -> Result<proto::Answer, Refusal> {
        let Some(control) = self.connection.control() else {
            return Err(Refusal::Declined(format!(
                "{} is not connected, so nothing was sent; it is reconnecting on its own",
                self.description
            )));
        };
        let answer = control.ask(service).wait(PATIENCE).map_err(|why| match why {
            Unanswered::TimedOut => Refusal::Unanswered(format!(
                "{} did not answer within {}s; the change may still happen, and arrives on its \
                 events if it does",
                self.description,
                PATIENCE.as_secs()
            )),
            Unanswered::Ended => Refusal::Unanswered(format!(
                "the connection to {} ended before it answered, so the change may or may not \
                 have happened",
                self.description
            )),
        })?;
        match answer.outcome() {
            proto::Outcome::Done | proto::Outcome::AlreadySo => Ok(answer),
            proto::Outcome::NotThere => Err(Refusal::NotThere(answer.reason)),
            proto::Outcome::Refused | proto::Outcome::Unspecified => {
                Err(Refusal::Declined(answer.reason))
            }
        }
    }

    fn pane_request(&self, request: pane_request::Request) -> Result<proto::Answer, Refusal> {
        self.ask(Service::Pane(proto::PaneRequest { request: Some(request) }))
    }

    /// One page of a pane's history, from `first_row` to the last row or the daemon's 4 MiB.
    fn read_page(&self, pane: &PaneId, first_row: u64) -> Result<proto::PaneText, Refusal> {
        let answer = self.pane_request(pane_request::Request::Read(pane_request::Read {
            pane: pane.to_string(),
            first_row,
            rows: 0,
        }))?;
        let Some(answer::Detail::Text(read)) = answer.detail else {
            return Err(Refusal::Declined(format!(
                "{} answered a read with no text; this is likely a bug in the daemon",
                self.description
            )));
        };
        Ok(read)
    }

    fn tab_request(&self, request: tab_request::Request) -> Result<proto::Answer, Refusal> {
        self.ask(Service::Tab(proto::TabRequest { request: Some(request) }))
    }

    fn mint_pane(&self) -> PaneId {
        self.minter.lock().unwrap_or_else(PoisonError::into_inner).pane()
    }

    fn create(
        &self,
        placement: placement::Where,
        cwd: Option<String>,
        run: Option<String>,
        name: Option<String>,
    ) -> Result<PaneId, Refusal> {
        let pane = self.mint_pane();
        self.pane_request(pane_request::Request::Create(pane_request::Create {
            pane: pane.to_string(),
            placement: Some(proto::Placement { r#where: Some(placement) }),
            grid: None,
            cwd,
            env: self.environment.clone().into_iter().collect(),
            command: run,
            label: name,
        }))?;
        Ok(pane)
    }

    /// Where a pane moved into an existing tab lands: beside the tab's last pane, when this
    /// daemon holds part of the tab, and otherwise in a new part of it here.
    fn placement_in(&self, tab: &TabId) -> placement::Where {
        let mirror = self.mirror.lock().unwrap_or_else(PoisonError::into_inner);
        match mirror.tree(tab).and_then(|root| root.panes().last().copied().cloned()) {
            Some(last) => beside(&last, Side::Right),
            // A part of a grouped tab starts unnamed, and the window names it after the rest
            // when the daemon announces it.
            None => placement::Where::NewTab(placement::NewTab {
                tab: tab.to_string(),
                label: Some(proto::Label::default()),
            }),
        }
    }

    /// Where a moved pane goes, and the tab the move makes, if it makes one.
    fn destination(&self, to: MoveDestination) -> (placement::Where, Option<TabId>) {
        match to {
            MoveDestination::Beside { after, .. } => (beside(&after, Side::Right), None),
            MoveDestination::NewTab { tab, name } => {
                let label = proto::Label { generation: u64::from(name.is_some()), text: name };
                let placement = placement::Where::NewTab(placement::NewTab {
                    tab: tab.to_string(),
                    label: Some(label),
                });
                (placement, Some(tab))
            }
            MoveDestination::Tab { tab } => (self.placement_in(&tab), None),
        }
    }

    fn is_zoomed(&self, pane: &PaneId) -> bool {
        let mirror = self.mirror.lock().unwrap_or_else(PoisonError::into_inner);
        mirror
            .pane(pane)
            .and_then(|held| mirror.tab(&held.tab))
            .is_some_and(|tab| tab.zoomed.as_ref() == Some(pane))
    }
}

fn reaches_the_end(page: &proto::PaneText) -> bool {
    page.first_row + u64::from(page.rows) >= page.total_rows
}

fn beside(pane: &PaneId, side: Side) -> placement::Where {
    placement::Where::Beside(placement::Beside {
        pane: pane.to_string(),
        side: convert::side(side).into(),
        ratio: None,
    })
}

impl BackendChannel for DaemonBackend {
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, Refusal> {
        let done = Ok(Outcome::default());
        match intent.clone() {
            BackendIntent::SplitPane { pane, side, ratio, cwd, run, name } => {
                let placement = placement::Where::Beside(placement::Beside {
                    pane: pane.to_string(),
                    side: convert::side(side).into(),
                    ratio,
                });
                let created = self.create(placement, cwd, run, name)?;
                Ok(Outcome { created: Some(created), created_tab: None })
            }
            BackendIntent::CreateTab { tab, cwd, run, name } => {
                let placement = placement::Where::NewTab(placement::NewTab {
                    tab: tab.to_string(),
                    label: Some(proto::Label::default()),
                });
                let created = self.create(placement, cwd, run, name)?;
                Ok(Outcome { created: Some(created), created_tab: Some(tab) })
            }
            BackendIntent::ClosePane { pane } => {
                self.pane_request(pane_request::Request::Close(pane_request::Close {
                    pane: pane.to_string(),
                }))?;
                done
            }
            BackendIntent::ResizePane { pane, direction, fraction } => {
                self.pane_request(pane_request::Request::Resize(pane_request::Resize {
                    pane: pane.to_string(),
                    direction: convert::side(direction).into(),
                    fraction,
                }))?;
                done
            }
            BackendIntent::ZoomPane { pane } => {
                let zoomed = !self.is_zoomed(&pane);
                self.pane_request(pane_request::Request::Zoom(pane_request::Zoom {
                    pane: pane.to_string(),
                    zoomed,
                }))?;
                done
            }
            BackendIntent::SwapPanes { pane, with } => {
                self.pane_request(pane_request::Request::Swap(pane_request::Swap {
                    pane: pane.to_string(),
                    with: with.to_string(),
                }))?;
                done
            }
            BackendIntent::MovePane { pane, to } => {
                let (placement, created_tab) = self.destination(to);
                self.pane_request(pane_request::Request::Move(pane_request::Move {
                    pane: pane.to_string(),
                    placement: Some(proto::Placement { r#where: Some(placement) }),
                }))?;
                Ok(Outcome { created: None, created_tab })
            }
            BackendIntent::CloseTab { tab } => {
                self.tab_request(tab_request::Request::Close(tab_request::Close {
                    tab: tab.to_string(),
                }))?;
                done
            }
            BackendIntent::SetSplitRatio { tab, path, ratio } => {
                self.tab_request(tab_request::Request::SetSplitRatio(
                    tab_request::SetSplitRatio {
                        tab: tab.to_string(),
                        path: path.into_iter().map(|turn| convert::branch(turn).into()).collect(),
                        ratio,
                    },
                ))?;
                done
            }
            BackendIntent::RenamePane { pane, name } => {
                self.pane_request(pane_request::Request::Rename(pane_request::Rename {
                    pane: pane.to_string(),
                    label: name,
                }))?;
                done
            }
            BackendIntent::RenameTab { tab, name, generation } => {
                self.tab_request(tab_request::Request::Rename(tab_request::Rename {
                    tab: tab.to_string(),
                    label: Some(proto::Label { text: name, generation }),
                }))?;
                done
            }
        }
    }

    fn read(&self, pane: &PaneId) -> Result<PaneText, Refusal> {
        let whole = self.read_page(pane, 0)?;
        if reaches_the_end(&whole) {
            return Ok(PaneText { text: whole.text, truncated: false });
        }
        // A page stops at the daemon's 4 MiB, and what a read is for is the newest rows, so the
        // page wanted is the one ending at the last row. Rows differ in length, so start as far
        // from the end as the first page reached from the start, and move on by however far a
        // page still falls short. A few tries settle it; the cap only bounds a pane printing
        // faster than it can be read.
        let mut newest = whole;
        for _ in 0..8 {
            let first_row = newest.total_rows.saturating_sub(u64::from(newest.rows));
            newest = self.read_page(pane, first_row)?;
            if reaches_the_end(&newest) || newest.rows == 0 {
                break;
            }
        }
        Ok(PaneText { text: newest.text, truncated: true })
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// One daemon's panes' input, on its input connection.
#[derive(Debug)]
pub struct DaemonInput {
    connection: Arc<Connection>,
    /// libghostty's code for a key, from the library that has it.
    key_code: fn(Key) -> u32,
    description: String,
}

impl DaemonInput {
    pub fn new(
        connection: Arc<Connection>,
        key_code: fn(Key) -> u32,
        description: String,
    ) -> DaemonInput {
        DaemonInput { connection, key_code, description }
    }
}

impl InputSink for DaemonInput {
    fn send(&self, pane: &PaneId, event: InputEvent) -> Result<(), NotSent> {
        self.connection.send_input(convert::input(pane, event, self.key_code))
    }

    fn description(&self) -> &str {
        &self.description
    }
}
