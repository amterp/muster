import Foundation

/// What ending this window's sessions would actually end, in words.
///
/// Pure, and separate from the sheet that shows it, for the reason every other decision in this
/// module is separate from its view: what a person is told before the one irreversible action in
/// Muster is worth having cases for, and a sheet needs a window and a run loop.
///
/// **It names directories rather than counting panes.** "3 panes" is a number somebody agrees to;
/// `~/src/muster, ~/src/nook` is a thing they recognise, and recognising it is the whole point.
/// A daemon Muster adopted rather than started is called out for the same reason: it was already
/// running when this window opened, so what is in it may predate the window and belong to
/// somebody who is not looking at this sheet (kan a_28YghIUw2).
@MainActor
public enum QuitSummary {
  /// The heading: what is about to happen, in one line.
  public static func question(machines: [Core.Machine]) -> String {
    let panes = machines.reduce(0) { $0 + $1.panes }
    switch panes {
    case 0: return "Quit and close sessions?"
    case 1: return "Quit and close 1 pane?"
    default: return "Quit and close \(panes) panes?"
    }
  }

  /// The body: what would end, and what it means for whatever is running in it.
  public static func body(machines: [Core.Machine]) -> String {
    guard !machines.isEmpty else {
      return "This window is attached to no machines, so nothing would be ended. Quitting "
        + "normally does the same thing."
    }
    var said = machines.map(line(for:))
    said.append(
      "Every pane's process is asked to stop and given a moment to finish - it is a hangup "
        + "rather than a kill, and a harness that handles one gets to act on it. Anything "
        + "mid-task does not finish. Quitting normally leaves all of this running.")
    return said.joined(separator: "\n\n")
  }

  private static func line(for machine: Core.Machine) -> String {
    let host = machine.host.isEmpty ? "this machine" : machine.host
    let panes =
      switch machine.panes {
      case 0: "no panes"
      case 1: "1 pane"
      default: "\(machine.panes) panes"
      }
    var line = "\(machine.daemon) on \(host): \(panes)"
    if !machine.directories.isEmpty {
      line += " in \(machine.directories.joined(separator: ", "))"
    }
    // The one thing that separates a session this window made from one it walked into.
    if !machine.startedByMuster {
      line +=
        ". This session was already running when the window opened, so it may hold work "
        + "that started somewhere else."
    }
    return line
  }
}
