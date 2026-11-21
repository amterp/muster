//! The table in `docs/cli/harnesses.md` (MIP-5, section 5): which harness has which capability,
//! for the capabilities Muster can tell from its own files - what a manifest reads and types, and
//! what the hooks and statusline in `extras/<harness>/` call. The table is generated here and the test fails when the
//! page says otherwise; `MUSTER_WRITE_HARNESSES=1` rewrites it.

use std::path::{Path, PathBuf};

use muster_adapters::catalog;
use muster_detect::{Agent, Manifests};

/// What a harness's adapter supplies, as far as files can say.
#[allow(clippy::struct_excessive_bools)] // one flag per capability
struct Supplied {
    prompt: bool,
    prompt_at_work: bool,
    reported_state: bool,
    context: bool,
    cost: bool,
    subagents: bool,
    fetches_messages: bool,
    woken_by_command: bool,
    takes_pane_name: bool,
    names_pane: bool,
    compacts: bool,
    clears: bool,
    says_cleared: bool,
    says_compacted: bool,
    transcript: bool,
    resumes: bool,
    image_paste: bool,
}

/// A capability's row: its name on the page, and how it is read off what a harness supplies.
type Row = (&'static str, fn(&Supplied) -> bool);

const ROWS: [Row; 17] = [
    ("Its own report of its state", |supplied| supplied.reported_state),
    ("Context used and model", |supplied| supplied.context),
    ("Cost", |supplied| supplied.cost),
    ("Sub-agents counted", |supplied| supplied.subagents),
    ("Rung at an empty prompt", |supplied| supplied.prompt),
    ("Rung while it works, for an urgent post", |supplied| supplied.prompt_at_work),
    ("Messages fetched by its hooks", |supplied| supplied.fetches_messages),
    ("Woken by its own command, typing nothing", |supplied| supplied.woken_by_command),
    ("Its session named after the pane", |supplied| supplied.takes_pane_name),
    ("The pane named after its session", |supplied| supplied.names_pane),
    ("Compacted when asked", |supplied| supplied.compacts),
    ("Cleared when asked", |supplied| supplied.clears),
    ("Says when its context was cleared", |supplied| supplied.says_cleared),
    ("Says when its context was compacted", |supplied| supplied.says_compacted),
    ("Reports its session transcript path", |supplied| supplied.transcript),
    ("Resumed after a restart", |supplied| supplied.resumes),
    ("Clipboard image pasted into a remote composer", |supplied| supplied.image_paste),
];

fn page() -> PathBuf {
    conformance::repo_root().join("docs/cli/harnesses.md")
}

/// Everything under `directory` but its prose, as one text: hooks, scripts, plugin files.
fn wiring(directory: &Path) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(directory).unwrap().map(Result::unwrap) {
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&wiring(&path));
        } else if path.extension().is_none_or(|extension| extension != "md") {
            text.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    text
}

/// `wiring` with quotes, commas and brackets taken out, so a hook's shell command and a plugin's
/// argument list read the same: `report --agent codex --state` and `["--agent", "opencode",
/// "--state", ...]`.
fn words(wiring: &str) -> String {
    let spaced: String = wiring
        .chars()
        .map(|character| if "\"',[]".contains(character) { ' ' } else { character })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn supplied(manifests: &Manifests, wiring: &str, agent: &Agent) -> Supplied {
    Supplied {
        prompt: manifests.reads_prompt(agent),
        prompt_at_work: manifests.reads_prompt_at_work(agent),
        reported_state: words(wiring).contains(&format!("--agent {} --state", agent.id())),
        context: reports(wiring, "context-used") && reports(wiring, "model"),
        cost: reports(wiring, "cost-usd"),
        subagents: wiring.contains("--subagent-started"),
        fetches_messages: wiring.contains("msg read --if-unread"),
        woken_by_command: manifests.session_wake(agent, "id", "wake").is_some()
            && reports(wiring, "session-id"),
        takes_pane_name: manifests.session_rename(agent, "name").is_some(),
        names_pane: reports(wiring, "session-name"),
        compacts: manifests.session_compact(agent, None).is_some(),
        clears: manifests.session_clear(agent).is_some(),
        says_cleared: reports(wiring, "session-source"),
        says_compacted: reports(wiring, "context-compacted") || reports(wiring, "compacted-in"),
        transcript: reports(wiring, "session-transcript"),
        // The manifest says how, and the adapter reports the session to resume.
        resumes: manifests.session_resume(agent, "id", &[], None).is_some()
            && reports(wiring, "session-id"),
        image_paste: manifests.supports_image_paste(agent),
    }
}

/// Whether the wiring reports `flag`: given outright, or read out of a harness's JSON with
/// `--from`.
fn reports(wiring: &str, flag: &str) -> bool {
    wiring.contains(&format!("--{flag}")) || wiring.contains(&format!("--from {flag}="))
}

/// The table, and the line naming the harnesses the last column stands for.
fn generated() -> Vec<String> {
    let manifests = Manifests::built_in();
    let named: Vec<_> = catalog::adapters().collect();
    let extras = conformance::repo_root().join("extras");
    let mut others: Vec<Agent> = manifests
        .agents()
        .filter(|agent| !named.iter().any(|(id, _)| *id == agent.id()))
        .cloned()
        .collect();
    others.sort_by(|a, b| a.id().cmp(b.id()));
    let installed: Vec<_> = named
        .iter()
        .map(|(id, adapter)| {
            supplied(&manifests, &wiring(&extras.join(adapter.dir)), &Agent::new(id))
        })
        .collect();
    let rest: Vec<(Agent, Supplied)> =
        others.iter().map(|agent| (agent.clone(), supplied(&manifests, "", agent))).collect();

    let header: Vec<&str> = named.iter().map(|(_, adapter)| adapter.name).collect();
    let mut lines = vec![
        format!("| Capability | {} | Every other harness |", header.join(" | ")),
        format!("|---|{}---|", "---|".repeat(named.len())),
        format!("| Its state read off the screen | {}yes |", "yes | ".repeat(named.len())),
    ];
    for (row, has) in ROWS {
        let cells: Vec<&str> =
            installed.iter().map(|supplied| if has(supplied) { "yes" } else { "no" }).collect();
        let some: Vec<&str> = rest
            .iter()
            .filter(|(_, supplied)| has(supplied))
            .map(|(agent, _)| agent.id())
            .collect();
        let last = if some.is_empty() { "no".to_string() } else { some.join(", ") };
        lines.push(format!("| {row} | {} | {last} |", cells.join(" | ")));
    }
    let ids: Vec<&str> = others.iter().map(Agent::id).collect();
    lines.push(String::new());
    lines.extend(wrapped(&format!("Every other harness: {}.", ids.join(", "))));
    lines
}

/// `text` in lines of at most the page's width, as the rest of it is.
fn wrapped(text: &str) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.len() + 1 + word.len() > 100 {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

#[test]
fn the_harness_table_says_what_the_manifests_and_extras_supply() {
    let path = page();
    let text = std::fs::read_to_string(&path).expect("docs/cli/harnesses.md exists");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("| Capability |"))
        .expect("the page has its table");
    let named = lines[start..]
        .iter()
        .position(|line| line.starts_with("Every other harness: "))
        .map(|offset| start + offset)
        .expect("the page names the harnesses its last column stands for");
    let end = lines[named..]
        .iter()
        .position(|line| line.is_empty())
        .map_or(lines.len(), |offset| named + offset);
    let generated = generated();
    let current: Vec<String> = lines[start..end].iter().map(ToString::to_string).collect();
    if current == generated {
        return;
    }
    if std::env::var_os("MUSTER_WRITE_HARNESSES").is_some() {
        let mut rewritten: Vec<String> = lines[..start].iter().map(ToString::to_string).collect();
        rewritten.extend(generated);
        rewritten.extend(lines[end..].iter().map(ToString::to_string));
        std::fs::write(&path, rewritten.join("\n") + "\n").unwrap();
        return;
    }
    panic!(
        "docs/cli/harnesses.md's table is stale. It says:\n{}\nThe manifests and extras/ say:\n{}\n\
         Rewrite it with MUSTER_WRITE_HARNESSES=1 cargo test -p muster-detect --test detect harnesses",
        current.join("\n"),
        generated.join("\n")
    );
}
