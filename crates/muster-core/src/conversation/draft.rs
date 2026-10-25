//! The message box's draft, and whom it addresses (MIP-9, section 3).
//!
//! A draft is kept whole per window and group: the text, the recipients and Urgent. It opens
//! addressed to whoever last wrote to the human, when they can be addressed. A message that
//! starts with @ and a member's name addresses that member instead, and Backspace straight after
//! a recipient turns it back into text. A message arriving never changes a draft.
//!
//! Cursors count Unicode scalar values, which every shell's strings can count.

use crate::conversation::{Group, What};
use crate::posting;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// Participant names, as a post addresses them. Empty leaves it to the group's policy.
    pub to: Vec<String>,
    pub urgent: bool,
    pub addressed: Addressed,
}

/// How a draft's recipients came to be, which decides what a leading @ does to them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Addressed {
    /// As the draft opened.
    #[default]
    Opening,
    /// As the human chose them, with Reply or by taking one away.
    Chosen,
    /// By names at the start of the text.
    At,
}

/// The message box as the shell holds it after the human did something to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub cursor: usize,
    pub to: Vec<String>,
    pub urgent: bool,
    /// Backspace was pressed straight after the last recipient.
    pub unaddress: bool,
    /// The member, by participant name, to complete the `@name` being typed at the cursor to.
    pub complete: Option<String>,
}

/// A member a name being typed after an @ may complete to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub name: String,
    pub shown: String,
}

/// A draft after an edit, with where the cursor now is and what the name at it may complete to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub draft: Draft,
    pub cursor: usize,
    pub completions: Vec<Completion>,
}

/// The draft a conversation opens with: addressed to the author of the newest message that woke
/// the human, when that author can be addressed, and otherwise to nobody, which leaves whom the
/// post wakes to the group's policy.
pub fn opening(group: &Group) -> Draft {
    let last_to_human = group.log.entries().rev().find_map(|entry| match &entry.what {
        What::Message(message) if group.woke_human(message) => Some(message),
        _ => None,
    });
    let to = last_to_human
        .map(|message| &message.author)
        .filter(|author| group.addressable(author))
        .map(|author| vec![author.clone()])
        .unwrap_or_default();
    Draft { to, ..Draft::default() }
}

/// What the human's edit makes of `draft`.
pub fn edited(draft: &Draft, edit: Edit, group: &Group) -> Edited {
    let addressed = if edit.to == draft.to { draft.addressed } else { Addressed::Chosen };
    let mut next = Draft { text: edit.text, to: edit.to, urgent: edit.urgent, addressed };
    let mut cursor = edit.cursor.min(next.text.chars().count());
    if let Some(name) = &edit.complete {
        cursor = complete(&mut next.text, cursor, name, group);
    }
    if edit.unaddress {
        if let Some(last) = next.to.pop() {
            let shown = group.shown(&last);
            let gap = next.text.chars().next().is_some_and(|c| !c.is_whitespace());
            next.text = format!("@{shown}{}{}", if gap { " " } else { "" }, next.text);
            cursor = 1 + shown.chars().count();
        }
    } else {
        while let Some((name, prefix)) = leading_member(&next.text, group) {
            // Names a previous edit already left as text, as Backspace after a recipient does.
            if draft.text.starts_with(prefix) {
                break;
            }
            let taken = prefix.chars().count();
            next.text = next.text[prefix.len()..].to_string();
            cursor = cursor.saturating_sub(taken);
            if next.addressed != Addressed::At {
                next.to.clear();
                next.addressed = Addressed::At;
            }
            if !next.to.contains(&name) {
                next.to.push(name);
            }
        }
    }
    let completions = completions(&next.text, cursor, group);
    Edited { draft: next, cursor, completions }
}

/// Completes the `@name` being typed at `cursor` to the member called `name`, with a space after
/// it, and says where the cursor goes: after that space. Done before a leading name is taken, so a completion at
/// the start of the text addresses the member and one later stays text, as typing it would.
fn complete(text: &mut String, cursor: usize, name: &str, group: &Group) -> usize {
    let Some((_, shown)) = addressable(group).find(|(member, _)| member == name) else {
        return cursor;
    };
    let before: String = text.chars().take(cursor).collect();
    let word = before.rsplit(char::is_whitespace).next().unwrap_or("");
    if !word.starts_with('@') {
        return cursor;
    }
    let start = before.len() - word.len();
    let after: String = text.chars().skip(cursor).collect();
    // One space after the name, the one already there when the name was typed mid-sentence.
    let spaced = after.starts_with(char::is_whitespace);
    let completed = format!("@{shown}{}", if spaced { "" } else { " " });
    let moved = before[..start].chars().count() + completed.chars().count() + usize::from(spaced);
    *text = format!("{}{completed}{after}", &before[..start]);
    moved
}

/// The member a text's leading `@name ` names, and that prefix with the whitespace after it.
fn leading_member<'a>(text: &'a str, group: &Group) -> Option<(String, &'a str)> {
    let rest = text.strip_prefix('@')?;
    let end = rest.find(char::is_whitespace)?;
    let typed = &rest[..end];
    let spaces = rest[end..].len() - rest[end..].trim_start().len();
    let member = addressable(group)
        .find(|(name, shown)| name.as_str() == typed || shown.eq_ignore_ascii_case(typed))?;
    Some((member.0, &text[..1 + end + spaces]))
}

/// What the `@name` being typed at the cursor may complete to.
fn completions(text: &str, cursor: usize, group: &Group) -> Vec<Completion> {
    let before: String = text.chars().take(cursor).collect();
    let word = before.rsplit(char::is_whitespace).next().unwrap_or("");
    let Some(typed) = word.strip_prefix('@') else { return Vec::new() };
    let typed = typed.to_lowercase();
    let mut found: Vec<Completion> = addressable(group)
        .filter(|(name, shown)| {
            shown.to_lowercase().starts_with(&typed) || name.to_lowercase().starts_with(&typed)
        })
        .map(|(name, shown)| Completion { name, shown })
        .collect();
    found.sort_by(|a, b| a.shown.cmp(&b.shown));
    found
}

/// The members a post can address, with what each is called.
fn addressable(group: &Group) -> impl Iterator<Item = (String, String)> + '_ {
    group
        .members
        .iter()
        .flatten()
        .filter(|member| !posting::is_human(&member.name) && !posting::is_muster(&member.name))
        .map(|member| (member.name.clone(), group.shown(&member.name)))
}
