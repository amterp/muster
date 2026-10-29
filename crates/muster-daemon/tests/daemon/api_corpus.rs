//! The API corpus: conversations a program has with the daemon (`corpus/conformance/api-*.json`),
//! played against the daemon built from this commit.
//!
//! What a client sends and hears is protobuf's canonical JSON, encoded and decoded through
//! `muster_api.proto` alone and never through the generated Rust types, so the daemon is judged
//! as a client generated from the published schema hears it. `docs/testing.md` has the steps a
//! conversation is made of and the tokens for what differs per run.
//!
//! The same conversations, as the last release shipped them, are kept in `corpus/api-baseline/`,
//! and stand unchanged in the corpus while the daemon still serves clients of that release's
//! version.

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use conformance::{Case, CaseError, Conformance};
use muster_api::connection::LARGEST_MESSAGE;
use muster_api::version::{API, SERVES_FROM};
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde_json::{Map, Value, json};

use crate::attention::{DONE, msg, named, report, subscribed, the_human};
use crate::persistence::stop;
use crate::support::*;
use proto::msg_request::{self, Request as Asked};

/// How long a step waits for each frame it expects before giving up on it.
const PATIENCE: Duration = Duration::from_secs(10);

#[test]
fn handshake() {
    played(&Conformance::load("api-handshake.json"));
}

#[test]
fn requests() {
    played(&Conformance::load("api-requests.json"));
}

#[test]
fn subscribing() {
    played(&Conformance::load("api-subscribing.json"));
}

#[test]
fn panes() {
    played(&Conformance::load("api-panes.json"));
}

#[test]
fn reading() {
    played(&Conformance::load("api-reading.json"));
}

#[test]
fn messages() {
    played(&Conformance::load("api-messages.json"));
}

#[test]
fn attention() {
    played(&Conformance::load("api-attention.json"));
}

#[test]
fn acting() {
    played(&Conformance::load("api-acting.json"));
}

#[test]
fn driving() {
    played(&Conformance::load("api-driving.json"));
}

#[test]
fn posting() {
    played(&Conformance::load("api-posting.json"));
}

#[test]
fn ending() {
    played(&Conformance::load("api-ending.json"));
}

/// While the daemon still serves clients of the version the last release shipped, every
/// conversation it shipped is still in the corpus as it was, but for a list of features that has
/// grown. The corpus is played green above, so an answer can only change by editing its case, and
/// this is what makes that edit come with a break the daemon admits to: the rule until the API's
/// 1.0, under which a break moves the oldest version served up to the new one and updates the
/// conversations in the same change (MIP-10, section 1). Raising the minor alone is not enough,
/// since every change to the schema does that and a client of the old minor is still welcomed.
#[test]
fn the_conversations_a_release_shipped_stand_while_its_clients_are_served() {
    let directory = conformance::repo_root().join("corpus/api-baseline");
    let shipped = shipped(&directory);
    if shipped.is_empty() {
        println!(
            "{} holds no conversations: no release has shipped the API yet, and the next one \
             copies them there",
            directory.display()
        );
        return;
    }
    let baseline = shipped_version(&directory);
    if (SERVES_FROM.major, SERVES_FROM.minor) > baseline {
        return;
    }
    let changed = changed_since(&shipped, &conformance::repo_root().join("corpus/conformance"));
    assert!(
        changed.is_empty(),
        "The daemon serves clients of API {}.{} and later, and {}.{} is the version the last \
         release shipped, yet conversations that release recorded have changed:\n  {}\nImpact: \
         a client written against {2}.{3} is still welcomed and hears answers it was not written \
         for, with nothing to tell it so.\nFix: give the daemon its old answers back; or, if the \
         change is a break, move SERVES_FROM in crates/muster-api/src/version.rs up to API \
         (raising API's minor first if this release has not already) and say what broke in the \
         release notes. A rewording of a case's `why` is not a change, and neither is a request \
         or topic added to a features list.",
        SERVES_FROM.major,
        SERVES_FROM.minor,
        baseline.0,
        baseline.1,
        changed.join("\n  ")
    );
}

/// Each shipped case the corpus in `current` no longer holds as it was shipped.
fn changed_since(shipped: &[Conformance], current: &Path) -> Vec<String> {
    let mut changed = Vec::new();
    for corpus in shipped {
        let name = corpus.file.trim_start_matches("api-baseline/");
        let Ok(text) = std::fs::read_to_string(current.join(name)) else {
            changed.push(format!("{name} is gone from the corpus"));
            continue;
        };
        let now = Conformance::parse(name, &text);
        for case in &corpus.cases {
            match now.cases.iter().find(|held| held.name == case.name) {
                None => changed.push(format!("{name}: `{}` is gone", case.name)),
                Some(held) if held.given != case.given || !stands(&case.expect, &held.expect) => {
                    changed.push(format!("{name}: `{}` says something else now", case.name));
                }
                Some(_) => {}
            }
        }
    }
    changed
}

/// Whether a shipped expectation is still what the corpus expects: the same, but for features
/// added since, which a client finds in the Welcome and does not miss when they were not there.
fn stands(shipped: &Value, now: &Value) -> bool {
    stands_within(shipped, now, false)
}

fn stands_within(shipped: &Value, now: &Value, in_features: bool) -> bool {
    match (shipped, now) {
        (Value::Object(was), Value::Object(is)) => {
            let kept = was.iter().all(|(key, value)| {
                is.get(key).is_some_and(|held| {
                    stands_within(value, held, in_features || key == "features")
                })
            });
            kept && (in_features || is.keys().all(|key| was.contains_key(key)))
        }
        (Value::Array(was), Value::Array(is)) if in_features => {
            let mut rest = is.iter();
            was.iter().all(|value| rest.any(|held| held == value))
        }
        (Value::Array(was), Value::Array(is)) => {
            was.len() == is.len()
                && was.iter().zip(is).all(|(value, held)| stands_within(value, held, false))
        }
        _ => shipped == now,
    }
}

/// The version the copy in `directory` was shipped as, which its `VERSION` file says. Its own
/// rather than the schema baseline's, which a release moves before its gate runs: judged by that,
/// the last release's conversations would be held to the next release's version.
fn shipped_version(directory: &Path) -> (u32, u32) {
    let file = directory.join("VERSION");
    let text = std::fs::read_to_string(&file).unwrap_or_else(|error| {
        panic!(
            "{} holds conversations and no VERSION saying which release shipped them ({error}); \
             ./release writes both",
            directory.display()
        )
    });
    let version = text
        .trim()
        .split_once('.')
        .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)));
    version.unwrap_or_else(|| panic!("{} says {text:?}, not a major.minor", file.display()))
}

fn shipped(directory: &Path) -> Vec<Conformance> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|kind| kind == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
            Conformance::parse(&format!("api-baseline/{}", name.unwrap_or_default()), &text)
        })
        .collect()
}

fn played(corpus: &Conformance) {
    let schema = Schema::compiled();
    let cases = corpus.run_cases(|case| Conversation::start(case, &schema)?.play(&case.expect));
    assert!(cases > 0, "{} ran no cases", corpus.file);
}

/// The API's messages as `muster_api.proto` declares them.
struct Schema {
    pool: DescriptorPool,
}

impl Schema {
    fn compiled() -> Schema {
        let set = muster_baseline::compile(&muster_baseline::proto_dir(), "muster_api.proto");
        let pool = DescriptorPool::from_file_descriptor_set(set)
            .unwrap_or_else(|error| panic!("muster_api.proto makes no descriptor pool: {error}"));
        Schema { pool }
    }

    /// This build's version, as canonical JSON spells it: a zero major is left out like any
    /// default.
    fn api(&self) -> Value {
        let version = json!({"major": API.major, "minor": API.minor});
        let encoded = self.encode("Version", &version).expect("a Version encodes");
        self.decode("Version", &encoded).expect("a Version decodes")
    }

    fn encode(&self, message: &str, value: &Value) -> Result<Vec<u8>, CaseError> {
        let descriptor = self.descriptor(message)?;
        let dynamic = DynamicMessage::deserialize(descriptor, value.clone())
            .map_err(|error| CaseError::new(format!("{value} is not a {message}: {error}")))?;
        Ok(prost::Message::encode_to_vec(&dynamic))
    }

    fn decode(&self, message: &str, bytes: &[u8]) -> Result<Value, CaseError> {
        let descriptor = self.descriptor(message)?;
        let dynamic = DynamicMessage::decode(descriptor, bytes).map_err(|error| {
            CaseError::new(format!("the daemon sent something that is not a {message}: {error}"))
        })?;
        serde_json::to_value(&dynamic)
            .map_err(|error| CaseError::new(format!("a {message} has no JSON: {error}")))
    }

    fn descriptor(&self, message: &str) -> Result<prost_reflect::MessageDescriptor, CaseError> {
        self.pool
            .get_message_by_name(&format!("muster.api.{message}"))
            .ok_or_else(|| CaseError::new(format!("the schema declares no {message}")))
    }
}

/// One case played: its daemon, its clients' connections, and what has been bound so far.
struct Conversation<'a> {
    schema: &'a Schema,
    daemon: Daemon,
    clients: BTreeMap<String, Client>,
    /// Control connections standing in for Muster's app, by what they stand for.
    app: Option<Control>,
    bound: Map<String, Value>,
    /// What was wrong with clients that are gone, for the played conversation to show.
    faults: Vec<Value>,
}

impl<'a> Conversation<'a> {
    fn start(case: &Case, schema: &'a Schema) -> Result<Conversation<'a>, CaseError> {
        let given = case.given.get("daemon").cloned().unwrap_or_else(|| json!({}));
        let detecting = given.get("detecting").and_then(Value::as_bool).unwrap_or(false);
        let daemon = if detecting { Daemon::start_detecting() } else { daemon() };
        // Under the daemon's root, so it goes with it, and as the kernel names it, which is how
        // a pane's directory comes back.
        let scratch = daemon.root().join("scratch");
        std::fs::create_dir_all(&scratch).map_err(|error| CaseError::new(error.to_string()))?;
        let mut bound = Map::new();
        bound.insert("api".to_string(), schema.api());
        bound.insert("scratch".to_string(), json!(canonical(&scratch).to_string_lossy()));
        let clients = BTreeMap::new();
        Ok(Conversation { schema, daemon, clients, app: None, bound, faults: Vec::new() })
    }

    /// Plays every step of `expect`'s conversation, and answers with the same conversation
    /// holding what each step heard, settled into the expectation's tokens where it matched, and
    /// what was wrong with a client's connection once it was gone.
    fn play(mut self, expect: &Value) -> Result<Value, CaseError> {
        let steps = expect
            .get("conversation")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("`expect` holds no `conversation`"))?;
        let mut played = Vec::with_capacity(steps.len());
        for (index, step) in steps.iter().enumerate() {
            let heard = self.step(step).map_err(|CaseError(error)| {
                CaseError::new(format!("step {} ({step}): {error}", index + 1))
            })?;
            let mut settled = step.clone();
            // A step that expects to hear nothing, a send that hangs up, need not say so.
            let silent = |heard: &Vec<Value>| heard.is_empty() && step.get("heard").is_none();
            if let Some(heard) = heard.filter(|heard| !silent(heard)) {
                let expected = step.get("heard").cloned().unwrap_or(Value::Array(Vec::new()));
                settled["heard"] = settle(&expected, &Value::Array(heard), &mut self.bound);
            }
            played.append(&mut self.faults);
            played.push(settled);
        }
        let names: Vec<String> = self.clients.keys().cloned().collect();
        for name in names {
            self.finish(&name)?;
        }
        played.append(&mut self.faults);
        Ok(json!({"conversation": played}))
    }

    /// Holds `name` to hearing nothing after its last step. A client that was told it is over
    /// hears its connection end. Any other is sent a request every daemon refuses and nothing
    /// comes of, and hears only its answer: everything the daemon queued for it before that is
    /// read, without waiting on a clock. What a `passing` the client declared allows is passed
    /// over, as in its reads.
    fn finish(&mut self, name: &str) -> Result<(), CaseError> {
        let Some(mut client) = self.clients.remove(name) else { return Ok(()) };
        if !client.over {
            let request = json!({"correlation": AFTER_THE_LAST_STEP});
            write(&mut client.stream, &self.schema.encode("Request", &request)?);
        }
        let unheard = client.rest(self.schema, &self.bound)?;
        self.gone(name, &client, unheard);
        Ok(())
    }

    /// Puts on record what was wrong with `name`'s connection, now it is gone.
    fn gone(&mut self, name: &str, client: &Client, unheard: Vec<Value>) {
        let mut wrong = Map::new();
        if !client.misnumbered.is_empty() {
            wrong.insert("misnumbered".to_string(), json!(client.misnumbered));
        }
        if !unheard.is_empty() {
            wrong.insert("heard after its last step".to_string(), Value::Array(unheard));
        }
        if !wrong.is_empty() {
            wrong.insert("as".to_string(), json!(name));
            self.faults.push(Value::Object(wrong));
        }
    }

    /// Takes one step, and what the client heard if it was a client's.
    fn step(&mut self, step: &Value) -> Result<Option<Vec<Value>>, CaseError> {
        if let Some(client) = step.get("as").and_then(Value::as_str) {
            return self.client_step(client, step).map(Some);
        }
        self.around_the_client(step)?;
        Ok(None)
    }

    fn client_step(&mut self, client: &str, step: &Value) -> Result<Vec<Value>, CaseError> {
        if let Some(hello) = step.get("hello") {
            self.finish(client)?;
            let hello = substitute(hello, &self.bound)?;
            let mut stream = dial(&self.daemon);
            write(&mut stream, &self.schema.encode("Hello", &hello)?);
            let heard = match read(&mut stream) {
                Some(frame) => vec![self.schema.decode("HelloAnswer", &frame)?],
                None => Vec::new(),
            };
            let over = heard.iter().any(|answer| answer.get("welcome").is_none());
            self.clients.insert(client.to_string(), Client::new(stream, over));
            return Ok(heard);
        }
        if step.get("hang_up").and_then(Value::as_bool) == Some(true) {
            self.hang_up(client);
            return Ok(Vec::new());
        }
        let schema = self.schema;
        let name = client;
        let client = self
            .clients
            .get_mut(name)
            .ok_or_else(|| CaseError::new(format!("`{name}` has not said hello")))?;
        if let Some(request) = step.get("send") {
            let request = substitute(request, &self.bound)?;
            write(&mut client.stream, &schema.encode("Request", &request)?);
            if step.get("then").and_then(Value::as_str) == Some("hang_up") {
                self.hang_up(name);
                return Ok(Vec::new());
            }
            let correlation = request.get("correlation").cloned().unwrap_or(Value::Null);
            let mut heard = Vec::new();
            while let Some(frame) = read(&mut client.stream) {
                let message = schema.decode("DaemonMessage", &frame)?;
                client.hear(&message);
                let answered = message.pointer("/answer/correlation") == Some(&correlation);
                heard.push(message);
                if answered {
                    break;
                }
            }
            return Ok(heard);
        }
        if let Some(count) = step.get("read").and_then(Value::as_u64) {
            let wanted = step.get("heard").and_then(Value::as_array).cloned().unwrap_or_default();
            let passing = step.get("passing");
            client.passing.extend(passing.cloned());
            let mut heard = Vec::new();
            while heard.len() < usize::try_from(count).unwrap_or(usize::MAX) {
                let Some(frame) = read(&mut client.stream) else { break };
                let message = schema.decode("DaemonMessage", &frame)?;
                client.hear(&message);
                let next = wanted.get(heard.len());
                let passed = passing.is_some_and(|passing| matches(passing, &message, &self.bound))
                    && !next.is_some_and(|next| matches(next, &message, &self.bound));
                if !passed {
                    heard.push(message);
                }
            }
            return Ok(heard);
        }
        Err(CaseError::new("a client's step says neither hello, send, read nor hang_up"))
    }

    fn hang_up(&mut self, name: &str) {
        if let Some(client) = self.clients.remove(name) {
            self.gone(name, &client, Vec::new());
        }
    }

    /// What happens in Muster around the clients, brought about as the daemon's own tests do.
    fn around_the_client(&mut self, step: &Value) -> Result<(), CaseError> {
        let text = |key: &str| step.get(key).and_then(Value::as_str);
        if let Some(action) = text("daemon") {
            match action {
                "hand_off" => {
                    let answer = self.daemon.replace(None);
                    if answer.outcome() != proto::Outcome::Done {
                        return Err(CaseError::new(format!("the handoff failed: {answer:?}")));
                    }
                }
                "restart" => {
                    let mut control = self.daemon.connect();
                    stop(&mut self.daemon, &mut control);
                    self.daemon.restart();
                    until_current(&self.daemon);
                }
                other => return Err(CaseError::new(format!("no daemon step `{other}`"))),
            }
            return Ok(());
        }
        if let Some(pane) = text("pane") {
            let pane = substitute(&json!(pane), &self.bound)?;
            let pane = pane.as_str().unwrap_or_default();
            let mut control = self.daemon.connect();
            if step.get("made").and_then(Value::as_bool) == Some(true) {
                make(&mut control, create(pane, in_new_tab(&format!("t-{pane}"))));
            } else if step.get("closed").and_then(Value::as_bool) == Some(true) {
                expect(&mut control, close_request(pane), proto::Outcome::Done);
            } else if let Some(shown) = text("shows") {
                until_text(&mut control, pane, shown);
            } else if let Some(state) = text("agent") {
                self.agent_in(pane, state)?;
            } else if let Some(session) = text("session") {
                // As the agent's adapter reports it, which is what makes the agent restartable.
                let report = proto::pane_request::Report {
                    pane: pane.to_string(),
                    agent: "claude".to_string(),
                    session_id: Some(session.to_string()),
                    ..Default::default()
                };
                let report = proto::pane_request::Request::Report(report);
                expect(&mut control, crate::support::pane(report), DONE);
            } else {
                return Err(CaseError::new(
                    "a pane's step says neither made, closed, shows, agent nor session",
                ));
            }
            return Ok(());
        }
        if let Some(agent) = text("agent") {
            return self.agent_acts(agent, step);
        }
        if text("human") == Some("reads") {
            let window = self.app.as_mut().ok_or_else(|| {
                CaseError::new("the human reads at the desk, so the app has to be attending")
            })?;
            let read = msg_request::Read { group: text("group").map(ToString::to_string) };
            expect(window, msg(&the_human(), Asked::Read(read)), DONE);
            return Ok(());
        }
        match text("app") {
            Some("attending") => {
                let mut window = self.daemon.connect();
                subscribed(&mut window, attending_request());
                expect(&mut window, report(true, None, 60_000), DONE);
                self.app = Some(window);
                Ok(())
            }
            Some("gone") => {
                self.app = None;
                Ok(())
            }
            _ => Err(CaseError::new("a step names no client, daemon, agent, pane or app")),
        }
    }

    /// An agent, or the desk as `@human`, joining, posting to or deleting a group.
    fn agent_acts(&mut self, agent: &str, step: &Value) -> Result<(), CaseError> {
        let text = |key: &str| step.get(key).and_then(Value::as_str);
        let mut control = self.daemon.connect();
        let caller = if agent == "@human" { the_human() } else { named(agent) };
        if let Some(group) = text("joins") {
            let join = msg_request::Join {
                name: Some(agent.to_string()),
                group: Some(group.to_string()),
                pull: false,
            };
            expect(&mut control, msg(&caller, Asked::Join(join)), DONE);
        } else if let Some(body) = text("posts") {
            let answering = step.get("answering").map(|answering| msg_request::Answering {
                question: answering.get("question").and_then(Value::as_u64).unwrap_or(0),
                answer: answering.get("answer").and_then(Value::as_str).unwrap_or("").into(),
            });
            let post = msg_request::Post {
                group: text("group").map(ToString::to_string),
                to: strings(step, "to"),
                body: body.to_string(),
                answers: strings(step, "answers"),
                answering,
                ..Default::default()
            };
            expect(&mut control, msg(&caller, Asked::Post(post)), DONE);
        } else if let Some(group) = text("deletes") {
            let delete = msg_request::GroupDelete { group: group.to_string() };
            expect(&mut control, msg(&caller, Asked::GroupDelete(delete)), DONE);
        } else {
            return Err(CaseError::new("an agent's step says neither joins, posts nor deletes"));
        }
        Ok(())
    }

    fn agent_in(&mut self, pane: &str, state: &str) -> Result<(), CaseError> {
        let state = match state {
            "started" => {
                self.daemon.run_agent(pane);
                return Ok(());
            }
            "working" => proto::AgentState::Working,
            "blocked" => proto::AgentState::Blocked,
            "idle" => proto::AgentState::Idle,
            other => return Err(CaseError::new(format!("no agent state `{other}`"))),
        };
        self.daemon.set_agent_state(pane, state);
        Ok(())
    }
}

/// The correlation id of the request a client is sent after its last step.
const AFTER_THE_LAST_STEP: &str = "after the last step";

/// A client's connection, and where it has got to in its subscription's numbers.
struct Client {
    stream: UnixStream,
    /// The epoch of the subscription it holds and the number of the last event it heard in it.
    numbered: Option<(u64, u64)>,
    /// What it heard out of the numbering, in words.
    misnumbered: Vec<String>,
    /// Refused at its Hello, or sent an Ended: the daemon hangs up next.
    over: bool,
    /// What it may hear on its own, from every `passing` its reads declared.
    passing: Vec<Value>,
}

impl Client {
    fn new(stream: UnixStream, over: bool) -> Client {
        Client { stream, numbered: None, misnumbered: Vec::new(), over, passing: Vec::new() }
    }

    /// What the client hears that it should not, through the answer to the request it was sent
    /// after its last step, or through its connection's end when it was told it is over. Only an
    /// end between frames is an end: a connection left open past [`PATIENCE`], or a frame cut
    /// short or too long, is heard as a fault.
    fn rest(
        &mut self,
        schema: &Schema,
        bound: &Map<String, Value>,
    ) -> Result<Vec<Value>, CaseError> {
        let last = json!(AFTER_THE_LAST_STEP);
        let mut unheard = Vec::new();
        loop {
            let frame = match muster_frame::read_frame_or_end(&mut self.stream, LARGEST_MESSAGE) {
                Ok(Some(frame)) => frame,
                Ok(None) if self.over => return Ok(unheard),
                Ok(None) => {
                    unheard.push(json!("the connection's end, with no answer"));
                    return Ok(unheard);
                }
                Err(error) => {
                    let wanted = if self.over { "the connection's end" } else { "an answer" };
                    unheard.push(json!(format!(
                        "no frame and no end, where {wanted} was due: {error}"
                    )));
                    return Ok(unheard);
                }
            };
            let message = schema.decode("DaemonMessage", &frame)?;
            self.hear(&message);
            if message.pointer("/answer/correlation") == Some(&last) {
                return Ok(unheard);
            }
            if !self.passing.iter().any(|passing| matches(passing, &message, bound)) {
                unheard.push(message);
            }
        }
    }

    /// Holds `message` to the numbering the header promises, whatever the case expects of it:
    /// each event is one past the last in its subscription's epoch, an answer is through the
    /// last event heard, and a subscribe's answer starts the count again.
    fn hear(&mut self, message: &Value) {
        let at = self.numbered.unwrap_or((0, 0));
        let heard = if let Some(event) = message.get("event") {
            let numbered = (uint(event, "epoch"), uint(event, "number"));
            let next = self.numbered.is_some() && numbered == (at.0, at.1 + 1);
            self.numbered = Some(numbered);
            (!next).then(|| format!("event {} of epoch {}", numbered.1, numbered.0))
        } else if let Some(answer) = message.get("answer") {
            let through = (uint(answer, "epoch"), uint(answer, "through"));
            let subscribed = answer.get("outcome") == Some(&json!("OUTCOME_DONE"));
            match answer.get("snapshot").filter(|_| subscribed) {
                Some(snapshot) => {
                    self.numbered = Some(through);
                    let current = (uint(snapshot, "epoch"), uint(snapshot, "through"));
                    (current != through).then(|| {
                        format!(
                            "a snapshot current to {current:?} in an answer through {through:?}"
                        )
                    })
                }
                None => (through != at).then(|| {
                    format!("an answer through event {} of epoch {}", through.1, through.0)
                }),
            }
        } else if let Some(ended) = message.get("ended") {
            self.over = true;
            let reached = (uint(ended, "epoch"), uint(ended, "reached"));
            (reached != at).then(|| format!("an end at event {} of epoch {}", reached.1, reached.0))
        } else {
            None
        };
        if let Some(heard) = heard {
            self.misnumbered.push(format!("{heard}, after event {} of epoch {}", at.1, at.0));
        }
    }
}

/// A 64-bit field of `message`, which canonical JSON writes as a string and leaves out at zero.
fn uint(message: &Value, field: &str) -> u64 {
    match message.get(field) {
        Some(Value::String(text)) => text.parse().unwrap_or(u64::MAX),
        Some(Value::Number(number)) => number.as_u64().unwrap_or(u64::MAX),
        _ => 0,
    }
}

/// Waits until a restarted daemon has brought back what it saved, as a client learns it: from
/// its own source's state in a snapshot, and the change to current that follows.
fn until_current(daemon: &Daemon) {
    let is_current = |status: &api::SourceStatus| status.state() == api::SourceState::Current;
    let mut client = daemon.api();
    if client.subscribe(&[]).snapshot().sources.iter().all(is_current) {
        return;
    }
    client.event_matching("the restarted daemon to be current", |event| {
        matches!(&event.event, Some(api::event::Event::SourceChanged(changed))
            if changed.status.as_ref().is_some_and(is_current))
    });
}

fn strings(step: &Value, key: &str) -> Vec<String> {
    let items = step.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    items.iter().filter_map(Value::as_str).map(ToString::to_string).collect()
}

fn dial(daemon: &Daemon) -> UnixStream {
    let stream = UnixStream::connect(daemon.socket_path())
        .unwrap_or_else(|error| panic!("the daemon's socket would not connect: {error}"));
    stream.set_read_timeout(Some(PATIENCE)).expect("a read timeout");
    stream
}

fn write(stream: &mut UnixStream, frame: &[u8]) {
    // A daemon that hung up is heard as the end of the connection by the next read.
    let _ = muster_frame::write_frame(stream, frame);
}

/// The next frame, or nothing once the connection has ended or stays quiet past [`PATIENCE`].
fn read(stream: &mut UnixStream) -> Option<Vec<u8>> {
    muster_frame::read_frame_or_end(stream, LARGEST_MESSAGE).ok().flatten()
}

/// `value` with every `$name` replaced by what it is bound to, and every `${name}` inside a
/// string by its text.
fn substitute(value: &Value, bound: &Map<String, Value>) -> Result<Value, CaseError> {
    Ok(match value {
        Value::String(text) => match variable(text) {
            Some(name) => bound
                .get(name)
                .cloned()
                .ok_or_else(|| CaseError::new(format!("`${name}` is sent before it is bound")))?,
            None => Value::String(interpolated(text, bound)?),
        },
        Value::Array(items) => Value::Array(
            items.iter().map(|item| substitute(item, bound)).collect::<Result<_, _>>()?,
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| Ok((key.clone(), substitute(item, bound)?)))
                .collect::<Result<_, CaseError>>()?,
        ),
        other => other.clone(),
    })
}

/// What was heard, in the expectation's tokens wherever it matches them: `*` for anything that
/// is there, `?` for anything or nothing, `$name` for what a name is bound to (binding it on first
/// sight, and never to a value another name holds), and the expected
/// text for a `${name}` string that comes out equal. Everything else is left as heard, so the
/// comparison after this shows exactly where the two differ.
fn settle(expected: &Value, heard: &Value, bound: &mut Map<String, Value>) -> Value {
    match (expected, heard) {
        (Value::String(token), _) if token == "*" => expected.clone(),
        (Value::String(token), _) if variable(token).is_some() => {
            let name = variable(token).unwrap_or_default();
            match bound.get(name) {
                Some(value) if value == heard => expected.clone(),
                Some(_) => heard.clone(),
                // A second name says the value is another one: a new epoch is not the old.
                None if bound.values().any(|value| value == heard) => heard.clone(),
                None => {
                    bound.insert(name.to_string(), heard.clone());
                    expected.clone()
                }
            }
        }
        (Value::String(text), Value::String(said)) if text.contains("${") => {
            match interpolated(text, bound) {
                Ok(meant) if &meant == said => expected.clone(),
                _ => heard.clone(),
            }
        }
        (Value::String(token), _) if token == "?" => expected.clone(),
        (Value::Object(want), Value::Object(got)) => {
            let mut settled: Map<String, Value> = got
                .iter()
                .map(|(key, value)| {
                    let settled = match want.get(key) {
                        Some(wanted) => settle(wanted, value, bound),
                        None => value.clone(),
                    };
                    (key.clone(), settled)
                })
                .collect();
            for (key, wanted) in want {
                if wanted == "?" && !got.contains_key(key) {
                    settled.insert(key.clone(), wanted.clone());
                }
            }
            Value::Object(settled)
        }
        (Value::Array(want), Value::Array(got)) => Value::Array(
            got.iter()
                .enumerate()
                .map(|(index, value)| match want.get(index) {
                    Some(wanted) => settle(wanted, value, bound),
                    None => value.clone(),
                })
                .collect(),
        ),
        _ => heard.clone(),
    }
}

/// Whether `message` is what `pattern` expects, binding nothing.
fn matches(pattern: &Value, message: &Value, bound: &Map<String, Value>) -> bool {
    settle(pattern, message, &mut bound.clone()) == *pattern
}

/// The name in a `$name` token: a whole value that is a dollar and a name.
fn variable(text: &str) -> Option<&str> {
    let name = text.strip_prefix('$')?;
    let named = !name.is_empty()
        && name.chars().all(|character| character.is_ascii_alphanumeric() || character == '_');
    named.then_some(name)
}

fn interpolated(text: &str, bound: &Map<String, Value>) -> Result<String, CaseError> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end =
            after.find('}').ok_or_else(|| CaseError::new(format!("`{text}` leaves a ${{ open")))?;
        let name = &after[..end];
        match bound.get(name) {
            Some(Value::String(value)) => out.push_str(value),
            Some(value) => out.push_str(&value.to_string()),
            None => {
                return Err(CaseError::new(format!("`${{{name}}}` is used before it is bound")));
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The check above, on a shipped copy the test makes: no release has shipped one yet.
#[test]
fn a_shipped_conversation_that_changed_or_went_is_named_and_a_reworded_why_is_not() {
    let current = conformance::repo_root().join("corpus/conformance");
    let as_shipped = Conformance::load("api-acting.json");
    assert_eq!(changed_since(std::slice::from_ref(&as_shipped), &current), Vec::<String>::new());

    let mut reworded = Conformance::load("api-acting.json");
    reworded.cases[0].why = "said another way".to_string();
    let mut answered_otherwise = Conformance::load("api-acting.json");
    answered_otherwise.cases[1].expect = json!({"conversation": []});
    let mut removed = Conformance::load("api-acting.json");
    removed.file = "api-gone.json".to_string();
    let changed = changed_since(&[reworded, answered_otherwise, removed], &current);
    assert_eq!(
        changed,
        [
            format!("api-acting.json: `{}` says something else now", as_shipped.cases[1].name),
            "api-gone.json is gone from the corpus".to_string(),
        ]
    );

    let features = "/conversation/0/heard/0/welcome/machine/features/requests";
    let welcomed = Conformance::load("api-handshake.json");
    let mut fewer = Conformance::load("api-handshake.json");
    let requests = fewer.cases[0].expect.pointer_mut(features).and_then(Value::as_array_mut);
    requests.expect("the first handshake case lists the requests").remove(1);
    let mut more = Conformance::load("api-handshake.json");
    let requests = more.cases[0].expect.pointer_mut(features).and_then(Value::as_array_mut);
    requests.expect("the first handshake case lists the requests").push(json!("pane.fly"));
    assert_eq!(
        changed_since(&[fewer, more], &current),
        [format!("api-handshake.json: `{}` says something else now", welcomed.cases[0].name)],
        "a request added since shipping stands, and one gone since does not"
    );
}

/// The end of a client told it is over is its connection closing, not its connection going quiet.
#[test]
fn a_client_told_it_is_over_hears_its_connection_end_and_not_a_silence() {
    let schema = Schema::compiled();
    let bound = Map::new();
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    ours.set_read_timeout(Some(Duration::from_millis(50))).expect("a read timeout");
    let mut quiet = Client::new(ours, true);
    let unheard = quiet.rest(&schema, &bound).expect("nothing to decode");
    assert_eq!(unheard.len(), 1, "a peer left open is not an end: {unheard:?}");

    drop(theirs);
    let unheard = quiet.rest(&schema, &bound).expect("nothing to decode");
    assert_eq!(unheard, Vec::<Value>::new(), "a peer that hung up is");
}

mod matching {
    use serde_json::{Map, Value, json};

    use super::settle;

    /// What `expected` makes of `heard`, and whether they agree.
    fn agrees(expected: &Value, heard: &Value, bound: &mut Map<String, Value>) -> bool {
        settle(expected, heard, bound) == *expected
    }

    #[test]
    fn a_name_binds_once_and_two_names_never_bind_one_value() {
        let mut bound = Map::new();
        assert!(agrees(&json!({"epoch": "$first"}), &json!({"epoch": "7"}), &mut bound));
        assert!(agrees(&json!({"epoch": "$first"}), &json!({"epoch": "7"}), &mut bound));
        assert!(!agrees(&json!({"epoch": "$first"}), &json!({"epoch": "8"}), &mut bound));
        assert!(
            !agrees(&json!({"epoch": "$second"}), &json!({"epoch": "7"}), &mut bound),
            "a daemon reusing the first epoch for the second subscription is caught"
        );
        assert!(agrees(&json!({"epoch": "$second"}), &json!({"epoch": "8"}), &mut bound));
    }

    #[test]
    fn a_star_needs_a_value_a_question_mark_does_not_and_nothing_else_is_spared() {
        let mut bound = Map::new();
        assert!(agrees(&json!({"title": "*"}), &json!({"title": "zsh"}), &mut bound));
        assert!(!agrees(&json!({"title": "*"}), &json!({}), &mut bound));
        assert!(agrees(&json!({"title": "?"}), &json!({}), &mut bound));
        assert!(agrees(&json!({"title": "?"}), &json!({"title": "zsh"}), &mut bound));
        assert!(
            !agrees(&json!({}), &json!({"title": "zsh"}), &mut bound),
            "a field the case does not name is an answer that changed"
        );
        assert!(!agrees(&json!(["a"]), &json!(["a", "b"]), &mut bound));
    }

    #[test]
    fn a_name_inside_a_string_is_its_text() {
        let mut bound = Map::new();
        bound.insert("scratch".to_string(), json!("/tmp/s"));
        assert!(agrees(&json!("${scratch}/work"), &json!("/tmp/s/work"), &mut bound));
        assert!(!agrees(&json!("${scratch}/work"), &json!("/tmp/s/play"), &mut bound));
    }
}
