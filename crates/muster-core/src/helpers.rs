//! Which helpers a list of panes shows under their maker (MIP-7, section 3), decided once for
//! the agent list and for `muster window`, so that the two never disagree about where a helper
//! is. And whether a quiet helper asks for the person after all, decided once for a window and
//! for the daemon keeping what waits on the human (MIP-10, section 4).

/// Whether a pane's maker can hear from it, as whoever asks can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerHearing {
    /// The pane is no helper, or its maker has closed.
    None,
    /// The maker's pane holds an agent, on a machine that answers.
    Hears,
    /// The maker's pane holds no agent, or its machine is not answering.
    CannotHear,
}

/// Whether a pane asks for nobody: it is quiet, and it is not a helper whose maker cannot hear
/// it. Such a helper asks for the person as if it were not quiet, since nobody else can act
/// (MIP-7, section 4).
#[must_use]
pub fn quiet_in_effect(quiet: bool, maker: MakerHearing) -> bool {
    quiet && maker != MakerHearing::CannotHear
}

/// A helper listed under a pane rather than where its tab would put it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nested<'a> {
    pub helper: &'a str,
    /// The pane whose row it is listed beneath: its maker.
    pub under: &'a str,
    /// How many levels in its row is, one for a helper of a pane that keeps its own row.
    pub depth: usize,
}

/// Which of `hidden`'s helpers go under their maker, given every pane in `listed` that has a row.
///
/// `listed` is each pane a list shows, hidden ones among them, in the order it shows them;
/// `hidden` is each hidden pane with the pane that made it, if one did. A hidden helper goes
/// under its maker when the maker has a row: a pane in a listed tab, or a hidden pane, a helper
/// among them. A helper in a tab somebody can see keeps its place there, and one whose maker is
/// not listed - closed, or on a machine or in a window the list leaves out - has nowhere else to
/// go.
///
/// Walked down from every pane that keeps a row of its own, so each helper is listed once and
/// none is lost to a loop of makers, which no daemon writes and nothing here relies on. Helpers
/// come out in the order the walk meets them, each maker's in the order `hidden` gives them.
pub fn nest<'a>(listed: &[&'a str], hidden: &[(&'a str, Option<&'a str>)]) -> Vec<Nested<'a>> {
    let children: Vec<(&str, &str)> = hidden
        .iter()
        .filter_map(|(helper, maker)| Some((*helper, (*maker)?)))
        .filter(|(_, maker)| listed.contains(maker))
        .collect();
    let goes_under = |pane: &str| children.iter().any(|(helper, _)| *helper == pane);

    let mut nested: Vec<Nested<'a>> = Vec::new();
    let mut waiting: Vec<(&'a str, usize)> =
        listed.iter().rev().filter(|pane| !goes_under(pane)).map(|pane| (*pane, 0)).collect();
    while let Some((maker, depth)) = waiting.pop() {
        let mut under: Vec<&'a str> = Vec::new();
        for (helper, _) in children.iter().filter(|(_, of)| *of == maker) {
            if !nested.iter().any(|nested| nested.helper == *helper) {
                nested.push(Nested { helper, under: maker, depth: depth + 1 });
                under.push(helper);
            }
        }
        waiting.extend(under.into_iter().rev().map(|helper| (helper, depth + 1)));
    }
    nested
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(nested: &[Nested<'_>]) -> Vec<String> {
        nested.iter().map(|n| format!("{} under {} at {}", n.helper, n.under, n.depth)).collect()
    }

    #[test]
    fn a_hidden_helper_goes_under_a_maker_that_has_a_row() {
        let listed = ["p1", "p2", "p3"];
        let hidden = [("p2", Some("p1")), ("p3", None)];
        assert_eq!(said(&nest(&listed, &hidden)), ["p2 under p1 at 1"]);
    }

    #[test]
    fn a_helpers_own_helpers_go_under_it_a_level_further_in() {
        let listed = ["p1", "p2", "p3", "p4"];
        let hidden = [("p2", Some("p1")), ("p3", Some("p2")), ("p4", Some("p1"))];
        assert_eq!(
            said(&nest(&listed, &hidden)),
            ["p2 under p1 at 1", "p4 under p1 at 1", "p3 under p2 at 2"]
        );
    }

    #[test]
    fn a_helper_whose_maker_has_no_row_stays_where_it_is() {
        let listed = ["p2"];
        let hidden = [("p2", Some("p1"))];
        assert!(nest(&listed, &hidden).is_empty());
    }

    #[test]
    fn a_quiet_helper_asks_only_while_its_maker_cannot_hear_it() {
        assert!(quiet_in_effect(true, MakerHearing::None));
        assert!(quiet_in_effect(true, MakerHearing::Hears));
        assert!(!quiet_in_effect(true, MakerHearing::CannotHear));
        assert!(!quiet_in_effect(false, MakerHearing::Hears));
    }

    #[test]
    fn a_loop_of_makers_stays_where_it_is() {
        let listed = ["p0", "p1", "p2"];
        let hidden = [("p1", Some("p2")), ("p2", Some("p1"))];
        assert!(nest(&listed, &hidden).is_empty(), "neither keeps a row to start the walk from");
    }
}
