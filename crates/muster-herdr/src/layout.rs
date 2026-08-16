//! herdr's rectangles, rebuilt into Muster's tree - and its own tree, taken as it comes.
//!
//! herdr publishes a tab's arrangement twice and neither form is the one a view wants.
//! `layout.export` has the tree but none of the live fields and costs a request of its own;
//! `layout_updated` and a snapshot's `layouts[]` are live and free but describe the
//! arrangement as a flat list of pane rectangles plus a flat list of split borders, with no
//! parent or child links between them.
//!
//! Both are read here, because both arrive without being asked for. Everything that pushes
//! uses the rectangles, and so does every mutation that answers with a settled arrangement -
//! except `layout.set_split_ratio`, which answers with the exported tree. That one is a
//! dragged divider, the highest-frequency arrangement change there is, so the second reader is
//! what keeps a drag on the answer rather than on the broadcast a hundred milliseconds later.
//!
//! Rebuilding the tree from those rectangles is exact, and recorded as such: for every
//! split in the exported tree there is a border covering exactly the panes beneath it, with
//! the same axis and the same ratio, and that held at every size up to sixteen panes
//! (`observations/herdr-0.8.0.md` section 13). So this is the whole adapter for structure,
//! and nothing has to ask a second time.
//!
//! The rectangles stop here. They are cells in a terminal area herdr keeps for itself -
//! fixed whether a client is attached or not - so what leaves this module is proportions.

use muster_core::mirror::backend::{Layout, LayoutNode, PaneId, SplitAxis, TabId};
use serde_json::Value;

/// A rectangle in herdr's own coordinate space.
///
/// Deliberately private. Every number in one is about a terminal nobody is looking at, and
/// the moment one escapes into the core it looks like geometry a window could use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl Rect {
    fn end_x(self) -> i64 {
        self.x + self.width
    }

    fn end_y(self) -> i64 {
        self.y + self.height
    }

    fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            width: self.end_x().max(other.end_x()) - x,
            height: self.end_y().max(other.end_y()) - y,
        }
    }
}

/// One of herdr's split borders: a divider, and the area it divides.
struct Border {
    axis: SplitAxis,
    ratio: f32,
    rect: Rect,
}

/// Reads one tab's arrangement, from a snapshot's `layouts[]` or a `layout_updated`.
///
/// The two carry the same object, which is why there is one reader. `None` means the tab's
/// arrangement did not read - a missing id, or rectangles that do not describe a tree - and
/// the caller keeps whatever it had rather than rendering a tab as empty.
pub fn read_layout(value: &Value) -> Option<Layout> {
    let tab = TabId::new(non_empty(value, "tab_id")?);

    let mut panes = Vec::new();
    for pane in value.get("panes").and_then(Value::as_array).into_iter().flatten() {
        let id = PaneId::new(non_empty(pane, "pane_id")?);
        panes.push((id, rect(pane.get("rect")?)?));
    }
    // A tab always has at least one pane, so an empty list is a payload that will not
    // describe anything rather than a tab that happens to be bare.
    let (_, first_rect) = panes.first()?;
    let root_rect = panes.iter().fold(*first_rect, |whole, (_, rect)| whole.union(*rect));

    let mut borders = Vec::new();
    for border in value.get("splits").and_then(Value::as_array).into_iter().flatten() {
        borders.push(Border {
            axis: axis(border.get("direction").and_then(Value::as_str)?)?,
            ratio: ratio(border)?,
            rect: rect(border.get("rect")?)?,
        });
    }

    Some(assemble(value, tab, build(root_rect, &panes, &borders)?))
}

/// Reads one tab's arrangement from herdr's exported tree.
///
/// The second shape herdr states an arrangement in, and the reason there are two readers
/// rather than one. `layout.export` publishes it, and so does the result of
/// `layout.set_split_ratio` - which is the one that matters, because a client that reads its
/// own answer sees a dragged divider about a hundred milliseconds before the broadcast
/// describing it arrives (`observations/herdr-0.8.0.md` section 14).
///
/// Easier than the rectangles and not a replacement for them: the tree is exact but carries no
/// live fields, so everything that arrives unasked-for is still the flat shape. The two are
/// told apart by which key they have - `panes` against `root` - rather than by asking the
/// caller, so a verb that starts answering with either needs no change here.
pub fn read_exported_layout(value: &Value) -> Option<Layout> {
    let tab = TabId::new(non_empty(value, "tab_id")?);
    Some(assemble(value, tab, read_node(value.get("root")?)?))
}

/// One node of an exported tree, and everything under it.
///
/// A node states its own kind, so an unknown one is `None` rather than a guess: a shape this
/// does not recognise is a herdr that publishes something new, and rendering half of it would
/// put panes in places no daemon agreed to.
fn read_node(value: &Value) -> Option<LayoutNode> {
    match value.get("type").and_then(Value::as_str)? {
        "pane" => Some(LayoutNode::Pane(PaneId::new(non_empty(value, "pane_id")?))),
        "split" => Some(LayoutNode::Split {
            axis: axis(value.get("direction").and_then(Value::as_str)?)?,
            ratio: ratio(value)?,
            first: Box::new(read_node(value.get("first")?)?),
            second: Box::new(read_node(value.get("second")?)?),
        }),
        _ => None,
    }
}

/// The fields both shapes carry, put around whichever tree was read.
///
/// Total, because everything that can fail has already failed by here: a tab and a tree are
/// what a reader has to produce, and the rest is a cursor and a flag that mean the same thing
/// absent as they do false.
fn assemble(value: &Value, tab: TabId, root: LayoutNode) -> Layout {
    let focused = value.get("focused_pane_id").and_then(Value::as_str).map(PaneId::new);
    Layout {
        tab,
        root,
        // Only when something is zoomed, and then it is the tab's focused pane: herdr
        // publishes a bare flag and leaves every pane at its ordinary rect, so this is the
        // one place the two get put together (`observations/herdr-0.8.0.md` section 13).
        zoomed: if value.get("zoomed").and_then(Value::as_bool) == Some(true) {
            focused.clone()
        } else {
            None
        },
        focused,
    }
}

/// The node covering exactly this rectangle, or nothing if the rectangles do not agree.
///
/// The root is the union of every pane rather than the layout's own `area`, because the
/// area is a property of herdr's window and the union is a property of the panes. They are
/// the same today; only one of them stays true if herdr ever reserves space of its own.
fn build(rect: Rect, panes: &[(PaneId, Rect)], borders: &[Border]) -> Option<LayoutNode> {
    if let Some((id, _)) = panes.iter().find(|(_, pane)| *pane == rect) {
        return Some(LayoutNode::Pane(id.clone()));
    }
    let border = borders.iter().find(|border| border.rect == rect)?;

    // The first child is the largest thing that starts where this rectangle starts and
    // spans it across the axis without filling it along the axis. Largest, because a
    // grandchild flush against the same corner also qualifies, and it is always smaller.
    let (first_rect, second_rect) = match border.axis {
        SplitAxis::Columns => {
            let width = candidates(panes, borders)
                .filter(|c| c.x == rect.x && c.y == rect.y && c.height == rect.height)
                .map(|c| c.width)
                .filter(|width| *width < rect.width)
                .max()?;
            (Rect { width, ..rect }, Rect { x: rect.x + width, width: rect.width - width, ..rect })
        }
        SplitAxis::Rows => {
            let height = candidates(panes, borders)
                .filter(|c| c.x == rect.x && c.y == rect.y && c.width == rect.width)
                .map(|c| c.height)
                .filter(|height| *height < rect.height)
                .max()?;
            (
                Rect { height, ..rect },
                Rect { y: rect.y + height, height: rect.height - height, ..rect },
            )
        }
    };

    Some(LayoutNode::Split {
        axis: border.axis,
        ratio: border.ratio,
        first: Box::new(build(first_rect, panes, borders)?),
        second: Box::new(build(second_rect, panes, borders)?),
    })
}

fn candidates<'a>(
    panes: &'a [(PaneId, Rect)],
    borders: &'a [Border],
) -> impl Iterator<Item = Rect> + 'a {
    panes.iter().map(|(_, rect)| *rect).chain(borders.iter().map(|border| border.rect))
}

/// herdr names a split for where the new pane went; a view needs to know how to arrange
/// two children that were split long ago.
fn axis(direction: &str) -> Option<SplitAxis> {
    match direction {
        "right" => Some(SplitAxis::Columns),
        "down" => Some(SplitAxis::Rows),
        _ => None,
    }
}

/// A divider's position, taken from the border rather than divided out of the rectangles.
///
/// The rectangles are rounded to whole cells, so computing this from them turns 0.3 of a
/// 54-column area into 0.296 - not wrong enough to look wrong, and wrong again every time
/// a divider moves.
///
/// The narrowing is the point rather than a risk taken: herdr holds these as `f32` and JSON
/// has only one number type, so this recovers the value the daemon actually has instead of
/// carrying a wider one that would print its own rounding noise back at a reader.
#[allow(clippy::cast_possible_truncation)]
fn ratio(border: &Value) -> Option<f32> {
    Some(border.get("ratio").and_then(Value::as_f64)? as f32)
}

fn rect(value: &Value) -> Option<Rect> {
    Some(Rect {
        x: value.get("x")?.as_i64()?,
        y: value.get("y")?.as_i64()?,
        width: value.get("width")?.as_i64()?,
        height: value.get("height")?.as_i64()?,
    })
}

fn non_empty(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
}
