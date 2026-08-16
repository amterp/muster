import AppKit

/// Every pane every attached daemon holds, as the core listed them.
///
/// The shell's mirror of `RosterChanged`, translated once here the way `WindowContents`
/// mirrors the view. Order and labels arrive decided: a shell that sorted or named for
/// itself would be a second place those answers live.
public struct Roster: Equatable {
  public struct Pane: Equatable {
    public let key: PaneKey

    /// What to call this pane to somebody who did not open it.
    public let label: String

    /// What its agent is working on, when the core decided that was worth a line. Empty for
    /// most panes, which is what keeps a list of fifteen readable at a glance.
    public let subtitle: String

    /// The name somebody gave this pane, empty when nobody has. What a rename starts from.
    public let givenName: String

    /// Whether a region is showing it right now.
    public let onScreen: Bool

    public init(
      key: PaneKey, label: String, subtitle: String = "", givenName: String = "",
      onScreen: Bool
    ) {
      self.key = key
      self.label = label
      self.subtitle = subtitle
      self.givenName = givenName
      self.onScreen = onScreen
    }
  }

  public struct Tab: Equatable {
    public let key: TabKey

    /// Where this tab sits in the window's whole tab order, counting from one. What ⌘N names.
    public let place: Int

    /// What to call this tab to somebody who did not open it.
    public let label: String

    /// Whether a region is showing this tab right now. Not the same question as any of its
    /// panes being on screen - a zoomed tab is on screen while all but one of them are not.
    public let onScreen: Bool

    /// The name somebody gave this tab, empty when nobody has. What a rename starts from -
    /// not recoverable from `label`, which may carry the tab's workspace in front of it.
    public let givenName: String

    public let panes: [Pane]

    public init(
      key: TabKey, place: Int, label: String, onScreen: Bool, givenName: String = "",
      panes: [Pane]
    ) {
      self.key = key
      self.place = place
      self.label = label
      self.onScreen = onScreen
      self.givenName = givenName
      self.panes = panes
    }
  }

  public struct Daemon: Equatable {
    public let id: String
    public let tabs: [Tab]

    public init(id: String, tabs: [Tab]) {
      self.id = id
      self.tabs = tabs
    }
  }

  public let daemons: [Daemon]

  public init(daemons: [Daemon]) {
    self.daemons = daemons
  }

  /// Every tab in the window, in the order they are numbered.
  public var tabs: [Tab] { daemons.flatMap(\.tabs) }

  /// Every pane in the window, in the order they are listed.
  public var panes: [Pane] { tabs.flatMap(\.panes) }
}

/// What the window shows of itself, as the core decided it.
///
/// The shell's mirror of `PresentationChanged`, on the same terms as `Roster` and
/// `WindowContents`: a value that arrives whole and is applied, never one this side decides.
/// It is written down beside the arrangement, so it comes back on the next launch.
public struct Presentation: Equatable {
  /// Whether the roster is on screen.
  public let sidebar: Bool

  /// Points to add to the font size the config file named, or the renderer chose.
  ///
  /// An offset rather than a size: the size it is offsetting from may be the renderer's own,
  /// and nothing on this side of the seam knows what that is.
  public let fontSizeOffset: Int32

  public init(sidebar: Bool, fontSizeOffset: Int32 = 0) {
    self.sidebar = sidebar
    self.fontSizeOffset = fontSizeOffset
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
  /// What one line in the list is.
  public enum Kind: Equatable {
    /// A machine's name, over the tabs it holds.
    case daemon
    /// A tab, over the panes in it. Carries the place a numbered chord names.
    case tab(place: Int)
    case pane
  }

  /// One line in the list.
  public struct Row: Equatable {
    public let kind: Kind

    /// The daemon this row belongs to, whichever kind it is.
    public let daemon: String

    /// The tab this row is or sits under. Nil only on a daemon heading.
    public let tab: TabKey?

    /// The pane this row is, on a pane row and nowhere else.
    public let pane: PaneKey?
    public let label: String

    /// A second line under the label, or empty for no second line. Only pane rows ever have
    /// one, and most of those do not: what earns it is decided in the core.
    public let subtitle: String

    /// The name somebody gave this row's subject, empty when nobody has. What a rename starts
    /// from, so that renaming `muster · claude` opens an empty field rather than that text.
    public let givenName: String

    /// The backend's spelling of what this pane's agent is doing, or `unknown` when the core
    /// has said nothing about it yet. Empty on the rows that are not panes.
    public let state: String

    /// Whether a region is showing this row's subject. Rows for panes nobody is showing are
    /// the reason the list exists, and they are drawn as reachable rather than as absent.
    public let onScreen: Bool

    /// Whether this is the pane the keyboard feeds.
    ///
    /// Exactly one row can carry it, and it answers a question the window already answers
    /// with a border - which is the point. A list of a dozen panes beside a window of two is
    /// hard to read back against; marking the same pane in both is what joins them.
    public let hasKeyboard: Bool

    public var isHeader: Bool { kind == .daemon }

    /// Whether picking this row means something. A daemon heading names no destination.
    public var isDestination: Bool { kind != .daemon }
  }

  /// The rows to draw, in order: a daemon heading, then a caption per tab, then its panes.
  ///
  /// Inserted here rather than by the view because where a group starts is a property of the
  /// order the core chose, and the view should not have to re-derive it.
  ///
  /// A daemon holding no tabs contributes no heading. An attached daemon whose subscription
  /// has not bootstrapped is an ordinary moment on the way up, and a heading over nothing
  /// reads as a machine that lost its session.
  ///
  /// **A window with one tab draws no caption.** There is nothing to navigate between, so a
  /// row saying which tab you are in is a line that answers a question nobody has - and this
  /// is the common case, so paying a level of nesting for it would make the list worse for
  /// most people to make it better for some. The moment a second tab exists anywhere in the
  /// window, every tab gets its caption and its number, including the tabs on a daemon that
  /// only holds one: the numbering counts across the whole window, so showing it in patches
  /// would be worse than not showing it.
  ///
  /// `keyboard` is the pane the core's view says has the keyboard, or nil when no region
  /// does. Passed in rather than derived here: which pane that is arrives on the view, and
  /// the roster is a separate message - the same join the window already makes for states.
  public static func rows(roster: Roster, states: [PaneKey: String], keyboard: PaneKey? = nil)
    -> [Row]
  {
    let captions = roster.tabs.count > 1
    var rows: [Row] = []
    for daemon in roster.daemons where !daemon.tabs.isEmpty {
      rows.append(
        Row(
          kind: .daemon, daemon: daemon.id, tab: nil, pane: nil, label: daemon.id, subtitle: "",
          givenName: "", state: "", onScreen: true, hasKeyboard: false))
      for tab in daemon.tabs {
        if captions {
          rows.append(
            Row(
              kind: .tab(place: tab.place), daemon: daemon.id, tab: tab.key, pane: nil,
              label: tab.label, subtitle: "", givenName: tab.givenName, state: "",
              onScreen: tab.onScreen, hasKeyboard: false))
        }
        for pane in tab.panes {
          rows.append(
            Row(
              kind: .pane, daemon: daemon.id, tab: tab.key, pane: pane.key, label: pane.label,
              subtitle: pane.subtitle, givenName: pane.givenName,
              // A pane the core has said nothing about is unknown, not idle. An agent we have
              // not heard from is not an agent that finished
              // (`corpus/conformance/agent-state.json`).
              state: states[pane.key] ?? "unknown",
              onScreen: pane.onScreen,
              hasKeyboard: pane.key == keyboard))
        }
      }
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

  /// How tall a row is.
  ///
  /// **Two heights and no more**, and deliberately not a function of how long the text is. A
  /// list of fifteen agents is read by scanning it, so a height that varied with what an agent
  /// happened to be writing would move every row below it each time one of them wrote a longer
  /// sentence. The only thing that can move a row is a second line arriving or going away.
  public static let oneLine: CGFloat = 20
  public static let twoLines: CGFloat = 32
  public static func height(of row: Row) -> CGFloat {
    row.subtitle.isEmpty ? oneLine : twoLines
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

  /// Called when somebody picks a tab caption, meaning they want to be looking at that tab.
  ///
  /// The mouse's half of what ⌘N and next-tab do with the keyboard, and it goes through the
  /// same core path: a place in the window's tab order, resolved there.
  public var onTabPicked: ((Int) -> Void)?

  public private(set) var rows: [SidebarModel.Row] = []

  private let table = NSTableView()
  private let scroll = NSScrollView()

  public override init(frame: NSRect) {
    super.init(frame: frame)
    let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("pane"))
    column.resizingMask = .autoresizingMask
    table.addTableColumn(column)
    table.headerView = nil
    // Custom rather than `.small`, because a row is one line or two depending on whether its
    // agent said what it is working on, and a table with a size style of its own ignores what
    // `heightOfRow` answers.
    table.rowSizeStyle = .custom
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

  public func apply(roster: Roster, states: [PaneKey: String], keyboard: PaneKey? = nil) {
    rows = SidebarModel.rows(roster: roster, states: states, keyboard: keyboard)
    table.reloadData()
  }

  @objc private func rowClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked) else { return }
    switch rows[clicked].kind {
    case .pane:
      guard let pane = rows[clicked].pane else { return }
      onPanePicked?(pane)
    case .tab(let place):
      onTabPicked?(place)
    case .daemon:
      break
    }
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

  /// Daemon headings are labels, not destinations. Selecting one would move the keyboard
  /// nowhere and leave a highlight suggesting it had. A tab caption is a destination, because
  /// showing a tab is a thing this app does.
  public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
    rows.indices.contains(row) && rows[row].isDestination
  }

  /// Rows are one line or two, so the table cannot use a single row height any more.
  public func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
    guard rows.indices.contains(row) else { return SidebarModel.oneLine }
    return SidebarModel.height(of: rows[row])
  }
}

/// One row: a state dot or a tab number, a name, and whether anything is showing it.
@MainActor
final class SidebarRowView: NSView {
  private let dot = CALayer()
  private let name = NSTextField(labelWithString: "")
  private let subtitle = NSTextField(labelWithString: "")
  private let number = NSTextField(labelWithString: "")
  private let highlight = CALayer()
  private let indented: Bool

  init(row: SidebarModel.Row) {
    // Panes indent under their tab caption, and sit flush when there is none. The list is
    // 200pt wide, so a level of nesting that buys nothing is a level that costs a word off
    // every label.
    indented = row.kind == .pane && row.tab != nil
    super.init(frame: .zero)
    wantsLayer = true

    // The pane the keyboard feeds, marked the way the window already marks it. Drawn behind
    // everything else and only for the one row, so a list of a dozen panes beside a window
    // of two can be read back against it.
    if row.hasKeyboard {
      highlight.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.22).cgColor
      highlight.cornerRadius = 5
      layer?.addSublayer(highlight)
    }

    switch row.kind {
    case .daemon:
      name.font = .systemFont(ofSize: 10, weight: .semibold)
      name.stringValue = row.label.uppercased()
      name.textColor = .secondaryLabelColor
    case .tab(let place):
      // The tab on screen is named in full, and the ones behind it are quieter. This says a
      // different thing from the keyboard highlight on purpose: one is where you are
      // looking, the other is where you are typing, and in a two-region window those are
      // two different tabs.
      name.font = .systemFont(ofSize: 11, weight: row.onScreen ? .semibold : .regular)
      name.stringValue = row.label
      name.textColor = row.onScreen ? .labelColor : .secondaryLabelColor
      // The number a chord names, drawn where a tab bar would put it. Only up to nine,
      // because that is how far ⌘N goes - a tenth tab is reachable by next-tab and by
      // clicking, and a number nothing is bound to would be a promise the keyboard breaks.
      number.stringValue = place <= 9 ? String(place) : ""
      number.font = .monospacedDigitSystemFont(ofSize: 10, weight: .regular)
      number.textColor = .tertiaryLabelColor
      addSubview(number)
    case .pane:
      name.font = .systemFont(ofSize: 12, weight: .regular)
      name.stringValue = row.label
      // A pane no region is showing is reachable, not absent - dimming it says "not here yet"
      // rather than "gone", which is the difference between a row worth clicking and one that
      // looks broken.
      name.textColor = row.onScreen ? .labelColor : .secondaryLabelColor
      dot.backgroundColor = SidebarModel.dotColor(state: row.state).cgColor
      dot.cornerRadius = SidebarRowView.dotSize / 2
      layer?.addSublayer(dot)
      if !row.subtitle.isEmpty {
        subtitle.font = .systemFont(ofSize: 10, weight: .regular)
        subtitle.stringValue = row.subtitle
        subtitle.textColor = .secondaryLabelColor
        // Truncated rather than wrapped, and the full text on hover. Wrapping would make a
        // row's height a function of what its agent is doing, so the list would jump under
        // somebody reading it every time an agent wrote a longer sentence. In a list whose
        // whole value is being scannable, a stable row beats a complete one.
        subtitle.lineBreakMode = .byTruncatingTail
        subtitle.toolTip = row.subtitle
        addSubview(subtitle)
      }
    }
    // Long directory names truncate rather than spilling past the row, for the same reason.
    name.lineBreakMode = .byTruncatingTail
    name.toolTip = row.label
    addSubview(name)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  static let dotSize: CGFloat = 7
  static let inset: CGFloat = 8
  static let indent: CGFloat = 10
  static let numberWidth: CGFloat = 12

  override func layout() {
    super.layout()
    if highlight.superlayer != nil {
      highlight.frame = bounds.insetBy(dx: 4, dy: 1)
    }
    let left = SidebarRowView.inset + (indented ? SidebarRowView.indent : 0)
    var textLeft = left
    if dot.superlayer != nil {
      dot.frame = CGRect(
        x: left, y: (bounds.height - SidebarRowView.dotSize) / 2,
        width: SidebarRowView.dotSize, height: SidebarRowView.dotSize)
      textLeft = left + SidebarRowView.inset + SidebarRowView.dotSize
    }
    if number.superview != nil {
      let height = min(bounds.height, number.fittingSize.height)
      number.frame = CGRect(
        x: left, y: (bounds.height - height) / 2,
        width: SidebarRowView.numberWidth, height: height)
      textLeft = left + SidebarRowView.numberWidth + 4
    }
    // Sized to the text and then centred, rather than given the whole row. A label draws its
    // text at the top of whatever frame it is handed, so a full-height frame puts the words
    // above the dot beside them - which reads as the dot being wrong rather than the text.
    let width = max(0, bounds.width - textLeft - SidebarRowView.inset)
    let textHeight = min(bounds.height, name.fittingSize.height)
    guard subtitle.superview != nil else {
      name.frame = CGRect(
        x: textLeft, y: (bounds.height - textHeight) / 2, width: width, height: textHeight)
      return
    }
    // Two lines share the row: the pair is centred together, so a one-line row and a two-line
    // row read as the same list rather than as two lists. The dot stays on the row's centre
    // rather than on the first line's, which keeps the column of dots straight.
    let secondHeight = min(bounds.height, subtitle.fittingSize.height)
    let top = (bounds.height - textHeight - secondHeight) / 2
    name.frame = CGRect(
      x: textLeft, y: top + secondHeight, width: width, height: textHeight)
    subtitle.frame = CGRect(x: textLeft, y: top, width: width, height: secondHeight)
  }
}
