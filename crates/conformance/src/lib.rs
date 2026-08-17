//! A corpus of cases, and the Rust driver that runs them.
//!
//! The core's behavior is defined by data rather than by any language's tests, so a core
//! rewritten in another language is verified by cases a working implementation already
//! passed (MIP-1, `docs/testing.md`). There was briefly a Swift twin of this file, and the
//! rule it existed under still holds for the next one: two drivers that validate differently
//! would let a corpus mean two things, which is the failure this whole arrangement exists to
//! prevent.
//!
//! Everything it refuses to load is deliberate. A corpus that silently accepts a case with
//! no stated reason, or a file that does not say where its expectations came from, is a
//! corpus that decays into a record of whatever the implementation happened to do.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// How much a file's expectations are worth trusting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Captured from a real herdr, a real libghostty-vt, a real terminal. Re-derivable.
    Recorded,
    /// Lifted from an existing suite: trusted exactly as far as that implementation was.
    Ported,
    /// Our own policy, with a citation. No oracle beyond the citation.
    Authored,
}

impl Source {
    fn parse(name: &str) -> Option<Source> {
        match name {
            "recorded" => Some(Source::Recorded),
            "ported" => Some(Source::Ported),
            "authored" => Some(Source::Authored),
            _ => None,
        }
    }
}

/// The width a file's numbers are compared at.
///
/// JSON has one number type and an implementation has several, so a case written `0.05`
/// against a quantity the wire carries as an `f32` never matches: the driver's answer comes
/// back widened as `0.05000000074505806`, and the only decimals that survive are the ones
/// binary can spell exactly. Writing the long form into the file would fix the comparison
/// and cost the corpus the thing it is for, which is being text a reviewer can read.
///
/// So a file whose subject is narrower than JSON says so, and both sides are narrowed before
/// they are compared. It is stated per file rather than assumed everywhere because narrowing
/// is a loss: under `f32`, two `f64`s that differ only past the seventh digit compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numbers {
    /// JSON's own width, and what a file that says nothing gets.
    F64,
    /// The width herdr's ratios and amounts reach the wire at.
    F32,
}

impl Numbers {
    fn parse(name: &str) -> Option<Numbers> {
        match name {
            "f64" => Some(Numbers::F64),
            "f32" => Some(Numbers::F32),
            _ => None,
        }
    }

    /// Whether two numbers are one number at this width.
    ///
    /// Exact equality at the declared width rather than a tolerance: two spellings of one
    /// number, not two numbers that are close. A tolerance would make the corpus accept a
    /// wrong answer.
    #[allow(clippy::float_cmp)]
    fn same(self, left: f64, right: f64) -> bool {
        match self {
            Numbers::F64 => left == right,
            // The narrowing the wire already did to one side, applied to both so the
            // comparison happens where the value actually lives.
            #[allow(clippy::cast_possible_truncation)]
            Numbers::F32 => left as f32 == right as f32,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub why: String,
    pub given: Value,
    pub expect: Value,
}

#[derive(Debug)]
pub struct Conformance {
    pub file: String,
    pub concept: String,
    pub source: Source,
    pub numbers: Numbers,
    pub cases: Vec<Case>,
    /// Data a file carries that is not cases, for the drivers that read it.
    ///
    /// Some behavior is one matrix with one reason rather than N behaviors with N
    /// justifications, and the honest shape for that is a rendered snapshot. But the inputs
    /// to a rendering are still the corpus's to state: left in a driver, the next language
    /// re-types them, and a snapshot both languages agree on says nothing about a list only
    /// one of them has. So a file may carry one beside its cases, and the `why` rule above
    /// stays about `cases` alone - a survey states its reason once, which is the whole
    /// argument for it not being cases.
    ///
    /// Unvalidated here beyond existing: what shape a survey has is its driver's business,
    /// and tools/corpus-lint.py is the language-neutral authority on the rest.
    pub survey: Option<Value>,
}

/// Why a case could not be evaluated, as opposed to evaluating to the wrong answer.
///
/// Kept distinct in the report: a driver that cannot run a case has proved nothing about
/// the behavior, and reading that as a failing behavior sends someone after the wrong bug.
#[derive(Debug)]
pub struct CaseError(pub String);

impl CaseError {
    pub fn new(detail: impl Into<String>) -> CaseError {
        CaseError(detail.into())
    }
}

impl fmt::Display for CaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a driver returns for one case.
pub type Outcome = Result<Value, CaseError>;

impl Conformance {
    /// Loads `corpus/conformance/<file>`, or panics saying exactly what is wrong with it.
    ///
    /// Panicking rather than returning an error: every caller is a test whose only
    /// response would be to fail, and a corpus that will not load is a repo problem
    /// rather than a behavior under test.
    pub fn load(file: &str) -> Conformance {
        let path = corpus_dir().join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("conformance corpus {file} could not be read: {e}"));
        Conformance::parse(file, &text)
    }

    /// Validates one corpus document, whatever it was read from.
    ///
    /// Split out from `load` so that every refusal below is itself testable. A guard that
    /// has never been seen to fire is a guard nobody knows the shape of, and this one
    /// decides whether a checked-in file counts as coverage.
    pub fn parse(file: &str, text: &str) -> Conformance {
        let document: Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("conformance corpus {file} is malformed: not JSON ({e})"));

        let concept =
            non_empty(&document, "concept").unwrap_or_else(|| malformed(file, "no `concept`"));
        let source_name = document.get("source").and_then(Value::as_str).unwrap_or("");
        let source = Source::parse(source_name).unwrap_or_else(|| {
            malformed(
                file,
                "`source` must be one of recorded, ported, authored - it says how far these \
                 expectations can be trusted, and a file without one cannot be judged",
            )
        });
        // A recorded file that cannot be re-derived is indistinguishable from an authored
        // one claiming provenance it does not have.
        if source == Source::Recorded && non_empty(&document, "regenerate").is_none() {
            malformed(
                file,
                "a `recorded` corpus must carry the `regenerate` command that produced it, \
                 or its provenance is a claim rather than a fact",
            );
        }
        if non_empty(&document, "why").is_none() {
            malformed(file, "no file-level `why`");
        }
        let numbers = match document.get("numbers") {
            None => Numbers::F64,
            Some(declared) => {
                let name = declared.as_str().unwrap_or("");
                Numbers::parse(name).unwrap_or_else(|| {
                    malformed(
                        file,
                        "`numbers` must be f64 or f32 - it is the width this file's \
                         expectations are compared at, and a file that names another one is \
                         asking for a comparison no driver can make",
                    )
                })
            }
        };

        let raw_cases = document.get("cases").and_then(Value::as_array).filter(|c| !c.is_empty());
        let raw_cases = raw_cases.unwrap_or_else(|| {
            malformed(
                file,
                "no cases. An empty corpus passes every driver, which reads as coverage and \
                 is not",
            )
        });

        let mut cases = Vec::with_capacity(raw_cases.len());
        let mut names: Vec<&str> = Vec::with_capacity(raw_cases.len());
        for (index, raw) in raw_cases.iter().enumerate() {
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| malformed(file, &format!("case {index} has no `name`")));
            if names.contains(&name) {
                malformed(
                    file,
                    &format!("two cases named `{name}`, so a failure could not say which"),
                );
            }
            names.push(name);
            // The load-bearing rule. The comments in the tests these came from are the
            // best documentation in the repo, and a table row does not carry them by
            // default.
            let why =
                raw.get("why").and_then(Value::as_str).filter(|w| !w.is_empty()).unwrap_or_else(
                    || {
                        malformed(
                            file,
                            &format!(
                                "case `{name}` has no `why`. A case that does not say what it \
                             protects cannot be judged when it fails, and gets deleted by \
                             whoever it inconveniences"
                            ),
                        )
                    },
                );
            let (given, expect) = match (raw.get("given"), raw.get("expect")) {
                (Some(given), Some(expect)) => (given.clone(), expect.clone()),
                _ => malformed(file, &format!("case `{name}` has no `given`/`expect`")),
            };
            cases.push(Case { name: name.to_string(), why: why.to_string(), given, expect });
        }

        let survey = document.get("survey").cloned();
        Conformance { file: file.to_string(), concept, source, numbers, cases, survey }
    }

    /// Runs every case through `subject` and compares what comes back to `expect`.
    ///
    /// Every case runs before anything fails, so one run reports every disagreement rather
    /// than the first. A port fixes them in batches; reporting one at a time turns that
    /// into a loop of full rebuilds.
    ///
    /// Returns how many cases ran, so a driver can assert it rather than assume it.
    pub fn run(&self, mut subject: impl FnMut(&Value) -> Outcome) -> usize {
        let mut failures: Vec<String> = Vec::new();
        for case in &self.cases {
            match subject(&case.given) {
                Err(error) => failures.push(self.report(case, None, Some(&error))),
                Ok(actual) => {
                    if !equivalent(&actual, &case.expect, self.numbers) {
                        failures.push(self.report(case, Some(&actual), None));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "\n{}\n", failures.join("\n\n"));
        self.cases.len()
    }

    /// What a reader sees when a case fails.
    ///
    /// The `why` is in here on purpose: a failure is the one moment someone needs to know
    /// what the case was protecting, and it is the moment they are least inclined to go
    /// looking for it.
    fn report(&self, case: &Case, actual: Option<&Value>, error: Option<&CaseError>) -> String {
        let mut lines = vec![
            format!("{} · {}", self.file, case.name),
            format!("  why:      {}", case.why),
            format!("  given:    {}", case.given),
            format!("  expected: {}", case.expect),
        ];
        if let Some(actual) = actual {
            lines.push(format!("  actual:   {actual}"));
        }
        if let Some(error) = error {
            lines.push(format!("  the driver could not run this case: {error}"));
            lines.push(
                "  That is a corpus or driver problem rather than a failing behavior - the \
                 case was never evaluated."
                    .to_string(),
            );
        }
        if self.source == Source::Ported {
            lines.push(
                "  This corpus is `ported`, so it is trusted only as far as the \
                 implementation it came from. If this expectation is the thing that is \
                 wrong, fix it from a recording or from the dependency's source - never by \
                 matching whichever implementation is in front of you."
                    .to_string(),
            );
        }
        lines.join("\n")
    }
}

/// Builds an object, dropping the absent fields.
///
/// The twin of Swift's `JSONValue.fields`. A case's `expect` states what it cares about,
/// so a field that does not apply must be missing rather than null - otherwise every
/// corpus row grows nulls for the shapes it is not about.
pub fn fields<const N: usize>(entries: [(&str, Option<Value>); N]) -> Value {
    let mut map = Map::new();
    for (name, value) in entries {
        if let Some(value) = value {
            map.insert(name.to_string(), value);
        }
    }
    Value::Object(map)
}

/// Bytes as the corpus spells them: lowercase hex, no separators.
///
/// Here rather than in each driver because `bytes_hex` is corpus vocabulary - three files
/// use it - and two drivers rendering it two ways would be a disagreement about the format
/// rather than about the behavior.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The strings at `key`, or none. Absent and empty are the same answer here, as in Swift.
pub fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Compares two documents the way the Swift driver does.
///
/// serde_json holds `3` and `3.0` as different values; Swift's `JSONValue` does not. That
/// difference belongs to the two JSON libraries rather than to Muster, and letting it
/// through would fail half the corpus on number shape rather than on behavior - which
/// reads as a port that broke something when nothing broke.
fn equivalent(left: &Value, right: &Value, numbers: Numbers) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => numbers.same(a, b),
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b, numbers))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, a)| b.get(key).is_some_and(|b| equivalent(a, b, numbers)))
        }
        _ => left == right,
    }
}

fn non_empty(document: &Value, key: &str) -> Option<String> {
    document.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string)
}

fn malformed(file: &str, detail: &str) -> ! {
    panic!("conformance corpus {file} is malformed: {detail}");
}

/// Walks up from this crate to the checkout, looking for the corpus itself.
///
/// By what it is looking for rather than by a marker file: the answer is the same from
/// either language's tree, and it stays right if the build system underneath changes.
fn corpus_dir() -> PathBuf {
    repo_root().join("corpus/conformance")
}

/// The checkout this test is running inside.
///
/// Found by walking up from the crate rather than from the working directory, which cargo
/// sets per-crate. Public because some drivers need the rest of the corpus: the cases that
/// pin what Muster sends a daemon are only worth as much as the recorded schema they are
/// checked against, and that lives beside `conformance/` rather than in it.
pub fn repo_root() -> PathBuf {
    let mut directory: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        if directory.join("corpus/conformance").is_dir() {
            return directory.to_path_buf();
        }
        directory = directory.parent().unwrap_or_else(|| {
            panic!(
                "no corpus/conformance directory above {}. The corpus is the definition of \
                 what the core does, so without it a driver verifies nothing.",
                env!("CARGO_MANIFEST_DIR")
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A document that loads, so each case below can spoil exactly one thing.
    fn good() -> Value {
        json!({
            "concept": "example.concept",
            "source": "authored",
            "citation": "docs/testing.md",
            "why": "what this corpus is for",
            "cases": [{
                "name": "a case",
                "why": "what this case protects",
                "given": { "in": 1 },
                "expect": { "out": 1 },
            }],
        })
    }

    fn parse(document: &Value) -> Conformance {
        Conformance::parse("example.json", &document.to_string())
    }

    #[test]
    fn a_good_document_loads() {
        let corpus = parse(&good());
        assert_eq!(corpus.concept, "example.concept");
        assert_eq!(corpus.cases.len(), 1);
    }

    #[test]
    #[should_panic(expected = "no cases")]
    fn an_empty_corpus_is_refused() {
        // The whole reason these guards exist: an empty corpus passes every driver, and a
        // driver that asserts nothing reports green forever.
        let mut document = good();
        document["cases"] = json!([]);
        parse(&document);
    }

    #[test]
    #[should_panic(expected = "has no `why`")]
    fn a_case_with_no_reason_is_refused() {
        let mut document = good();
        document["cases"][0].as_object_mut().unwrap().remove("why");
        parse(&document);
    }

    #[test]
    #[should_panic(expected = "must be one of recorded, ported, authored")]
    fn a_file_that_does_not_say_where_it_came_from_is_refused() {
        let mut document = good();
        document["source"] = json!("vibes");
        parse(&document);
    }

    #[test]
    #[should_panic(expected = "provenance is a claim rather than a fact")]
    fn a_recorded_file_with_no_way_to_re_derive_it_is_refused() {
        let mut document = good();
        document["source"] = json!("recorded");
        parse(&document);
    }

    #[test]
    #[should_panic(expected = "two cases named")]
    fn two_cases_with_one_name_are_refused() {
        // A failure that cannot say which case it was is a failure someone has to bisect.
        let mut document = good();
        let case = document["cases"][0].clone();
        document["cases"] = json!([case.clone(), case]);
        parse(&document);
    }

    #[test]
    #[should_panic(expected = "no `given`/`expect`")]
    fn a_case_with_nothing_to_compare_is_refused() {
        let mut document = good();
        document["cases"][0].as_object_mut().unwrap().remove("expect");
        parse(&document);
    }

    #[test]
    fn a_wrong_answer_fails_and_says_why_the_case_existed() {
        let corpus = parse(&good());
        let failure = std::panic::catch_unwind(|| {
            corpus.run(|_| Ok(json!({ "out": 2 })));
        })
        .expect_err("a wrong answer must fail the run");
        let report = failure.downcast_ref::<String>().expect("the failure carries a report");

        // The `why` is the load-bearing part: a failure is the one moment someone needs to
        // know what the case protected, and the moment they are least inclined to look.
        assert!(report.contains("what this case protects"), "no `why` in:\n{report}");
        assert!(report.contains("expected:"), "no expectation in:\n{report}");
        assert!(report.contains("actual:"), "no actual value in:\n{report}");
    }

    #[test]
    fn a_case_the_driver_cannot_run_is_reported_as_that_and_not_as_a_wrong_answer() {
        let corpus = parse(&good());
        let failure = std::panic::catch_unwind(|| {
            corpus.run(|_| Err(CaseError::new("the driver does not understand `in`")));
        })
        .expect_err("a case that cannot run must fail the run");
        let report = failure.downcast_ref::<String>().expect("the failure carries a report");

        assert!(report.contains("could not run this case"), "misreported as a behavior:\n{report}");
        assert!(report.contains("never evaluated"), "misreported as a behavior:\n{report}");
    }

    #[test]
    fn a_number_written_two_ways_is_one_number() {
        // serde_json holds 3 and 3.0 apart and Swift's JSONValue does not. That difference
        // belongs to two JSON libraries rather than to Muster, and letting it through
        // would fail cases on number shape rather than on behavior.
        let mut document = good();
        document["cases"][0]["expect"] = json!({ "out": 3 });
        parse(&document).run(|_| Ok(json!({ "out": 3.0 })));
    }

    /// The number an `f32` file exists to let a case write.
    ///
    /// 0.05 is herdr's own default resize step, and it is not spellable in binary, so a
    /// driver whose quantity is an f32 answers with the long form. Both spellings name one
    /// number at the width that quantity lives at, and a corpus that could not say so was
    /// restricted to powers of two for a reason no reader of the case could see.
    #[test]
    fn a_declared_f32_file_compares_at_the_width_the_wire_carries() {
        let mut document = good();
        document["numbers"] = json!("f32");
        document["cases"][0]["expect"] = json!({ "out": 0.05 });
        parse(&document).run(|_| Ok(json!({ "out": 0.050_000_000_745_058_06 })));
    }

    #[test]
    #[should_panic(expected = "actual:")]
    fn the_same_pair_is_two_numbers_at_json_s_own_width() {
        // The other half, and the reason narrowing is declared rather than assumed: it is a
        // loss. A file that has not asked for it keeps every digit it wrote.
        let mut document = good();
        document["cases"][0]["expect"] = json!({ "out": 0.05 });
        parse(&document).run(|_| Ok(json!({ "out": 0.050_000_000_745_058_06 })));
    }

    #[test]
    #[should_panic(expected = "`numbers` must be f64 or f32")]
    fn a_width_no_driver_can_compare_at_is_refused() {
        let mut document = good();
        document["numbers"] = json!("f16");
        parse(&document);
    }
}
