import Foundation

/// What one window is showing, in the shell's own words.
///
/// The mirror image of the core's `convert.rs`: the seam's vocabulary is translated once,
/// here, and everything below works in these types. That is what lets the geometry be a pure
/// function a test can call with no protobuf, no core and no window - and the geometry is
/// where a rendering bug hides, because a wrong frame looks like a rendering problem and is
/// arithmetic.
public struct WindowContents: Equatable {
  public struct Region: Equatable {
    public let id: String

    /// Which daemon this region's tab lives on. Carried down to every pane in it rather than
    /// repeated per pane, because a leaf only ever arrives inside the region that names it.
    public let daemon: String
    public let tab: String

    /// The pane in this region Muster's keyboard feeds while the region is focused. Nil
    /// while the daemon has not said what is in this tab.
    public let keyboardPane: String?

    /// How much of the window's width this region gets, relative to the others. A weight
    /// rather than a fraction, so laying the strip out is a division by the sum and no
    /// region needs to know another's business.
    public let weight: CGFloat

    /// Nil while the daemon has not said how this tab is arranged, which is an ordinary
    /// moment and a different answer from a tab with no panes: a window told nil leaves its
    /// surfaces alone, where one told "no panes" would tear down surfaces that are about to
    /// be described.
    public let tree: PaneTree?

    /// Whether `tree` is one pane filling the region rather than the tab's whole tree.
    /// Already resolved by the core, so a window that ignores this renders the right thing.
    public let zoomed: Bool

    /// How this region's panes are reached, when they are on another machine. Nil is a daemon
    /// on this one, which is the only difference the shell ever notices between local and
    /// remote: these are relayed onto the bridge's command line and nothing else changes.
    public let transport: Transport?

    /// Which daemon this region's frame streams come from, on this machine.
    ///
    /// Relayed onto the bridge's command line, because a bridge that found a daemon for
    /// itself would find whichever one is on the default socket - and Muster runs its own on
    /// a session of its own. Nil for a remote region: that bridge asks the far machine, where
    /// a path from this one names nothing.
    public let backendSocket: String?

    public struct Transport: Equatable {
      public let sshHost: String
      public let sshControlPath: String

      public init(sshHost: String, sshControlPath: String) {
        self.sshHost = sshHost
        self.sshControlPath = sshControlPath
      }
    }

    public init(
      id: String, daemon: String, tab: String, keyboardPane: String?, weight: CGFloat = 1,
      tree: PaneTree?, zoomed: Bool, transport: Transport? = nil, backendSocket: String? = nil
    ) {
      self.id = id
      self.daemon = daemon
      self.tab = tab
      self.backendSocket = backendSocket
      self.keyboardPane = keyboardPane
      // Defaulted so that a test describing a window it is not about the widths of does not
      // have to say so. Equal shares are what every region starts at.
      self.weight = weight
      self.tree = tree
      self.zoomed = zoomed
      self.transport = transport
    }
  }

  /// In the order they are laid out, side by side. Muster owns no split tree over regions.
  public let regions: [Region]

  /// Which region's pane the keyboard feeds. Nil when none does, which is what a window with
  /// no daemon behind it looks like.
  public let focusedRegion: String?

  public init(regions: [Region], focusedRegion: String?) {
    self.regions = regions
    self.focusedRegion = focusedRegion
  }

  /// The pane the keyboard feeds, if the focused region names one.
  public var keyboardPane: String? {
    guard let focusedRegion else { return nil }
    return regions.first { $0.id == focusedRegion }?.keyboardPane
  }
}

extension Roster {
  /// The seam's roster, in the shell's own words.
  init(_ changed: Muster_RosterChanged) {
    let daemons = changed.daemons.map { daemon in
      Roster.Daemon(
        id: daemon.daemonID,
        tabs: daemon.tabs.map { tab in
          Roster.Tab(
            key: TabKey(daemon: tab.daemonID, tab: tab.tabID),
            place: Int(tab.place),
            number: Int(tab.number),
            label: tab.label,
            onScreen: tab.onScreen,
            givenName: tab.givenName,
            panes: tab.panes.map { pane in
              Roster.Pane(
                key: PaneKey(daemon: pane.daemonID, pane: pane.paneID),
                place: Int(pane.place),
                number: Int(pane.number),
                label: pane.label,
                subtitle: pane.subtitle,
                givenName: pane.givenName,
                onScreen: pane.onScreen)
            })
        })
    }
    self.init(daemons: daemons, numbering: Roster.Numbering(changed.counting))
  }
}

extension Roster.Numbering {
  /// What the chords are counting, in the shell's own words.
  init(_ counting: Muster_RosterChanged.Counting) {
    switch counting {
    case .tabs: self = .tabs
    case .panesInTab: self = .panesInTab
    // A counting this build has no word for is the settled scheme, which is the answer that
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
/// Inside a single region the pane id is enough, because a region shows one daemon; this is
/// for the maps that span them.
public struct PaneKey: Hashable {
  public let daemon: String
  public let pane: String

  public init(daemon: String, pane: String) {
    self.daemon = daemon
    self.pane = pane
  }
}

/// A tab, named on the same terms as a pane and for the same reason.
public struct TabKey: Hashable {
  public let daemon: String
  public let tab: String

  public init(daemon: String, tab: String) {
    self.daemon = daemon
    self.tab = tab
  }
}

/// How a split divides its area.
///
/// Muster's spelling, not a backend's: herdr says right and down, which describe the moment
/// of splitting rather than how to lay two children out long afterwards.
public enum SplitAxis: String, Equatable {
  case columns
  case rows
}

/// One region's arrangement.
public indirect enum PaneTree: Equatable {
  case pane(Leaf)
  case split(axis: SplitAxis, ratio: CGFloat, first: PaneTree, second: PaneTree)

  public struct Leaf: Equatable {
    public let paneID: String

    /// Where this pane's bridge dials back. Nil means no channel is open for this pane yet,
    /// and a surface spawned against it would render and never be typeable - so a window
    /// must not start a bridge pointed at one.
    public let controlSocketPath: String?

    /// What the pane's own daemon calls it, for the bridge's command line.
    ///
    /// The bridge streams frames from the daemon directly, so it is the one thing up here that
    /// speaks the backend's vocabulary. Never used to address a pane: `paneID` is what every
    /// request takes, and the two differ.
    public let backendPaneID: String

    /// How big this pane's text is, in points away from what the config file asked for. Zero
    /// is a pane nobody has sized, which is most of them.
    ///
    /// On the leaf because that is what a surface is built from. A second message carrying
    /// sizes would be one this window had to join against the tree, and a pane would be drawn
    /// at the wrong size for as long as the two disagreed.
    public let fontSizeOffset: Int32

    public init(
      paneID: String, controlSocketPath: String?, backendPaneID: String = "",
      fontSizeOffset: Int32 = 0
    ) {
      self.paneID = paneID
      self.controlSocketPath = controlSocketPath
      self.backendPaneID = backendPaneID
      // Defaulted so that a test describing a window it is not about the text size of does not
      // have to say so, on the same terms as a region's weight.
      self.fontSizeOffset = fontSizeOffset
    }
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
    /// The turns from the region's root down to this split: false takes the first child,
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
    self.init(
      regions: changed.regions.map { region in
        Region(
          id: region.regionID,
          daemon: region.daemonID,
          tab: region.tabID,
          // Proto3 spells absence as the empty string, and here the two genuinely differ:
          // no pane named is a region whose tab the daemon has not described yet.
          keyboardPane: region.paneID.isEmpty ? nil : region.paneID,
          weight: CGFloat(region.weight),
          tree: region.hasRoot ? PaneTree(region.root) : nil,
          zoomed: region.zoomed,
          // Both or neither: half a target names no machine, and the core sends both when it
          // has opened a connection at all.
          transport: region.sshHost.isEmpty || region.sshControlPath.isEmpty
            ? nil
            : Region.Transport(sshHost: region.sshHost, sshControlPath: region.sshControlPath),
          backendSocket: region.backendSocket.isEmpty ? nil : region.backendSocket)
      },
      focusedRegion: changed.focusedRegion.isEmpty ? nil : changed.focusedRegion)
  }
}

extension PaneTree {
  /// Reads a node, treating an unset one as a pane with no id.
  ///
  /// A node with neither arm set cannot happen - the core sets one on every path - and if it
  /// ever does, an empty leaf renders as a gap rather than crashing a window.
  init(_ node: Muster_ViewNode) {
    switch node.node {
    case .pane(let pane):
      self = .pane(
        Leaf(
          paneID: pane.paneID,
          controlSocketPath: pane.controlSocketPath.isEmpty ? nil : pane.controlSocketPath,
          backendPaneID: pane.backendPaneID,
          fontSizeOffset: pane.fontSizeOffset))
    case .split(let split):
      self = .split(
        axis: SplitAxis(rawValue: split.axis) ?? .columns,
        ratio: CGFloat(split.ratio),
        first: PaneTree(split.first),
        second: PaneTree(split.second))
    case nil:
      self = .pane(Leaf(paneID: "", controlSocketPath: nil))
    }
  }
}
