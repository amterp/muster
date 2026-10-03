//! A tab drawn as boxes, one per pane, in proportion and in the order the window lays them out.
//!
//! Pure: a tree and some text in, lines of a given width out, so every rule here is a unit test
//! against the literal drawing. The arrangement is the core's answer; what is decided here is
//! only how to fit it into a terminal's grid of characters.
//!
//! **Proportional where it can be, legible where it cannot.** Each divider goes where its ratio
//! puts it, unless that would leave a box too small for its name, and then as close as it can
//! get. A 70/30 split still reads as uneven; a split of a narrow pane into five still names all
//! five.
//!
//! **Borders are drawn as connections, not characters.** Each cell records which of its four
//! sides a line leaves by, and the glyph is chosen from that afterwards - so where two boxes
//! share an edge, or three meet, the junction is right by construction rather than by a case
//! somebody remembered.

use anstyle::Style;
use unicode_width::UnicodeWidthChar;

/// How a part of a tab divides, down to its panes.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A pane: an index into the boxes `draw` is given.
    Pane(usize),
    Split {
        /// Side by side when true, stacked when false.
        columns: bool,
        /// The first child's share, 0 to 1.
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// One machine's part of a tab: its share of the width, and its tree when the daemon has said.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub weight: f32,
    pub root: Option<Node>,
}

/// What a pane's box says, a line at a time, each line a run of styled pieces.
pub type Lines = Vec<Vec<(String, Style)>>;

/// The narrowest a box is let get inside its borders, which fits a pane's name.
const NARROWEST: usize = 12;

/// The fewest lines a drawing is given inside its outer border, so that a stack's proportions
/// have room to show.
const SHORTEST: usize = 10;

/// What a part says while its daemon has not said how the tab is arranged.
const UNARRANGED: &str = "not arranged yet";

/// The tab, `width` columns wide.
///
/// `boxes[i]` is what pane `i` says. A part with no tree is drawn as one box saying so.
pub fn draw(parts: &[Part], boxes: &[Lines], width: usize) -> Vec<String> {
    let unarranged: Lines = vec![vec![(UNARRANGED.to_string(), Style::new().dimmed())]];
    let wanted = |node: Option<&Node>| -> (usize, usize) {
        match node {
            Some(node) => least(node, boxes),
            None => (NARROWEST, 1),
        }
    };
    let tallest = parts.iter().map(|part| wanted(part.root.as_ref()).1).max().unwrap_or(1);
    let height = tallest.max(SHORTEST) + 2;
    let width = width.max(3);
    let mut canvas = Canvas::new(width, height);

    let spans = divide(
        0,
        width - 1,
        &parts.iter().map(|part| part.weight.max(0.0)).collect::<Vec<f32>>(),
        &parts.iter().map(|part| wanted(part.root.as_ref()).0).collect::<Vec<usize>>(),
    );
    for (part, (left, right)) in parts.iter().zip(spans) {
        let area = Area { left, top: 0, right, bottom: height - 1 };
        if let Some(root) = &part.root {
            lay(&mut canvas, root, area, boxes);
        } else {
            canvas.frame(area);
            canvas.write(area, &unarranged);
        }
    }
    canvas.lines()
}

/// The least a node needs inside its outer border: columns, then lines.
fn least(node: &Node, boxes: &[Lines]) -> (usize, usize) {
    match node {
        Node::Pane(index) => (NARROWEST, boxes.get(*index).map_or(1, Vec::len).max(1)),
        Node::Split { columns, first, second, .. } => {
            let (first, second) = (least(first, boxes), least(second, boxes));
            if *columns {
                (first.0 + second.0 + 1, first.1.max(second.1))
            } else {
                (first.0.max(second.0), first.1 + second.1 + 1)
            }
        }
    }
}

/// A box, by the coordinates of its borders, which it shares with whatever is beside it.
#[derive(Debug, Clone, Copy)]
struct Area {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

fn lay(canvas: &mut Canvas, node: &Node, area: Area, boxes: &[Lines]) {
    match node {
        Node::Pane(index) => {
            canvas.frame(area);
            if let Some(lines) = boxes.get(*index) {
                canvas.write(area, lines);
            }
        }
        Node::Split { columns: true, ratio, first, second } => {
            let at = between(
                area.left,
                area.right,
                *ratio,
                least(first, boxes).0,
                least(second, boxes).0,
            );
            lay(canvas, first, Area { right: at, ..area }, boxes);
            lay(canvas, second, Area { left: at, ..area }, boxes);
        }
        Node::Split { columns: false, ratio, first, second } => {
            let at = between(
                area.top,
                area.bottom,
                *ratio,
                least(first, boxes).1,
                least(second, boxes).1,
            );
            lay(canvas, first, Area { bottom: at, ..area }, boxes);
            lay(canvas, second, Area { top: at, ..area }, boxes);
        }
    }
}

/// Where a divider goes between two borders, `ratio` of the way along, moved only as far as it
/// takes to leave each side `first` and `second` inside it. Halfway when there is not room for
/// both, which is a drawing too narrow to be faithful and still the least wrong one.
fn between(low: usize, high: usize, ratio: f32, first: usize, second: usize) -> usize {
    let span = high - low;
    let earliest = low + first + 1;
    let latest = high.saturating_sub(second + 1);
    if earliest > latest {
        return low + span / 2;
    }
    (low + share(ratio, span)).clamp(earliest, latest)
}

/// `ratio` of `span` columns or lines, to the nearest whole one.
///
/// A terminal is at most a few hundred cells across, so neither float conversion can lose
/// anything, and the ratio is clamped first, so the result is never negative.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn share(ratio: f32, span: usize) -> usize {
    (ratio.clamp(0.0, 1.0) * span as f32).round() as usize
}

/// The borders between parts laid side by side, from their weights: each part's span, sharing
/// the border between it and the next.
fn divide(low: usize, high: usize, weights: &[f32], least: &[usize]) -> Vec<(usize, usize)> {
    let Some((weight, rest)) = weights.split_first() else { return Vec::new() };
    if rest.is_empty() {
        return vec![(low, high)];
    }
    let total: f32 = weights.iter().sum();
    let ratio = if total > 0.0 { weight / total } else { 0.5 };
    let others: usize = least[1..].iter().sum::<usize>() + least.len() - 2;
    let at = between(low, high, ratio, least[0], others);
    let mut spans = vec![(low, at)];
    spans.extend(divide(at, high, rest, &least[1..]));
    spans
}

/// Which sides of a cell a line leaves by.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

#[derive(Debug, Clone)]
enum Cell {
    Lines(u8),
    Text(char, Style),
    /// The second column of a character two columns wide.
    Covered,
}

struct Canvas {
    cells: Vec<Vec<Cell>>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Canvas {
        Canvas { cells: vec![vec![Cell::Lines(0); width]; height] }
    }

    fn join(&mut self, x: usize, y: usize, sides: u8) {
        if let Some(Cell::Lines(bits)) = self.cells.get_mut(y).and_then(|row| row.get_mut(x)) {
            *bits |= sides;
        }
    }

    fn frame(&mut self, area: Area) {
        for x in area.left..area.right {
            for y in [area.top, area.bottom] {
                self.join(x, y, RIGHT);
                self.join(x + 1, y, LEFT);
            }
        }
        for y in area.top..area.bottom {
            for x in [area.left, area.right] {
                self.join(x, y, DOWN);
                self.join(x, y + 1, UP);
            }
        }
    }

    /// Writes as many lines as fit inside a box, each cut to the box's width, one space in from
    /// its left border.
    fn write(&mut self, area: Area, lines: &Lines) {
        let inside = (area.right - area.left).saturating_sub(1);
        let room = inside.saturating_sub(2);
        let rows = (area.bottom - area.top).saturating_sub(1);
        for (row, line) in lines.iter().take(rows).enumerate() {
            let y = area.top + 1 + row;
            let mut x = area.left + 2;
            let end = area.left + 2 + room;
            'line: for (text, style) in line {
                for ch in text.chars() {
                    let wide = ch.width().unwrap_or(0);
                    if wide == 0 {
                        continue;
                    }
                    if x + wide > end {
                        break 'line;
                    }
                    self.cells[y][x] = Cell::Text(ch, *style);
                    if wide == 2 {
                        self.cells[y][x + 1] = Cell::Covered;
                    }
                    x += wide;
                }
            }
        }
    }

    fn lines(&self) -> Vec<String> {
        self.cells
            .iter()
            .map(|row| {
                let mut line = String::new();
                let mut open: Option<Style> = None;
                for cell in row {
                    let (text, style) = match cell {
                        Cell::Covered => continue,
                        Cell::Text(ch, style) => (ch.to_string(), *style),
                        Cell::Lines(bits) => (glyph(*bits).to_string(), Style::new()),
                    };
                    if open != Some(style) {
                        if let Some(was) = open {
                            line.push_str(&was.render_reset().to_string());
                        }
                        line.push_str(&style.render().to_string());
                        open = Some(style);
                    }
                    line.push_str(&text);
                }
                if let Some(was) = open {
                    line.push_str(&was.render_reset().to_string());
                }
                line
            })
            .collect()
    }
}

/// The box-drawing character for the sides a line leaves a cell by.
fn glyph(bits: u8) -> char {
    match bits {
        0 => ' ',
        b if b == UP | DOWN || b == UP || b == DOWN => '│',
        b if b == LEFT | RIGHT || b == LEFT || b == RIGHT => '─',
        b if b == DOWN | RIGHT => '┌',
        b if b == DOWN | LEFT => '┐',
        b if b == UP | RIGHT => '└',
        b if b == UP | LEFT => '┘',
        b if b == UP | DOWN | RIGHT => '├',
        b if b == UP | DOWN | LEFT => '┤',
        b if b == LEFT | RIGHT | DOWN => '┬',
        b if b == LEFT | RIGHT | UP => '┴',
        _ => '┼',
    }
}

#[cfg(test)]
mod tests {
    use super::{Lines, Node, Part, draw};
    use anstyle::Style;
    use unicode_width::UnicodeWidthStr;

    fn says(lines: &[&str]) -> Lines {
        lines.iter().map(|line| vec![((*line).to_string(), Style::new())]).collect()
    }

    fn split(columns: bool, ratio: f32, first: Node, second: Node) -> Node {
        Node::Split { columns, ratio, first: Box::new(first), second: Box::new(second) }
    }

    fn one(root: Node) -> Vec<Part> {
        vec![Part { weight: 1.0, root: Some(root) }]
    }

    /// The lines with any styling taken off, which is what a terminal shows of their layout.
    fn plain(lines: Vec<String>) -> Vec<String> {
        lines.into_iter().map(|line| anstream::adapter::strip_str(&line).to_string()).collect()
    }

    #[test]
    fn one_pane_fills_the_drawing() {
        let drawn = plain(draw(&one(Node::Pane(0)), &[says(&["p1"])], 20));
        assert_eq!(drawn.len(), 12, "the least height a drawing is given, borders included");
        assert_eq!(drawn[0], "┌──────────────────┐");
        assert_eq!(drawn[1], "│ p1               │");
        assert_eq!(drawn[2], "│                  │");
        assert_eq!(drawn[11], "└──────────────────┘");
    }

    #[test]
    fn panes_side_by_side_share_a_border_and_their_junctions_join() {
        let root = split(true, 0.5, Node::Pane(0), Node::Pane(1));
        let drawn = plain(draw(&one(root), &[says(&["a"]), says(&["b"])], 31));
        assert_eq!(drawn[0], "┌──────────────┬──────────────┐");
        assert_eq!(drawn[1], "│ a            │ b            │");
        assert_eq!(drawn[11], "└──────────────┴──────────────┘");
    }

    #[test]
    fn a_stack_inside_a_column_meets_it_with_tees() {
        let root = split(true, 0.5, Node::Pane(0), split(false, 0.5, Node::Pane(1), Node::Pane(2)));
        let drawn = plain(draw(&one(root), &[says(&["a"]), says(&["b"]), says(&["c"])], 31));
        let divider = drawn.iter().position(|line| line.contains('├')).expect("a tee");
        assert!(drawn[divider].ends_with('┤'), "{drawn:#?}");
        assert_eq!(drawn[divider].chars().nth(15), Some('├'), "{drawn:#?}");
        assert!(drawn[divider + 1].contains("│ c"), "the lower pane is named below: {drawn:#?}");
    }

    #[test]
    fn an_uneven_split_is_drawn_uneven() {
        let root = split(true, 0.7, Node::Pane(0), Node::Pane(1));
        let drawn = plain(draw(&one(root), &[says(&["a"]), says(&["b"])], 61));
        assert_eq!(drawn[0].chars().position(|ch| ch == '┬'), Some(42), "{drawn:#?}");
    }

    #[test]
    fn machines_side_by_side_take_their_weights() {
        let parts = vec![
            Part { weight: 2.0, root: Some(Node::Pane(0)) },
            Part { weight: 1.0, root: Some(Node::Pane(1)) },
        ];
        let drawn = plain(draw(&parts, &[says(&["laptop"]), says(&["devenv"])], 61));
        assert_eq!(drawn[0].chars().position(|ch| ch == '┬'), Some(40), "{drawn:#?}");
    }

    #[test]
    fn a_part_nobody_has_described_says_so() {
        let parts = vec![Part { weight: 1.0, root: None }];
        let drawn = plain(draw(&parts, &[], 30));
        assert!(drawn[1].contains("not arranged yet"), "{drawn:#?}");
    }

    #[test]
    fn text_too_long_for_its_box_is_cut_and_every_line_is_the_width_asked() {
        let root = split(true, 0.5, Node::Pane(0), Node::Pane(1));
        let boxes = [says(&["▸ p1w3r07bsd", "🤖 A · reading AGENTS.md all day"]), says(&["b"])];
        let drawn = plain(draw(&one(root), &boxes, 33));
        for line in &drawn {
            assert_eq!(line.width(), 33, "{line:?} in {drawn:#?}");
        }
        assert!(drawn[1].starts_with("│ ▸ p1w3r07bsd"), "{drawn:#?}");
        assert!(drawn[2].starts_with("│ 🤖 A · readi"), "{drawn:#?}");
    }

    #[test]
    fn a_split_too_narrow_to_honor_still_names_every_pane() {
        let root = split(true, 0.95, Node::Pane(0), Node::Pane(1));
        let drawn = plain(draw(&one(root), &[says(&["left"]), says(&["right"])], 40));
        assert!(drawn[1].contains("right"), "the narrow side keeps room for its name: {drawn:#?}");
    }
}
