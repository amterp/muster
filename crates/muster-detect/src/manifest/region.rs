//! The named slices of a screen a rule can read.
//!
//! Ported from herdr v0.8.0 `src/detect/manifest.rs` (`region()` and the functions under it;
//! Apache-2.0), and changed: a region is parsed once into this enum instead of from its name on
//! every evaluation. The slicing itself is herdr's to the byte, since a manifest's rules only
//! mean what they meant to herdr if every region cuts the text where herdr's did.

use super::Input;

/// herdr's bound on `top_non_empty_lines`, kept so a manifest valid there is valid here.
const MAX_TOP_REGION_LINE_COUNT: usize = u16::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    WholeRecent,
    AfterLastPromptMarker,
    BeforeCurrentPromptMarker,
    WholeRecentWithoutCurrentPromptMarker,
    CurrentPromptBlockMarker,
    AfterCurrentPromptBlockMarker,
    CurrentPrompt,
    PromptBoxBody,
    AbovePromptBox,
    LastNonEmptyAbovePromptBox,
    AfterLastHorizontalRule,
    BottomLines(usize),
    BottomNonEmptyLines(usize),
    TopNonEmptyLines(usize),
    OscTitle,
    OscProgress,
}

impl Region {
    pub(super) fn parse(spec: &str) -> Option<Region> {
        let trimmed = spec.trim();
        Some(match trimmed {
            "whole_recent" => Region::WholeRecent,
            "after_last_prompt_marker" => Region::AfterLastPromptMarker,
            "before_current_prompt_marker" => Region::BeforeCurrentPromptMarker,
            "whole_recent_without_current_prompt_marker" => {
                Region::WholeRecentWithoutCurrentPromptMarker
            }
            "current_prompt_block_marker" => Region::CurrentPromptBlockMarker,
            "after_current_prompt_block_marker" => Region::AfterCurrentPromptBlockMarker,
            "current_prompt" => Region::CurrentPrompt,
            "prompt_box_body" => Region::PromptBoxBody,
            "above_prompt_box" => Region::AbovePromptBox,
            "last_non_empty_above_prompt_box" => Region::LastNonEmptyAbovePromptBox,
            "after_last_horizontal_rule" => Region::AfterLastHorizontalRule,
            "osc_title" => Region::OscTitle,
            "osc_progress" => Region::OscProgress,
            _ => {
                if let Some(count) = region_count(trimmed, "bottom_lines") {
                    Region::BottomLines(count)
                } else if let Some(count) = region_count(trimmed, "bottom_non_empty_lines") {
                    Region::BottomNonEmptyLines(count)
                } else {
                    Region::TopNonEmptyLines(top_region_count(trimmed)?)
                }
            }
        })
    }

    pub(super) fn slice(self, input: Input<'_>) -> &str {
        let content = input.screen;
        match self {
            Region::OscTitle => input.title,
            Region::OscProgress => input.progress,
            Region::WholeRecent => content,
            Region::AfterLastPromptMarker => after_last_prompt_marker(content),
            Region::BeforeCurrentPromptMarker => before_current_prompt_marker(content),
            Region::WholeRecentWithoutCurrentPromptMarker => {
                whole_recent_without_current_prompt_marker(content)
            }
            Region::CurrentPromptBlockMarker => current_prompt_block_marker(content).unwrap_or(""),
            Region::AfterCurrentPromptBlockMarker => {
                after_current_prompt_block_marker(content).unwrap_or("")
            }
            Region::CurrentPrompt => current_prompt(content),
            Region::PromptBoxBody => prompt_box_body(content).unwrap_or(""),
            Region::AbovePromptBox => above_prompt_box(content),
            Region::LastNonEmptyAbovePromptBox => last_non_empty_line(above_prompt_box(content)),
            Region::AfterLastHorizontalRule => after_last_horizontal_rule(content),
            Region::BottomLines(count) => bottom_lines(content, count),
            Region::BottomNonEmptyLines(count) => bottom_non_empty_lines(content, count),
            Region::TopNonEmptyLines(count) => top_non_empty_lines(content, count),
        }
    }
}

fn region_count(spec: &str, name: &str) -> Option<usize> {
    spec.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?.parse::<usize>().ok()
}

/// Stricter than `region_count`, as herdr's is: a count with a sign or a leading zero is not
/// the canonical spelling of a number, and engine 3 refused it.
fn top_region_count(spec: &str) -> Option<usize> {
    let count = spec.strip_prefix("top_non_empty_lines")?.strip_prefix('(')?.strip_suffix(')')?;
    if count.starts_with('0') || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    count.parse::<usize>().ok().filter(|count| *count <= MAX_TOP_REGION_LINE_COUNT)
}

fn bottom_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    slice_from_line_index(content, &lines, start)
}

fn bottom_non_empty_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start_index) = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(count)
        .last()
        .map(|(index, _)| index)
    else {
        return "";
    };
    slice_from_line_index(content, &lines, start_index)
}

fn top_non_empty_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(end_index) = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(count)
        .last()
        .map(|(index, _)| index)
    else {
        return "";
    };
    &content[..line_start_offset(content, &lines, end_index + 1)]
}

fn after_last_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = lines.iter().rposition(|line| codex_prompt_line(line)) else {
        return content;
    };
    slice_from_line_index(content, &lines, index + 1)
}

fn before_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = current_codex_prompt_index(&lines) else {
        return content;
    };
    let byte_offset = lines[..index].iter().map(|line| line.len() + 1).sum::<usize>();
    &content[..byte_offset.min(content.len())]
}

fn whole_recent_without_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    if current_codex_prompt_index(&lines).is_some() { "" } else { content }
}

fn current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    lines[..prompt_index].iter().rev().find(|line| codex_block_marker_line(line)).copied()
}

fn after_current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    let block_index =
        lines[..prompt_index].iter().rposition(|line| codex_block_marker_line(line))?;
    Some(slice_from_line_index(content, &lines, block_index))
}

/// Muster's, not herdr's: Codex's prompt line and the lines that continue it, down to the blank
/// line above the footer Codex draws under its composer - what is typed into Codex and nothing
/// else, where `after_last_prompt_marker` runs on into the footer.
fn current_prompt(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = current_codex_prompt_index(&lines) else {
        return "";
    };
    let end = lines[index..]
        .iter()
        .position(|line| line.trim().is_empty())
        .map_or(lines.len(), |relative| index + relative);
    let start = line_start_offset(content, &lines, index);
    &content[start..line_start_offset(content, &lines, end).max(start)]
}

/// Codex's prompt line, unless a block started after it - then the prompt on screen is an old
/// one scrolled up, not the one being typed into.
fn current_codex_prompt_index(lines: &[&str]) -> Option<usize> {
    let prompt_index = lines.iter().rposition(|line| codex_prompt_line(line))?;
    if lines[prompt_index + 1..].iter().any(|line| codex_block_marker_line(line)) {
        return None;
    }
    Some(prompt_index)
}

fn codex_prompt_line(line: &str) -> bool {
    line == "›" || line.starts_with("› ")
}

fn codex_block_marker_line(line: &str) -> bool {
    line.starts_with('•') || line.starts_with('■') || line.starts_with('✗') || line.starts_with('✓')
}

fn prompt_box_body(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let top = prompt_box_top_border_index(&lines)?;
    let start = line_start_offset(content, &lines, top + 1);
    let end_index = lines[top + 1..]
        .iter()
        .position(|line| is_horizontal_rule(line))
        .map_or(lines.len(), |relative| top + 1 + relative);
    let end = line_start_offset(content, &lines, end_index);
    Some(&content[start.min(content.len())..end.min(content.len())])
}

fn above_prompt_box(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(top) = prompt_box_top_border_index(&lines) else {
        return content;
    };
    &content[..line_start_offset(content, &lines, top).min(content.len())]
}

fn after_last_horizontal_rule(content: &str) -> &str {
    let mut last_rule_end = 0usize;
    let mut offset = 0usize;
    for line in content.lines() {
        let next_offset = offset + line.len() + 1;
        if is_horizontal_rule(line) {
            last_rule_end = next_offset.min(content.len());
        }
        offset = next_offset;
    }
    &content[last_rule_end..]
}

fn last_non_empty_line(content: &str) -> &str {
    content.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("")
}

/// The second horizontal rule up from the bottom: a prompt box is drawn between two.
fn prompt_box_top_border_index(lines: &[&str]) -> Option<usize> {
    let mut border_count = 0;
    for index in (0..lines.len()).rev() {
        if is_horizontal_rule(lines[index]) {
            border_count += 1;
            if border_count == 2 {
                return Some(index);
            }
        }
    }
    None
}

/// A line of `─`, possibly with a label after at least three of them - the way Claude Code and
/// Devin title the border of a prompt box.
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let rule_chars = trimmed.chars().take_while(|&ch| ch == '─').count();
    if rule_chars == 0 {
        return false;
    }
    let rule_bytes = trimmed.char_indices().nth(rule_chars).map_or(trimmed.len(), |(i, _)| i);
    trimmed[rule_bytes..].trim_start().is_empty() || rule_chars >= 3
}

fn slice_from_line_index<'a>(content: &'a str, lines: &[&str], index: usize) -> &'a str {
    &content[line_start_offset(content, lines, index).min(content.len())..]
}

/// Where line `index` starts, counting one byte per separator - the screen text is joined
/// with `\n` alone, so this is exact.
fn line_start_offset(content: &str, lines: &[&str], index: usize) -> usize {
    lines[..index.min(lines.len())]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>()
        .min(content.len())
}
