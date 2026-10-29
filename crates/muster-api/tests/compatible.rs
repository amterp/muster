//! The API's schema still reads everything its baseline wrote, unless it is before 1.0 and the
//! version says it broke.
//!
//! A program built against one minor talks to a daemon of another, so this is the check
//! `muster-baseline` describes, held to `muster_api.v<major>.baseline.proto`. Before 1.0 the API
//! may break clients (the schema's header), and a break is allowed once it has raised
//! `SERVES_FROM` past the baseline, which is what makes a daemon refuse the clients it would
//! mis-decode.

use muster_api::Version;
use muster_api::version::{API, SERVES_FROM};
use muster_baseline::{breaks, compile, proto_dir, recorded_version, same_schema, version_rule};

const SCHEMA: &str = "muster_api.proto";

fn baseline_name() -> String {
    format!("muster_api.v{}.baseline.proto", API.major)
}

fn baseline_version() -> (u32, u32) {
    let name = baseline_name();
    let text = std::fs::read_to_string(proto_dir().join(&name))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    recorded_version(&text).unwrap_or_else(|| {
        panic!(
            "{name} does not say which version it is.\n  Impact: nothing ties the baseline to a \
             published schema, so it can fall behind.\n  Fix: its header carries a line \
             `// Version: <major>.<minor>`."
        )
    })
}

#[test]
fn the_schema_reads_everything_its_baseline_wrote_or_says_it_broke() {
    let name = baseline_name();
    assert!(
        proto_dir().join(&name).exists(),
        "proto/{name} does not exist, and API.major is {}.\n  Impact: nothing checks that this \
         schema is compatible with the clients already written.\n  Fix: a major bump replaces \
         the baseline in the same change - copy proto/{SCHEMA} to that name.",
        API.major
    );
    let found = breaks(&compile(&proto_dir(), &name), &compile(&proto_dir(), SCHEMA));
    if let Err(why) = break_rule(baseline_version(), API, SERVES_FROM, &found) {
        panic!("{why}");
    }
}

#[test]
fn the_baseline_is_the_last_published_schema() {
    let name = baseline_name();
    let changed = !same_schema(&compile(&proto_dir(), &name), &compile(&proto_dir(), SCHEMA));
    if let Err(why) = version_rule(SCHEMA, baseline_version(), (API.major, API.minor), changed) {
        panic!("{why}");
    }
}

/// Whether the schema at `api`, serving clients from `serves_from`, may make `breaks` against a
/// baseline published as `baseline`.
fn break_rule(
    baseline: (u32, u32),
    api: Version,
    serves_from: Version,
    breaks: &[String],
) -> Result<(), String> {
    if breaks.is_empty() || (api.major == 0 && serves_from.minor > baseline.1) {
        return Ok(());
    }
    let fix = if api.major == 0 {
        "keep the field's number and type, or reserve a removed number. A break is allowed \
         before 1.0: raise API.minor, move SERVES_FROM up to it, and say so in the release notes."
    } else {
        "keep the field's number and type, or reserve a removed number. From 1.0 a break is a \
         new major."
    };
    Err(format!(
        "proto/{SCHEMA} no longer reads what the {}.{} baseline wrote:\n  - {}\n  Impact: a \
         client built from the baseline mis-decodes this daemon, silently.\n  Fix: {fix}",
        baseline.0,
        baseline.1,
        breaks.join("\n  - ")
    ))
}

mod the_break_rule {
    use super::*;

    const BROKE: &[&str] = &["muster.api.Snapshot.epoch (field 1) is gone"];

    fn version(major: u32, minor: u32) -> Version {
        Version { major, minor }
    }

    #[test]
    fn a_schema_that_breaks_nothing_passes_at_any_version() {
        assert!(break_rule((0, 1), version(0, 1), version(0, 1), &[]).is_ok());
        assert!(break_rule((1, 3), version(1, 4), version(1, 0), &[]).is_ok());
    }

    #[test]
    fn before_1_0_a_break_passes_once_the_oldest_served_has_moved_past_the_baseline() {
        let broke: Vec<String> = BROKE.iter().map(ToString::to_string).collect();
        assert!(break_rule((0, 1), version(0, 2), version(0, 2), &broke).is_ok());
        let unannounced = break_rule((0, 1), version(0, 2), version(0, 1), &broke).unwrap_err();
        assert!(unannounced.contains("SERVES_FROM"), "{unannounced}");
    }

    #[test]
    fn from_1_0_a_break_never_passes_within_a_major() {
        let broke: Vec<String> = BROKE.iter().map(ToString::to_string).collect();
        let refused = break_rule((1, 1), version(1, 2), version(1, 2), &broke).unwrap_err();
        assert!(refused.contains("new major"), "{refused}");
    }
}
