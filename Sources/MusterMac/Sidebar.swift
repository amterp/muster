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

    /// The press that names this pane's tab, or 0 when none does - which is every pane under
    /// the scheme Muster ships, where one press names a pane and no press names a tab.
    public let tabPress: Int

    /// The press that names this pane: inside its tab under `numbered_chords =
    /// "tab_then_pane"`, and down the whole window under the scheme Muster ships. 0 when none
    /// does.
    ///
    /// With `tabPress`, the presses that reach this pane in the order a hand makes them, and
    /// what the row draws. Distinct from `place` above, which is where the row sits whether or
    /// not anything gets you there.
    ///
    /// Both 0 means nothing reaches this pane, and there is no half-reachable state: a pane
    /// past the ninth in its tab carries neither press rather than a tab press that would land
    /// on another tab.
    public let press: Int

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
      key: PaneKey, place: Int = 0, tabPress: Int = 0, press: Int = 0, label: String,
      subtitle: String = "", givenName: String = "", onScreen: Bool
    ) {
      self.key = key
      self.place = place
      self.tabPress = tabPress
      self.press = press
      self.label = label
      self.subtitle = subtitle
      self.givenName = givenName
      self.onScreen = onScreen
    }
  }

  public struct Tab: Equatable {
    public let id: String

    /// The machines holding panes in it, in the order their regions sit on screen. One for
    /// every tab until somebody groups two.
    public let daemons: [String]

    /// Where this tab sits in the window's whole tab order, counting from one. The number in
    /// the caption of a tab nobody named.
    public let place: Int

    /// The press that names this tab, or 0 when none does - which is every tab under the
    /// scheme Muster ships, where ⌘N names panes. See `Pane.press`.
    ///
    /// It stays drawn while a press is outstanding, even though ⌘N means something else for
    /// as long as one is. `armed` below is where that is said instead: taking the digit away
    /// is what used to make the numbers move under somebody reading them.
    public let press: Int

    /// Whether the next press names a pane inside this tab.
    ///
    /// At most one tab carries it, and only under `numbered_chords = "tab_then_pane"` with a
    /// press outstanding. What the window says in place of moving its numbers - the presses
    /// the next keystroke can make are this tab's panes', and the list draws those as live.
    public let armed: Bool

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
      id: String, daemons: [String] = [], place: Int, press: Int = 0, armed: Bool = false,
      label: String, onScreen: Bool, givenName: String = "", panes: [Pane]
    ) {
      self.id = id
      self.daemons = daemons
      self.place = place
      self.press = press
      self.armed = armed
      self.label = label
      self.onScreen = onScreen
      self.givenName = givenName
      self.panes = panes
    }
  }

  /// One attached machine, for the two states no pane row can carry.
  ///
  /// A machine that is connected and holding panes says it through its panes; what needs
  /// somewhere else to go is a machine that is unreachable and a machine holding nothing at all.
  /// Without a heading per machine over the tabs, the second would vanish from the window
  /// entirely, which is the state kan a_2HpkpfIfq was about.
  public struct Machine: Equatable {
    public let id: String

    /// `connected`, `stale` or `disconnected`.
    public let state: String

    /// How many panes it holds, on screen or not. Zero is the state worth drawing.
    public let panes: Int

    public init(id: String, state: String, panes: Int) {
      self.id = id
      self.state = state
      self.panes = panes
    }

    /// Whether this machine has something to say that its panes do not.
    public var worthDrawing: Bool { panes == 0 || state != "connected" }
  }

  /// What a press is counting, and so whether one is half-typed.
  ///
  /// Not a second answer to what reaches which row - that is the presses on the row, and this
  /// side reads them rather than working them out. What this adds is the question a row cannot
  /// answer: under `numbered_chords = "tab_then_pane"` a first press leaves the window waiting
  /// for a second, and three things here need to know it. Panes draw a number over themselves
  /// only then, the window ends the gesture when the modifier comes up, and the list reserves
  /// the room a two-press chord takes.
  ///
  /// *Which* tab was named is `Tab.armed` rather than anything here. It used to be readable off
  /// the rows - it was the tab whose panes held the numbers - and once every pane row carries
  /// its own chord, the rows no longer say it.
  public enum Numbering: Equatable {
    /// Panes, down the whole window. What Muster does.
    case panes
    /// Tabs, across the window. `tab_then_pane`, with no press outstanding.
    case tabs
    /// The panes inside the tab a press named. `tab_then_pane`, half-typed.
    case panesInTab

    /// Whether a chord is half-typed, waiting for the press that names a pane.
    public var isHalfTyped: Bool { self == .panesInTab }

    /// Whether reaching a pane in this window takes two presses.
    ///
    /// What decides how much room the list reserves for a chord. Under the settled scheme one
    /// press names a pane, tab captions carry nothing, and there is no gutter to reserve.
    public var takesTwoPresses: Bool { self != .panes }
  }

  /// The window's tabs, in the order it walks them.
  public let tabs: [Tab]

  /// The machines behind them.
  public let machines: [Machine]

  public let numbering: Numbering

  public init(tabs: [Tab], machines: [Machine] = [], numbering: Numbering = .panes) {
    self.tabs = tabs
    self.machines = machines
    self.numbering = numbering
  }

  /// Every pane in the window, in the order they are listed.
  public var panes: [Pane] { tabs.flatMap(\.panes) }

  /// Whether more than one machine is attached, which is when a pane row says which it is on.
  ///
  /// On one machine the answer is the same on every row and says nothing, and the window reads
  /// exactly as it did before a tab could span two.
  public var spansMachines: Bool { machines.count > 1 }
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
    /// A tab, over the panes in it, carrying the press that reaches it or 0.
    case tab(press: Int)
    /// A pane, carrying the presses that reach it in order, either or both 0 for none.
    case pane(tabPress: Int, press: Int)
    /// A machine with something to say its panes cannot: unreachable, or holding nothing.
    case machine
  }

  /// One line in the list.
  public struct Row: Equatable {
    public let kind: Kind

    /// The machine this row is about: the one holding the pane, or the machine itself.
    ///
    /// Empty on a tab row, and that is the decision the flattening rests on: a tab may hold
    /// panes on two machines, so any single answer there would be wrong for some of its panes.
    public let daemon: String

    /// The tab this row is or sits under. Empty on a machine row.
    public let tab: String

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

    /// How many presses this row leaves room for, whether or not it carries them.
    ///
    /// Two on a pane row and one on a caption under `numbered_chords = "tab_then_pane"`, which
    /// is what each kind of row can carry. Reserved rather than measured per row because the
    /// rows that carry nothing are scattered through the list - a pane past the ninth in its
    /// tab, a tab past the ninth - and labels that lined up only where a chord happened to
    /// exist would read as a list with a column missing.
    ///
    /// Zero under the settled scheme, where a caption carries nothing and reserving space for a
    /// press no tab will ever have would be an indent that buys nothing.
    public let reservedPresses: Int

    /// Whether this row says which machine its pane is on.
    ///
    /// True on every pane row while more than one machine is attached, and false on all of them
    /// otherwise - the answer is a property of the window rather than of the row, because a
    /// column that appeared on some rows and not others would be worse than either.
    ///
    /// Here rather than in the view so the rule is testable, and because it is the visible half
    /// of the flattening: with no heading per machine, this is the only thing that says a pane
    /// is on the devenv (MIP-2).
    public let showsMachine: Bool

    /// Whether this row's pane press is the second press of a chord already begun.
    ///
    /// Only under `tab_then_pane`, and only on the pane rows of the tab a press has named.
    /// Drawn brighter than a press at rest, because at that moment it is not a reference - it
    /// is the thing the hand is about to do, and the window has to say which presses are live
    /// while the modifier is still down.
    ///
    /// It is also the whole of what ⌘4 changes in the list. The presses beside the rows do not
    /// move any more, so the accent arriving down one tab's group is what says which tab was
    /// named and what the next keystroke will do.
    public let isSecondPress: Bool

    public var isHeader: Bool { kind == .machine }

    /// Whether picking this row means something.
    ///
    /// Every row does now, including a machine's: picking one asks for a pane on it, which is
    /// the only way into a machine holding nothing (kan a_2HpkpfIfq) and what makes it safe to
    /// stop giving every machine a column of its own (kan a_2I6h18OU6).
    public var isDestination: Bool { true }

    public var isMachine: Bool { kind == .machine }

    public var isTab: Bool {
      if case .tab = kind { return true }
      return false
    }

    public var isPane: Bool {
      if case .pane = kind { return true }
      return false
    }
  }

  /// The rows to draw, in order: a caption per tab with its panes under it, then the machines
  /// that have something to say.
  ///
  /// **Flat, because the window is.** A window holds an ordered list of tabs and shows one, so
  /// the list reads the way ⌘1 to ⌘9 and next-tab walk it. Grouping by machine is what this
  /// stopped doing (MIP-2): a tab may hold panes on two, so a heading over it would be wrong for
  /// some of its panes, and the list would no longer describe the window beside it.
  ///
  /// **The machine goes on the pane row, and only with more than one attached.** On one machine
  /// the answer is the same on every row and says nothing, which is the common case and reads
  /// exactly as it did before.
  ///
  /// **A window with one tab draws no caption, unless a chord names it.** There is nothing to
  /// navigate between, so a row saying which tab you are in is a line that answers a question
  /// nobody has - and this is the common case, so paying a level of nesting for it would make
  /// the list worse for most people to make it better for some. The moment a second tab exists
  /// anywhere in the window, every tab gets a caption: captions in patches would read as a
  /// boundary that comes and goes.
  ///
  /// The exception is a numbered tab, which is drawn whatever else this rule says: a number
  /// nothing draws is a chord nobody can find. The core stopped producing one in a window of a
  /// single tab - under `numbered_chords = "tab_then_pane"` such a window numbers panes,
  /// because with one tab the two numberings are the same numbers - so the exception guards a
  /// state rather than describing one. It stays because which rows carry numbers is the core's
  /// answer and not this one's, and a list that hid one would be worse than a caption nobody
  /// needed.
  ///
  /// **What reaches a row is the core's answer, not this one's.** Every row arrives with the
  /// presses that reach it or with none, so a row drawing a chord the core did not give it is
  /// not a state this side can produce. What this side decides is emphasis: which of those
  /// presses the very next keystroke can make, from `Tab.armed`.
  ///
  /// **A machine gets a row only when it has something to say**: it is unreachable, or it is
  /// holding no panes at all. A machine that is connected and holding panes says so through its
  /// panes, and a row repeating it would be a heading in everything but name. The rows that do
  /// appear are what keeps an empty machine visible and reachable now that nothing is owed a
  /// column of its own (kan a_2HpkpfIfq, a_2I6h18OU6).
  ///
  /// `keyboard` is the pane the core's view says has the keyboard, or nil when no region
  /// does. Passed in rather than derived here: which pane that is arrives on the view, and
  /// the roster is a separate message - the same join the window already makes for states.
  public static func rows(roster: Roster, states: [PaneKey: String], keyboard: PaneKey? = nil)
    -> [Row]
  {
    let captions = roster.tabs.count > 1 || roster.tabs.contains { $0.press > 0 }
    // Reserved from what a kind of row can carry in this window rather than from what each row
    // happens to carry, so the column is a property of the list - which is the whole point of
    // reserving it.
    let twoPress = roster.numbering.takesTwoPresses
    let sayMachine = roster.spansMachines
    var rows: [Row] = []
    for tab in roster.tabs {
      if captions {
        rows.append(
          Row(
            kind: .tab(press: tab.press), daemon: "", tab: tab.id, pane: nil,
            label: tab.label, subtitle: "", givenName: tab.givenName, state: "",
            onScreen: tab.onScreen, hasKeyboard: false, reservedPresses: twoPress ? 1 : 0,
            showsMachine: false,
            // A caption carries the one press that reaches its tab, which is a first press or
            // none - so there is no such thing as a second press onto a caption.
            isSecondPress: false))
      }
      for pane in tab.panes {
        rows.append(
          Row(
            kind: .pane(tabPress: pane.tabPress, press: pane.press), daemon: pane.key.daemon,
            tab: tab.id, pane: pane.key, label: pane.label,
            subtitle: pane.subtitle, givenName: pane.givenName,
            // A pane the core has said nothing about is unknown, not idle. An agent we have
            // not heard from is not an agent that finished
            // (`corpus/conformance/agent-state.json`).
            state: states[pane.key] ?? "unknown",
            onScreen: pane.onScreen,
            hasKeyboard: pane.key == keyboard,
            reservedPresses: twoPress ? 2 : 0,
            showsMachine: sayMachine,
            isSecondPress: tab.armed))
      }
    }
    for machine in roster.machines where machine.worthDrawing {
      rows.append(
        Row(
          kind: .machine, daemon: machine.id, tab: "", pane: nil, label: machine.id,
          subtitle: "", givenName: "", state: machine.state, onScreen: false,
          hasKeyboard: false, reservedPresses: 0, showsMachine: false, isSecondPress: false))
    }
    return rows
  }

  /// Whether dragging one pane onto a row is a gesture Muster can carry out.
  ///
  /// Here rather than in the view so that the rule is testable: a decision inside
  /// `validateDrop` is a decision no test can reach, and this one has a case that is easy to
  /// get wrong and impossible to see - two daemons hand out the same pane ids, so a rule
  /// comparing ids alone would call a cross-machine drop legal.
  ///
  /// **A drop on a pane row must be on the same machine.** A pane is a PTY its daemon owns, so
  /// moving one to another machine would mean killing a process on one host and starting a
  /// different one on another, which is not what dragging a row looks like it does.
  ///
  /// **A drop on a tab caption may cross machines**, and is how a tab comes to hold a laptop
  /// pane beside a devenv one (MIP-2, stage four). The pane stays where it is; what changes is
  /// which tab it belongs to, and a tab is a grouping Muster made rather than anything a daemon
  /// holds. So the two destinations differ in exactly the way the requests behind them do.
  ///
  /// A machine's row is not a place a pane can go: it names no tab, and dropping onto a machine
  /// that already holds the pane would mean nothing at all.
  ///
  /// Dropping a row on itself is legal and does nothing, which is what an accidental drag is.
  public static func canArrange(_ pane: PaneKey, onto row: Row) -> Bool {
    if row.isTab {
      return true
    }
    guard let target = row.pane else { return false }
    return target.daemon == pane.daemon
  }

  /// Whether a tab dragged from somewhere may be dropped on this window's list.
  ///
  /// Only from another window. A tab belongs to exactly one window (kan a_2Mhi0EZlv), so a tab
  /// row dropped into another window's list is a move into that window - the same request as
  /// `muster tab move` - and one dropped back into its own list is going nowhere.
  public static func acceptsTab(fromThisWindow: Bool) -> Bool {
    !fromThisWindow
  }

  /// Whether a pane dragged from somewhere may be dropped on this window's list.
  ///
  /// Only from this window's own. A pane dragged in from another window would need that window
  /// to let go of it, which moving a pane between windows does not do yet (kan a_29bpH8BZ5).
  public static func acceptsPane(fromThisWindow: Bool) -> Bool {
    fromThisWindow
  }

  /// The dot beside a row, and whether to draw one at all.
  ///
  /// The same colors the pane borders use, because they are the same five states and a
  /// sidebar that disagreed with the window beside it would be worse than no sidebar. Unlike
  /// a border, the dot is drawn for every state including idle: a border exists to be
  /// noticed against a resting default, where a list with gaps in a column reads as missing
  /// data rather than as calm.
  @MainActor
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
  public var onTabPicked: ((String) -> Void)?

  /// Called when somebody picks a machine's row, meaning they want a pane on it.
  ///
  /// The only way into a machine holding nothing, and what makes it safe for Muster to stop
  /// giving every machine a column of its own: a machine you have finished with stays empty,
  /// and one row gets you back (kan a_2HpkpfIfq, a_2I6h18OU6).
  public var onMachinePicked: ((String) -> Void)?

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

  /// Called when somebody drags an agent's row onto a tab caption, meaning they want it in that
  /// tab - which may be a tab holding panes on another machine.
  ///
  /// Its own callback rather than the one above, because the request behind it is a different
  /// one: that names two panes and this names a tab, and only this one may cross machines.
  public var onPaneGrouped: ((PaneKey, String) -> Void)?

  /// Called when somebody drops another window's tab row on this list, meaning they want that
  /// tab in this window.
  public var onTabReceived: ((String) -> Void)?

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

  /// A tab row, which is dragged between windows rather than within one: dropped into another
  /// window's list, it moves the tab there.
  static let draggedTab = NSPasteboard.PasteboardType("dev.muster.tab")

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
    table.registerForDraggedTypes([SidebarView.draggedPane, SidebarView.draggedTab])
    // A move rather than a copy: there is no second copy of an agent to make. Offered outside
    // this window too, because a tab row goes to another window - which is another process, so
    // to AppKit another application. The types are Muster's own, so nothing else accepts one,
    // and which of them may land where is `SidebarModel.acceptsTab` and `acceptsPane`.
    table.setDraggingSourceOperationMask(.move, forLocal: false)
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
  /// Redraws every row against the colours in force, after the config file was read again.
  ///
  /// Whole rather than diffed, unlike everything else here: the rows have not changed, only
  /// what they are painted in, so the diff that keeps a fifteen-row list from flickering on
  /// every agent transition would correctly decide there was nothing to do.
  public func reloadColors() {
    table.reloadData()
  }

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
      onTabPicked?(rows[clicked].tab)
    case .machine:
      onMachinePicked?(rows[clicked].daemon)
    }
  }

  /// A double-click asks to rename. A machine's row names nothing renameable, so it does
  /// nothing - the machine's name is the config file's and not Muster's to change.
  @objc private func rowDoubleClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked), !rows[clicked].isMachine else { return }
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
    guard rows.indices.contains(row) else { return nil }
    let item = NSPasteboardItem()
    if rows[row].isTab {
      item.setString(rows[row].tab, forType: SidebarView.draggedTab)
      return item
    }
    guard let pane = rows[row].pane else { return nil }
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
    // A drag from another window has no source this process can see, which is what tells the
    // two kinds of drop apart.
    let fromThisWindow = info.draggingSource != nil
    if draggedTab(info) != nil {
      guard SidebarModel.acceptsTab(fromThisWindow: fromThisWindow) else { return [] }
      // Onto the list as a whole: a tab joins the end of this window's list, whichever row the
      // pointer happens to be over.
      tableView.setDropRow(-1, dropOperation: .on)
      return .move
    }
    guard let pane = dragged(info), rows.indices.contains(row),
      SidebarModel.acceptsPane(fromThisWindow: fromThisWindow)
    else { return [] }
    if operation == .above {
      tableView.setDropRow(row, dropOperation: .on)
    }
    return SidebarModel.canArrange(pane, onto: rows[row]) ? .move : []
  }

  public func tableView(
    _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
    dropOperation operation: NSTableView.DropOperation
  ) -> Bool {
    let fromThisWindow = info.draggingSource != nil
    if let tab = draggedTab(info) {
      guard SidebarModel.acceptsTab(fromThisWindow: fromThisWindow) else { return false }
      onTabReceived?(tab)
      return true
    }
    guard let pane = dragged(info), rows.indices.contains(row),
      SidebarModel.acceptsPane(fromThisWindow: fromThisWindow),
      SidebarModel.canArrange(pane, onto: rows[row])
    else { return false }
    if rows[row].isTab {
      onPaneGrouped?(pane, rows[row].tab)
      return true
    }
    guard let onto = rows[row].pane else { return false }
    onPaneArranged?(pane, onto)
    return true
  }

  /// The tab a drag is carrying, or nil when it is carrying something else.
  private func draggedTab(_ info: NSDraggingInfo) -> String? {
    info.draggingPasteboard.string(forType: SidebarView.draggedTab).flatMap {
      $0.isEmpty ? nil : $0
    }
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

/// One row: a state dot or the chord that reaches it, a name, and whether anything is showing it.
@MainActor
final class SidebarRowView: NSView {
  private let dot = CALayer()
  private let showing = CALayer()
  private let name = NSTextField(labelWithString: "")
  private let subtitle = NSTextField(labelWithString: "")
  private let tabPress = NSTextField(labelWithString: "")
  private let press = NSTextField(labelWithString: "")
  private let highlight = CALayer()
  private let indented: Bool
  private let isTab: Bool

  init(row: SidebarModel.Row) {
    // Panes indent under their tab caption, and sit flush when there is none. The list is
    // 200pt wide, so a level of nesting that buys nothing is a level that costs a word off
    // every label.
    indented = row.isPane && row.tab != nil
    isTab = row.isTab
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
    case .machine:
      name.font = .systemFont(ofSize: 10, weight: .semibold)
      name.stringValue = row.label.uppercased()
      name.textColor = .secondaryLabelColor
    case .tab(let reached):
      // Its own press sits in the tab column, drawn as a whole chord rather than as the prefix
      // to one - a caption is somewhere you go, not a step on the way to a pane.
      // The tab on screen is named in full, and the ones behind it are quieter. This says a
      // different thing from the keyboard highlight on purpose: one is where you are
      // looking, the other is where you are typing, and in a two-region window those are
      // two different tabs.
      name.font = .systemFont(ofSize: 11, weight: row.onScreen ? .semibold : .regular)
      name.stringValue = row.label
      name.textColor = row.onScreen ? .labelColor : .secondaryLabelColor
      draw(tabPress: reached, press: 0, in: row)
      // A mark rather than only the weight above: one row in the list is the tab you are
      // looking at, and a font weight is something you compare where a mark is something you
      // see. It answers a narrower question than it used to - the window shows one tab now, so
      // this is the ordinary current-tab mark rather than the "which of these share the screen"
      // mark a column-per-machine window needed (kan a_2HtF52Itm).
      //
      // Quiet, and deliberately not the accent colour: this says where you are looking, and the
      // accent highlight on a pane row already says where you are typing.
      if row.onScreen {
        showing.backgroundColor = NSColor.tertiaryLabelColor.cgColor
        showing.cornerRadius = SidebarRowView.showingSize / 2
        layer?.addSublayer(showing)
      }
    case .pane(let first, let second):
      name.font = .systemFont(ofSize: 12, weight: .regular)
      name.stringValue = row.label
      draw(tabPress: first, press: second, in: row)
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
  /// The dot is what the row is for and the chord is how to get there, so a row wants both.
  /// Zero draws nothing, which is what a press a row does not have carries - the tab column
  /// under the scheme Muster ships, and both columns on a pane nothing reaches.
  ///
  /// One function for every kind of row, because a chord means one thing wherever it lands and
  /// two implementations of "draw the chord" would be two chances for them to look different
  /// depending on which row they were on.
  ///
  /// **The last press of a chord is the operative one and reads at full weight; a press before
  /// it is a prefix and stays quiet.** So a caption's own press and the lone press onto a
  /// one-pane tab both read as loudly as a pane digit does - each is a whole chord - while the
  /// tab digit repeating down a group of panes recedes, which is the digit that says nothing
  /// new about the row it is on.
  ///
  /// A row reserving a column still adds the field with nothing in it, which is what keeps
  /// every label in the list on one edge.
  private func draw(tabPress first: Int, press second: Int, in row: SidebarModel.Row) {
    let hasFirst = SidebarRowView.pressable(first)
    let hasSecond = SidebarRowView.pressable(second)
    if hasFirst || row.reservedPresses >= 1 {
      tabPress.stringValue = hasFirst ? String(first) : ""
      style(tabPress, operative: !hasSecond, live: false)
      addSubview(tabPress)
    }
    if hasSecond || row.reservedPresses >= 2 {
      press.stringValue = hasSecond ? String(second) : ""
      style(press, operative: true, live: row.isSecondPress)
      addSubview(press)
    }
  }

  /// How loudly one press of a chord is drawn.
  ///
  /// `live` is brighter and heavier because at that moment the press is not a reference
  /// somebody might consult - it is the keystroke about to be made, and the modifier is still
  /// down. At rest everything stays quiet: a chord beside every row, drawn as loudly as the
  /// name it sits next to, is a list that is harder to read for the sake of something you
  /// already know.
  private func style(_ field: NSTextField, operative: Bool, live: Bool) {
    field.font = .monospacedDigitSystemFont(
      ofSize: operative ? 10 : 9, weight: live ? .semibold : .regular)
    field.textColor =
      live ? .controlAccentColor : (operative ? .tertiaryLabelColor : .quaternaryLabelColor)
  }

  /// Whether a press is one of the nine ⌘1 to ⌘9 name.
  ///
  /// The core sends nothing else, and this is the shell agreeing rather than deciding: a digit
  /// drawn beside a row that no key can produce would be worse than an unlabelled row.
  private static func pressable(_ press: Int) -> Bool {
    (1...9).contains(press)
  }

  /// One press's column, centred on the row, advancing the edge the next thing starts from.
  private func place(_ field: NSTextField, from left: inout CGFloat, width: CGFloat) {
    let height = min(bounds.height, field.fittingSize.height)
    field.frame = CGRect(
      x: left, y: (bounds.height - height) / 2, width: width, height: height)
    left += width
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  static let dotSize: CGFloat = 7
  /// Smaller than a state dot, and in the same column. A mark the size of a pane's dot would
  /// read as a state on a row that has no agent to have one.
  static let showingSize: CGFloat = 4
  static let inset: CGFloat = 8
  static let indent: CGFloat = 10
  /// Wide enough for the digit a press is drawn as, at the size an operative press is drawn.
  static let pressWidth: CGFloat = 12
  /// Narrower, because a prefix is drawn a point smaller - and because the two columns
  /// together come off a 200pt list that is already truncating labels.
  static let prefixWidth: CGFloat = 10
  /// Between the two presses of one chord: close enough to read as one address rather than as
  /// two columns that happen to be adjacent.
  static let pressGap: CGFloat = 3

  override func layout() {
    super.layout()
    if highlight.superlayer != nil {
      highlight.frame = bounds.insetBy(dx: 4, dy: 1)
    }
    let left = SidebarRowView.inset + (indented ? SidebarRowView.indent : 0)
    // Chord, then dot, then label, laid out from a running left edge rather than each from
    // `left`, because a pane row carries both: the chord says how to reach the row and the dot
    // says why you would want to, and they used to be alternatives only because a row was
    // either a caption or a pane. A caption now takes the dot's column too, for the mark saying
    // a region is showing it.
    var textLeft = left
    let chorded = tabPress.superview != nil || press.superview != nil
    if tabPress.superview != nil {
      place(tabPress, from: &textLeft, width: SidebarRowView.prefixWidth)
      if press.superview != nil { textLeft += SidebarRowView.pressGap }
    }
    if press.superview != nil {
      place(press, from: &textLeft, width: SidebarRowView.pressWidth)
    }
    if chorded { textLeft += 4 }
    if dot.superlayer != nil {
      dot.frame = CGRect(
        x: textLeft, y: (bounds.height - SidebarRowView.dotSize) / 2,
        width: SidebarRowView.dotSize, height: SidebarRowView.dotSize)
      textLeft += SidebarRowView.inset + SidebarRowView.dotSize
    } else if isTab {
      // The column is taken on every caption and filled on the ones on screen, so a tab that
      // goes behind another does not slide its label fifteen points sideways as it happens.
      // Centred on where a pane's dot sits, so the marks read down the list as one column.
      let size = SidebarRowView.showingSize
      showing.frame = CGRect(
        x: textLeft + (SidebarRowView.dotSize - size) / 2, y: (bounds.height - size) / 2,
        width: size, height: size)
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
