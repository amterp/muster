//! Reading a published view: the pane the keyboard is on, and the panes the tab's tree holds.

use muster::proto::{ViewChanged, ViewPane, ViewTree, view_tree};

/// The pane the keyboard feeds, or `None` while it feeds none.
pub(crate) fn keyboard_pane(view: &ViewChanged) -> Option<String> {
    Some(view.keyboard_pane_id.clone()).filter(|pane| !pane.is_empty())
}

/// Every leaf of a tree as the machine running it and its pane, in the order the tree lays
/// them out.
pub(crate) fn leaf_panes(tree: &ViewTree) -> Vec<(&str, &ViewPane)> {
    fn walk<'a>(tree: &'a ViewTree, into: &mut Vec<(&'a str, &'a ViewPane)>) {
        match &tree.node {
            Some(view_tree::Node::Pane(leaf)) => {
                if let Some(pane) = &leaf.pane {
                    into.push((&leaf.daemon_id, pane));
                }
            }
            Some(view_tree::Node::Split(split)) => {
                for child in split.first.iter().chain(split.second.iter()) {
                    walk(child, into);
                }
            }
            None => {}
        }
    }
    let mut found = Vec::new();
    walk(tree, &mut found);
    found
}

/// Every leaf of a tree as `(daemon, pane)`, in layout order.
pub(crate) fn leaves(tree: &ViewTree) -> Vec<(String, String)> {
    leaf_panes(tree)
        .into_iter()
        .map(|(daemon, pane)| (daemon.to_owned(), pane.pane_id.clone()))
        .collect()
}

/// Every pane the view's tree holds, in layout order.
pub(crate) fn panes(view: &ViewChanged) -> Vec<&ViewPane> {
    view.tree
        .as_ref()
        .map_or_else(Vec::new, |tree| leaf_panes(tree).into_iter().map(|(_, pane)| pane).collect())
}

/// The leaf for one pane, wherever in the tab's tree it is.
pub(crate) fn find_pane<'a>(view: &'a ViewChanged, pane: &str) -> Option<&'a ViewPane> {
    panes(view).into_iter().find(|leaf| leaf.pane_id == pane)
}

/// The leaf for one pane on one machine.
pub(crate) fn find_leaf<'a>(
    view: &'a ViewChanged,
    daemon: &str,
    pane: &str,
) -> Option<&'a ViewPane> {
    leaf_panes(view.tree.as_ref()?)
        .into_iter()
        .find(|(held, leaf)| *held == daemon && leaf.pane_id == pane)
        .map(|(_, leaf)| leaf)
}

/// The machine whose daemon runs a pane, as that pane's leaf names it.
pub(crate) fn daemon_of(view: &ViewChanged, pane: &str) -> Option<String> {
    let tree = view.tree.as_ref()?;
    leaf_panes(tree)
        .into_iter()
        .find(|(_, leaf)| leaf.pane_id == pane)
        .map(|(daemon, _)| daemon.to_owned())
}

/// A tree as text: `rows(p1@laptop, columns(p2@devenv, p3@laptop))`, without its ratios.
pub(crate) fn shape(tree: &ViewTree) -> String {
    match &tree.node {
        Some(view_tree::Node::Pane(leaf)) => {
            let pane = leaf.pane.as_ref().map_or("?", |pane| pane.pane_id.as_str());
            format!("{pane}@{}", leaf.daemon_id)
        }
        Some(view_tree::Node::Split(split)) => {
            let child =
                |child: &Option<Box<ViewTree>>| child.as_deref().map_or_else(String::new, shape);
            format!("{}({}, {})", split.axis, child(&split.first), child(&split.second))
        }
        None => String::new(),
    }
}
