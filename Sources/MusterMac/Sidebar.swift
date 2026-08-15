import AppKit

/// Every pane every attached daemon holds, as the core listed them.
///
/// The shell's mirror of `RosterChanged`, translated once here the way `WindowContents`
/// mirrors the view. Order and labels arrive decided: a shell that sorted or named for
/// itself would be a second place those answers live.
public struct Roster: Equatable {
  public struct Pane: Equatable {
    public let key: PaneKey
    public let tab: String

    /// What to call this pane to somebody who did not open it.
    public let label: String

    /// Whether a region is showing it right now.
    public let onScreen: Bool

    public init(key: PaneKey, tab: String, label: String, onScreen: Bool) {
      self.key = key
      self.tab = tab
      self.label = label
      self.onScreen = onScreen
    }
  }

  public let panes: [Pane]

  public init(panes: [Pane]) {
    self.panes = panes
  }
}

/// What the window shows of itself, as the core decided it.
///
/// The shell's mirror of `PresentationChanged`, on the same terms as `Roster` and
/// `WindowContents`: a value that arrives whole and is applied, never one this side decides.
/// It is written down beside the arrangement, so it comes back on the next launch.
public struct Presentation: Equatable {
  /// Whether the roster is on screen.
  public let sidebar: Bool

  public init(sidebar: Bool) {
    self.sidebar = sidebar
  }
}

/// What the sidebar draws, worked out from the roster and the states beside it.
///
/// Pure, and separate from the view for the same reason `PaneAppearance` is: these are the
/// decisions - which rows group under which daemon, what a row says when no agent has been
/// heard from - and a decision inside `draw` is a decision no test can reach.
///
/// The join lives here because the two halves arrive separately and on purpose. A roster is
/// mostly stable and an agent state blinks, so they are two messages; the shell holds both
/// and puts them together, which it already does to paint a pane's border.
public enum SidebarModel {
  /// One line in the list.
  public struct Row: Equatable {
    /// The daemon this row's group is under, or nil for a pane row.
    public let daemon: String?
    public let pane: PaneKey?
    public let label: String

    /// The backend's spelling of what this pane's agent is doing, or `unknown` when the core
    /// has said nothing about it yet.
    public let state: String

    /// Whether a region is showing this pane. Rows for panes nobody is showing are the
    /// reason the list exists, and they are drawn as reachable rather than as absent.
    public let onScreen: Bool

    public var isHeader: Bool { daemon != nil && pane == nil }
  }

  /// The rows to draw, in order, with a header before each daemon's panes.
  ///
  /// Headers are inserted here rather than by the view because where a group starts is a
  /// property of the order the core chose, and the view should not have to re-derive it.
  ///
  /// A daemon holding no panes contributes no header. An attached daemon whose subscription
  /// has not bootstrapped is an ordinary moment on the way up, and a heading over nothing
  /// reads as a machine that lost its session.
  public static func rows(roster: Roster, states: [PaneKey: String]) -> [Row] {
    var rows: [Row] = []
    var current: String?
    for pane in roster.panes {
      if pane.key.daemon != current {
        current = pane.key.daemon
        rows.append(
          Row(daemon: pane.key.daemon, pane: nil, label: pane.key.daemon, state: "", onScreen: true)
        )
      }
      rows.append(
        Row(
          daemon: nil, pane: pane.key, label: pane.label,
          // A pane the core has said nothing about is unknown, not idle. An agent we have
          // not heard from is not an agent that finished (`corpus/conformance/agent-state.json`).
          state: states[pane.key] ?? "unknown",
          onScreen: pane.onScreen))
    }
    return rows
  }

  /// The dot beside a row, and whether to draw one at all.
  ///
  /// The same colors the pane borders use, because they are the same five states and a
  /// sidebar that disagreed with the window beside it would be worse than no sidebar. Unlike
  /// a border, the dot is drawn for every state including idle: a border exists to be
  /// noticed against a resting default, where a list with gaps in a column reads as missing
  /// data rather than as calm.
  public static func dotColor(state: String) -> NSColor {
    PaneAppearance.borderColor(state: state)
  }

  /// Wide enough for a directory and a harness name, narrow enough to leave a full window of
  /// panes readable beside it.
  public static let width: CGFloat = 200

  /// How wide the list is, and how much is left for panes.
  ///
  /// Here rather than on the view because it is arithmetic, and arithmetic inside `layout`
  /// is arithmetic no test can call. A window too narrow to hold both gives the list up
  /// rather than squeezing the panes to nothing - the terminals are what the app is for, and
  /// a two-column sidebar beside a two-column pane helps nobody.
  ///
  /// Two ways to end up with no list, and they are not the same thing. `shown` is what the
  /// core was asked for and remembers; the width check is this window being too small right
  /// now. A window narrowed until the list disappears and then widened again gets it back,
  /// because nothing about that was a decision.
  public static func widths(in total: CGFloat, shown: Bool = true) -> (
    sidebar: CGFloat, regions: CGFloat
  ) {
    guard shown, total >= width * 2 else { return (0, max(0, total)) }
    return (width, total - width)
  }
}

/// The list down the side of the window.
///
/// A table rather than a stack of views, because it is a list of a few dozen rows that wants
/// selection and scrolling, and AppKit already has all three.
@MainActor
public final class SidebarView: NSView {
  /// Called when somebody picks a pane, meaning they want the keyboard there.
  ///
  /// A request, like every other click in this app: the core decides what focusing a pane no
  /// region is showing means, and the window changes when the view that comes back says so.
  public var onPanePicked: ((PaneKey) -> Void)?

  public private(set) var rows: [SidebarModel.Row] = []

  private let table = NSTableView()
  private let scroll = NSScrollView()

  public override init(frame: NSRect) {
    super.init(frame: frame)
    let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("pane"))
    column.resizingMask = .autoresizingMask
    table.addTableColumn(column)
    table.headerView = nil
    table.rowSizeStyle = .small
    table.selectionHighlightStyle = .regular
    table.backgroundColor = .clear
    table.dataSource = self
    table.delegate = self
    table.target = self
    table.action = #selector(rowClicked)

    scroll.documentView = table
    scroll.hasVerticalScroller = true
    scroll.drawsBackground = false
    scroll.autoresizingMask = [.width, .height]
    scroll.frame = bounds
    addSubview(scroll)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  public func apply(roster: Roster, states: [PaneKey: String]) {
    rows = SidebarModel.rows(roster: roster, states: states)
    table.reloadData()
  }

  @objc private func rowClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked), let pane = rows[clicked].pane else { return }
    onPanePicked?(pane)
  }
}

extension SidebarView: NSTableViewDataSource, NSTableViewDelegate {
  public func numberOfRows(in tableView: NSTableView) -> Int {
    rows.count
  }

  public func tableView(
    _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
  ) -> NSView? {
    guard rows.indices.contains(row) else { return nil }
    return SidebarRowView(row: rows[row])
  }

  /// Headers are labels, not destinations. Selecting one would move the keyboard nowhere and
  /// leave a highlight suggesting it had.
  public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
    rows.indices.contains(row) && !rows[row].isHeader
  }
}

/// One row: a state dot, a name, and whether anything is showing it.
@MainActor
final class SidebarRowView: NSView {
  private let dot = CALayer()
  private let name = NSTextField(labelWithString: "")

  init(row: SidebarModel.Row) {
    super.init(frame: .zero)
    wantsLayer = true
    name.font = .systemFont(
      ofSize: row.isHeader ? 10 : 12, weight: row.isHeader ? .semibold : .regular)
    name.stringValue = row.isHeader ? row.label.uppercased() : row.label
    // A pane no region is showing is reachable, not absent - dimming it says "not here yet"
    // rather than "gone", which is the difference between a row worth clicking and one that
    // looks broken.
    name.textColor = row.isHeader || row.onScreen ? .labelColor : .secondaryLabelColor
    addSubview(name)

    if !row.isHeader {
      dot.backgroundColor = SidebarModel.dotColor(state: row.state).cgColor
      dot.cornerRadius = SidebarRowView.dotSize / 2
      layer?.addSublayer(dot)
    }
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  static let dotSize: CGFloat = 7
  static let inset: CGFloat = 8

  override func layout() {
    super.layout()
    let hasDot = dot.superlayer != nil
    let textLeft = hasDot ? SidebarRowView.inset * 2 + SidebarRowView.dotSize : SidebarRowView.inset
    dot.frame = CGRect(
      x: SidebarRowView.inset, y: (bounds.height - SidebarRowView.dotSize) / 2,
      width: SidebarRowView.dotSize, height: SidebarRowView.dotSize)
    name.frame = CGRect(
      x: textLeft, y: 0, width: max(0, bounds.width - textLeft - SidebarRowView.inset),
      height: bounds.height)
  }
}
