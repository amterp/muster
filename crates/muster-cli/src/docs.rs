//! Muster's documentation, inside the binary that the documentation is about.
//!
//! `muster docs` rather than a page on a website, following `rad` and `kan`: what an agent can
//! reach without a browser is what an agent will read, and a document shipped inside the binary
//! cannot describe a version somebody is not running.
//!
//! The markdown lives in `docs/cli/` and is embedded with `include_str!`, so it is reviewed as
//! prose in the repo and cannot be edited apart from the code it describes.

/// One document, and what it is about.
#[derive(Debug, Clone, Copy)]
pub struct Topic {
    pub name: &'static str,

    /// The one line `muster docs` lists it under.
    pub about: &'static str,

    pub text: &'static str,
}

/// Every document, in reading order rather than alphabetically: somebody working through the list
/// wants the vocabulary before the fields and the fields before the limits.
pub const TOPICS: &[Topic] = &[
    Topic {
        name: "overview",
        about: "the vocabulary, addressing a pane, finding a window, exit codes",
        text: include_str!("../../../docs/cli/overview.md"),
    },
    Topic {
        name: "window",
        about: "what `muster window` answers, and what every field of it means",
        text: include_str!("../../../docs/cli/window.md"),
    },
    Topic {
        name: "agents",
        about: "making panes, starting agents in them, and telling them what to do",
        text: include_str!("../../../docs/cli/agents.md"),
    },
    Topic {
        name: "limits",
        about: "what this surface cannot do, and what to do instead",
        text: include_str!("../../../docs/cli/limits.md"),
    },
];

/// What `muster docs` prints on its own: what there is to read.
pub fn listing() -> String {
    let widest = TOPICS.iter().map(|topic| topic.name.len()).max().unwrap_or(0);
    let mut lines = vec!["Topics, for `muster docs <topic>` or `muster docs all`:".to_string()];
    for topic in TOPICS {
        lines.push(format!("  {:widest$}  {}", topic.name, topic.about));
    }
    lines.join("\n")
}

pub fn topic(name: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|topic| topic.name == name)
}

/// Every document at once, for a caller that would rather read one thing than four.
///
/// Separated by a rule, because each document is already a markdown page with its own heading and
/// two of them run together read as one.
pub fn everything() -> String {
    TOPICS.iter().map(|topic| topic.text.trim_end()).collect::<Vec<_>>().join("\n\n---\n\n")
}

/// What to say to somebody who named a topic that is not there.
pub fn no_such_topic(named: &str) -> String {
    format!(
        "muster has no `{named}` documentation. There is {}, or `all` for every one of them.",
        TOPICS.iter().map(|topic| topic.name).collect::<Vec<_>>().join(", ")
    )
}
