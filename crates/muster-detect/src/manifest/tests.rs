//! Ported from herdr v0.8.0 `src/detect/manifest/tests.rs` (Apache-2.0): the evaluator,
//! validation and region tests, rewritten onto `Manifest::parse` where herdr went through its
//! override directory. The tests of herdr's loader live with `Manifests`, and those of its
//! bundled manifests in `corpus/conformance/agent-detection.json`.

use super::region::Region;
use super::*;

fn rules_manifest(rules: &str) -> String {
    format!("id = \"codex\"\n\n{rules}\n")
}

fn screen(text: &str) -> Input<'_> {
    Input { screen: text, ..Input::default() }
}

fn region(content: &str, spec: &str) -> String {
    Region::parse(spec).expect("a valid region").slice(screen(content)).to_string()
}

#[test]
fn rule_semantics_apply_gates_priority_and_line_regex() {
    let manifest = Manifest::parse(&rules_manifest(
        r#"
[[rules]]
id = "low_contains"
state = "idle"
priority = 1
contains = ["match"]

[[rules]]
id = "high_nested_gates"
state = "working"
priority = 10
contains = ["match"]
all = [
  { any = [{ regex = ["w[io]n"] }, { contains = ["fallback"] }] },
]
not = [
  { contains = ["blocked"] },
]

[[rules]]
id = "line_regex"
state = "blocked"
priority = 20
line_regex = ["^exact line$"]
"#,
    ))
    .unwrap();

    let high = manifest.evaluate(screen("match win"));
    assert_eq!(high.state, State::Working);
    assert_eq!(high.rule.as_deref(), Some("high_nested_gates"));

    let not_gate = manifest.evaluate(screen("match win blocked"));
    assert_eq!(not_gate.state, State::Idle);
    assert_eq!(not_gate.rule.as_deref(), Some("low_contains"));

    let line = manifest.evaluate(screen("before\nexact line\nafter"));
    assert_eq!(line.state, State::Blocked);
    assert_eq!(line.rule.as_deref(), Some("line_regex"));
}

#[test]
fn contains_is_case_folded_and_regex_is_not() {
    let manifest = Manifest::parse(&rules_manifest(
        r#"
[[rules]]
id = "folded"
state = "working"
contains = ["Esc To Interrupt"]

[[rules]]
id = "exact"
state = "blocked"
priority = 5
regex = ["Allow"]
"#,
    ))
    .unwrap();

    assert_eq!(manifest.evaluate(screen("ESC TO INTERRUPT")).rule.as_deref(), Some("folded"));
    assert_eq!(manifest.evaluate(screen("allow")).rule, None);
    assert_eq!(manifest.evaluate(screen("Allow")).rule.as_deref(), Some("exact"));
}

#[test]
fn the_first_of_two_equal_priority_rules_wins() {
    let manifest = Manifest::parse(&rules_manifest(
        r#"
[[rules]]
id = "first"
state = "working"
priority = 7
contains = ["x"]

[[rules]]
id = "second"
state = "blocked"
priority = 7
contains = ["x"]
"#,
    ))
    .unwrap();

    assert_eq!(manifest.evaluate(screen("x")).rule.as_deref(), Some("first"));
}

#[test]
fn a_known_agent_matching_nothing_is_idle_without_visible_evidence() {
    let manifest = Manifest::parse(&rules_manifest(
        "[[rules]]\nid = \"w\"\nstate = \"working\"\ncontains = [\"busy\"]\n",
    ))
    .unwrap();

    let detection = manifest.evaluate(screen("ordinary prompt text"));
    assert_eq!(detection, Detection::fallback(State::Idle));
}

#[test]
fn a_rule_with_no_state_means_unknown_and_visible_flags_need_their_own_state() {
    let manifest = Manifest::parse(&rules_manifest(
        "[[rules]]\nid = \"odd\"\nvisible_working = true\ncontains = [\"x\"]\n",
    ))
    .unwrap();

    let detection = manifest.evaluate(screen("x"));
    assert_eq!(detection.state, State::Unknown);
    assert!(!detection.visible);
}

#[test]
fn osc_regions_read_the_title_and_progress_rather_than_the_screen() {
    let manifest = Manifest::parse(&rules_manifest(
        r#"
[[rules]]
id = "title"
state = "working"
priority = 2
region = "osc_title"
regex = ['^\x{2800}']

[[rules]]
id = "progress"
state = "idle"
priority = 1
region = "osc_progress"
regex = ['^4;0;0$']
"#,
    ))
    .unwrap();

    let input = |title, progress| Input { screen: "⠋ 4;0;0", title, progress };
    assert_eq!(manifest.evaluate(input("⠀ task", "")).rule.as_deref(), Some("title"));
    assert_eq!(manifest.evaluate(input("", "4;0;0")).rule.as_deref(), Some("progress"));
    assert_eq!(manifest.evaluate(input("", "")).rule, None);
}

#[test]
fn manifest_validation_rejects_unknown_fields_empty_rules_invalid_regions_and_regexes() {
    for (why, rules) in [
        (
            "a misspelt matcher",
            "[[rules]]\nid = \"typo\"\nstate = \"working\"\ncontain = [\"Working\"]",
        ),
        ("a rule with no matcher", "[[rules]]\nid = \"empty\"\nstate = \"working\""),
        (
            "a misspelt region",
            "[[rules]]\nid = \"bad_region\"\nstate = \"working\"\n\
             region = \"after_last_promt_marker\"\ncontains = [\"Working\"]",
        ),
        ("a bad regex", "[[rules]]\nid = \"bad_regex\"\nstate = \"working\"\nregex = [\"[\"]"),
        (
            "a bad nested regex",
            "[[rules]]\nid = \"bad_nested_regex\"\nstate = \"working\"\n\
             any = [{ line_regex = [\"[\"] }]",
        ),
    ] {
        assert!(Manifest::parse(&rules_manifest(rules)).is_err(), "accepted {why}");
    }
    assert!(Manifest::parse("id = \"codex\"\n").is_err(), "accepted a manifest with no rules");
    assert!(
        Manifest::parse(
            "id = \"codex\"\nexecutables = [\"x\"]\n[[rules]]\nid = \"r\"\ncontains = [\"x\"]\n"
        )
        .is_err(),
        "accepted an unknown top-level key"
    );
}

#[test]
fn manifest_validation_keeps_skip_rules_neutral() {
    assert!(
        Manifest::parse(&rules_manifest(
            "[[rules]]\nid = \"bad_skip_state\"\nstate = \"idle\"\n\
             skip_state_update = true\ncontains = [\"menu\"]"
        ))
        .is_err()
    );
    assert!(
        Manifest::parse(&rules_manifest(
            "[[rules]]\nid = \"bad_skip_visible\"\nstate = \"unknown\"\n\
             skip_state_update = true\nvisible_blocker = true\ncontains = [\"menu\"]"
        ))
        .is_err()
    );
}

#[test]
fn manifest_validation_rejects_excessive_rule_count() {
    let rule = |index| format!("[[rules]]\nid = \"rule_{index}\"\ncontains = [\"ready\"]\n");
    let rules = (0..129).map(rule).collect::<Vec<_>>().join("\n");
    assert!(Manifest::parse(&rules_manifest(&rules)).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_gate_depth() {
    let manifest = r#"
id = "codex"

[[rules]]
id = "deep"
state = "idle"
contains = ["ready"]
all = [
  { contains = ["1"], all = [
    { contains = ["2"], all = [
      { contains = ["3"], all = [
        { contains = ["4"], all = [
          { contains = ["5"], all = [
            { contains = ["6"], all = [
              { contains = ["7"], all = [
                { contains = ["8"], all = [
                  { contains = ["9"] },
                ] },
              ] },
            ] },
          ] },
        ] },
      ] },
    ] },
  ] },
]
"#;
    assert!(Manifest::parse(manifest).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_matchers() {
    let matchers = (0..33).map(|index| format!("\"m{index}\"")).collect::<Vec<_>>().join(", ");
    let rules = format!("[[rules]]\nid = \"many\"\nstate = \"idle\"\ncontains = [{matchers}]");
    assert!(Manifest::parse(&rules_manifest(&rules)).is_err());
}

#[test]
fn bottom_non_empty_lines_uses_bottom_occurrence_for_repeated_text() {
    let content = "marker\nold\n\nmiddle\nmarker\nnew\n";
    assert_eq!(region(content, "bottom_non_empty_lines(2)"), "marker\nnew\n");
}

#[test]
fn top_non_empty_lines_uses_top_occurrence_for_repeated_text() {
    let content = "\nmarker\nold\n\nmiddle\nmarker\nnew\n";
    assert_eq!(region(content, "top_non_empty_lines(2)"), "\nmarker\nold\n");
}

#[test]
fn top_non_empty_lines_requires_a_canonical_positive_bounded_count() {
    let name = "top_non_empty_lines";
    assert!(Region::parse(&format!("{name}(1)")).is_some());
    assert!(Region::parse(&format!("{name}({})", u16::MAX)).is_some());
    for count in ["0", "01", "+1", "65536", "999999999999999999999999"] {
        assert!(Region::parse(&format!("{name}({count})")).is_none(), "accepted {count}");
    }
}

#[test]
fn top_non_empty_lines_requires_engine_three_when_declared() {
    let manifest = r#"
id = "grok"
version = "1"
min_engine_version = 2

[[rules]]
id = "background"
state = "working"
region = " top_non_empty_lines(1) "
contains = ["active"]
"#;
    assert!(Manifest::parse(manifest).is_err());
}

#[test]
fn prompt_box_regions_split_at_the_second_rule_from_the_bottom() {
    let content = "history\n──────\n❯ typing\n──── label\nfooter\n";
    assert_eq!(region(content, "prompt_box_body"), "❯ typing\n");
    assert_eq!(region(content, "above_prompt_box"), "history\n");
    assert_eq!(region(content, "last_non_empty_above_prompt_box"), "history");
    assert_eq!(region(content, "after_last_horizontal_rule"), "footer\n");
    assert_eq!(region("no box here\n", "prompt_box_body"), "");
}

#[test]
fn codex_prompt_regions_ignore_a_prompt_a_block_has_started_after() {
    let current = "• ran tests\nok\n› write docs\n";
    assert_eq!(region(current, "before_current_prompt_marker"), "• ran tests\nok\n");
    assert_eq!(region(current, "current_prompt_block_marker"), "• ran tests");
    assert_eq!(region(current, "after_current_prompt_block_marker"), current);
    assert_eq!(region(current, "whole_recent_without_current_prompt_marker"), "");

    let stale = "› old prompt\n• working on it\n";
    assert_eq!(region(stale, "before_current_prompt_marker"), stale);
    assert_eq!(region(stale, "after_last_prompt_marker"), "• working on it\n");
    assert_eq!(region(stale, "whole_recent_without_current_prompt_marker"), stale);
}

#[test]
fn versions_compare_numerically_with_trailing_zeros_insignificant() {
    let v = |text| Version::parse(text).unwrap();
    assert!(v("2026.06.10.1") < v("2026.07.1"));
    assert!(v("2026.06.10.10") > v("2026.06.10.9"));
    assert_eq!(v("1.2.0"), v("1.2"));
    assert!(v("1.2.1") > v("1.2"));
    assert_eq!(v("01"), v("1"));
    for bad in ["", "1..2", "1.a", "99999999999999999999999"] {
        assert!(Version::parse(bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn required_engine_is_read_without_the_strict_schema() {
    let newer = "id = \"x\"\nmin_engine_version = 9\nsomething_new = true\n";
    assert_eq!(Manifest::required_engine(newer), Some(9));
    assert!(Manifest::parse(newer).is_err());
    assert_eq!(Manifest::required_engine("id = \"x\"\n"), None);
}
