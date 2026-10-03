//! One agent's detection manifest: what it is called, and the rules that read its screen.
//!
//! Ported from herdr v0.8.0 `src/detect/manifest.rs` and the version type in
//! `src/detect/manifest_update.rs` (Apache-2.0), and changed: the loader, remote catalog and
//! `agent.explain` are gone, a rule's region is parsed once when the manifest is compiled
//! rather than on every evaluation, and a manifest may name `script_paths`.
//!
//! What a manifest means is this file. herdr's manifests are data written against herdr's
//! engine, so each thing here - how a region is sliced, that `contains` is case-folded and
//! `line_regex` per line, that the first of two equal-priority rules wins, that a known agent
//! with nothing matching is idle - is reproduced exactly rather than improved.

mod region;

use std::cmp::Ordering;
use std::fmt;

use regex::Regex;
use serde::Deserialize;

use crate::State;
use region::Region;

/// The manifest engine this crate implements. 1 to 3 are herdr's, so herdr's manifests keep
/// the gates they were written with; 4 is herdr's engine 3 plus `script_paths`, 5 adds a
/// rule's `prompt`, 6 lets a working rule carry one, read where its `prompt_region` says, and
/// 7 adds the `current_prompt` region, Codex's composer alone.
pub const ENGINE_VERSION: u32 = 7;

/// The engine version that introduced a rule's `prompt`.
const PROMPT_ENGINE_VERSION: u32 = 5;

/// The engine version that introduced a working rule's `prompt`, and `prompt_region`.
const PROMPT_AT_WORK_ENGINE_VERSION: u32 = 6;

/// The engine version that introduced the `current_prompt` region.
const CURRENT_PROMPT_ENGINE_VERSION: u32 = 7;

/// The engine version that introduced the `top_non_empty_lines` region, in herdr.
const TOP_NON_EMPTY_LINES_ENGINE_VERSION: u32 = 3;

const MAX_RULES_PER_MANIFEST: usize = 128;
const MAX_GATE_DEPTH: usize = 8;
const MAX_TOTAL_GATES: usize = 512;
const MAX_MATCHERS_PER_GATE: usize = 32;
const MAX_TOTAL_MATCHERS: usize = 1024;
const MAX_MATCHER_CHARS: usize = 512;

/// What a manifest is evaluated against. Each part is empty when there is nothing to say,
/// which matches exactly as herdr's engine did before it read OSC at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct Input<'a> {
    /// The detection text (`crate::screen_text`).
    pub screen: &'a str,
    /// The terminal's title, normalized (`crate::title`).
    pub title: &'a str,
    /// The last OSC 9 payload, raw (`crate::Progress`).
    pub progress: &'a str,
}

/// A manifest's verdict on one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub state: State,
    /// The state was read off chrome the agent draws for exactly that state - a prompt box, a
    /// permission menu, a spinner - so it needs no confirming. herdr kept a flag per state
    /// (`visible_idle`, `visible_blocker`, `visible_working`), each true only alongside its own
    /// state, which is this one flag.
    pub visible: bool,
    /// A rule matched that says the screen is something to wait out - a transcript viewer,
    /// a picker - rather than evidence of any state.
    pub skip_state_update: bool,
    /// The id of the rule that decided, or none for the idle fallback and unknown agents.
    pub rule: Option<String>,
}

/// What an agent's prompt holds, read off its screen ([`Manifest::prompt`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    Empty,
    /// Text someone or something typed, its runs of whitespace one space each, a prompt
    /// wrapped over several lines read as one.
    Holds(String),
}

impl Detection {
    /// What a pane with no identified agent is.
    pub fn unknown() -> Self {
        Detection::fallback(State::Unknown)
    }

    pub(crate) fn fallback(state: State) -> Self {
        Detection { state, visible: false, skip_state_update: false, rule: None }
    }
}

/// A manifest that parsed, validated and compiled.
#[derive(Debug, Clone)]
pub struct Manifest {
    id: String,
    version: Option<Version>,
    aliases: Vec<String>,
    script_paths: Vec<String>,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
struct Rule {
    id: String,
    state: State,
    priority: i32,
    region: Region,
    visible: bool,
    skip_state_update: bool,
    gate: Gate,
    /// Where the prompt's text starts on its line, for a rule that decides the screen is the
    /// agent at its prompt, or at work with its prompt showing.
    prompt: Option<Regex>,
    /// Where on the screen the prompt is read, when not the rule's own region.
    prompt_region: Option<Region>,
}

#[derive(Debug, Clone)]
struct Gate {
    all: Vec<Gate>,
    any: Vec<Gate>,
    not: Vec<Gate>,
    /// Lowercased, since the text they are looked for in is.
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
}

impl Manifest {
    /// Parses, validates and compiles a manifest. The error is a sentence about the manifest,
    /// for a log line that also names where it came from.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let raw = toml::from_str::<RawManifest>(text).map_err(|error| error.to_string())?;
        validate(&raw)?;
        compile(raw)
    }

    /// The engine a manifest says it needs, read without holding it to this engine's schema -
    /// so that a manifest written for a newer engine is refused for that, rather than for
    /// whichever of its new keys the strict parse trips on first.
    pub fn required_engine(text: &str) -> Option<u32> {
        let table = toml::from_str::<toml::Table>(text).ok()?;
        u32::try_from(table.get("min_engine_version")?.as_integer()?).ok()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> Option<&Version> {
        self.version.as_ref()
    }

    /// The names the agent answers to besides its id.
    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Package paths that identify the agent when a runtime runs a script inside one.
    pub fn script_paths(&self) -> &[String] {
        &self.script_paths
    }

    /// Every rule is evaluated; the highest priority that matches wins, the earlier of two
    /// equal ones; and a known agent that matches nothing is idle.
    pub fn evaluate(&self, input: Input<'_>) -> Detection {
        let Some(rule) = self.decide(input) else {
            return Detection::fallback(State::Idle);
        };
        Detection {
            state: rule.state,
            visible: rule.visible,
            skip_state_update: rule.skip_state_update,
            rule: Some(rule.id.clone()),
        }
    }

    /// Whether any rule can say the screen is the agent at its prompt: an agent whose manifest
    /// has none is never read as at an empty prompt.
    pub fn reads_prompt(&self) -> bool {
        self.rules.iter().any(|rule| rule.prompt.is_some())
    }

    /// Whether a rule can read the prompt of the agent at work, which takes what is typed there
    /// into its running turn: what an urgent ring needs.
    pub fn reads_prompt_at_work(&self) -> bool {
        self.rules.iter().any(|rule| rule.prompt.is_some() && rule.state == State::Working)
    }

    /// What the agent's prompt holds, when the rule that decides the screen is one that says
    /// it is idle at its prompt; none when it is anything else - a dialog, a menu, at work, or
    /// nothing a rule recognizes.
    ///
    /// `typed` is the same screen with every cell nobody typed blanked - a suggestion drawn
    /// faint, say - and has a line for each of `input.screen`'s, with as many characters. The
    /// rule is decided and its region found on the screen as drawn, since what frames a prompt
    /// may be drawn faint too; only the prompt's own text is read from `typed`.
    pub fn prompt(&self, input: Input<'_>, typed: &str) -> Option<Prompt> {
        let rule = self.decide(input).filter(|rule| rule.state == State::Idle)?;
        read_prompt(rule, input, typed)
    }

    /// What the prompt holds of an agent at work, for an agent that takes what is typed while
    /// it works into the turn it is running; none when the screen is anything else.
    ///
    /// Two rules have to agree. The one deciding the screen with its title says the agent is
    /// working, and the one deciding the screen alone, title and progress left out, is a
    /// working rule that says where the prompt is. The second is what keeps a dialog, a menu
    /// or a viewer from being read as the prompt: a title says what the agent is doing, never
    /// what a keystroke would land in, so a title rule outranking them must not decide here.
    pub fn prompt_at_work(&self, input: Input<'_>, typed: &str) -> Option<Prompt> {
        self.decide(input).filter(|rule| rule.state == State::Working)?;
        let screen = Input { screen: input.screen, title: "", progress: "" };
        let rule = self.decide(screen).filter(|rule| rule.state == State::Working)?;
        read_prompt(rule, screen, typed)
    }

    /// The rule that decides: every rule is evaluated, and the highest priority that matches
    /// wins, the earlier of two equal ones.
    fn decide(&self, input: Input<'_>) -> Option<&Rule> {
        let mut matched: Option<&Rule> = None;
        for rule in &self.rules {
            if !rule.gate.matches_text(rule.region.slice(input)) {
                continue;
            }
            match matched {
                Some(previous) if previous.priority >= rule.priority => {}
                _ => matched = Some(rule),
            }
        }
        matched
    }
}

/// What the prompt holds, on a screen `rule` decides: read from the line its marker is found
/// on, in its prompt region, to the region's end.
fn read_prompt(rule: &Rule, input: Input<'_>, typed: &str) -> Option<Prompt> {
    let marker = rule.prompt.as_ref()?;
    let region = rule.prompt_region.unwrap_or(rule.region).slice(input);
    let screen = input.screen;
    let start = (region.as_ptr() as usize).checked_sub(screen.as_ptr() as usize)?;
    if start + region.len() > screen.len() {
        return None;
    }
    let first = screen[..start].matches('\n').count();
    let count = region.lines().count();
    let drawn: Vec<&str> = screen.lines().skip(first).take(count).collect();
    let typed: Vec<&str> = typed.lines().skip(first).take(count).collect();
    let (at, found) = drawn.iter().enumerate().find_map(|(at, line)| {
        marker.find(line).map(|found| (at, line[..found.end()].chars().count()))
    })?;
    let rest: String =
        typed.get(at).map_or(String::new(), |line| line.chars().skip(found).collect());
    let held: Vec<&str> = std::iter::once(rest.as_str())
        .chain(typed.iter().skip(at + 1).copied())
        .flat_map(str::split_whitespace)
        .collect();
    let joined = held.join(" ");
    Some(if joined.is_empty() { Prompt::Empty } else { Prompt::Holds(joined) })
}

impl Gate {
    fn matches_text(&self, text: &str) -> bool {
        self.matches(text, &text.to_lowercase())
    }

    fn matches(&self, text: &str, lower: &str) -> bool {
        self.contains.iter().all(|needle| lower.contains(needle.as_str()))
            && self.regex.iter().all(|regex| regex.is_match(text))
            && self.line_regex.iter().all(|regex| text.lines().any(|line| regex.is_match(line)))
            && self.all.iter().all(|nested| nested.matches(text, lower))
            && (self.any.is_empty() || self.any.iter().any(|nested| nested.matches(text, lower)))
            && !self.not.iter().any(|nested| nested.matches(text, lower))
    }
}

/// A manifest's version: dotted numbers, compared numerically, with trailing zero segments
/// insignificant - so `2026.06.10.1` is older than `2026.07.1` and `1.2.0` equals `1.2`.
#[derive(Debug, Clone)]
pub struct Version(String);

impl Version {
    pub fn parse(value: &str) -> Result<Version, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("version must not be empty".to_string());
        }
        for segment in trimmed.split('.') {
            if segment.is_empty() {
                return Err(format!("version {trimmed:?} contains an empty segment"));
            }
            if !segment.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(format!("version {trimmed:?} must be dotted numeric"));
            }
            segment
                .parse::<u64>()
                .map_err(|_| format!("version {trimmed:?} contains an oversized segment"))?;
        }
        Ok(Version(trimmed.to_string()))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Version::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // Every segment was checked to parse when the version was.
        let number = |segment: &str| segment.parse::<u64>().unwrap_or(0);
        let mut left = self.0.split('.');
        let mut right = other.0.split('.');
        loop {
            match (left.next(), right.next()) {
                (Some(l), Some(r)) => match number(l).cmp(&number(r)) {
                    Ordering::Equal => {}
                    ordering => return ordering,
                },
                (Some(l), None) if number(l) != 0 => return Ordering::Greater,
                (None, Some(r)) if number(r) != 0 => return Ordering::Less,
                (Some(_), None) | (None, Some(_)) => {}
                (None, None) => return Ordering::Equal,
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    id: String,
    version: Option<Version>,
    min_engine_version: Option<u32>,
    #[serde(rename = "updated_at")]
    _updated_at: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    script_paths: Vec<String>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(clippy::struct_excessive_bools, reason = "these are the manifest's own keys")]
struct RawRule {
    id: String,
    state: Option<RawState>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_region")]
    region: String,
    #[serde(default)]
    visible_idle: bool,
    #[serde(default)]
    visible_blocker: bool,
    #[serde(default)]
    visible_working: bool,
    #[serde(default)]
    skip_state_update: bool,
    // The gate's fields, spelled out rather than flattened: serde cannot deny unknown fields
    // through a flatten, and a misspelt matcher has to be refused rather than never match.
    #[serde(default)]
    all: Vec<RawGate>,
    #[serde(default)]
    any: Vec<RawGate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<RawGate>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    prompt: Option<String>,
    prompt_region: Option<String>,
}

impl RawRule {
    fn gate(&self) -> RawGate {
        RawGate {
            all: self.all.clone(),
            any: self.any.clone(),
            not_gate: self.not_gate.clone(),
            contains: self.contains.clone(),
            regex: self.regex.clone(),
            line_regex: self.line_regex.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGate {
    #[serde(default)]
    all: Vec<RawGate>,
    #[serde(default)]
    any: Vec<RawGate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<RawGate>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RawState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

impl From<RawState> for State {
    fn from(value: RawState) -> Self {
        match value {
            RawState::Idle => State::Idle,
            RawState::Working => State::Working,
            RawState::Blocked => State::Blocked,
            RawState::Unknown => State::Unknown,
        }
    }
}

fn default_region() -> String {
    "whole_recent".to_string()
}

fn validate(manifest: &RawManifest) -> Result<(), String> {
    if manifest.rules.is_empty() {
        return Err("manifest must contain at least one rule".to_string());
    }
    if manifest.rules.len() > MAX_RULES_PER_MANIFEST {
        return Err(format!(
            "manifest contains {} rules, max is {MAX_RULES_PER_MANIFEST}",
            manifest.rules.len()
        ));
    }

    let mut complexity = Complexity::default();
    for rule in &manifest.rules {
        if rule.id.trim().is_empty() {
            return Err("manifest rule id must not be empty".to_string());
        }
        if rule.skip_state_update {
            if rule.state != Some(RawState::Unknown) {
                return Err(format!(
                    "rule {} uses skip_state_update without state = \"unknown\"",
                    rule.id
                ));
            }
            if rule.visible_idle || rule.visible_blocker || rule.visible_working {
                return Err(format!(
                    "rule {} uses skip_state_update with visible state evidence",
                    rule.id
                ));
            }
        }
        let region = Region::parse(&rule.region).ok_or_else(|| {
            format!("rule {} uses invalid region: {}", rule.id, rule.region.trim())
        })?;
        if matches!(region, Region::TopNonEmptyLines(_))
            && manifest
                .min_engine_version
                .is_some_and(|version| version < TOP_NON_EMPTY_LINES_ENGINE_VERSION)
        {
            return Err(format!(
                "rule {} uses top_non_empty_lines but min_engine_version is below \
                 {TOP_NON_EMPTY_LINES_ENGINE_VERSION}",
                rule.id
            ));
        }
        reads_current_prompt(manifest, rule, region)?;
        if rule.prompt.is_some() || rule.prompt_region.is_some() {
            validate_prompt(manifest, rule, region)?;
        }
        validate_gate(&rule.gate(), "rule", 0, &mut complexity)
            .map_err(|error| format!("rule {} has invalid matcher gates: {error}", rule.id))?;
    }
    Ok(())
}

/// An engine before 7 does not know `current_prompt`, so a manifest using it says it needs 7.
fn reads_current_prompt(
    manifest: &RawManifest,
    rule: &RawRule,
    region: Region,
) -> Result<(), String> {
    if region == Region::CurrentPrompt
        && manifest.min_engine_version.unwrap_or(0) < CURRENT_PROMPT_ENGINE_VERSION
    {
        return Err(format!(
            "rule {} uses current_prompt but min_engine_version is below \
             {CURRENT_PROMPT_ENGINE_VERSION}",
            rule.id
        ));
    }
    Ok(())
}

/// A prompt is read off the screen, and only a rule that says the agent is idle, or at work
/// and taking what is typed into its turn, can say where it is.
fn validate_prompt(manifest: &RawManifest, rule: &RawRule, region: Region) -> Result<(), String> {
    let engine = manifest.min_engine_version.unwrap_or(0);
    if rule.prompt.is_none() {
        return Err(format!("rule {} uses prompt_region without prompt", rule.id));
    }
    if engine < PROMPT_ENGINE_VERSION {
        return Err(format!(
            "rule {} uses prompt but min_engine_version is below {PROMPT_ENGINE_VERSION}",
            rule.id
        ));
    }
    let at_work = rule.state == Some(RawState::Working);
    if !(rule.state == Some(RawState::Idle) || at_work) || rule.skip_state_update {
        return Err(format!(
            "rule {} uses prompt without state = \"idle\" or \"working\"",
            rule.id
        ));
    }
    if (at_work || rule.prompt_region.is_some()) && engine < PROMPT_AT_WORK_ENGINE_VERSION {
        return Err(format!(
            "rule {} uses prompt at work or prompt_region but min_engine_version is below \
             {PROMPT_AT_WORK_ENGINE_VERSION}",
            rule.id
        ));
    }
    let read_in = match &rule.prompt_region {
        Some(text) => Region::parse(text).ok_or_else(|| {
            format!("rule {} uses invalid prompt_region: {}", rule.id, text.trim())
        })?,
        None => region,
    };
    reads_current_prompt(manifest, rule, read_in)?;
    if matches!(read_in, Region::OscTitle | Region::OscProgress) {
        return Err(format!(
            "rule {} reads its prompt from a region that is not the screen",
            rule.id
        ));
    }
    Ok(())
}

#[derive(Default)]
struct Complexity {
    total_gates: usize,
    total_matchers: usize,
}

fn validate_gate(
    gate: &RawGate,
    context: &str,
    depth: usize,
    complexity: &mut Complexity,
) -> Result<(), String> {
    if depth > MAX_GATE_DEPTH {
        return Err(format!("{context} exceeds max gate depth {MAX_GATE_DEPTH}"));
    }
    count_gate(complexity)?;
    validate_matcher_limits(gate, context, complexity)?;
    if !gate.has_positive_matcher() {
        return Err(format!("{context} must contain a positive matcher"));
    }
    validate_regex_patterns(&gate.regex, context, "regex")?;
    validate_regex_patterns(&gate.line_regex, context, "line_regex")?;
    for nested in &gate.all {
        validate_gate(nested, "all gate", depth + 1, complexity)?;
    }
    for nested in &gate.any {
        validate_gate(nested, "any gate", depth + 1, complexity)?;
    }
    for nested in &gate.not_gate {
        if !nested.has_any_matcher() {
            return Err(format!("{context} contains an empty not gate"));
        }
        validate_not_gate(nested, depth + 1, complexity)?;
    }
    Ok(())
}

/// A `not` gate may hold only further `not`s, which a positive gate may not.
fn validate_not_gate(
    gate: &RawGate,
    depth: usize,
    complexity: &mut Complexity,
) -> Result<(), String> {
    if depth > MAX_GATE_DEPTH {
        return Err(format!("not gate exceeds max gate depth {MAX_GATE_DEPTH}"));
    }
    count_gate(complexity)?;
    validate_matcher_limits(gate, "not gate", complexity)?;
    if !gate.has_any_matcher() {
        return Err("not gate must contain a matcher".to_string());
    }
    validate_regex_patterns(&gate.regex, "not gate", "regex")?;
    validate_regex_patterns(&gate.line_regex, "not gate", "line_regex")?;
    for nested in &gate.all {
        validate_gate(nested, "not all gate", depth + 1, complexity)?;
    }
    for nested in &gate.any {
        validate_gate(nested, "not any gate", depth + 1, complexity)?;
    }
    for nested in &gate.not_gate {
        validate_not_gate(nested, depth + 1, complexity)?;
    }
    Ok(())
}

fn count_gate(complexity: &mut Complexity) -> Result<(), String> {
    complexity.total_gates += 1;
    if complexity.total_gates > MAX_TOTAL_GATES {
        return Err(format!("manifest exceeds max gate count {MAX_TOTAL_GATES}"));
    }
    Ok(())
}

fn validate_matcher_limits(
    gate: &RawGate,
    context: &str,
    complexity: &mut Complexity,
) -> Result<(), String> {
    let matcher_count = gate.contains.len() + gate.regex.len() + gate.line_regex.len();
    if matcher_count > MAX_MATCHERS_PER_GATE {
        return Err(format!(
            "{context} has {matcher_count} direct matchers, max is {MAX_MATCHERS_PER_GATE}"
        ));
    }
    complexity.total_matchers += matcher_count;
    if complexity.total_matchers > MAX_TOTAL_MATCHERS {
        return Err(format!("manifest exceeds max matcher count {MAX_TOTAL_MATCHERS}"));
    }
    if gate
        .contains
        .iter()
        .chain(&gate.regex)
        .chain(&gate.line_regex)
        .any(|value| value.chars().count() > MAX_MATCHER_CHARS)
    {
        return Err(format!("{context} matcher exceeds max length {MAX_MATCHER_CHARS}"));
    }
    Ok(())
}

fn validate_regex_patterns(patterns: &[String], context: &str, field: &str) -> Result<(), String> {
    for pattern in patterns {
        Regex::new(pattern).map_err(|error| {
            format!("{context} contains invalid {field} pattern {pattern:?}: {error}")
        })?;
    }
    Ok(())
}

impl RawGate {
    fn has_positive_matcher(&self) -> bool {
        !self.contains.is_empty()
            || !self.regex.is_empty()
            || !self.line_regex.is_empty()
            || !self.all.is_empty()
            || !self.any.is_empty()
    }

    fn has_any_matcher(&self) -> bool {
        self.has_positive_matcher() || !self.not_gate.is_empty()
    }
}

fn compile(raw: RawManifest) -> Result<Manifest, String> {
    let rules =
        raw.rules
            .into_iter()
            .map(|rule| {
                let gate = compile_gate(&rule.gate())
                    .map_err(|error| format!("rule {} could not be compiled: {error}", rule.id))?;
                let region = Region::parse(&rule.region).ok_or_else(|| {
                    format!("rule {} uses invalid region: {}", rule.id, rule.region)
                })?;
                let state = rule.state.map_or(State::Unknown, State::from);
                let prompt =
                    rule.prompt.as_deref().map(Regex::new).transpose().map_err(|error| {
                        format!("rule {} has an invalid prompt: {error}", rule.id)
                    })?;
                let prompt_region = rule
                    .prompt_region
                    .as_deref()
                    .map(|text| {
                        Region::parse(text).ok_or_else(|| {
                            format!("rule {} uses invalid prompt_region: {text}", rule.id)
                        })
                    })
                    .transpose()?;
                Ok(Rule {
                    prompt,
                    prompt_region,
                    state,
                    priority: rule.priority,
                    region,
                    visible: match state {
                        State::Idle => rule.visible_idle,
                        State::Blocked => rule.visible_blocker,
                        State::Working => rule.visible_working,
                        State::Unknown => false,
                    },
                    skip_state_update: rule.skip_state_update,
                    gate,
                    id: rule.id,
                })
            })
            .collect::<Result<_, String>>()?;
    Ok(Manifest {
        id: raw.id,
        version: raw.version,
        aliases: raw.aliases,
        script_paths: raw.script_paths,
        rules,
    })
}

fn compile_gate(gate: &RawGate) -> Result<Gate, String> {
    let compile_all =
        |gates: &[RawGate]| gates.iter().map(compile_gate).collect::<Result<Vec<_>, _>>();
    let compile_regex = |patterns: &[String]| {
        patterns
            .iter()
            .map(|pattern| Regex::new(pattern).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(Gate {
        all: compile_all(&gate.all)?,
        any: compile_all(&gate.any)?,
        not: compile_all(&gate.not_gate)?,
        contains: gate.contains.iter().map(|needle| needle.to_lowercase()).collect(),
        regex: compile_regex(&gate.regex)?,
        line_regex: compile_regex(&gate.line_regex)?,
    })
}

#[cfg(test)]
mod tests;
