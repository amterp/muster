//! `api.read_pane`: a page of a pane's text, read by the same `pane.read` the CLI asks for, so
//! the two cannot page differently.

use muster_api as api;
use muster_daemon_proto as proto;
use proto::pane_request;

/// The `pane.read` that reads the page `asked` names.
pub(crate) fn asked(asked: &api::ReadPane) -> pane_request::Read {
    use api::read_pane::Rows;
    let read = pane_request::Read { pane: asked.pane.clone(), ..pane_request::Read::default() };
    match &asked.rows {
        // As many of the newest rows as a page holds: the page is cut at its size, newest kept.
        None | Some(Rows::Newest(0)) => pane_request::Read { last: u32::MAX, ..read },
        Some(Rows::Newest(rows)) => pane_request::Read { last: *rows, ..read },
        Some(Rows::From(from)) => {
            pane_request::Read { first_row: from.row, rows: from.count, ..read }
        }
        Some(Rows::Turn(api::LastTurn {})) => pane_request::Read { turn: true, ..read },
    }
}

/// `text` as the API says it.
pub(crate) fn page(text: proto::PaneText) -> api::PaneText {
    api::PaneText {
        text: text.text,
        first_row: text.first_row,
        rows: text.rows,
        oldest_row: text.oldest_row,
        end_row: text.total_rows,
        turn_row: text.turn,
        turn_moved: text.turn_moved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::read_pane::Rows;

    fn read(rows: Option<Rows>) -> pane_request::Read {
        asked(&api::ReadPane { pane: "p1".to_string(), rows })
    }

    #[test]
    fn each_way_of_choosing_rows_asks_for_one_page() {
        let newest_page = read(None);
        assert_eq!((newest_page.pane.as_str(), newest_page.last), ("p1", u32::MAX));
        assert_eq!(read(Some(Rows::Newest(0))), newest_page);
        assert_eq!(read(Some(Rows::Newest(20))).last, 20);
        let from = read(Some(Rows::From(api::RowsFrom { row: 40, count: 3 })));
        assert_eq!((from.first_row, from.rows, from.last, from.turn), (40, 3, 0, false));
        let turn = read(Some(Rows::Turn(api::LastTurn {})));
        assert_eq!((turn.turn, turn.last), (true, 0));
        assert!(!turn.prompt, "a read never reads the prompt");
    }

    #[test]
    fn a_page_says_where_the_pane_ends_and_where_its_turn_began() {
        let text = proto::PaneText {
            text: "a\nb".to_string(),
            first_row: 7,
            rows: 2,
            total_rows: 12,
            oldest_row: 3,
            turn: Some(6),
            turn_moved: true,
            prompt: Some("draft".to_string()),
        };
        let page = page(text);
        assert_eq!((page.first_row, page.rows, page.oldest_row, page.end_row), (7, 2, 3, 12));
        assert_eq!((page.turn_row, page.turn_moved, page.text.as_str()), (Some(6), true, "a\nb"));
    }
}
