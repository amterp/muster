//! The daemon's schema still reads everything its major version's baseline wrote.
//!
//! An app adopts a daemon from another build, so this is the check `muster-baseline` describes,
//! held to `muster_daemon.v<major>.baseline.proto`.

use muster_baseline::{breaks, compile, proto_dir, recorded_version, same_schema, version_rule};
use muster_daemon_proto::version::PROTOCOL;

const SCHEMA: &str = "muster_daemon.proto";

#[test]
fn the_schema_reads_everything_its_baseline_wrote() {
    let baseline_name = format!("muster_daemon.v{}.baseline.proto", PROTOCOL.major);
    let baseline_path = proto_dir().join(&baseline_name);
    assert!(
        baseline_path.exists(),
        "{} does not exist, and PROTOCOL.major is {}.\n  Impact: nothing checks that this \
         schema is compatible with the daemons already running.\n  Fix: a major bump \
         replaces the baseline in the same change - copy proto/muster_daemon.proto to that \
         name.",
        baseline_path.display(),
        PROTOCOL.major
    );
    let baseline = compile(&proto_dir(), &baseline_name);
    let current = compile(&proto_dir(), SCHEMA);
    let breaks = breaks(&baseline, &current);
    assert!(
        breaks.is_empty(),
        "proto/muster_daemon.proto no longer reads what {baseline_name} wrote:\n  - {}\n  \
         Impact: an app built from this schema mis-decodes a daemon built from the baseline, \
         silently.\n  Fix: keep the field's number and type, or reserve a removed number. If \
         the change is meant, and no release has shipped the daemon yet, replace the baseline; \
         once one has, bump PROTOCOL.major too.",
        breaks.join("\n  - ")
    );
}

#[test]
fn the_baseline_is_the_last_published_schema() {
    let baseline_name = format!("muster_daemon.v{}.baseline.proto", PROTOCOL.major);
    let text = std::fs::read_to_string(proto_dir().join(&baseline_name))
        .unwrap_or_else(|error| panic!("{baseline_name}: {error}"));
    let recorded = recorded_version(&text).unwrap_or_else(|| {
        panic!(
            "{baseline_name} does not say which version it is.\n  Impact: nothing ties the \
             baseline to a published schema, so it can fall behind.\n  Fix: its header carries \
             a line `// Version: <major>.<minor>`."
        )
    });
    let changed =
        !same_schema(&compile(&proto_dir(), &baseline_name), &compile(&proto_dir(), SCHEMA));
    if let Err(why) = version_rule(SCHEMA, recorded, (PROTOCOL.major, PROTOCOL.minor), changed) {
        panic!("{why}");
    }
}
