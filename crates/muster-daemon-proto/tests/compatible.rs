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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
