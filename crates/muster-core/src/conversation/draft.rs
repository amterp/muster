//! The message box's draft, and whom it addresses (MIP-9, section 3).
//!
//! A draft is kept whole per window and group: the text, the recipients and Urgent. It opens
//! addressed to whoever last wrote to the human, when they can be addressed. A message that
//! starts with @ and a member's name addresses that member instead, and Backspace straight after
//! a recipient turns it back into text. A message arriving never changes a draft.
//!
//! Cursors count Unicode scalar values, which every shell's strings can count.

use crate::conversation::{Answering, Group, What};
use crate::posting;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// Participant names, as a post addresses them. Empty leaves it to the group's policy.
    pub to: Vec<String>,
    pub urgent: bool,
    pub addressed: Addressed,
    /// The answer picked from a question in the thread, to go with the text (MIP-9, section 6).
    /// Kept whole, the question's number and the answer's words, so a draft written down never
    /// depends on a message the log may not hold when it is read back.
    pub picked: Option<Answering>,
}

impl Draft {
    /// Whether posting it sends anything: words, or an answer picked.
    pub fn sends(&self) -> bool {
        !self.text.trim().is_empty() || self.picked.is_some()
    }

    /// Whether the human has done nothing with it since it opened: no words, no answer picked,
    /// not made urgent, and addressed as it opened. Such a draft is the conversation's, and
    /// opening the conversation again opens it afresh.
    pub fn untouched(&self) -> bool {
        self.text.trim().is_empty()
            && self.picked.is_none()
            && !self.urgent
            && self.addressed == Addressed::Opening
    }

    /// The body a post of it carries: the text, or the picked answer first and the text as its
    /// note, as every reply that picks an answer is written.
    pub fn body(&self) -> String {
        match &self.picked {
            Some(picked) => posting::answer_body(&picked.answer, &self.text),
            None => self.text.clone(),
        }
    }
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
    let mut next = Draft {
        text: edit.text,
        to: edit.to,
        urgent: edit.urgent,
        addressed,
        picked: draft.picked.clone(),
    };
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
/// it, and says where the cursor goes: after that space. Done before a leading name is taken, so
/// a completion at the start of the text addresses the member and one later stays text, as
/// typing it would.
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

/// What picking answer `answer` of question `question`, counted from 0, makes of `draft`, or
/// `None` taking the pick back. A pick addresses the question's author, as Reply does, since an
/// answer goes to whoever asked. Picking the answer already picked takes it back.
pub fn pick(
    draft: &Draft,
    question: u64,
    answer: Option<usize>,
    group: &Group,
) -> Result<Draft, String> {
    let Some(answer) = answer else {
        return Ok(Draft { picked: None, ..draft.clone() });
    };
    let asked = group.log.entries().find_map(|entry| match &entry.what {
        What::Message(message) if entry.seq == question => Some(message),
        _ => None,
    });
    let Some(asked) = asked.filter(|message| group.asks_human(message)) else {
        return Err(format!(
            "#{question} is not a question asked of you in this conversation, so nothing was \
             picked"
        ));
    };
    if let Some((by, given)) = group.answers_given().get(&question) {
        return Err(format!(
            "#{question} was answered already, by {}: {given}; nothing was picked",
            group.shown(by)
        ));
    }
    let Some(words) = asked.answers.get(answer) else {
        return Err(format!(
            "#{question} offers {} answers, so answer {} is none of them; nothing was picked",
            asked.answers.len(),
            answer + 1
        ));
    };
    let picked = Answering { question, answer: words.clone() };
    if draft.picked.as_ref() == Some(&picked) {
        return Ok(Draft { picked: None, ..draft.clone() });
    }
    let to = if group.addressable(&asked.author) { vec![asked.author.clone()] } else { Vec::new() };
    Ok(Draft { to, addressed: Addressed::Chosen, picked: Some(picked), ..draft.clone() })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_is_untouched_until_the_human_writes_picks_marks_or_addresses_it() {
        let opened = Draft { to: vec!["otter".to_string()], ..Draft::default() };
        assert!(opened.untouched());
        assert!(Draft { text: " \n".to_string(), ..opened.clone() }.untouched());
        assert!(!Draft { text: "ok".to_string(), ..opened.clone() }.untouched());
        assert!(!Draft { urgent: true, ..opened.clone() }.untouched());
        assert!(!Draft { addressed: Addressed::Chosen, ..opened.clone() }.untouched());
        let picked = Answering { question: 4, answer: "Skip it".to_string() };
        assert!(!Draft { picked: Some(picked), ..opened }.untouched());
    }
}
