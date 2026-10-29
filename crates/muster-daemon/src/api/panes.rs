//! A pane as the API describes it (`TOPIC_PANES`), from the daemon's record of it.

use muster_api as api;
use muster_core::harnesses::{self, Standing};
use muster_core::mirror::backend::Adapter;
use muster_daemon_proto as proto;

/// `record` as a client reads it, `hidden` being whether its tab is.
pub(crate) fn pane(record: &proto::Pane, hidden: bool, source: &api::Source) -> api::Pane {
    let facts = record.facts.clone().unwrap_or_default();
    api::Pane {
        pane: record.pane.clone(),
        source: Some(source.clone()),
        callsign: record.callsign.clone().unwrap_or_default(),
        name: record.label.as_deref().map(str::trim).unwrap_or_default().to_string(),
        title: record.title.clone(),
        directory: record.cwd.clone(),
        harness: record.agent.clone().unwrap_or_default(),
        state: state(record.shown_state()).into(),
        since_ms: record.state_since_ms,
        facts: Some(api::Facts {
            context_used: facts.context_used,
            subagents: facts.subagents,
            model: facts.model.unwrap_or_default(),
            cost_usd: facts.cost_usd,
            waiting: facts.waiting.unwrap_or_default(),
            other: facts.other.into_iter().collect(),
        }),
        unread_input: record.unread_input.as_ref().map(|unread| api::UnreadInput {
            bytes: unread.bytes,
            since_ms: unread.since_ms,
            program: unread.program.clone(),
            pid: unread.pid,
        }),
        session: record.session_id.clone().unwrap_or_default(),
        maker: record.maker.clone().unwrap_or_default(),
        hidden,
        quiet: record.quiet,
        adapter: adapter(record).into(),
    }
}

/// Whether `now` says anything a client has not been told by `before`. A title alone does not:
/// a harness redraws its title while it works, which would make every pane an event a second.
pub(crate) fn news(before: &api::Pane, now: &api::Pane) -> bool {
    api::Pane { title: before.title.clone(), ..now.clone() } != *before
}

fn state(state: proto::PaneState) -> api::PaneState {
    match state {
        proto::PaneState::Unknown => api::PaneState::Unknown,
        proto::PaneState::Working => api::PaneState::Working,
        proto::PaneState::Blocked => api::PaneState::Blocked,
        proto::PaneState::Idle => api::PaneState::Idle,
        proto::PaneState::Waiting => api::PaneState::Waiting,
        proto::PaneState::Stuck => api::PaneState::Stuck,
        proto::PaneState::Done => api::PaneState::Done,
    }
}

/// As `muster window --json` says it, by the same rule (`harnesses::standing`).
fn adapter(record: &proto::Pane) -> api::Adapter {
    let said = match record.adapter() {
        proto::Adapter::Unsaid => Adapter::Unsaid,
        proto::Adapter::Reporting => Adapter::Reporting,
        proto::Adapter::Silent => Adapter::Silent,
    };
    match harnesses::standing(record.agent.as_deref(), said) {
        Standing::Unsaid => api::Adapter::Unsaid,
        Standing::Reporting => api::Adapter::Reporting,
        Standing::Silent(_) => api::Adapter::Silent,
        Standing::None => api::Adapter::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> api::Source {
        api::Source { daemon: "d1".to_string(), instance: 7 }
    }

    fn record() -> proto::Pane {
        let mut record = proto::Pane {
            pane: "p1".to_string(),
            callsign: Some("otter".to_string()),
            label: Some(" build ".to_string()),
            cwd: "/src/muster".to_string(),
            agent: Some("claude".to_string()),
            title: "✳ building".to_string(),
            state_since_ms: Some(1_700_000_000_000),
            maker: Some("p0".to_string()),
            quiet: true,
            facts: Some(proto::AgentFacts {
                waiting: Some("a build".to_string()),
                context_used: Some(42.0),
                ..proto::AgentFacts::default()
            }),
            ..proto::Pane::default()
        };
        record.set_agent_state(proto::AgentState::Idle);
        record
    }

    #[test]
    fn a_record_reads_as_its_fields_with_the_state_a_view_shows() {
        let pane = pane(&record(), true, &source());
        assert_eq!((pane.callsign.as_str(), pane.name.as_str()), ("otter", "build"));
        assert_eq!(pane.state(), api::PaneState::Waiting, "waiting over the idle it said");
        assert_eq!(pane.since_ms, Some(1_700_000_000_000), "dated from the idle under it");
        assert_eq!((pane.hidden, pane.quiet, pane.maker.as_str()), (true, true, "p0"));
        assert_eq!(pane.facts.and_then(|facts| facts.context_used), Some(42.0));
        assert_eq!(pane.source, Some(source()));

        let undated = proto::Pane { state_since_ms: None, label: None, ..record() };
        let undated = super::pane(&undated, false, &source());
        assert_eq!((undated.since_ms, undated.name.as_str()), (None, ""));
    }

    #[test]
    fn an_adapter_reads_as_the_window_says_it() {
        let silent = |agent: &str| {
            let mut record = proto::Pane { agent: Some(agent.to_string()), ..record() };
            record.set_adapter(proto::Adapter::Silent);
            pane(&record, false, &source()).adapter()
        };
        assert_eq!(silent("claude"), api::Adapter::Silent, "Muster ships Claude Code's adapter");
        assert_eq!(silent("aider"), api::Adapter::None, "and none for this harness");
        assert_eq!(pane(&record(), false, &source()).adapter(), api::Adapter::Unsaid);
    }

    #[test]
    fn a_title_alone_is_not_news() {
        let before = pane(&record(), false, &source());
        let titled = proto::Pane { title: "✶ building".to_string(), ..record() };
        assert!(!news(&before, &pane(&titled, false, &source())), "a spinner is not news");
        let mut working = proto::Pane { title: "✶ building".to_string(), ..record() };
        working.set_agent_state(proto::AgentState::Working);
        assert!(news(&before, &pane(&working, false, &source())));
        assert!(news(&before, &pane(&record(), true, &source())), "hidden is news");
    }
}
