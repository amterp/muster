//! A problem's remedy, as a case spells it: `reattach local/p1w3r07bsd`, `restart local/p1`.
//!
//! One spelling for the three corpora that name remedies - the problem list itself and the two
//! watches that raise problems with one - so that a remedy reads the same in each.

use conformance::CaseError;
use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::problems::Remedy;
use serde_json::{Value, json};

pub(crate) fn spell(remedy: Option<&Remedy>) -> Value {
    match remedy {
        Some(Remedy::Reattach(pane)) => json!(format!("reattach {pane}")),
        Some(Remedy::Restart(pane)) => json!(format!("restart {pane}")),
        None => Value::Null,
    }
}

pub(crate) fn parse(spelled: &str) -> Result<Remedy, CaseError> {
    let refused = || {
        CaseError::new(format!(
            "`{spelled}` is not a remedy - it wants `reattach daemon/pane` or `restart daemon/pane`"
        ))
    };
    let (remedy, pane) = spelled.split_once(' ').ok_or_else(refused)?;
    let (daemon, pane) = pane.split_once('/').ok_or_else(refused)?;
    let pane = PaneKey::new(&DaemonId::new(daemon), &PaneId::new(pane));
    match remedy {
        "reattach" => Ok(Remedy::Reattach(pane)),
        "restart" => Ok(Remedy::Restart(pane)),
        _ => Err(refused()),
    }
}
