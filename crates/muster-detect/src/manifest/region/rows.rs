use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use super::Region;

const MAX_ROW_SELECTORS: usize = 32;
const MAX_REGIONS: usize = 32;
const MAX_NAME_BYTES: usize = 64;
const MAX_OFFSET: i32 = 32;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::manifest) struct RawRow {
    id: String,
    regex: String,
    #[serde(default)]
    take: Take,
    nth: Option<usize>,
    start: Option<RawEdge>,
    end: Option<RawEdge>,
    reject_after: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::manifest) struct RawRegion {
    start: Option<RawEdge>,
    end: Option<RawEdge>,
    line: Option<String>,
    #[serde(default)]
    otherwise: Fallback,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEdge {
    row: String,
    #[serde(default)]
    offset: i32,
    if_missing: Option<EdgeOfScreen>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Take {
    First,
    #[default]
    Last,
    RunStart,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EdgeOfScreen {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Fallback {
    #[default]
    Empty,
    WholeRecent,
}

#[derive(Debug, Clone)]
struct FindRow {
    regex: Regex,
    take: Take,
    nth: usize,
    start: Option<Edge>,
    end: Option<Edge>,
    reject_after: Option<Regex>,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    row: usize,
    offset: i32,
    if_missing: Option<EdgeOfScreen>,
}

#[derive(Debug, Clone, Copy)]
enum Select {
    Range { start: Option<Edge>, end: Option<Edge> },
    Line(usize),
}

#[derive(Debug, Clone, Copy)]
struct RowRegion {
    select: Select,
    otherwise: Fallback,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LocatedRow {
    Unsearched,
    Missing,
    Found(usize),
}

#[derive(Debug, Clone, Default)]
pub(in crate::manifest) struct Definitions {
    selectors: Vec<FindRow>,
    regions: Vec<(String, RowRegion)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    rows: Vec<RawRow>,
    regions: BTreeMap<String, RawRegion>,
}

pub(super) fn legacy() -> &'static Definitions {
    static DEFS: OnceLock<Definitions> = OnceLock::new();
    DEFS.get_or_init(|| {
        let raw: Legacy =
            toml::from_str(include_str!("legacy.toml")).expect("bundled legacy regions");
        Definitions::compile(raw.rows, raw.regions).expect("valid bundled legacy regions")
    })
}

impl Definitions {
    pub(in crate::manifest) fn new(
        rows: Vec<RawRow>,
        regions: BTreeMap<String, RawRegion>,
    ) -> Result<Self, String> {
        for name in regions.keys() {
            if Region::fixed(name).is_some() || legacy().index(name).is_some() {
                return Err(format!("region {name} is a built-in region name"));
            }
        }
        Self::compile(rows, regions)
    }

    fn compile(rows: Vec<RawRow>, regions: BTreeMap<String, RawRegion>) -> Result<Self, String> {
        if rows.len() > MAX_ROW_SELECTORS || regions.len() > MAX_REGIONS {
            return Err(format!(
                "a manifest can define up to {MAX_ROW_SELECTORS} row selectors and {MAX_REGIONS} regions"
            ));
        }
        let mut selectors = Vec::with_capacity(rows.len());
        let mut indices = HashMap::with_capacity(rows.len());
        for row in rows {
            valid_name(&row.id).map_err(|why| format!("row {} {why}", row.id))?;
            let selector =
                compile_row(&row, &indices).map_err(|why| format!("row {} {why}", row.id))?;
            if indices.insert(row.id.clone(), selectors.len()).is_some() {
                return Err(format!("row {} is defined more than once", row.id));
            }
            selectors.push(selector);
        }
        let mut compiled = Vec::with_capacity(regions.len());
        for (name, region) in regions {
            valid_name(&name).map_err(|why| format!("region {name} {why}"))?;
            let select =
                compile_region(&region, &indices).map_err(|why| format!("region {name} {why}"))?;
            compiled.push((name, RowRegion { select, otherwise: region.otherwise }));
        }
        Ok(Self { selectors, regions: compiled })
    }

    pub(super) fn index(&self, name: &str) -> Option<usize> {
        self.regions.iter().position(|(found, _)| found == name)
    }

    pub(super) fn slice<'a>(
        &self,
        index: usize,
        text: &'a str,
        lines: &[&'a str],
        selected: &mut [LocatedRow],
    ) -> &'a str {
        let region = self.regions[index].1;
        let span = match region.select {
            Select::Range { start, end } => {
                let start = self.edge(start, 0, lines, selected);
                let end = self.edge(end, lines.len(), lines, selected);
                start.zip(end).map(|(start, end)| {
                    let offset = |index| {
                        lines.get(index).map_or(text.len(), |line: &&str| {
                            (line.as_ptr() as usize) - (text.as_ptr() as usize)
                        })
                    };
                    &text[offset(start)..offset(end.max(start))]
                })
            }
            Select::Line(row) => self.find(row, lines, selected).map(|index| lines[index]),
        };
        span.unwrap_or(match region.otherwise {
            Fallback::Empty => "",
            Fallback::WholeRecent => text,
        })
    }

    pub(super) fn row_count(&self) -> usize {
        self.selectors.len()
    }

    fn edge(
        &self,
        edge: Option<Edge>,
        default: usize,
        lines: &[&str],
        selected: &mut [LocatedRow],
    ) -> Option<usize> {
        let Some(edge) = edge else { return Some(default) };
        match self.find(edge.row, lines, selected) {
            Some(row) => Some(row.saturating_add_signed(edge.offset as isize).min(lines.len())),
            None => edge.if_missing.map(|side| match side {
                EdgeOfScreen::Top => 0,
                EdgeOfScreen::Bottom => lines.len(),
            }),
        }
    }

    fn find(&self, index: usize, lines: &[&str], selected: &mut [LocatedRow]) -> Option<usize> {
        match selected[index] {
            LocatedRow::Found(row) => return Some(row),
            LocatedRow::Missing => return None,
            LocatedRow::Unsearched => {}
        }
        let rule = &self.selectors[index];
        let bounds = self.edge(rule.start, 0, lines, selected).zip(self.edge(
            rule.end,
            lines.len(),
            lines,
            selected,
        ));
        let found = bounds.and_then(|(start, end)| rule.find_in(lines, start, end.max(start)));
        selected[index] = found.map_or(LocatedRow::Missing, LocatedRow::Found);
        found
    }
}

impl FindRow {
    fn find_in(&self, lines: &[&str], start: usize, end: usize) -> Option<usize> {
        let rows = &lines[start..end];
        let matches = |line: &&str| self.regex.is_match(line);
        let offset = match self.take {
            Take::First => {
                rows.iter().enumerate().filter(|(_, row)| matches(row)).nth(self.nth - 1)?.0
            }
            Take::Last => {
                rows.iter().enumerate().rev().filter(|(_, row)| matches(row)).nth(self.nth - 1)?.0
            }
            Take::RunStart => {
                let count = rows.iter().rev().take_while(|row| matches(row)).count();
                if count == 0 {
                    return None;
                }
                rows.len() - count
            }
        };
        let index = start + offset;
        if self
            .reject_after
            .as_ref()
            .is_some_and(|pattern| rows[offset + 1..].iter().any(|line| pattern.is_match(line)))
        {
            return None;
        }
        Some(index)
    }
}

fn compile_row(row: &RawRow, indices: &HashMap<String, usize>) -> Result<FindRow, String> {
    let nth = row.nth.unwrap_or(1);
    if !(1..=usize::from(u16::MAX)).contains(&nth) {
        return Err("has an nth outside 1..=65535".into());
    }
    if matches!(row.take, Take::RunStart) && row.nth.is_some() {
        return Err("cannot use nth with run_start".into());
    }
    let regex = compile_regex(&row.regex, "regex")?;
    let reject_after = row
        .reject_after
        .as_deref()
        .map(|pattern| compile_regex(pattern, "reject_after"))
        .transpose()?;
    Ok(FindRow {
        regex,
        take: row.take,
        nth,
        start: compile_edge(row.start.as_ref(), indices)?,
        end: compile_edge(row.end.as_ref(), indices)?,
        reject_after,
    })
}

fn compile_region(region: &RawRegion, indices: &HashMap<String, usize>) -> Result<Select, String> {
    if let Some(line) = &region.line {
        if region.start.is_some() || region.end.is_some() {
            return Err("cannot use line with start or end".into());
        }
        return lookup(line, indices).map(Select::Line);
    }
    Ok(Select::Range {
        start: compile_edge(region.start.as_ref(), indices)?,
        end: compile_edge(region.end.as_ref(), indices)?,
    })
}

fn compile_edge(
    edge: Option<&RawEdge>,
    indices: &HashMap<String, usize>,
) -> Result<Option<Edge>, String> {
    edge.map(|edge| {
        if !(-MAX_OFFSET..=MAX_OFFSET).contains(&edge.offset) {
            return Err(format!("has a row offset outside -{MAX_OFFSET}..={MAX_OFFSET}"));
        }
        Ok(Edge {
            row: lookup(&edge.row, indices)?,
            offset: edge.offset,
            if_missing: edge.if_missing,
        })
    })
    .transpose()
}

fn compile_regex(pattern: &str, field: &str) -> Result<Regex, String> {
    if pattern.is_empty() || pattern.chars().count() > super::super::MAX_MATCHER_CHARS {
        return Err(format!("has an empty or too long {field}"));
    }
    Regex::new(pattern).map_err(|error| format!("has an invalid {field}: {error}"))
}

fn lookup(name: &str, indices: &HashMap<String, usize>) -> Result<usize, String> {
    indices.get(name).copied().ok_or_else(|| format!("refers to undefined row {name}"))
}

fn valid_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("must have a name of up to 64 lowercase ASCII letters, digits and underscores");
    }
    Ok(())
}
