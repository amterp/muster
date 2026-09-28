//! What an agent says about itself: its context, its sub-agents, its model and cost, what it is
//! waiting on, and whatever else its harness passes on (`PaneRequest.Report`).
//!
//! The daemon keeps these on the pane's record because the view is a function of the daemon's
//! state: they survive the app quitting, and reach a window from a devenv with nothing else in
//! between. They are what the agent says, bounded, never what the daemon read off its screen.

use muster_daemon_proto as proto;
use proto::pane_request::Report;

const MODEL_BYTES: usize = 128;
const OTHER_KEYS: usize = 16;
const KEY_BYTES: usize = 64;
const VALUE_BYTES: usize = 256;

/// The pane's facts after `report`, or why the report is refused. None when nothing is known.
pub(crate) fn apply(
    current: Option<&proto::AgentFacts>,
    report: Report,
) -> Result<Option<proto::AgentFacts>, String> {
    let subagent = report.subagent();
    let mut facts = if report.clear {
        proto::AgentFacts::default()
    } else {
        current.cloned().unwrap_or_default()
    };
    if let Some(context_used) = report.context_used {
        if !(0.0..=100.0).contains(&context_used) {
            return Err(format!("context_used is {context_used}, and must be 0 to 100"));
        }
        facts.context_used = Some(context_used);
    }
    // Empty, as for any other fact, removes it.
    if let Some(model) = report.model {
        text("model", &model, MODEL_BYTES)?;
        facts.model = Some(model).filter(|model| !model.is_empty());
    }
    if let Some(cost_usd) = report.cost_usd {
        if !cost_usd.is_finite() || cost_usd < 0.0 {
            return Err(format!("cost_usd is {cost_usd}, and must be a sum not below zero"));
        }
        facts.cost_usd = Some(cost_usd);
    }
    if let Some(waiting) = report.waiting {
        text("waiting", &waiting, VALUE_BYTES)?;
        facts.waiting = Some(waiting).filter(|waiting| !waiting.is_empty());
    }
    match subagent {
        proto::SubagentChange::Started => facts.subagents = facts.subagents.saturating_add(1),
        proto::SubagentChange::Stopped => facts.subagents = facts.subagents.saturating_sub(1),
        proto::SubagentChange::None => {}
    }
    for (key, value) in report.facts {
        if key.is_empty() {
            return Err("a fact needs a name".to_string());
        }
        text("a fact's name", &key, KEY_BYTES)?;
        if value.is_empty() {
            facts.other.remove(&key);
            continue;
        }
        text(&format!("fact {key}"), &value, VALUE_BYTES)?;
        facts.other.insert(key, value);
    }
    if facts.other.len() > OTHER_KEYS {
        return Err(format!(
            "a pane holds at most {OTHER_KEYS} other facts, and this would make {}",
            facts.other.len()
        ));
    }
    Ok((facts != proto::AgentFacts::default()).then_some(facts))
}

fn text(what: &str, text: &str, most: usize) -> Result<(), String> {
    if text.len() > most {
        return Err(format!("{what} is {} bytes, and may be at most {most}", text.len()));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{what} holds a control character"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report { pane: "p1".to_string(), ..Report::default() }
    }

    fn applied(current: Option<&proto::AgentFacts>, report: Report) -> proto::AgentFacts {
        apply(current, report).expect("a valid report").expect("something known")
    }

    #[test]
    fn what_an_agent_waits_on_is_set_bounded_and_cleared_by_an_empty_one() {
        let waiting = applied(None, Report { waiting: Some("the full gate".into()), ..report() });
        assert_eq!(waiting.waiting.as_deref(), Some("the full gate"));
        let cleared = apply(Some(&waiting), Report { waiting: Some(String::new()), ..report() });
        assert_eq!(cleared, Ok(None));
        let long = Report { waiting: Some("x".repeat(VALUE_BYTES + 1)), ..report() };
        assert!(apply(None, long).unwrap_err().contains("waiting"));
    }

    #[test]
    fn a_report_replaces_what_it_names_and_keeps_the_rest() {
        let first = applied(
            None,
            Report { context_used: Some(12.5), model: Some("Opus".into()), ..report() },
        );
        let second = applied(Some(&first), Report { cost_usd: Some(0.42), ..report() });
        assert_eq!(second.context_used, Some(12.5));
        assert_eq!(second.model.as_deref(), Some("Opus"));
        assert_eq!(second.cost_usd, Some(0.42));
        let cleared = apply(Some(&second), Report { clear: true, ..report() });
        assert_eq!(cleared, Ok(None), "cleared, the pane knows nothing");
    }

    #[test]
    fn sub_agents_are_counted_from_their_starts_and_stops_and_never_below_none() {
        let started = |facts: Option<&proto::AgentFacts>, change: proto::SubagentChange| {
            apply(facts, Report { subagent: change.into(), ..report() }).unwrap()
        };
        let one = started(None, proto::SubagentChange::Started);
        let two = started(one.as_ref(), proto::SubagentChange::Started);
        let back = started(two.as_ref(), proto::SubagentChange::Stopped);
        assert_eq!(back.as_ref().map(|facts| facts.subagents), Some(1));
        let none = started(back.as_ref(), proto::SubagentChange::Stopped);
        assert_eq!(started(none.as_ref(), proto::SubagentChange::Stopped), None);
    }

    #[test]
    fn an_empty_model_clears_the_model() {
        let set =
            applied(None, Report { model: Some("Opus".into()), cost_usd: Some(1.0), ..report() });
        let cleared = applied(Some(&set), Report { model: Some(String::new()), ..report() });
        assert_eq!((cleared.model, cleared.cost_usd), (None, Some(1.0)));
    }

    #[test]
    fn other_facts_are_set_and_removed_by_name() {
        let set = applied(
            None,
            Report { facts: [("branch".to_string(), "main".to_string())].into(), ..report() },
        );
        assert_eq!(set.other.get("branch").map(String::as_str), Some("main"));
        let removed = apply(
            Some(&set),
            Report { facts: [("branch".to_string(), String::new())].into(), ..report() },
        );
        assert_eq!(removed, Ok(None));
    }

    #[test]
    fn a_report_out_of_bounds_is_refused_whole() {
        let refused = |report: Report| apply(None, report).expect_err("out of bounds");
        refused(Report { context_used: Some(101.0), ..report() });
        refused(Report { context_used: Some(f32::NAN), ..report() });
        refused(Report { cost_usd: Some(-1.0), ..report() });
        refused(Report { model: Some("m".repeat(MODEL_BYTES + 1)), ..report() });
        refused(Report { model: Some("tab\there".into()), ..report() });
        refused(Report { facts: [(String::new(), "v".into())].into(), ..report() });
        refused(Report { facts: [("k".into(), "v".repeat(VALUE_BYTES + 1))].into(), ..report() });
        let many = (0..=OTHER_KEYS).map(|key| (format!("k{key}"), "v".to_string())).collect();
        refused(Report { facts: many, ..report() });
        let partly = Report { model: Some("fine".into()), context_used: Some(-5.0), ..report() };
        assert!(apply(None, partly).is_err(), "nothing of a refused report is kept");
    }
}
