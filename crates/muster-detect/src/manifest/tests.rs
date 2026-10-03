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
fn current_prompt_is_codexs_composer_down_to_the_blank_line_above_its_footer() {
    let screen = "› fix the bug\n\n• done\n\n› half\n  typed\n\n  gpt-5 · /work\n";
    assert_eq!(region(screen, "current_prompt"), "› half\n  typed\n");
    assert_eq!(region("› just this\n", "current_prompt"), "› just this\n");
    assert_eq!(region("› old prompt\n• working on it\n", "current_prompt"), "");
    let paragraphs = "› \n\n  second paragraph\n\n  gpt-5 · /work\n";
    assert_eq!(region(paragraphs, "current_prompt"), "› \n\n  second paragraph\n");
}

#[test]
fn only_a_working_rule_with_a_prompt_reads_the_prompt_at_work() {
    let at_work = Manifest::parse(&with_prompt(6, "working", "whole_recent")).unwrap();
    let idle = Manifest::parse(&with_prompt(6, "idle", "whole_recent")).unwrap();
    assert!(at_work.reads_prompt_at_work());
    assert!(!idle.reads_prompt_at_work());
    assert!(idle.reads_prompt());
}

#[test]
fn current_prompt_needs_engine_seven() {
    let reading = |engine: u32| with_prompt(engine, "idle", "current_prompt");
    assert!(Manifest::parse(&reading(7)).is_ok());
    assert!(Manifest::parse(&reading(6)).is_err(), "as a rule's region");
    let read_in = with_prompt(6, "idle", "whole_recent")
        .replace("prompt = '^> ?'", "prompt = '^> ?'\nprompt_region = \"current_prompt\"");
    assert!(Manifest::parse(&read_in).is_err(), "as where a prompt is read");
}

#[test]
fn a_session_table_says_how_to_rename_the_session_and_needs_engine_eight() {
    let with = |engine: u32, rename: &str| {
        format!("{}\n[session]\nrename = {rename}\n", with_prompt(engine, "idle", "whole_recent"))
    };
    let manifest = Manifest::parse(&with(8, "'/rename {name}'")).unwrap();
    assert_eq!(manifest.session_rename("🤖 A").as_deref(), Some("/rename 🤖 A"));
    assert!(Manifest::parse(&with(7, "'/rename {name}'")).is_err(), "below engine 8");
    assert!(Manifest::parse(&with(8, "'/rename'")).is_err(), "without the name's place");
    assert!(Manifest::parse(&with(8, "\"/rename {name}\\n\"")).is_err(), "with a newline");
    let without = Manifest::parse(&with_prompt(8, "idle", "whole_recent")).unwrap();
    assert_eq!(without.session_rename("A"), None);
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

fn with_prompt(engine: u32, state: &str, region: &str) -> String {
    format!(
        r#"
id = "agent"
min_engine_version = {engine}

[[rules]]
id = "at_prompt"
state = "{state}"
region = "{region}"
contains = ["> "]
prompt = '^> ?'
"#
    )
}

#[test]
fn a_prompt_is_read_only_by_an_idle_rule_on_the_screen_at_engine_five() {
    assert!(Manifest::parse(&with_prompt(5, "idle", "whole_recent")).is_ok());
    assert!(Manifest::parse(&with_prompt(4, "idle", "whole_recent")).is_err(), "engine 4");
    assert!(Manifest::parse(&with_prompt(5, "working", "whole_recent")).is_err(), "working");
    assert!(Manifest::parse(&with_prompt(5, "idle", "osc_title")).is_err(), "the title");
    assert!(Manifest::parse(&with_prompt(6, "blocked", "whole_recent")).is_err(), "blocked");
}

#[test]
fn a_working_rule_and_a_prompt_region_need_engine_six_and_the_screen() {
    assert!(Manifest::parse(&with_prompt(6, "working", "whole_recent")).is_ok());
    let region = |engine: u32, prompt: &str, read_in: &str| {
        with_prompt(engine, "idle", "whole_recent")
            .replace("prompt = '^> ?'", &format!("{prompt}prompt_region = \"{read_in}\""))
    };
    let prompt = "prompt = '^> ?'\n";
    assert!(Manifest::parse(&region(6, prompt, "prompt_box_body")).is_ok());
    assert!(Manifest::parse(&region(5, prompt, "prompt_box_body")).is_err(), "engine 5");
    assert!(Manifest::parse(&region(6, prompt, "osc_title")).is_err(), "the title");
    assert!(Manifest::parse(&region(6, prompt, "nowhere")).is_err(), "no such region");
    assert!(Manifest::parse(&region(6, "", "prompt_box_body")).is_err(), "no prompt");
}

#[test]
fn a_prompt_holds_what_follows_its_marker_as_typed() {
    let manifest = Manifest::parse(&with_prompt(5, "idle", "whole_recent")).unwrap();
    let input = |screen| Input { screen, title: "", progress: "" };
    assert_eq!(manifest.prompt(input("> \n"), "> \n"), Some(Prompt::Empty));
    assert_eq!(
        manifest.prompt(input("> half  typed\n"), "> half  typed\n"),
        Some(Prompt::Holds("half typed".to_string()))
    );
    assert_eq!(manifest.prompt(input("> hint\n"), ">     \n"), Some(Prompt::Empty));
    assert_eq!(manifest.prompt(input("nothing\n"), "nothing\n"), None);
}

/// A working screen whose request is drawn above its prompt box with the box's own caret, under
/// a title that says it works: a title rule outranks the screen's, as Claude's spinner does.
const AT_WORK: &str = r#"
id = "agent"
min_engine_version = 6

[[rules]]
id = "title_working"
state = "working"
priority = 1100
region = "osc_title"
contains = ["busy"]

# Below the title rule, as Claude's Bash permission rule is: only the screen read alone sees it.
[[rules]]
id = "dialog"
state = "blocked"
priority = 980
region = "whole_recent"
contains = ["proceed?"]

[[rules]]
id = "live_turn"
state = "working"
priority = 970
region = "whole_recent"
contains = ["thinking"]
prompt = '^> ?'
prompt_region = "prompt_box_body"

[[rules]]
id = "at_prompt"
state = "idle"
priority = 950
region = "prompt_box_body"
line_regex = ['^>']
prompt = '^> ?'
"#;

#[test]
fn the_prompt_of_an_agent_at_work_is_read_in_its_box_and_never_as_the_idle_prompt() {
    fn busy(screen: &str) -> Input<'_> {
        Input { screen, title: "busy", progress: "" }
    }
    let manifest = Manifest::parse(AT_WORK).unwrap();
    let rule = "\u{2500}".repeat(10);
    let screen = |box_line: &str, above: &str| {
        format!("> the request\n{above}\n{rule}\n{box_line}\n{rule}\n")
    };
    let working = screen("> ", "thinking");
    assert_eq!(manifest.prompt_at_work(busy(&working), &working), Some(Prompt::Empty));
    assert_eq!(manifest.prompt(busy(&working), &working), None, "read as the idle prompt");

    let draft = screen("> half typed", "thinking");
    assert_eq!(
        manifest.prompt_at_work(busy(&draft), &draft),
        Some(Prompt::Holds("half typed".to_string()))
    );

    // The title says it works, and only the title: the screen is its idle prompt, or a dialog.
    let idle = screen("> ", "");
    assert_eq!(manifest.prompt_at_work(busy(&idle), &idle), None, "an idle screen");
    let dialog = screen("> 1. Yes", "thinking\nproceed?");
    assert_eq!(manifest.prompt_at_work(busy(&dialog), &dialog), None, "a dialog");

    // The screen works, and the title says nothing: still at work.
    let quiet = Input { screen: &working, title: "", progress: "" };
    assert_eq!(manifest.prompt_at_work(quiet, &working), Some(Prompt::Empty));
}
