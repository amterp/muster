import Foundation

/// What one window is showing, in the shell's own words.
///
/// The mirror image of the core's `convert.rs`: the seam's vocabulary is translated once,
/// here, and everything below works in these types. That is what lets the geometry be a pure
/// function a test can call with no protobuf, no core and no window - and the geometry is
/// where a rendering bug hides, because a wrong frame looks like a rendering problem and is
/// arithmetic.
public struct WindowContents: Equatable {
  /// A machine the tab's panes run on, as this window's bridges reach it.
  public struct Machine: Equatable {
    /// Where a bridge dials to draw a pane on this machine: the daemon's own socket, or for a
    /// daemon on another machine the local end of the ssh forward to it. Relayed onto the
    /// bridge's command line and never interpreted. Nil for a daemon the core has not reached.
    public let socket: String?

    /// Whether that daemon is on another machine. The only difference the shell ever notices
    /// between local and remote, and it only passes it on: a remote bridge asks for a window of
    /// unacknowledged output sized for the link.
    public let remote: Bool

    public init(socket: String?, remote: Bool = false) {
      self.socket = socket
      self.remote = remote
    }
  }

  /// The tab on screen, or nil when this window holds none it may open onto.
  public let tab: String?

  /// The tab as it is drawn: its whole tree over every machine it spans, or the one pane
  /// filling it while it is zoomed.
  ///
  /// Nil while no machine has said how the tab is arranged, which is an ordinary moment and a
  /// different answer from a tab with no panes: a window told nil leaves its surfaces alone,
  /// where one told "no panes" would tear down surfaces that are about to be described.
  public let tree: PaneTree?

  /// Whether `tree` is one pane filling the tab rather than its whole tree, which the chrome
  /// says: a zoomed tab and a tab with one pane are otherwise the same on screen.
  public let zoomed: Bool

  /// The pane the keyboard feeds. Nil when none does, which is what a window with no daemon
  /// behind it looks like.
  public let keyboard: PaneKey?

  /// Every machine a leaf of `tree` can name, by its daemon.
  public let machines: [String: Machine]

  public init(
    tab: String?, tree: PaneTree?, zoomed: Bool = false, keyboard: PaneKey?,
    machines: [String: Machine] = [:]
  ) {
    self.tab = tab
    self.tree = tree
    self.zoomed = zoomed
    self.keyboard = keyboard
    self.machines = machines
  }

  /// The panes this window will be showing once it has applied.
  ///
  /// Nil when no machine has said how the tab is arranged, on the same terms as `tree`: nil here
  /// means "whatever it already had" rather than "none".
  public var panes: Set<PaneKey>? {
    tree.map { Set($0.leaves.map(\.key)) }
  }
}

extension Roster {
  /// The seam's roster, in the shell's own words.
  init(_ changed: Muster_RosterChanged) {
    self.init(
      tabs: changed.tabs.map(Roster.Tab.init),
      machines: changed.machines.map { machine in
        Roster.Machine(
          id: machine.daemonID, state: machine.state, panes: Int(machine.panes),
          color: machine.color, restarted: machine.restarted)
      },
      numbering: Roster.Numbering(changed.counting),
      hidden: changed.hidden.map(Roster.Tab.init))
  }
}

extension Roster.Tab {
  init(_ tab: Muster_RosterTab) {
    self.init(
      id: tab.tabID,
      daemons: tab.daemonIds,
      place: Int(tab.place),
      press: Int(tab.tabPress),
      armed: tab.armed,
      label: tab.label,
      onScreen: tab.onScreen,
      givenName: tab.givenName,
      panes: tab.panes.map(Roster.Pane.init))
  }
}

extension Roster.Pane {
  init(_ pane: Muster_RosterPane) {
    self.init(
      key: PaneKey(daemon: pane.daemonID, pane: pane.paneID),
      place: Int(pane.place),
      tabPress: Int(pane.tabPress),
      press: Int(pane.panePress),
      label: pane.label,
      subtitle: pane.subtitle,
      givenName: pane.givenName,
      onScreen: pane.onScreen,
      compactable: pane.compactable,
      resumesAs: pane.resumesAs,
      quiet: pane.quiet,
      callsign: pane.callsign,
      besideCallsign: pane.besideCallsign,
      maker: pane.maker,
      under: pane.under)
  }
}

extension Roster.Numbering {
  /// What the chords are counting, in the shell's own words.
  init(_ counting: Muster_RosterChanged.Counting) {
    switch counting {
    case .tabs: self = .tabs
    case .panesInTab: self = .panesInTab
    // A counting this build has no word for is read as panes, which is the answer that
    // draws nothing extra. The alternative - guessing at a newer core's meaning - is a window
    // overlaying numbers for a gesture nobody made.
    case .panes, .UNRECOGNIZED: self = .panes
    }
  }
}

/// A pane, named the way a pane has to be named once a window shows two daemons.
///
/// Two daemons hand out the same ids, so anything the shell keys by pane alone - which agent
/// state belongs to which border - would let one machine's `w1:p1` answer for the other's.
public struct PaneKey: Hashable {
  public let daemon: String
  public let pane: String

  public init(daemon: String, pane: String) {
    self.daemon = daemon
    self.pane = pane
  }
}

/// How a split divides its area.
///
/// Muster's spelling, not a backend's: herdr said right and down, which describe the moment
/// of splitting rather than how to lay two children out long afterwards.
public enum SplitAxis: String, Equatable {
  case columns
  case rows
}

/// A tab's arrangement, over every machine it spans.
public indirect enum PaneTree: Equatable {
  case pane(Leaf)
  case split(axis: SplitAxis, ratio: CGFloat, first: PaneTree, second: PaneTree)

  public struct Leaf: Equatable {
    /// The machine running this pane, as `WindowContents.machines` names it.
    public let daemon: String
    public let paneID: String

    /// Where this pane's bridge reports that it attached, that it is painting, and why it
    /// exits. Nil means none is bound yet, and a bridge the window cannot hear from is one whose
    /// death nobody notices - so a window must not start a bridge until there is one.
    public let linkSocketPath: String?

    /// How big this pane's text is, in points away from what the config file asked for. Zero
    /// is a pane nobody has sized, which is most of them.
    ///
    /// On the leaf because that is what a surface is built from. A second message carrying
    /// sizes would be one this window had to join against the tree, and a pane would be drawn
    /// at the wrong size for as long as the two disagreed.
    public let fontSizeOffset: Int32

    /// How many times this window has replaced this pane's bridge, counting from zero.
    ///
    /// Two answers off one number, both decided by the core. A number that changed means this
    /// pane's bridge is gone and the surface running it has to be built again - a bridge is a
    /// surface's command, so there is no other way to start one. A number that is not zero
    /// means this window has had a bridge for this pane before, so its replacement takes the
    /// terminal over rather than being refused by whatever is still holding it; a first bridge
    /// never does, and so never takes one another window is legitimately showing.
    public let bridgeRestarts: UInt32

    public init(
      daemon: String = "local", paneID: String, linkSocketPath: String?,
      fontSizeOffset: Int32 = 0, bridgeRestarts: UInt32 = 0
    ) {
      // Defaulted so that a test describing a window on one machine does not have to say which.
      self.daemon = daemon
      self.paneID = paneID
      self.linkSocketPath = linkSocketPath
      // Defaulted so that a test describing a window it is not about the text size of does not
      // have to say so, on the same terms as a region's weight.
      self.fontSizeOffset = fontSizeOffset
      self.bridgeRestarts = bridgeRestarts
    }

    public var key: PaneKey { PaneKey(daemon: daemon, pane: paneID) }
  }

  /// Every pane in the tree, in reading order.
  public var leaves: [Leaf] {
    switch self {
    case .pane(let leaf): return [leaf]
    case .split(_, _, let first, let second): return first.leaves + second.leaves
    }
  }
}

/// Where each pane and each divider goes.
///
/// Top-left origin, matching the flipped container that renders it, so that "the first child
/// is the upper one" is what the arithmetic says rather than something to invert on the way
/// out.
public struct PaneFrames: Equatable {
  public struct Placement: Equatable {
    public let paneID: String
    public let frame: CGRect
  }

  public struct Divider: Equatable {
    /// The turns from the tab's root down to this split: false takes the first child,
    /// true the second. A divider has no id - it is a position in a shape that changes under
    /// it - so this is how a drag names it back to the daemon.
    public let path: [Bool]
    public let axis: SplitAxis
    public let frame: CGRect

    /// The rectangle the two children share, which is what turns a pointer position into a
    /// ratio. Carried rather than recomputed: the divider is the only thing that knows the
    /// area it divides, and a drag that guessed it would be wrong by the thickness.
    public let area: CGRect
  }

  public let panes: [Placement]
  public let dividers: [Divider]
}

extension PaneTree {
  /// How much room a divider takes, and its whole hit area.
  ///
  /// Thin enough to read as an edge rather than as furniture, and wide enough to grab. Panes
  /// get the rest, so the ratio means the share of the space panes actually have - which is
  /// what somebody dragging it is aiming at.
  public static let dividerThickness: CGFloat = 4

  /// The least room a pane may be squeezed into.
  ///
  /// A pane with no area is a surface libghostty is asked to render into nothing, and a PTY
  /// told it is zero columns wide. Neither is a state worth finding out about at render time,
  /// and a ratio of 0 or 1 is a state a daemon can legitimately publish.
  public static let minimumPaneSize: CGFloat = 8

  /// Where everything goes, for this tree in this rectangle.
  public func frames(in bounds: CGRect) -> PaneFrames {
    var panes: [PaneFrames.Placement] = []
    var dividers: [PaneFrames.Divider] = []
    place(in: bounds, path: [], panes: &panes, dividers: &dividers)
    return PaneFrames(panes: panes, dividers: dividers)
  }

  private func place(
    in rect: CGRect, path: [Bool],
    panes: inout [PaneFrames.Placement], dividers: inout [PaneFrames.Divider]
  ) {
    switch self {
    case .pane(let leaf):
      panes.append(PaneFrames.Placement(paneID: leaf.paneID, frame: rect))
    case .split(let axis, let ratio, let first, let second):
      let (firstRect, dividerRect, secondRect) = PaneTree.divide(rect, axis: axis, ratio: ratio)
      dividers.append(
        PaneFrames.Divider(path: path, axis: axis, frame: dividerRect, area: rect))
      first.place(in: firstRect, path: path + [false], panes: &panes, dividers: &dividers)
      second.place(in: secondRect, path: path + [true], panes: &panes, dividers: &dividers)
    }
  }

  /// Cuts a rectangle in two, with the divider between them.
  static func divide(_ rect: CGRect, axis: SplitAxis, ratio: CGFloat) -> (CGRect, CGRect, CGRect) {
    switch axis {
    case .columns:
      let usable = max(0, rect.width - dividerThickness)
      let first = share(ratio, of: usable)
      return (
        CGRect(x: rect.minX, y: rect.minY, width: first, height: rect.height),
        CGRect(x: rect.minX + first, y: rect.minY, width: dividerThickness, height: rect.height),
        CGRect(
          x: rect.minX + first + dividerThickness, y: rect.minY,
          width: usable - first, height: rect.height)
      )
    case .rows:
      let usable = max(0, rect.height - dividerThickness)
      let first = share(ratio, of: usable)
      return (
        CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: first),
        CGRect(x: rect.minX, y: rect.minY + first, width: rect.width, height: dividerThickness),
        CGRect(
          x: rect.minX, y: rect.minY + first + dividerThickness,
          width: rect.width, height: usable - first)
      )
    }
  }

  /// The first child's size, kept inside what the area can actually hold.
  ///
  /// A ratio arrives from a daemon and is not this window's to validate, so it is honored
  /// wherever it can be and clamped where it cannot. NaN is the one value with no sensible
  /// reading, and half is a better answer than a frame the layout engine rejects.
  private static func share(_ ratio: CGFloat, of usable: CGFloat) -> CGFloat {
    guard usable > 0 else { return 0 }
    let wanted = usable * min(max(ratio.isFinite ? ratio : 0.5, 0), 1)
    let floor = min(minimumPaneSize, usable / 2)
    return min(max(wanted, floor), usable - floor).rounded()
  }

  /// The ratio a pointer at this position is asking for.
  ///
  /// The inverse of `divide`, and it has to stay the inverse: a drag that computed a ratio
  /// the layout then places somewhere else makes the divider crawl away from the pointer.
  public static func ratio(at point: CGPoint, in area: CGRect, axis: SplitAxis) -> CGFloat {
    let (position, span) =
      switch axis {
      case .columns: (point.x - area.minX, area.width)
      case .rows: (point.y - area.minY, area.height)
      }
    let usable = span - dividerThickness
    guard usable > 0 else { return 0.5 }
    return min(max((position - dividerThickness / 2) / usable, 0), 1)
  }
}

// The seam's vocabulary, translated once. Internal because the generated types are, and they
// stop at this module's edge.

extension WindowContents {
  init(_ changed: Muster_ViewChanged) {
    let whole = changed.hasTree ? PaneTree(changed.tree) : nil
    // Resolved here, so a window that draws `tree` paints the one pane filling a zoomed tab. A
    // zoom on a pane the tree does not hold fills nothing, and the whole tree is drawn.
    let filling =
      changed.zoomedPaneID.isEmpty
      ? nil : whole?.leaves.first { $0.paneID == changed.zoomedPaneID }
    // Pane ids are unique across machines, so the keyboard's machine is the leaf's.
    let keyboard = whole?.leaves.first { $0.paneID == changed.keyboardPaneID }
    self.init(
      tab: changed.tabID.isEmpty ? nil : changed.tabID,
      tree: filling.map(PaneTree.pane) ?? whole,
      zoomed: filling != nil,
      keyboard: keyboard?.key,
      machines: Dictionary(
        changed.machines.map { machine in
          (
            machine.daemonID,
            Machine(
              socket: machine.daemonSocket.isEmpty ? nil : machine.daemonSocket,
              remote: machine.remote)
          )
        },
        uniquingKeysWith: { first, _ in first }))
  }
}

extension PaneTree {
  /// Reads a node, treating an unset one as a pane with no id.
  ///
  /// A node with neither arm set cannot happen - the core sets one on every path - and if it
  /// ever does, an empty leaf renders as a gap rather than crashing a window.
  init(_ node: Muster_ViewTree) {
    switch node.node {
    case .pane(let leaf):
      let pane = leaf.pane
      self = .pane(
        Leaf(
          daemon: leaf.daemonID,
          paneID: pane.paneID,
          linkSocketPath: pane.linkSocketPath.isEmpty ? nil : pane.linkSocketPath,
          fontSizeOffset: pane.fontSizeOffset,
          bridgeRestarts: pane.bridgeRestarts))
    case .split(let split):
      self = .split(
        axis: SplitAxis(rawValue: split.axis) ?? .columns,
        ratio: CGFloat(split.ratio),
        first: PaneTree(split.first),
        second: PaneTree(split.second))
    case nil:
      self = .pane(Leaf(daemon: "", paneID: "", linkSocketPath: nil))
    }
  }
}
