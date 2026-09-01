import AppKit

/// Every pane every attached daemon holds, as the core listed them.
///
/// The shell's mirror of `RosterChanged`, translated once here the way `WindowContents`
/// mirrors the view. Order and labels arrive decided: a shell that sorted or named for
/// itself would be a second place those answers live.
public struct Roster: Equatable {
  public struct Pane: Equatable {
    public let key: PaneKey

    /// Where this pane sits in the window's whole pane order, counting from one.
    public let place: Int

    /// Which numbered chord reaches this pane right now, or 0 when none does. What the row
    /// draws. Distinct from `place` above, which is where the row sits whether or not a chord
    /// gets you there - the two agree for the first nine panes under the scheme Muster ships,
    /// and part company under `numbered_chords = "tab_then_pane"`.
    public let number: Int

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
      key: PaneKey, place: Int = 0, number: Int = 0, label: String, subtitle: String = "",
      givenName: String = "", onScreen: Bool
    ) {
      self.key = key
      self.place = place
      self.number = number
      self.label = label
      self.subtitle = subtitle
      self.givenName = givenName
      self.onScreen = onScreen
    }
  }

  public struct Tab: Equatable {
    public let key: TabKey

    /// Where this tab sits in the window's whole tab order, counting from one. The number in
    /// the caption of a tab nobody named.
    public let place: Int

    /// Which numbered chord reaches this tab right now, or 0 when none does - which is every
    /// tab under the scheme Muster ships, where ⌘N names panes. See `Pane.number`.
    public let number: Int

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
      key: TabKey, place: Int, number: Int = 0, label: String, onScreen: Bool,
      givenName: String = "", panes: [Pane]
    ) {
      self.key = key
      self.place = place
      self.number = number
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

  /// What the numbered chords are counting, and so whether one is half-typed.
  ///
  /// Not a second answer to which chord reaches which row - that is `number` on the row, and
  /// this side reads it rather than working it out. What this adds is the question a row
  /// cannot answer: under `numbered_chords = "tab_then_pane"` a first press leaves the window
  /// waiting for a second, and three things here need to know it. Panes draw a number over
  /// themselves only then, the window ends the gesture when the modifier comes up, and the
  /// list reserves room for a digit that is about to arrive.
  ///
  /// Which tab was named is not carried, because nothing needs it: the tab whose panes hold
  /// the numbers is the tab a press named, and the rows already say that.
  public enum Numbering: Equatable {
    /// Panes, down the whole window. What Muster does.
    case panes
    /// Tabs, across the window. `tab_then_pane`, with no press outstanding.
    case tabs
    /// The panes inside the tab a press named. `tab_then_pane`, half-typed.
    case panesInTab

    /// Whether a chord is half-typed, waiting for the press that names a pane.
    public var isHalfTyped: Bool { self == .panesInTab }

    /// Whether the numbers can move between tab rows and pane rows in this window.
    ///
    /// What decides whether the list reserves a gutter. Under the settled scheme nothing
    /// moves, so nothing needs reserving and the caption rows keep sitting where they do.
    public var movesBetweenRows: Bool { self != .panes }
  }

  public let daemons: [Daemon]

  public let numbering: Numbering

  public init(daemons: [Daemon], numbering: Numbering = .panes) {
    self.daemons = daemons
    self.numbering = numbering
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
  /// What one line in the list is.
  public enum Kind: Equatable {
    /// A machine's name, over the tabs it holds.
    case daemon
    /// A tab, over the panes in it, carrying the numbered chord that reaches it or 0.
    case tab(number: Int)
    /// A pane, carrying the numbered chord that reaches it or 0.
    case pane(number: Int)
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

    /// Whether this row leaves room for a number even when it carries none.
    ///
    /// Under `numbered_chords = "tab_then_pane"` the numbers move between tab rows and pane
    /// rows as a chord is typed, and a row that only made room when it had a digit would slide
    /// its label sixteen points sideways every time. The list is what somebody reads to decide
    /// what to press next, so it has to hold still while they are reading it.
    ///
    /// False under the settled scheme, where nothing moves and a caption reserving space for a
    /// number no tab will ever carry would be an indent that buys nothing.
    public let reservesNumber: Bool

    /// Whether this row's number is the second press of a chord already begun.
    ///
    /// Only under `tab_then_pane`, and only once a press has named a tab. Drawn brighter than
    /// a number at rest, because at that moment it is not a reference - it is the thing the
    /// hand is about to do, and the window has to say which numbers are live while the
    /// modifier is still down.
    public let isSecondPress: Bool

    public var isHeader: Bool { kind == .daemon }

    /// Whether picking this row means something. A daemon heading names no destination.
    public var isDestination: Bool { kind != .daemon }

    public var isTab: Bool {
      if case .tab = kind { return true }
      return false
    }

    public var isPane: Bool {
      if case .pane = kind { return true }
      return false
    }
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
  /// **A window with one tab draws no caption, unless a chord names it.** There is nothing to
  /// navigate between, so a row saying which tab you are in is a line that answers a question
  /// nobody has - and this
  /// is the common case, so paying a level of nesting for it would make the list worse for
  /// most people to make it better for some. The moment a second tab exists anywhere in the
  /// window, every tab gets a caption, including the tabs on a daemon that only holds one:
  /// captions in patches would read as a boundary that comes and goes.
  ///
  /// The exception is a numbered tab, which is drawn whatever else this rule says: a number
  /// nothing draws is a chord nobody can find. The core stopped producing one in a window of a
  /// single tab - under `numbered_chords = "tab_then_pane"` such a window numbers panes,
  /// because with one tab the two numberings are the same numbers - so the exception guards a
  /// state rather than describing one. It stays because which rows carry numbers is the core's
  /// answer and not this one's, and a list that hid one would be worse than a caption nobody
  /// needed.
  ///
  /// **Which rows carry numbers is the core's answer, not this one's.** Every row arrives with
  /// the chord that reaches it or with none, so a caption numbered `2` and a ⌘2 that goes
  /// somewhere else is not a state this side can produce.
  ///
  /// `keyboard` is the pane the core's view says has the keyboard, or nil when no region
  /// does. Passed in rather than derived here: which pane that is arrives on the view, and
  /// the roster is a separate message - the same join the window already makes for states.
  public static func rows(roster: Roster, states: [PaneKey: String], keyboard: PaneKey? = nil)
    -> [Row]
  {
    let captions = roster.tabs.count > 1 || roster.tabs.contains { $0.number > 0 }
    // Reserved on every tab and pane row together rather than per row, so the gutter is a
    // property of the window and not of whichever rows happen to be numbered this instant -
    // which is the whole point of reserving it.
    let gutter = roster.numbering.movesBetweenRows
    let second = roster.numbering.isHalfTyped
    var rows: [Row] = []
    for daemon in roster.daemons where !daemon.tabs.isEmpty {
      rows.append(
        Row(
          kind: .daemon, daemon: daemon.id, tab: nil, pane: nil, label: daemon.id, subtitle: "",
          givenName: "", state: "", onScreen: true, hasKeyboard: false, reservesNumber: false,
          isSecondPress: false))
      for tab in daemon.tabs {
        if captions {
          rows.append(
            Row(
              kind: .tab(number: tab.number), daemon: daemon.id, tab: tab.key, pane: nil,
              label: tab.label, subtitle: "", givenName: tab.givenName, state: "",
              onScreen: tab.onScreen, hasKeyboard: false, reservesNumber: gutter,
              // A tab carries no number once one of them has been named, so there is no such
              // thing as a second press onto a caption.
              isSecondPress: false))
        }
        for pane in tab.panes {
          rows.append(
            Row(
              kind: .pane(number: pane.number), daemon: daemon.id, tab: tab.key, pane: pane.key,
              label: pane.label,
              subtitle: pane.subtitle, givenName: pane.givenName,
              // A pane the core has said nothing about is unknown, not idle. An agent we have
              // not heard from is not an agent that finished
              // (`corpus/conformance/agent-state.json`).
              state: states[pane.key] ?? "unknown",
              onScreen: pane.onScreen,
              hasKeyboard: pane.key == keyboard,
              reservesNumber: gutter,
              isSecondPress: second))
        }
      }
    }
    return rows
  }

  /// Whether dragging one pane onto another row is a gesture Muster can carry out.
  ///
  /// Here rather than in the view so that the rule is testable: a decision inside
  /// `validateDrop` is a decision no test can reach, and this one has a case that is easy to
  /// get wrong and impossible to see - two daemons hand out the same pane ids, so a rule
  /// comparing ids alone would call a cross-machine drop legal.
  ///
  /// **A drop must land on a pane row on the same daemon.** A daemon heading and a tab caption
  /// are not places a pane can go. Crossing daemons is refused because a pane is a PTY its
  /// daemon owns: moving one to another machine would mean killing a process on one host and
  /// starting a different one on another, which is not what dragging a row looks like it does.
  ///
  /// Dropping a row on itself is legal and does nothing, which is what an accidental drag is.
  public static func canArrange(_ pane: PaneKey, onto row: Row) -> Bool {
    guard let target = row.pane else { return false }
    return target.daemon == pane.daemon
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

  /// What the list has to be told when the rows come out different.
  ///
  /// Two answers rather than one, because a table treats them as two questions. `redraw` is
  /// the positions whose contents moved. `remeasure` is the subset whose *height* moved with
  /// them, and it is separate because redrawing a row rebuilds its view inside the frame it
  /// already had - the height it was last measured at stands until something asks for it
  /// again. A row that gains its second line and is only redrawn draws two lines in a
  /// one-line frame.
  ///
  /// Nil means the shape of the list moved - a pane opened, a tab closed, a drag reordered
  /// them - so the rows are not the same rows and comparing them by position would compare
  /// different things. The whole list wants redrawing then, which measures it too.
  ///
  /// Here rather than in the view for the reason `height(of:)` and `widths(in:shown:)` are:
  /// a decision inside a redraw is a decision no test can reach.
  public static func changes(from previous: [Row], to fresh: [Row]) -> (
    redraw: IndexSet, remeasure: IndexSet
  )? {
    guard previous.count == fresh.count else { return nil }
    let redraw = IndexSet(fresh.indices.filter { previous[$0] != fresh[$0] })
    let remeasure = redraw.filteredIndexSet { height(of: previous[$0]) != height(of: fresh[$0]) }
    return (redraw, remeasure)
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
  /// The mouse's half of what next-tab does with the keyboard. Names the tab rather than
  /// numbering it: the numbers name panes, and a click already knows which caption it hit.
  public var onTabPicked: ((TabKey) -> Void)?

  /// Called when somebody double-clicks a row, meaning they want to rename what it names.
  ///
  /// The list is where you are already looking to decide which agent is which, so it is where
  /// renaming should start. It dispatches the same action the menu item does rather than
  /// editing in place: a roster or a state arriving rebuilds every row, so an editor living
  /// inside one would be destroyed by an agent going idle mid-word.
  public var onRowRenamed: ((SidebarModel.Row) -> Void)?

  /// Called when somebody drags one agent's row onto another, meaning they want it there.
  ///
  /// A request like every other gesture here: which of the two arrangements this is - an
  /// exchange within a tab, or a move into another one - is decided in the core from where the
  /// two panes are, and the list changes when the roster that comes back says so.
  public var onPaneArranged: ((PaneKey, PaneKey) -> Void)?

  public private(set) var rows: [SidebarModel.Row] = []

  /// The frames the list has settled on for its rows.
  ///
  /// Not the same question as what `heightOfRow` would answer, which is the point: a table
  /// keeps the height it was last told, and the two part company exactly when this is worth
  /// asking. So a test that reads these is checking what somebody would see rather than what
  /// this side meant.
  var drawnRows: [CGRect] {
    table.layoutSubtreeIfNeeded()
    return rows.indices.map { table.rect(ofRow: $0) }
  }

  /// Muster's own pasteboard type, so nothing outside this window can offer a drop this
  /// accepts and nothing here accepts a file somebody dragged in from the Finder.
  static let draggedPane = NSPasteboard.PasteboardType("dev.muster.pane")

  private let table = NSTableView()
  private let scroll = NSScrollView()
  private let problemsView = ProblemsView()

  /// Everything wrong with the window, as the core last said it.
  private var outstanding: [Problem] = []

  /// Which problems somebody has waved away. Not persisted: a dismissal is about this sitting
  /// rather than about the condition, and a problem still true on the next launch is one
  /// nobody has seen yet in that window.
  private var dismissed: Set<String> = []

  /// What the area at the foot is showing, kept so a dismissal knows what it dismissed.
  public private(set) var problems: ProblemsModel.Display = .nothing

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
    table.doubleAction = #selector(rowDoubleClicked)
    table.registerForDraggedTypes([SidebarView.draggedPane])
    // Local only, and a move rather than a copy: there is no second copy of an agent to make,
    // and nothing outside this window has any use for a pane id.
    table.setDraggingSourceOperationMask([], forLocal: false)
    table.setDraggingSourceOperationMask(.move, forLocal: true)

    scroll.documentView = table
    scroll.hasVerticalScroller = true
    scroll.drawsBackground = false
    scroll.frame = bounds
    addSubview(scroll)

    problemsView.onDismiss = { [weak self] in
      guard let self, case .raised(let showing) = self.problems else { return }
      self.dismissed.formUnion(showing.map(\.key))
      self.redrawProblems()
    }
    problemsView.onReveal = { [weak self] in
      self?.dismissed.removeAll()
      self?.redrawProblems()
    }
    addSubview(problemsView)
  }

  /// Tells the roster what is wrong with the window.
  ///
  /// The list and the problems arrive separately because they change on completely different
  /// schedules - a roster moves whenever a pane does, and a problem is rare - so joining them
  /// here is the same arrangement the states already use.
  public func apply(problems: [Problem]) {
    outstanding = problems
    dismissed = ProblemsModel.retained(dismissed: dismissed, outstanding: problems)
    redrawProblems()
  }

  private func redrawProblems() {
    problems = ProblemsModel.display(problems: outstanding, dismissed: dismissed)
    problemsView.show(problems)
    needsLayout = true
  }

  /// Splits the sidebar between the list and whatever is wrong with the window.
  ///
  /// Hand-laid rather than autoresized because the problems area's height depends on how long
  /// its message is at this width, and an autoresizing mask cannot ask that question. The list
  /// gets everything left, which is all of it in the common case where nothing is wrong.
  public override func layout() {
    super.layout()
    let wanted = problemsView.height(forWidth: bounds.width)
    let height = min(wanted, bounds.height)
    problemsView.frame = CGRect(x: 0, y: 0, width: bounds.width, height: height)
    scroll.frame = CGRect(
      x: 0, y: height, width: bounds.width, height: max(0, bounds.height - height))
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Redraws the rows that would come out different, and only those.
  ///
  /// This is called on every agent transition, which is the most frequent thing that happens
  /// in a window full of agents - and one pane blinking used to reload the whole table, so
  /// AppKit threw away and rebuilt the view for every visible row to show a change on one of
  /// them. The core is careful about exactly this: an agent-state change is deliberately
  /// excluded from the republish path so that it costs that change and not a walk of every
  /// pane (`architecture.md`, fast is a feature), and the sidebar was undoing that
  /// downstream.
  ///
  /// A whole reload is still right when the shape of the list moves - a pane opened, a tab
  /// closed, rows reordered by a drag - because then the rows are not the same rows and
  /// comparing them position by position would be comparing different things. That case is
  /// rare; a state blinking is not.
  ///
  /// **Two calls, because `reloadData(forRowIndexes:)` does not re-measure.** It rebuilds a
  /// row's view inside the frame that row already had, and `noteHeightOfRows` is the only
  /// thing that makes a table ask again. Skipping it is how a pane whose agent titles itself
  /// came to draw two lines in a one-line frame until something unrelated reloaded the lot.
  ///
  /// Instantly rather than animated: a note animates by default, and a row growing under
  /// somebody reading the list is the movement the two heights exist to avoid.
  public func apply(roster: Roster, states: [PaneKey: String], keyboard: PaneKey? = nil) {
    let fresh = SidebarModel.rows(roster: roster, states: states, keyboard: keyboard)
    let previous = rows
    rows = fresh
    guard let changed = SidebarModel.changes(from: previous, to: fresh) else {
      table.reloadData()
      return
    }
    guard !changed.redraw.isEmpty else { return }
    table.reloadData(forRowIndexes: changed.redraw, columnIndexes: IndexSet(integer: 0))
    guard !changed.remeasure.isEmpty else { return }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0
      table.noteHeightOfRows(withIndexesChanged: changed.remeasure)
    }
  }

  @objc private func rowClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked) else { return }
    switch rows[clicked].kind {
    case .pane:
      guard let pane = rows[clicked].pane else { return }
      onPanePicked?(pane)
    case .tab:
      guard let tab = rows[clicked].tab else { return }
      onTabPicked?(tab)
    case .daemon:
      break
    }
  }

  /// A double-click asks to rename. A daemon heading names nothing renameable, so it does
  /// nothing - the machine's name is not Muster's to change.
  @objc private func rowDoubleClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked), rows[clicked].isDestination else { return }
    onRowRenamed?(rows[clicked])
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

  /// What travels with a dragged row: the pane it names, not the row it was.
  ///
  /// A row index would be the obvious payload and would be wrong. Every roster message and
  /// every agent state rebuilds the whole list, and one of those arriving mid-drag would leave
  /// the index pointing at a different agent by the time the drop lands. A key survives that,
  /// because it names the pane rather than its position.
  public func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int)
    -> NSPasteboardWriting?
  {
    guard rows.indices.contains(row), let pane = rows[row].pane else { return nil }
    let item = NSPasteboardItem()
    item.setString("\(pane.daemon)\t\(pane.pane)", forType: SidebarView.draggedPane)
    return item
  }

  public func tableView(
    _ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
    proposedDropOperation operation: NSTableView.DropOperation
  ) -> NSDragOperation {
    // On a row rather than between two. The card's rule is that a drag exchanges two panes,
    // and an arrangement has no "between" to insert into - so the row you drop on is the place
    // you are asking for, and retargeting an above-row drop keeps the highlight honest.
    guard let pane = dragged(info), rows.indices.contains(row) else { return [] }
    if operation == .above {
      tableView.setDropRow(row, dropOperation: .on)
    }
    return SidebarModel.canArrange(pane, onto: rows[row]) ? .move : []
  }

  public func tableView(
    _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
    dropOperation operation: NSTableView.DropOperation
  ) -> Bool {
    guard let pane = dragged(info), rows.indices.contains(row),
      SidebarModel.canArrange(pane, onto: rows[row]), let onto = rows[row].pane
    else { return false }
    onPaneArranged?(pane, onto)
    return true
  }

  /// The pane a drag is carrying, or nil when it is carrying something else.
  private func dragged(_ info: NSDraggingInfo) -> PaneKey? {
    guard let carried = info.draggingPasteboard.string(forType: SidebarView.draggedPane) else {
      return nil
    }
    let parts = carried.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)
    guard parts.count == 2 else { return nil }
    return PaneKey(daemon: String(parts[0]), pane: String(parts[1]))
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
    indented = row.isPane && row.tab != nil
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
    case .tab(let reached):
      // The tab on screen is named in full, and the ones behind it are quieter. This says a
      // different thing from the keyboard highlight on purpose: one is where you are
      // looking, the other is where you are typing, and in a two-region window those are
      // two different tabs.
      name.font = .systemFont(ofSize: 11, weight: row.onScreen ? .semibold : .regular)
      name.stringValue = row.label
      name.textColor = row.onScreen ? .labelColor : .secondaryLabelColor
      draw(number: reached, in: row)
    case .pane(let reached):
      name.font = .systemFont(ofSize: 12, weight: .regular)
      name.stringValue = row.label
      draw(number: reached, in: row)
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

  /// The chord that reaches this row, drawn beside the dot rather than instead of it.
  ///
  /// The dot is what the row is for and the number is how to get there, so a row wants both.
  /// Zero draws nothing, which is what a row no chord reaches carries - every tab under the
  /// scheme Muster ships, and every pane past the ninth.
  ///
  /// One function for both kinds of row because it is one number meaning one thing. Under
  /// `numbered_chords = "tab_then_pane"` the numbers move between tab rows and pane rows as
  /// chords are pressed, and two implementations of "draw the number" would be two chances
  /// for them to look different depending on which row they landed on.
  ///
  /// A row that reserves the gutter still adds the field with nothing in it, which is what
  /// keeps every label where it was while the numbers move around them.
  private func draw(number reached: Int, in row: SidebarModel.Row) {
    let drawn = reached >= 1 && reached <= 9
    guard drawn || row.reservesNumber else { return }
    number.stringValue = drawn ? String(reached) : ""
    // Brighter and heavier while a chord is half-typed, because at that moment these are not a
    // reference somebody might consult - they are the keystroke about to be made, and the
    // modifier is still down. At rest they stay quiet: a number beside every row, drawn as
    // loudly as the name it sits next to, is a list that is harder to read for the sake of
    // something you already know.
    number.font = .monospacedDigitSystemFont(
      ofSize: 10, weight: row.isSecondPress ? .semibold : .regular)
    number.textColor = row.isSecondPress ? .controlAccentColor : .tertiaryLabelColor
    addSubview(number)
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
    // Number, then dot, then label. Both are laid out from a running left edge rather than
    // each from `left`, because a pane row now carries both: the number says how to reach the
    // row and the dot says why you would want to, and they used to be alternatives only
    // because a row was either a caption or a pane.
    var textLeft = left
    if number.superview != nil {
      let height = min(bounds.height, number.fittingSize.height)
      number.frame = CGRect(
        x: textLeft, y: (bounds.height - height) / 2,
        width: SidebarRowView.numberWidth, height: height)
      textLeft += SidebarRowView.numberWidth + 4
    }
    if dot.superlayer != nil {
      dot.frame = CGRect(
        x: textLeft, y: (bounds.height - SidebarRowView.dotSize) / 2,
        width: SidebarRowView.dotSize, height: SidebarRowView.dotSize)
      textLeft += SidebarRowView.inset + SidebarRowView.dotSize
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
