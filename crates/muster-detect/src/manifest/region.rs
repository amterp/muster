//! The named slices of a screen a rule can read.
//!
//! The fixed regions and legacy region semantics are from herdr v0.8.0
//! (`src/detect/manifest.rs`, Apache-2.0), plus Muster's prompt regions.

mod rows;

use super::Input;
use rows::LocatedRow;
pub(super) use rows::{Definitions, RawRegion, RawRow};

const MAX_TOP_REGION_LINE_COUNT: usize = u16::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    WholeRecent,
    BottomLines(usize),
    BottomNonEmptyLines(usize),
    TopNonEmptyLines(usize),
    OscTitle,
    OscProgress,
    Legacy(usize),
    Defined(usize),
}

impl Region {
    pub(super) fn parse(spec: &str, definitions: &Definitions) -> Option<Region> {
        let name = spec.trim();
        Self::fixed(name)
            .or_else(|| definitions.index(name).map(Self::Defined))
            .or_else(|| rows::legacy().index(name).map(Self::Legacy))
    }

    fn fixed(name: &str) -> Option<Region> {
        Some(match name {
            "whole_recent" => Self::WholeRecent,
            "osc_title" => Self::OscTitle,
            "osc_progress" => Self::OscProgress,
            _ => {
                if let Some(count) = region_count(name, "bottom_lines") {
                    Self::BottomLines(count)
                } else if let Some(count) = region_count(name, "bottom_non_empty_lines") {
                    Self::BottomNonEmptyLines(count)
                } else {
                    Self::TopNonEmptyLines(top_region_count(name)?)
                }
            }
        })
    }
}

/// Reuses row selections when several rules or the prompt read the same screen.
pub(super) struct Screen<'a, 'm> {
    input: Input<'a>,
    definitions: &'m Definitions,
    lines: Option<Vec<&'a str>>,
    legacy_rows: Vec<LocatedRow>,
    defined_rows: Vec<LocatedRow>,
}

impl<'a, 'm> Screen<'a, 'm> {
    pub(super) fn new(input: Input<'a>, definitions: &'m Definitions) -> Self {
        Self { input, definitions, lines: None, legacy_rows: Vec::new(), defined_rows: Vec::new() }
    }

    pub(super) fn text(&self) -> &'a str {
        self.input.screen
    }

    pub(super) fn slice(&mut self, region: Region, with_osc: bool) -> &'a str {
        let text = self.input.screen;
        let (definitions, selected, index) = match region {
            Region::OscTitle => return if with_osc { self.input.title } else { "" },
            Region::OscProgress => return if with_osc { self.input.progress } else { "" },
            Region::WholeRecent => return text,
            Region::BottomLines(count) => return bottom_lines(text, count),
            Region::BottomNonEmptyLines(count) => return bottom_non_empty_lines(text, count),
            Region::TopNonEmptyLines(count) => return top_non_empty_lines(text, count),
            Region::Legacy(index) => (rows::legacy(), &mut self.legacy_rows, index),
            Region::Defined(index) => (self.definitions, &mut self.defined_rows, index),
        };
        let lines = self.lines.get_or_insert_with(|| text.lines().collect());
        selected.resize(definitions.row_count(), LocatedRow::Unsearched);
        definitions.slice(index, text, lines, selected)
    }
}

fn region_count(spec: &str, name: &str) -> Option<usize> {
    spec.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?.parse::<usize>().ok()
}

/// The canonical spelling from engine 3 excludes signs and leading zeroes.
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
    &content[line_start_offset(content, &lines, start)..]
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
    &content[line_start_offset(content, &lines, start_index)..]
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

/// The daemon joins screen rows with `\n` alone.
fn line_start_offset(content: &str, lines: &[&str], index: usize) -> usize {
    lines[..index.min(lines.len())]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>()
        .min(content.len())
}
