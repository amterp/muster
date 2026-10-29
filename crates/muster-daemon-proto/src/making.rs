//! How a pane is asked for when no window is placing it: by the CLI's `muster pane new` with no
//! window answering, and by the API's `pane.new`. Both build their `Create` here, so the two
//! cannot place a pane differently.

use std::collections::HashMap;

use crate::{Label, Placement, Side, pane_request, placement};

/// A pane to make, as `muster pane new` describes one.
#[derive(Debug, Clone, Default)]
pub struct NewPane {
    /// The pane it starts from: beside it, or with `hidden` in its directory.
    pub beside: Option<String>,
    pub side: Side,
    /// The existing pane's share of a split. Absent: half.
    pub ratio: Option<f32>,
    /// In a hidden tab of its own, or for a helper in the tab its maker's helpers share.
    pub hidden: bool,
    pub quiet: bool,
    pub cwd: Option<String>,
    pub run: Option<String>,
    pub name: Option<String>,
    /// The pane it is a helper of, by its id.
    pub maker: Option<String>,
    pub env: HashMap<String, String>,
}

/// What the asker knows of a pane its daemon holds.
#[derive(Debug, Clone)]
pub struct Held {
    /// What the pane is called ([`crate::Pane::name`]).
    pub name: String,
    pub cwd: String,
}

/// The names the asker drew for the pane and for a tab it may need.
#[derive(Debug, Clone)]
pub struct Minted {
    pub pane: String,
    pub tab: String,
}

/// Why no `Create` was built.
#[derive(Debug, PartialEq, Eq)]
pub enum Unmade<E> {
    /// The daemon holds no pane by this id, and a pane to make starts from it.
    NotHeld(String),
    /// Looking a pane up failed.
    Lookup(E),
}

/// The `Create` that makes `new`, with `held` saying what the daemon holds of a pane by its id.
///
/// Beside the pane named, or with `hidden` in a hidden tab of its own that starts in that pane's
/// directory, or for a hidden helper in the tab its maker's helpers share. With no pane named it
/// gets a tab of its own, as a window showing nothing gives one.
pub fn create<E>(
    new: NewPane,
    mut held: impl FnMut(&str) -> Result<Option<Held>, E>,
    minted: Minted,
) -> Result<pane_request::Create, Unmade<E>> {
    let mut lookup = |pane: &str| match held(pane) {
        Ok(Some(found)) => Ok(found),
        Ok(None) => Err(Unmade::NotHeld(pane.to_string())),
        Err(error) => Err(Unmade::Lookup(error)),
    };
    // The maker's name, for the tab its hidden helpers share. One the daemon does not hold is
    // refused as a pane to start from is: nothing else can say it exists.
    let maker = match &new.maker {
        Some(maker) => Some((maker.clone(), lookup(maker)?.name)),
        None => None,
    };
    let mut cwd = new.cwd;
    let mut cwd_inherited = false;
    let placement = match &new.beside {
        Some(pane) if !new.hidden => placement::Where::Beside(placement::Beside {
            pane: pane.clone(),
            side: new.side.into(),
            ratio: new.ratio,
        }),
        _ => {
            if let Some(pane) = &new.beside {
                let from = lookup(pane)?;
                cwd_inherited = cwd.is_none() && !from.cwd.is_empty();
                cwd = cwd.or_else(|| Some(from.cwd).filter(|cwd| !cwd.is_empty()));
            }
            in_a_tab(new.hidden, maker.as_ref().map(|(_, name)| name.as_str()), minted.tab)
        }
    };
    Ok(pane_request::Create {
        pane: minted.pane,
        placement: Some(Placement { r#where: Some(placement) }),
        grid: None,
        cwd,
        cwd_inherited,
        env: new.env,
        command: new.run,
        label: new.name,
        quiet: new.quiet,
        maker: maker.map(|(pane, _)| pane),
        // Nothing that asks without a window sees another machine's panes.
        callsigns_elsewhere: Vec::new(),
    })
}

/// A new tab of its own, hidden or not, or for a hidden helper the tab its maker's helpers share,
/// named for the maker by `maker`, when there is one.
fn in_a_tab(hidden: bool, maker: Option<&str>, tab: String) -> placement::Where {
    match maker {
        Some(maker) if hidden => placement::Where::Helpers(placement::Helpers {
            tab,
            label: Some(Label {
                text: Some(muster_core::making::helpers_tab(maker)),
                generation: muster_core::making::HELPERS_TAB_GENERATION,
            }),
        }),
        _ => placement::Where::NewTab(placement::NewTab {
            tab,
            label: Some(Label::default()),
            hidden,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minted() -> Minted {
        Minted { pane: "p1".to_string(), tab: "t1".to_string() }
    }

    fn holding(panes: &[(&str, &str, &str)]) -> impl FnMut(&str) -> Result<Option<Held>, ()> {
        move |wanted| {
            Ok(panes
                .iter()
                .find(|(id, _, _)| *id == wanted)
                .map(|(_, name, cwd)| Held { name: (*name).to_string(), cwd: (*cwd).to_string() }))
        }
    }

    fn placed(create: &pane_request::Create) -> &placement::Where {
        create.placement.as_ref().and_then(|placement| placement.r#where.as_ref()).unwrap()
    }

    #[test]
    fn with_no_pane_named_it_gets_a_tab_of_its_own() {
        let new = NewPane { run: Some("make".to_string()), ..NewPane::default() };
        let create = create(new, holding(&[]), minted()).unwrap();
        assert_eq!(create.pane, "p1");
        assert_eq!(create.command.as_deref(), Some("make"));
        assert_eq!(
            placed(&create),
            &placement::Where::NewTab(placement::NewTab {
                tab: "t1".to_string(),
                label: Some(Label::default()),
                hidden: false,
            })
        );
        assert_eq!((create.cwd, create.cwd_inherited), (None, false));
    }

    #[test]
    fn beside_a_pane_it_is_a_split_and_the_pane_need_not_be_looked_up() {
        let new = NewPane {
            beside: Some("p0".to_string()),
            side: Side::Down,
            ratio: Some(0.3),
            ..NewPane::default()
        };
        let create = create(new, |_| Err::<Option<Held>, _>("asked"), minted()).unwrap();
        assert_eq!(
            placed(&create),
            &placement::Where::Beside(placement::Beside {
                pane: "p0".to_string(),
                side: Side::Down.into(),
                ratio: Some(0.3),
            })
        );
    }

    #[test]
    fn hidden_from_a_pane_it_starts_in_that_panes_directory_unless_one_is_named() {
        let held = [("p0", "otter", "/work")];
        let new = NewPane { beside: Some("p0".to_string()), hidden: true, ..NewPane::default() };
        let create = create(new.clone(), holding(&held), minted()).unwrap();
        assert_eq!((create.cwd.as_deref(), create.cwd_inherited), (Some("/work"), true));
        assert!(matches!(placed(&create), placement::Where::NewTab(tab) if tab.hidden));

        let named = NewPane { cwd: Some("/elsewhere".to_string()), ..new };
        let create = super::create(named, holding(&held), minted()).unwrap();
        assert_eq!((create.cwd.as_deref(), create.cwd_inherited), (Some("/elsewhere"), false));
    }

    #[test]
    fn a_hidden_helper_goes_in_the_tab_its_makers_helpers_share_named_for_the_maker() {
        let held = [("p0", "otter", "/work")];
        let new = NewPane {
            beside: Some("p0".to_string()),
            maker: Some("p0".to_string()),
            hidden: true,
            quiet: true,
            ..NewPane::default()
        };
        let create = create(new, holding(&held), minted()).unwrap();
        assert_eq!(create.maker.as_deref(), Some("p0"));
        assert!(create.quiet);
        assert_eq!(create.cwd.as_deref(), Some("/work"));
        assert_eq!(
            placed(&create),
            &placement::Where::Helpers(placement::Helpers {
                tab: "t1".to_string(),
                label: Some(Label {
                    text: Some(muster_core::making::helpers_tab("otter")),
                    generation: muster_core::making::HELPERS_TAB_GENERATION,
                }),
            })
        );
    }

    #[test]
    fn a_maker_or_a_pane_to_start_from_that_is_not_held_is_named() {
        let new = NewPane { maker: Some("p9".to_string()), ..NewPane::default() };
        assert_eq!(
            create(new, holding(&[]), minted()).unwrap_err(),
            Unmade::NotHeld("p9".to_string())
        );
        let new = NewPane { beside: Some("p8".to_string()), hidden: true, ..NewPane::default() };
        assert_eq!(
            create(new, holding(&[]), minted()).unwrap_err(),
            Unmade::NotHeld("p8".to_string())
        );
    }
}
