//! The daemon's schema still reads everything its major version's baseline wrote.
//!
//! The successor to herdr's schema drift check (`docs/testing.md`), for a protocol that is now
//! Muster's own. An app adopts a daemon from another build, so a field that changed number or
//! type between the two mis-decodes silently: nothing crashes, a value is just wrong. This fails
//! the build instead, naming what moved.
//!
//! The rule is wire compatibility, per protobuf's: every message, field number and enum value in
//! the baseline is still here with the same type, or its number is reserved. Names may change,
//! and anything may be added.
//!
//! That only protects what the baseline holds, so the baseline has to be the last schema that was
//! published, not the first. It records its version, and the second test holds the two together:
//! a schema that differs from its baseline is the next minor, and a minor past that means the
//! baseline was never brought forward. Otherwise a field added in 1.1 and deleted without being
//! reserved could come back in 1.3 under the same number with another type, and nothing here
//! would notice.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use muster_daemon_proto::Version;
use muster_daemon_proto::version::PROTOCOL;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorSet};

fn proto_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto")
}

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
    let current = compile(&proto_dir(), "muster_daemon.proto");
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
    let changed = !same_schema(
        &compile(&proto_dir(), &baseline_name),
        &compile(&proto_dir(), "muster_daemon.proto"),
    );
    if let Err(why) = version_rule(recorded, PROTOCOL, changed) {
        panic!("{why}");
    }
}

/// Whether the schema at `protocol` may sit on a baseline published as `baseline`.
fn version_rule(baseline: (u32, u32), protocol: Version, changed: bool) -> Result<(), String> {
    let (major, minor) = baseline;
    let fix = "Before any release has shipped the daemon, copy proto/muster_daemon.proto over the \
               baseline and keep its version line. After one has, the baseline is the schema as \
               the last minor shipped, and a changed schema is the next minor.";
    if major != protocol.major {
        return Err(format!(
            "the baseline says it is {major}.{minor}, and PROTOCOL.major is {}.\n  Fix: {fix}",
            protocol.major
        ));
    }
    match (protocol.minor.checked_sub(minor), changed) {
        (Some(0), false) | (Some(1), _) => Ok(()),
        (Some(0), true) => Err(format!(
            "proto/muster_daemon.proto differs from its baseline, and both say {major}.{minor}.\n  \
             Impact: a daemon and an app built from the two would claim one version and speak \
             two.\n  Fix: {fix}"
        )),
        _ => Err(format!(
            "PROTOCOL is {protocol} and the baseline is {major}.{minor}.\n  Impact: fields added \
             in the minors between are protected by nothing.\n  Fix: {fix}"
        )),
    }
}

/// The `// Version: 1.0` line in a baseline's header.
fn recorded_version(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().find_map(|line| line.strip_prefix("// Version: "))?;
    let (major, minor) = line.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether two schemas declare the same things, whatever their comments say.
fn same_schema(one: &FileDescriptorSet, other: &FileDescriptorSet) -> bool {
    let bare = |set: &FileDescriptorSet| -> Vec<prost_types::FileDescriptorProto> {
        set.file
            .iter()
            .map(|file| prost_types::FileDescriptorProto {
                name: None,
                source_code_info: None,
                ..file.clone()
            })
            .collect()
    };
    bare(one) == bare(other)
}

fn compile(directory: &Path, file: &str) -> FileDescriptorSet {
    protox::compile([directory.join(file)], [directory])
        .unwrap_or_else(|error| panic!("{file} does not compile: {error}"))
}

/// Everything in `before` that `after` no longer reads, one line each.
fn breaks(before: &FileDescriptorSet, after: &FileDescriptorSet) -> Vec<String> {
    let (before_messages, before_enums) = declarations(before);
    let (after_messages, after_enums) = declarations(after);
    let mut breaks = Vec::new();

    for (name, old) in &before_messages {
        let Some(new) = after_messages.get(name) else {
            breaks.push(format!("message {name} is gone"));
            continue;
        };
        for field in &old.field {
            let number = field.number();
            let reserved = new
                .reserved_range
                .iter()
                .any(|range| (range.start()..range.end()).contains(&number));
            match new.field.iter().find(|candidate| candidate.number() == number) {
                Some(now) if kind(now) != kind(field) => breaks.push(format!(
                    "{name}.{} (field {number}) was {} and is now {}",
                    field.name(),
                    describe(field),
                    describe(now)
                )),
                Some(_) => {}
                None if reserved => {}
                None => breaks.push(format!(
                    "{name}.{} (field {number}) is gone and its number is not reserved",
                    field.name()
                )),
            }
        }
    }

    for (name, old) in &before_enums {
        let Some(new) = after_enums.get(name) else {
            breaks.push(format!("enum {name} is gone"));
            continue;
        };
        for value in &old.value {
            let number = value.number();
            let kept = new.value.iter().any(|candidate| candidate.number() == number);
            let reserved = new
                .reserved_range
                .iter()
                .any(|range| (range.start()..=range.end()).contains(&number));
            if !kept && !reserved {
                breaks.push(format!(
                    "{name}.{} ({number}) is gone and its number is not reserved",
                    value.name()
                ));
            }
        }
    }
    breaks
}

/// What decides how a field's bytes are read: its wire type, what it names, and whether it
/// repeats.
fn kind(field: &prost_types::FieldDescriptorProto) -> (Type, String, bool) {
    (field.r#type(), field.type_name().to_string(), field.label() == Label::Repeated)
}

fn describe(field: &prost_types::FieldDescriptorProto) -> String {
    let (kind, name, repeated) = kind(field);
    let repeated = if repeated { "repeated " } else { "" };
    if name.is_empty() { format!("{repeated}{kind:?}") } else { format!("{repeated}{name}") }
}

type Declarations<'a> =
    (BTreeMap<String, &'a DescriptorProto>, BTreeMap<String, &'a EnumDescriptorProto>);

/// Every message and enum, nested ones included, by full name.
fn declarations(set: &FileDescriptorSet) -> Declarations<'_> {
    fn walk<'a>(
        prefix: &str,
        messages: &'a [DescriptorProto],
        enums: &'a [EnumDescriptorProto],
        into: &mut Declarations<'a>,
    ) {
        for declared in enums {
            into.1.insert(format!("{prefix}.{}", declared.name()), declared);
        }
        for message in messages {
            let name = format!("{prefix}.{}", message.name());
            walk(&name, &message.nested_type, &message.enum_type, into);
            into.0.insert(name, message);
        }
    }
    let mut found = (BTreeMap::new(), BTreeMap::new());
    for file in &set.file {
        walk(file.package(), &file.message_type, &file.enum_type, &mut found);
    }
    found
}

/// The version rule, on its own.
mod the_version_rule {
    use super::*;

    const AT: Version = Version { major: 1, minor: 2 };

    #[test]
    fn an_unchanged_schema_keeps_its_baselines_version_or_the_next() {
        assert!(version_rule((1, 2), AT, false).is_ok());
        assert!(version_rule((1, 1), AT, false).is_ok());
    }

    #[test]
    fn a_changed_schema_is_the_next_minor() {
        assert!(version_rule((1, 1), AT, true).is_ok());
        assert!(version_rule((1, 2), AT, true).is_err());
    }

    #[test]
    fn a_baseline_two_minors_behind_or_of_another_major_is_refused() {
        assert!(version_rule((1, 0), AT, false).is_err());
        assert!(version_rule((1, 3), AT, false).is_err());
        assert!(version_rule((2, 2), AT, false).is_err());
    }

    #[test]
    fn the_version_line_is_read_from_the_header() {
        assert_eq!(recorded_version("// intro\n// Version: 1.12\nsyntax"), Some((1, 12)));
        assert_eq!(recorded_version("// Version: one"), None);
    }
}

/// The check itself, against small schemas that each make one change.
mod the_check {
    use super::*;

    fn schema(body: &str) -> FileDescriptorSet {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "muster-daemon-proto-compat-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("fixture.proto"),
            format!("syntax = \"proto3\";\npackage fixture;\n{body}"),
        )
        .unwrap();
        let compiled = compile(&directory, "fixture.proto");
        let _ = std::fs::remove_dir_all(&directory);
        compiled
    }

    const BEFORE: &str = "
        message Pane { string pane = 1; uint64 row = 2; repeated string tags = 3;
                       message Inner { bool on = 1; } }
        enum State { STATE_UNKNOWN = 0; STATE_IDLE = 1; }";

    fn verdict(after: &str) -> Vec<String> {
        breaks(&schema(BEFORE), &schema(after))
    }

    #[test]
    fn adding_and_renaming_are_compatible() {
        assert_eq!(
            verdict(
                "message Pane { string name = 1; uint64 row = 2; repeated string tags = 3;
                                bool extra = 4; message Inner { bool on = 1; } }
                 message New {}
                 enum State { STATE_UNKNOWN = 0; STATE_IDLE = 1; STATE_BUSY = 2; }"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_removed_field_must_leave_its_number_reserved() {
        let unreserved = verdict(
            "message Pane { string pane = 1; repeated string tags = 3;
                            message Inner { bool on = 1; } }
             enum State { STATE_UNKNOWN = 0; STATE_IDLE = 1; }",
        );
        assert_eq!(
            unreserved,
            ["fixture.Pane.row (field 2) is gone and its number is not reserved"]
        );
        let reserved = verdict(
            "message Pane { reserved 2; string pane = 1; repeated string tags = 3;
                            message Inner { bool on = 1; } }
             enum State { STATE_UNKNOWN = 0; STATE_IDLE = 1; }",
        );
        assert!(reserved.is_empty(), "{reserved:?}");
    }

    #[test]
    fn a_changed_type_or_repetition_breaks() {
        let found = verdict(
            "message Pane { string pane = 1; uint32 row = 2; string tags = 3;
                            message Inner { bool on = 1; } }
             enum State { STATE_UNKNOWN = 0; STATE_IDLE = 1; }",
        );
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_lost_nested_message_or_enum_value_breaks() {
        let found = verdict(
            "message Pane { string pane = 1; uint64 row = 2; repeated string tags = 3; }
             enum State { STATE_UNKNOWN = 0; }",
        );
        assert_eq!(
            found,
            [
                "message fixture.Pane.Inner is gone",
                "fixture.State.STATE_IDLE (1) is gone and its number is not reserved"
            ]
        );
    }
}
