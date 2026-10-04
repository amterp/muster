//! MIP-4's proof that a council holds together: three Claude Code sessions and the human in a
//! directed council, run past forty messages with no agent leaving and none stranded. Each
//! session has the council skill in its project and `extras/claude-code/messaging-hooks.json`
//! as its hooks, and bypasses permission prompts, as workers do. The test is the human: it
//! tells each session its part, convenes the group from the directed preset, adds two of the
//! panes, and posts the third its brief as director; after that nothing prompts any session, so
//! every turn from there on was started by a wake. Once the agents have posted [`WRAP_UP_AT`]
//! messages, the human tells the director to finish, as a person dismisses a council: a model
//! counting its own way to a number stopped at 31 in one run.
//!
//! Ignored by the gate, which may not reach the network; `./dev --claude-code` runs it. It
//! prints what the sessions spent, read off their transcripts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::claude_code_hooks::{arguments_or_fail, environment, prompt, skipped, start};
use crate::claude_code_inbox::{agent_state, entries_of};
use crate::support::*;
use muster_harness::Input;
use proto::msg_answer::entry::What;

const EXTRAS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras");

/// How long the council has to reach its outcome.
const BUDGET: Duration = Duration::from_mins(25);

/// How long all three sessions may sit idle together, with nothing new in the log, before the
/// council counts as stranded: in a directed council somebody always holds the turn until the
/// outcome is posted.
const STRANDED: Duration = Duration::from_mins(3);

/// How often [`watch`] looks. Against [`STRANDED`], ten seconds late is nothing; what a sample
/// can miss is a turn that starts and ends between two, which [`Stillness`] catches by its post.
const SAMPLED: Duration = Duration::from_secs(10);

/// How many messages the agents post before the human tells the director to finish, which
/// clears the proof's forty with room for the rounds already under way.
const WRAP_UP_AT: usize = 45;

const MEMBERS: [&str; 2] = ["builder", "critic"];

const TOPIC: &str = "the design of a small to-do list tool for one person";

/// One aspect per round, more than the rounds [`WRAP_UP_AT`] messages take, since a director
/// that ran out of aspects declared the design complete at 27 messages.
const AGENDA: &str = "1. how items are stored on disk; 2. the commands; 3. due dates and \
    recurring items; 4. two terminals editing the list at once; 5. errors and their messages; \
    6. searching and filtering; 7. undo; 8. importing and exporting; 9. configuration; \
    10. output for scripts; 11. performance with ten thousand items; 12. how it is tested";

fn brief() -> String {
    format!(
        "You are the director of the council `council`. First run `muster msg join --name \
         director --group council`. Its members are builder, who proposes, and critic, who finds \
         what is wrong with a proposal. The question is {TOPIC}, and its agenda is: {AGENDA}.\n\n\
         Settle one aspect of the agenda per round, in order. In each round, address builder with \
         `muster msg post --group council --to builder` and ask for a proposal; when it answers, \
         address critic with the number of builder's message to review; when critic answers, \
         decide the aspect in a post to the group with no `--to`, and start the next round. \
         After each post end your turn: you are woken when the member you addressed answers, \
         and a member hears another's post only when you address it. Finishing is @human's \
         call, not yours: keep going round after round, starting the agenda again in more \
         detail if you reach its end, until @human tells you to finish, and do not address \
         @human before then. Then post the design in one paragraph to @human in the \
         group, starting with DONE:, and end your turn."
    )
}

/// What the human tells each session when starting it, as a person convening a council would:
/// its name and part, and nothing of the question, which arrives as a message.
fn role(name: &str) -> String {
    let part = if name == "director" {
        "its director. Your brief will arrive as a message"
    } else {
        "a member. Its director will give you your part in a message"
    };
    format!(
        "You are {name}, in a council held over `muster msg`: use the council skill. You are \
         {part}. Whatever you have to say to the council, post it with `muster msg post`. End \
         your turn now."
    )
}

#[test]
#[ignore = "reaches the network with the real Claude Code; run through ./dev --claude-code"]
fn a_directed_council_of_three_sessions_runs_past_forty_messages_with_nobody_leaving() {
    if skipped() {
        return;
    }
    let arguments = arguments_or_fail("that a council of Claude Code sessions holds together");
    let muster = muster_harness::built_daemon().with_file_name("muster");
    assert!(
        muster.is_file(),
        "no muster CLI at {}.\n  Impact: the sessions would have no `muster msg` to run.\n  Fix: \
         run ./dev -b, or cargo build -p muster-cli.",
        muster.display()
    );
    let environment = environment();
    let environment: Vec<(&str, &str)> =
        environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let daemon = daemon_with(&environment);
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());

    let snippet: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{EXTRAS}/claude-code/messaging-hooks.json")).unwrap(),
    )
    .unwrap();
    let panes = ["director", MEMBERS[0], MEMBERS[1]];
    let mut projects = Vec::new();
    for name in panes {
        let skill = daemon.root().join(name).join(".claude/skills/council");
        std::fs::create_dir_all(&skill).unwrap();
        for file in ["SKILL.md", "directed.toml", "roundtable.toml"] {
            std::fs::copy(format!("{EXTRAS}/skill/council/{file}"), skill.join(file)).unwrap();
        }
        projects.push(start(
            &daemon,
            &mut control,
            &mut input,
            &arguments,
            name,
            &snippet["hooks"],
        ));
    }
    for name in panes {
        prompt(&mut input, name, &role(name));
    }

    let human = |arguments: &[&str]| {
        let ran = Command::new(&muster)
            .args(arguments)
            .env_clear()
            .env("HOME", daemon.root())
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .stdin(Stdio::null())
            .output()
            .expect("the muster binary runs");
        assert!(
            ran.status.success(),
            "muster {}: {}{}",
            arguments.join(" "),
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        );
    };
    let preset = format!("{EXTRAS}/skill/council/directed.toml");
    human(&["msg", "group", "new", "council", "--policy", &preset]);
    human(&["msg", "group", "add", "council", MEMBERS[0], MEMBERS[1]]);
    let brief_file = daemon.root().join("brief.md");
    std::fs::write(&brief_file, brief()).unwrap();
    human(&["msg", "post", "--to", "director", "--file", &brief_file.display().to_string()]);

    let wrap_up = || {
        let finish = "That is enough rounds: post the design to @human now, starting with DONE:.";
        human(&["msg", "post", "--group", "council", "--to", "director", finish]);
    };
    let outcome = watch(&mut control, panes, wrap_up);

    let posts = outcome.posts();
    let messages: usize = posts.values().sum();
    let spent = spent(&projects);
    eprintln!(
        "claude-code: the council posted {messages} messages in {:?} ({posts:?}); all three \
         sessions were idle together for at most {:?}; {spent}",
        outcome.took, outcome.longest_idle
    );
    let left: Vec<&str> = outcome
        .entries
        .iter()
        .filter_map(|entry| match &entry.what {
            Some(What::Left(name)) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(left.is_empty(), "{left:?} left the council:\n{}", transcript(&outcome.entries));
    assert!(messages >= 40, "only {messages} messages:\n{}", transcript(&outcome.entries));
    for name in panes {
        let posted = posts.get(name).copied().unwrap_or(0);
        assert!(posted >= 5, "{name} posted {posted} times:\n{}", transcript(&outcome.entries));
    }
}

/// Watches the council's log until the director posts its outcome, having `wrap_up` tell it to
/// once the agents have posted [`WRAP_UP_AT`] messages, and failing if the budget runs out or all
/// three sessions sit idle together for longer than [`STRANDED`].
fn watch(control: &mut Control, panes: [&str; 3], wrap_up: impl Fn()) -> Outcome {
    let began = Instant::now();
    let mut stillness = Stillness::default();
    let mut longest_idle = Duration::ZERO;
    let mut told = false;
    loop {
        let entries = entries_of(control, "council");
        let done = entries.iter().any(|entry| {
            matches!(&entry.what, Some(What::Message(message))
                if message.author == "director" && message.body.trim_start().starts_with("DONE:"))
        });
        if done {
            return Outcome { entries, took: began.elapsed(), longest_idle };
        }
        let posted = entries
            .iter()
            .filter(|entry| {
                matches!(&entry.what, Some(What::Message(message))
                    if panes.contains(&message.author.as_str()))
            })
            .count();
        if !told && posted >= WRAP_UP_AT {
            wrap_up();
            told = true;
        }
        let all_idle =
            panes.iter().all(|pane| agent_state(control, pane) == proto::AgentState::Idle);
        let still = stillness.observe(all_idle, entries.len(), Instant::now());
        longest_idle = longest_idle.max(still);
        if still > STRANDED || began.elapsed() > BUDGET {
            for pane in panes {
                eprintln!("claude-code: {pane} shows:\n{}", read_text(control, pane, 0, 0).text);
            }
            let why = if began.elapsed() > BUDGET { "ran out of time" } else { "was stranded" };
            panic!(
                "the council {why} after {:?}, all three idle for {:?}; its log:\n{}",
                began.elapsed(),
                still,
                transcript(&entries)
            );
        }
        std::thread::sleep(SAMPLED);
    }
}

/// How long a council has gone without moving: every session idle at each sample, and no entry
/// added to its log since. Every turn in a directed council posts, so the log growing is a turn
/// taken even when no sample saw anyone working.
#[derive(Debug, Default)]
struct Stillness {
    since: Option<Instant>,
    entries: usize,
}

impl Stillness {
    fn observe(&mut self, all_idle: bool, entries: usize, now: Instant) -> Duration {
        let moved = std::mem::replace(&mut self.entries, entries) != entries;
        if !all_idle || moved {
            self.since = all_idle.then_some(now);
            return Duration::ZERO;
        }
        now - *self.since.get_or_insert(now)
    }
}

/// A turn too short for any sample to see working still restarts the clock, by what it posted.
#[test]
fn a_turn_between_two_samples_is_not_stillness() {
    let start = Instant::now();
    let at = |seconds| start + Duration::from_secs(seconds);
    let mut stillness = Stillness::default();
    assert_eq!(stillness.observe(true, 4, at(0)), Duration::ZERO, "the first log seen is news");
    assert_eq!(stillness.observe(true, 4, at(10)), Duration::from_secs(10));
    assert_eq!(stillness.observe(true, 5, at(20)), Duration::ZERO, "somebody posted");
    assert_eq!(stillness.observe(true, 5, at(200)), STRANDED);
    assert_eq!(stillness.observe(false, 5, at(210)), Duration::ZERO, "somebody is working");
    assert_eq!(stillness.observe(true, 5, at(220)), Duration::ZERO);
}

struct Outcome {
    entries: Vec<proto::msg_answer::Entry>,
    took: Duration,
    longest_idle: Duration,
}

impl Outcome {
    /// How many messages each author posted.
    fn posts(&self) -> BTreeMap<String, usize> {
        let mut posts = BTreeMap::new();
        for entry in &self.entries {
            if let Some(What::Message(message)) = &entry.what {
                *posts.entry(message.author.clone()).or_default() += 1;
            }
        }
        posts
    }
}

/// The log as one line per entry, for a failure to show.
fn transcript(entries: &[proto::msg_answer::Entry]) -> String {
    entries
        .iter()
        .map(|entry| match &entry.what {
            Some(What::Message(message)) => {
                let body: String = message.body.chars().take(160).collect();
                format!("#{} {} -> {:?}: {body}", entry.seq, message.author, message.to)
            }
            other => format!("#{} {other:?}", entry.seq),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Haiku 4.5's price per million tokens: input, cache writes, cache reads, output.
const PRICES: [f64; 4] = [1.0, 1.25, 0.10, 5.0];

/// What the sessions in `projects` spent, from the usage Claude Code writes into each
/// session's transcript under `~/.claude/projects`, counting each model reply once.
fn spent(projects: &[PathBuf]) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return "no HOME, so no cost".to_string();
    };
    let mut tokens = [0u64; 4];
    let mut replies = BTreeSet::new();
    for project in projects {
        let folder = Path::new(&home).join(".claude/projects").join(transcript_folder(project));
        let Ok(files) = std::fs::read_dir(&folder) else {
            return format!("no transcripts at {}, so no cost", folder.display());
        };
        for file in
            files.flatten().filter(|file| file.path().extension().is_some_and(|x| x == "jsonl"))
        {
            for line in std::fs::read_to_string(file.path()).unwrap_or_default().lines() {
                let Ok(line) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let message = &line["message"];
                let (Some(id), Some(usage)) = (message["id"].as_str(), message.get("usage")) else {
                    continue;
                };
                if !replies.insert(id.to_string()) {
                    continue;
                }
                let fields = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                    "output_tokens",
                ];
                for (total, field) in tokens.iter_mut().zip(fields) {
                    *total += usage[field].as_u64().unwrap_or(0);
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let dollars: f64 =
        tokens.iter().zip(PRICES).map(|(tokens, price)| *tokens as f64 * price / 1e6).sum();
    format!(
        "{} replies spent {} input, {} cache-write, {} cache-read and {} output tokens, about \
         ${dollars:.2} at Haiku 4.5's prices",
        replies.len(),
        tokens[0],
        tokens[1],
        tokens[2],
        tokens[3]
    )
}

/// The folder Claude Code keeps a project's transcripts in: its real path with every character
/// other than a letter or digit made a `-`.
pub(super) fn transcript_folder(project: &Path) -> String {
    let real = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    real.display()
        .to_string()
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() { character } else { '-' })
        .collect()
}
