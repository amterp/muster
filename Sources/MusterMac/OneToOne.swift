import Foundation

/// Where a conversation between the person and one agent is drawn (Fencil
/// `one-to-one-on-agent-row`): on the agent's row when this window draws that row, else in
/// no row, where the Conversations menus still reach it (`ConversationsMenu`).
///
/// The core says which pane is a group's partner and says it the same to every window; this side
/// decides from the one window's own roster, so a pane moved to another window takes its
/// conversation with it, and a pane that is gone leaves the conversation to the menus.
public enum OneToOne {
  /// The groups that pane rows in `drawn` claim, by pane.
  ///
  /// Two groups can name one partner - a pair left over from an earlier pane of the same callsign
  /// beside the current one - and a row carries one badge: the group with more unread claims it,
  /// the one listed first on a tie, and the other is in that pane's Conversations menu, where it can still be
  /// reached.
  public static func claims(
    groups: [SidebarModel.MessageGroup], among drawn: Set<PaneKey>
  ) -> [PaneKey: SidebarModel.MessageGroup] {
    var claimed: [PaneKey: SidebarModel.MessageGroup] = [:]
    for group in groups {
      guard let partner = group.partnerPane, drawn.contains(partner) else { continue }
      if let held = claimed[partner], held.unread >= group.unread { continue }
      claimed[partner] = group
    }
    return claimed
  }

  /// What to call the agent a group is a conversation with, or nil when this window has no pane
  /// for it: a banner and a card are then named as the group is.
  public static func name(
    daemon: String, group: String, groups: [SidebarModel.MessageGroup], roster: Roster
  ) -> String? {
    guard
      let partner = groups.first(where: { $0.daemon == daemon && $0.group == group })?.partnerPane,
      let pane = roster.pane(partner)
    else { return nil }
    return pane.callsign.isEmpty ? pane.label : pane.callsign
  }
}
