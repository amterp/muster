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

    /// The press that names this pane's tab, or 0 when none does - which is every pane in a
    /// window of one tab, where one press names a pane and no press names a tab.
    public let tabPress: Int

    /// The press that names this pane: inside its tab, or down the whole window when the window
    /// holds one tab. 0 when none does.
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

    /// Whether its daemon can compact its agent. What Compact Agent greys itself out on.
    public let compactable: Bool

    /// Whether its daemon can clear its agent. What Clear Context greys itself out on.
    public let clearable: Bool

    /// The harness its daemon detected in it (`claude`), empty for a pane running no agent. What
    /// closing the pane asks on: the same fact the row's label draws its harness from.
    public let agent: String

    /// What its daemon would run to resume its agent's session, for a person to read; empty
    /// when it cannot be restarted. What Restart Agent greys itself out on.
    public let resumesAs: String

    /// Whether nothing about it is raised for the person. What Quiet Pane is checked by.
    public let quiet: Bool

    /// Its callsign (`otter`), named since or not; empty on a daemon from before callsigns.
    /// Drawn in a style of its own at the front of the row.
    public let callsign: String

    /// What the row draws after `callsign`: the given name, or the directory and harness. The
    /// core takes the callsign out of `label` for it, so the callsign is drawn once.
    public let besideCallsign: String

    /// The directory the pane is in, whole, as its daemon reports it; empty when it has none.
    /// Apart from `label`, which names a renamed pane by its name and says nothing of where it is.
    public let directory: String

    /// The id of the pane that made it as its helper, empty for a pane nobody made (MIP-7).
    public let maker: String

    /// The id of the row it is listed beneath rather than where its tab puts it, as the core
    /// decided: its maker's, for a hidden helper whose maker has a row. Empty for most panes.
    public let under: String

    public init(
      key: PaneKey, place: Int = 0, tabPress: Int = 0, press: Int = 0, label: String,
      subtitle: String = "", givenName: String = "", onScreen: Bool, compactable: Bool = false,
      clearable: Bool = false, agent: String = "", resumesAs: String = "", quiet: Bool = false,
      callsign: String = "",
      besideCallsign: String? = nil, directory: String = "",
      maker: String = "", under: String = ""
    ) {
      self.key = key
      self.place = place
      self.tabPress = tabPress
      self.press = press
      self.label = label
      self.subtitle = subtitle
      self.givenName = givenName
      self.onScreen = onScreen
      self.compactable = compactable
      self.clearable = clearable
      self.agent = agent
      self.resumesAs = resumesAs
      self.quiet = quiet
      self.callsign = callsign
      self.besideCallsign = besideCallsign ?? label
      self.directory = directory
      self.maker = maker
      self.under = under
    }
  }

  public struct Tab: Equatable {
    public let folded: Bool
    public let id: String

    /// The machines holding panes in it, in the order their regions sit on screen. One for
    /// every tab until somebody groups two.
    public let daemons: [String]

    /// Where this tab sits in the window's whole tab order, counting from one. The number in
    /// the caption of a tab nobody named.
    public let place: Int

    /// The press that names this tab, or 0 when none does - which is the only tab in a window,
    /// where ⌘N names panes. See `Pane.press`.
    ///
    /// It stays drawn while a press is outstanding, even though ⌘N means something else for
    /// as long as one is. `armed` below is where that is said instead: taking the digit away
    /// is what used to make the numbers move under somebody reading them.
    public let press: Int

    /// Whether the next press names a pane inside this tab.
    ///
    /// At most one tab carries it, and only while a press is outstanding. What the window says in place of moving its numbers - the presses
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
      label: String, onScreen: Bool, givenName: String = "", folded: Bool = false, panes: [Pane]
    ) {
      self.id = id
      self.daemons = daemons
      self.place = place
      self.press = press
      self.armed = armed
      self.label = label
      self.onScreen = onScreen
      self.givenName = givenName
      self.folded = folded
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

    /// What marks its rows, as `#rrggbb`. The core's answer, so every window agrees on it.
    public let color: String

    /// What its last restart cost, in a few words, while that still needs somebody; empty
    /// otherwise. The problem list carries the whole sentence.
    public let restarted: String

    public init(id: String, state: String, panes: Int, color: String = "", restarted: String = "") {
      self.id = id
      self.state = state
      self.panes = panes
      self.color = color
      self.restarted = restarted
    }

    /// Whether this machine has something to say that its panes do not.
    public var worthDrawing: Bool { panes == 0 || state != "connected" || !restarted.isEmpty }
  }

  /// What a press is counting, and so whether one is half-typed.
  ///
  /// Not a second answer to what reaches which row - that is the presses on the row, and this
  /// side reads them rather than working them out. What this adds is the question a row cannot
  /// answer: a first press leaves the window waiting for a second, and three things here need to know it. Panes draw a number over themselves
  /// only then, the window ends the gesture when the modifier comes up, and the list reserves
  /// the room a two-press chord takes.
  ///
  /// *Which* tab was named is `Tab.armed` rather than anything here. It used to be readable off
  /// the rows - it was the tab whose panes held the numbers - and once every pane row carries
  /// its own chord, the rows no longer say it.
  public enum Numbering: Equatable {
    /// Panes, down the whole window. A window holding one tab.
    case panes
    /// Tabs, across the window, with no press outstanding.
    case tabs
    /// The panes inside the tab a press named, half-typed.
    case panesInTab

    /// Whether a chord is half-typed, waiting for the press that names a pane.
    public var isHalfTyped: Bool { self == .panesInTab }

    /// Whether reaching a pane in this window takes two presses.
    ///
    /// What decides how much room the list reserves for a chord. In a window of one tab one
    /// press names a pane, the tab has no caption, and there is no gutter to reserve.
    public var takesTwoPresses: Bool { self != .panes }
  }

  /// The window's tabs, in the order it walks them.
  public let tabs: [Tab]

  /// The machines behind them.
  public let machines: [Machine]

  public let numbering: Numbering

  /// The tabs no window shows, which hiding a pane makes. Nothing numbers or draws them.
  public let hidden: [Tab]

  public init(
    tabs: [Tab], machines: [Machine] = [], numbering: Numbering = .panes, hidden: [Tab] = []
  ) {
    self.tabs = tabs
    self.machines = machines
    self.numbering = numbering
    self.hidden = hidden
  }

  /// Every pane in the window, in the order they are listed. Not the hidden ones, which the
  /// window draws nothing of: a surface held for one would be a terminal kept for nobody.
  public var panes: [Pane] { tabs.flatMap(\.panes) }

  /// Every hidden pane, in the order they are listed.
  public var hiddenPanes: [Pane] { hidden.flatMap(\.panes) }

  /// The entry for a pane, hidden or not: a hidden pane's row can be renamed and opens a menu
  /// like any other.
  public func pane(_ key: PaneKey) -> Pane? {
    panes.first { $0.key == key } ?? hiddenPanes.first { $0.key == key }
  }

  /// What a pane's menu can do with its helpers, or nil for a pane that has none here.
  ///
  /// Show while any of them is hidden, and Hide while one of the window's tabs holds nothing
  /// but its helpers: the tabs the core would show and hide (`show_helpers`, `hide_helpers`).
  public func helpers(of key: PaneKey) -> ContextMenuModel.Helpers? {
    let made = { (pane: Pane) in pane.maker == key.pane }
    let hidden = hiddenPanes.contains(where: made)
    let shown = tabs.filter { $0.panes.contains(where: made) }
    guard hidden || !shown.isEmpty else { return nil }
    return ContextMenuModel.Helpers(
      canShow: hidden, canHide: shown.contains { $0.panes.allSatisfy(made) })
  }

  /// Whether more than one machine is attached, which is when a pane row says which it is on.
  ///
  /// On one machine the answer is the same on every row and says nothing, and the window reads
  /// exactly as it did before a tab could span two.
  public var spansMachines: Bool { machines.count > 1 }
}

public struct AskingEntry: Equatable {
  public let daemon: String
  public let pane: String
  public let group: String
  public let alert: String
  public let sinceMs: Int64
  public let preview: String
  /// A pane's names as the core gives them, empty on a group or a pane no daemon holds any more.
  public let callsign: String
  public let label: String
  /// What a blocked pane says it is blocked on, empty on every other entry.
  public let subtitle: String

  init(_ entry: Muster_AskingEntry) {
    daemon = entry.daemonID
    pane = entry.paneID
    group = entry.group
    alert = entry.alert
    sinceMs = entry.sinceMs
    preview = entry.preview
    callsign = entry.callsign
    label = entry.label
    subtitle = entry.subtitle
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

  /// How wide the roster is when it is on screen, already held to the core's limits.
  public let sidebarWidth: CGFloat

  /// Whether the roster's Hidden caption is folded over the panes under it.
  public let hiddenCollapsed: Bool
  public let needsYouFolded: Bool

  public init(
    sidebar: Bool, sidebarWidth: CGFloat = SidebarModel.width, hiddenCollapsed: Bool = false,
    needsYouFolded: Bool = false
  ) {
    self.sidebar = sidebar
    self.sidebarWidth = sidebarWidth
    self.hiddenCollapsed = hiddenCollapsed
    self.needsYouFolded = needsYouFolded
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
  /// Which machine a row is on, as the row draws it: a swatch in the machine's color.
  public struct MachineMark: Equatable {
    public let daemon: String
    /// `#rrggbb`, as the core sent it.
    public let color: String
  }

  /// What one line in the list is.
  public enum Kind: Equatable {
    /// A tab, over the panes in it, carrying the press that reaches it or 0.
    case tab(press: Int)
    /// A pane, carrying the presses that reach it in order, either or both 0 for none.
    case pane(tabPress: Int, press: Int)
    /// A machine with something to say its panes cannot: unreachable, or holding nothing.
    case machine
    /// The caption over the panes no window shows: how many there are, how many of those are in
    /// the state the caption's dot takes from them, and whether it is folded over them.
    case hidden(panes: Int, urgent: Int, collapsed: Bool)
  }

  /// How loudly a pane's state asks for the person (Fencil `agent-list-rows`).
  public enum Tier: Equatable {
    /// Blocked and stuck: waiting on you, so the row is tinted and carries a colored edge.
    case asks
    /// Done: finished a turn you have not looked at.
    case look
    /// Working, waiting and idle: nothing to do, so the row says only where the pane is.
    case calm
    /// Unknown and unreachable: Muster cannot say what the agent is doing.
    case absent

    public init(state: String) {
      switch state {
      case "blocked", "stuck": self = .asks
      case "done": self = .look
      case "working", "waiting", "idle": self = .calm
      default: self = .absent
      }
    }
  }

  /// A row's place in the raised panel behind the tab on screen and the rows under its caption,
  /// which the list draws as one rounded rectangle a row at a time.
  public enum PanelPlace: Equatable {
    case top, middle, bottom
    /// A caption with nothing under it.
    case only
  }

  /// One of a pane's hidden helpers, or a helper of one of those, as its maker's entry draws it: a
  /// pill whose height and fill say its state (MIP-7; Fencil `helper-pills`).
  public struct HelperPill: Equatable {
    public let key: PaneKey
    /// Its callsign, or its label without one.
    public let name: String
    public let state: String
    /// What its own row would say of its state, with the time worked out (`stateLine(for:now:)`).
    public let stateLine: String
    /// Whether `stateLine` says how long the state has held, so a minute passing can change it.
    public let timed: Bool
    public let directory: String
    /// The name of the pane that made it, which for a helper of a helper is not the entry's pane.
    public let madeBy: String
  }

  /// One pane a folded tab caption stands in for.
  public struct FoldedPane: Equatable {
    public let key: PaneKey
    /// What to call it in the caption's hover text: its callsign, or its label without one.
    public let name: String
    public let state: String
  }

  /// A group the human is in, as the core listed it (MIP-4, Decision 1b).
  public struct MessageGroup: Equatable {
    public let daemon: String
    public let group: String
    public let unread: Int
    public let toYou: Int
    public let partnerPane: PaneKey?

    public init(
      daemon: String, group: String, unread: Int, toYou: Int, partnerPane: PaneKey? = nil
    ) {
      self.daemon = daemon
      self.group = group
      self.unread = unread
      self.toYou = toYou
      self.partnerPane = partnerPane
    }
  }

  /// A group the human is in with one agent, as its agent's row carries it: which group it is and
  /// what waits there. The group's daemon is not the pane's, since a group is kept on one machine
  /// and its partner may be on another.
  public struct Conversation: Equatable {
    public let daemon: String
    public let group: String
    public let unread: Int
    public let toYou: Int

    init(_ group: MessageGroup) {
      daemon = group.daemon
      self.group = group.group
      unread = group.unread
      toYou = group.toYou
    }
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

    /// Everything the core said about this pane's agent, on a pane row and nowhere else.
    public let agent: PaneAgent?

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
    /// Two on a pane row and one on a caption, which is what each kind of row can carry. Reserved rather than measured per row because the
    /// rows that carry nothing are scattered through the list - a pane past the ninth in its
    /// tab, a tab past the ninth - and labels that lined up only where a chord happened to
    /// exist would read as a list with a column missing.
    ///
    /// Zero in a window of one tab, where one press names a pane and reserving space for a tab
    /// press would be an indent that buys nothing.
    public let reservedPresses: Int

    /// The machine this row marks itself with, or nil for none.
    ///
    /// On every pane row and machine heading while more than one machine is attached, and on
    /// none of them otherwise - the answer is a property of the window rather than of the row,
    /// because a column that appeared on some rows and not others would be worse than either.
    /// The heading carries the same mark as its machine's panes, so the list is its own legend
    /// whenever a heading is drawn.
    ///
    /// Here rather than in the view so the rule is testable, and because it is the visible half
    /// of the flattening: with no heading per machine, this is the only thing that says a pane
    /// is on the devenv (MIP-2).
    public let machine: MachineMark?

    /// Whether this row's pane press is the second press of a chord already begun.
    ///
    /// Only on the pane rows of the tab a press has named.
    /// Drawn brighter than a press at rest, because at that moment it is not a reference - it
    /// is the thing the hand is about to do, and the window has to say which presses are live
    /// while the modifier is still down.
    ///
    /// It is also the whole of what ⌘4 changes in the list. The presses beside the rows do not
    /// move any more, so the accent arriving down one tab's group is what says which tab was
    /// named and what the next keystroke will do.
    public let isSecondPress: Bool

    /// Whether this pane says nothing to the person, which the row marks.
    public var quiet = false

    /// Whether this pane is one no window shows, listed under the hidden caption. Picking it
    /// brings it back, since going to a hidden pane does.
    public var hidden = false

    /// On a pane row, its callsign and what goes after it, as the core split them
    /// (`Roster.Pane`). An empty `callsign` draws `label` alone.
    public var callsign = ""
    public var besideCallsign = ""

    /// How many levels further in than a pane row this row sits: one for a helper listed under
    /// its maker, and one more under a helper of its own.
    public var depth = 0

    /// Whether this pane's agent is a member of the group whose conversation the window shows,
    /// so the list says which panes are in the group (mip/0009-group-conversation.md, section 1).
    public var member = false

    /// The directory the pane is in, whole, and its last component as line two draws it. Empty on
    /// the rows that are not panes, and on a pane whose daemon reported none.
    public var directory = ""
    public var directoryName = ""

    /// The directory's last part as a pane's header draws it beside the label: only for a pane
    /// somebody named. An unnamed pane's label is already its directory and harness, which the core
    /// builds it from, so repeating it would say the same thing twice.
    ///
    /// Not a comparison of the two strings: the label says `a` for the home directory where
    /// `directoryName` says `~`, and what decides it is whether the core built the label from the
    /// directory, which is whether the pane has a given name. The root is the exception: the core
    /// leaves it out of the label as it does a missing directory, so the header keeps it.
    public var namedDirectory: String {
      givenName.isEmpty && directoryName != "/" ? "" : directoryName
    }

    /// What the pane's harness titles its session, as the core passed it on: empty for most panes
    /// (`pane_subtitle`, in `crates/muster-core/src/roster.rs`).
    public var title = ""

    /// What an entry's task line says: the name somebody gave the pane, or else its harness's title
    /// (Fencil `agent-list-rows`). The other of the two is in the hover text. A pane with no
    /// callsign already draws its name on line one, so its task line is the title.
    public var taskLine: String {
      givenName.isEmpty || callsign.isEmpty ? title : givenName
    }

    /// On a pane's entry, its hidden helpers and theirs in one flat list, in the order the roster
    /// lists them, and whether the list of them under the entry is open.
    public var helpers: [HelperPill] = []
    public var helpersOpen = false

    /// Whether this pane is drawn as one line under its maker's open entry.
    public var isHelper = false

    /// How much of the person's attention this pane's state is asking for, and nil on the rows
    /// that are not panes.
    public var tier: Tier?

    /// What a pane row's second line says before its directory: the state's word and how long the
    /// state has held for the states that want somebody, what a waiting agent waits on, and
    /// nothing at all for a calm one. Decided here, with the time already worked out, so a minute
    /// passing changes the row's value and is redrawn exactly when it shows something different.
    public var stateLine = ""

    /// Where this row sits in the raised panel the tab on screen is drawn on, or nil when it is
    /// not part of one.
    public var panel: PanelPlace?

    /// On a tab caption: whether it can be folded, and whether it is. A window with one tab draws a
    /// caption only when a chord names it, and folding its only tab would hide every agent, so
    /// that caption has no control.
    public var foldable = false
    public var folded = false

    /// On a folded caption, the panes it is folded over, in the order the roster lists them, so
    /// that a state changing inside it changes the caption and redraws exactly that row.
    public var foldedPanes: [FoldedPane] = []

    /// On a pane row, the conversation between the person and this pane's agent, when this window
    /// draws that row (`OneToOne`).
    public var conversation: Conversation?

    /// Whether the row is drawn with a tint and a colored edge: the states that ask for somebody.
    public var isTinted: Bool { tier == .asks }

    public var isHeader: Bool { kind == .machine || isHiddenCaption }

    /// Whether picking this row means something.
    ///
    /// Every row does but the Hidden caption, including a machine's: picking one asks for a
    /// pane on it, which is the only way into a machine holding nothing (kan a_2HpkpfIfq) and
    /// what makes it safe to stop giving every machine a column of its own (kan a_2I6h18OU6).
    public var isDestination: Bool { !isHiddenCaption }

    public var isMachine: Bool { kind == .machine }

    public var isTab: Bool {
      if case .tab = kind { return true }
      return false
    }

    public var isPane: Bool {
      if case .pane = kind { return true }
      return false
    }

    public var isHiddenCaption: Bool {
      if case .hidden = kind { return true }
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
  /// single tab - such a window numbers panes, because with one tab a pane's place in the
  /// window and in its tab are the same number - so the exception guards a
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
  ///
  /// **The human's groups are not listed.** The one with an agent is an envelope on that agent's
  /// row, a group holding something for the person has a card in the Needs you strip, and every
  /// group is in the Conversations menus (`ConversationsMenu`; Fencil `conversations-menus`).
  ///
  /// **A folded Hidden caption draws none of its panes**, and says what it is folded over
  /// instead: how many there are, and the dot of the most urgent state among them, so a hidden
  /// agent that is blocked is still something the list shows.
  ///
  /// **A pane's hidden helpers are pills on its own entry**, one each, helpers of helpers among
  /// them in one flat list, and `helpersOpen` holding the pane lists them under it a line apiece
  /// (MIP-7, section 3; Fencil `helper-pills`). Which helpers go under which pane is the
  /// core's answer (`Roster.Pane.under`), the one `muster window` lists them by, and their order
  /// is the roster's. They leave the Hidden caption, which keeps the panes somebody hid by hand,
  /// and carry no presses, as no hidden pane does.
  public static func rows(
    roster: Roster, agents: [PaneKey: PaneAgent], keyboard: PaneKey? = nil,
    groups: [MessageGroup] = [], hiddenCollapsed: Bool = false, helpersOpen: Set<PaneKey> = [],
    members: Set<PaneKey> = [], now: Date = Date()
  ) -> [Row] {
    let captions = roster.tabs.count > 1 || roster.tabs.contains { $0.press > 0 }
    // Reserved from what a kind of row can carry in this window rather than from what each row
    // happens to carry, so the column is a property of the list - which is the whole point of
    // reserving it.
    let twoPress = roster.numbering.takesTwoPresses
    let colors = Dictionary(
      roster.machines.map { ($0.id, $0.color) }, uniquingKeysWith: { first, _ in first })
    let mark = { (daemon: String) -> MachineMark? in
      roster.spansMachines ? MachineMark(daemon: daemon, color: colors[daemon] ?? "") : nil
    }
    var rows: [Row] = []
    var under: [String: [(pane: Roster.Pane, tab: String)]] = [:]
    for tab in roster.hidden {
      for pane in tab.panes where !pane.under.isEmpty {
        under[pane.under, default: []].append((pane, tab.id))
      }
    }
    func name(_ pane: Roster.Pane) -> String {
      pane.callsign.isEmpty ? pane.label : pane.callsign
    }
    func everyHelper(of maker: Roster.Pane) -> [(pane: Roster.Pane, tab: String, madeBy: String)] {
      (under[maker.key.pane] ?? []).flatMap {
        [($0.pane, $0.tab, name(maker))] + everyHelper(of: $0.pane)
      }
    }
    func appendHidden(_ pane: Roster.Pane, tab: String) {
      rows.append(
        paneRow(
          pane, in: tab, agent: agents[pane.key] ?? "unknown", now: now, presses: (0, 0),
          onScreen: false, reservedPresses: twoPress ? 2 : 0, machine: mark(pane.key.daemon),
          hidden: true, member: members.contains(pane.key)))
      appendHelpers(of: pane)
    }
    // Puts a pane's helpers on the entry just appended for it, and lists them under it when its
    // list is open.
    func appendHelpers(of maker: Roster.Pane) {
      let helpers = everyHelper(of: maker)
      guard !helpers.isEmpty else { return }
      let open = helpersOpen.contains(maker.key)
      rows[rows.count - 1].helpers = helpers.map { helper in
        let agent = agents[helper.pane.key] ?? "unknown"
        return HelperPill(
          key: helper.pane.key, name: name(helper.pane), state: agent.state,
          stateLine: stateLine(for: agent, now: now), timed: showsTime(of: agent),
          directory: helper.pane.directory,
          madeBy: helper.madeBy)
      }
      rows[rows.count - 1].helpersOpen = open
      guard open else { return }
      for helper in helpers {
        var row = paneRow(
          helper.pane, in: helper.tab, agent: agents[helper.pane.key] ?? "unknown", now: now,
          presses: (0, 0), onScreen: false, machine: mark(helper.pane.key.daemon), hidden: true,
          depth: 1, member: members.contains(helper.pane.key))
        row.isHelper = true
        rows.append(row)
      }
    }
    for tab in roster.tabs {
      let first = rows.count
      defer {
        if captions && tab.onScreen { markPanel(&rows, from: first) }
      }
      let foldable = roster.tabs.count > 1
      let folded = foldable && tab.folded
      if captions {
        var caption = Row(
          kind: .tab(press: tab.press), daemon: "", tab: tab.id, pane: nil,
          label: tab.label, subtitle: "", givenName: tab.givenName, state: "", agent: nil,
          onScreen: tab.onScreen, hasKeyboard: false, reservedPresses: twoPress ? 1 : 0,
          machine: nil,
          // A caption carries the one press that reaches its tab, which is a first press or
          // none - so there is no such thing as a second press onto a caption.
          isSecondPress: false)
        caption.foldable = foldable
        caption.folded = folded
        if folded {
          caption.foldedPanes = tab.panes.map {
            FoldedPane(
              key: $0.key, name: $0.callsign.isEmpty ? $0.label : $0.callsign,
              state: (agents[$0.key] ?? "unknown").state)
          }
        }
        rows.append(caption)
      }
      // A folded tab draws no pane of its own: its caption says what they are doing.
      for pane in tab.panes where !(captions && folded) {
        // A pane the core has said nothing about is unknown, not idle. An agent we have not
        // heard from is not an agent that finished (`corpus/conformance/agent-state.json`).
        rows.append(
          paneRow(
            pane, in: tab.id, agent: agents[pane.key] ?? "unknown", now: now,
            presses: (pane.tabPress, pane.press), onScreen: pane.onScreen,
            hasKeyboard: pane.key == keyboard, reservedPresses: twoPress ? 2 : 0,
            machine: mark(pane.key.daemon), isSecondPress: tab.armed,
            member: members.contains(pane.key)))
        appendHelpers(of: pane)
      }
    }
    for machine in roster.machines where machine.worthDrawing {
      rows.append(
        Row(
          kind: .machine, daemon: machine.id, tab: "", pane: nil, label: machine.id,
          subtitle: machine.restarted, givenName: "", state: machine.state, agent: nil,
          onScreen: false,
          hasKeyboard: false, reservedPresses: 0, machine: mark(machine.id), isSecondPress: false))
    }
    let hiddenRows = roster.hiddenPanes.filter { $0.under.isEmpty }
    if !hiddenRows.isEmpty {
      let states = hiddenRows.map { (agents[$0.key] ?? "unknown").state }
      let urgent = hiddenCollapsed ? mostUrgent(states) : nil
      rows.append(
        Row(
          kind: .hidden(
            panes: states.count, urgent: states.filter { $0 == urgent }.count,
            collapsed: hiddenCollapsed),
          daemon: "", tab: "", pane: nil, label: "Hidden", subtitle: "",
          givenName: "", state: urgent ?? "", agent: nil, onScreen: false, hasKeyboard: false,
          reservedPresses: 0, machine: nil, isSecondPress: false))
    }
    for tab in roster.hidden where !hiddenCollapsed {
      for pane in tab.panes where pane.under.isEmpty {
        appendHidden(pane, tab: tab.id)
      }
    }
    // A conversation sits on its agent's row only when this window draws that row: a folded tab or
    // a collapsed Hidden caption draws none, and a group claimed by a row nobody sees would be
    // reachable from nowhere.
    let claimed = OneToOne.claims(
      groups: groups, among: Set(rows.filter(\.isPane).compactMap(\.pane)))
    for index in rows.indices {
      guard rows[index].isPane, let key = rows[index].pane, let group = claimed[key] else {
        continue
      }
      rows[index].conversation = Conversation(group)
    }
    return rows
  }

  /// The row for one pane, as the list draws it and as the pane's header reads it.
  ///
  /// One place, so the two cannot disagree about what a pane's line says. A pane the core has
  /// said nothing about is unknown, not idle: an agent we have not heard from is not an agent
  /// that finished (`corpus/conformance/agent-state.json`), which is why the callers default
  /// `agent` that way. What a header does not know - the machine mark, the keyboard, the
  /// conversation - keeps its default.
  public static func paneRow(
    _ pane: Roster.Pane, in tab: String, agent: PaneAgent, now: Date,
    presses: (tab: Int, pane: Int), onScreen: Bool, hasKeyboard: Bool = false,
    reservedPresses: Int = 0, machine: MachineMark? = nil, isSecondPress: Bool = false,
    hidden: Bool = false, depth: Int = 0, member: Bool = false
  ) -> Row {
    var row = Row(
      kind: .pane(tabPress: presses.tab, press: presses.pane), daemon: pane.key.daemon, tab: tab,
      pane: pane.key, label: pane.label, subtitle: subtitle(pane.subtitle, agent: agent),
      givenName: pane.givenName, state: agent.state, agent: agent, onScreen: onScreen,
      hasKeyboard: hasKeyboard, reservedPresses: reservedPresses, machine: machine,
      isSecondPress: isSecondPress, quiet: pane.quiet, hidden: hidden, callsign: pane.callsign,
      besideCallsign: pane.besideCallsign, depth: depth, member: member,
      directory: pane.directory, directoryName: directoryName(pane.directory),
      tier: Tier(state: agent.state), stateLine: stateLine(for: agent, now: now))
    row.title = pane.subtitle
    return row
  }

  /// Marks the rows from `first` to the end as one raised panel: the tab on screen's caption, its
  /// panes and the helper lines under them.
  private static func markPanel(_ rows: inout [Row], from first: Int) {
    guard first < rows.count else { return }
    let last = rows.count - 1
    for index in first...last {
      rows[index].panel =
        first == last ? .only : (index == first ? .top : (index == last ? .bottom : .middle))
    }
  }

  /// Whether a row's second line shows a time, which is whether a minute passing can change it.
  ///
  /// A helper's pill says how long its state has held when hovered, so an entry whose helper is
  /// timed counts, though the entry's own lines show no time.
  public static func showsTime(in row: Row) -> Bool {
    guard row.isPane, let agent = row.agent else { return false }
    return showsTime(of: agent) || row.helpers.contains(where: \.timed)
  }

  /// Whether a line about this agent shows how long its state has held.
  public static func showsTime(of agent: PaneAgent) -> Bool {
    ["blocked", "stuck", "done"].contains(agent.state) && agent.sinceMS > 0
  }

  /// What a pane row's second line says before the directory, for `agent` as it is at `now`.
  ///
  /// The state's word and how long it has held for the three states that want somebody, what a
  /// waiting agent waits on, the state in words for the two Muster cannot read, and nothing for
  /// an agent that is working or idle: there the row says only where the pane is.
  public static func stateLine(for agent: PaneAgent, now: Date) -> String {
    let held = heldFor(sinceMS: agent.sinceMS, now: now)
    let timed = { (word: String) in held.isEmpty ? word : "\(word) \(held)" }
    switch agent.state {
    case "blocked": return timed("Blocked")
    case "stuck": return timed("Stuck")
    case "done": return timed("Done")
    case "waiting": return agent.waiting.isEmpty ? "Waiting" : "Waiting on \(agent.waiting)"
    case "working", "idle": return ""
    case "unreachable":
      // What follows is what the agent said before its machine went, and has to read as that. No
      // time: `since_ms` is when the daemon stopped answering, not how long the agent has held.
      return agent.lastState.isEmpty ? "Unreachable" : "Unreachable · was \(agent.lastState)"
    default: return "Unknown"
    }
  }

  /// How long ago a state began, as a row says it: `<1m`, `7m`, `2h`, `3d`, or empty when the
  /// daemon did not say. A start in the future - a remote machine's clock running ahead - reads as
  /// less than a minute rather than as a negative time.
  public static func heldFor(sinceMS: Int64, now: Date) -> String {
    guard sinceMS > 0 else { return "" }
    let seconds = max(0, Int(now.timeIntervalSince1970 - Double(sinceMS) / 1000))
    switch seconds {
    case ..<60: return "<1m"
    case ..<3600: return "\(seconds / 60)m"
    case ..<86400: return "\(seconds / 3600)h"
    default: return "\(seconds / 86400)d"
    }
  }

  /// The last component of a directory, which is what a row has room for: `muster` for
  /// `/Users/a/src/muster`, `~` for the home directory itself, `/` for the root, and nothing for no
  /// directory.
  public static func directoryName(_ path: String, home: String = NSHomeDirectory()) -> String {
    guard !path.isEmpty else { return "" }
    let trimmed = trimmed(path)
    guard !trimmed.isEmpty else { return "/" }
    if !home.isEmpty, trimmed == self.trimmed(home) { return "~" }
    return trimmed.split(separator: "/").last.map(String.init) ?? "/"
  }

  /// A directory as hovering a row says it: the whole path, with `~` for the home directory.
  ///
  /// Home is this machine's, so a pane on another machine whose path happens to start with it is
  /// shortened too. That needs the same user name on a machine with the same home layout, and the
  /// alternative is the shell guessing which machine is the local one.
  public static func abbreviated(_ path: String, home: String = NSHomeDirectory()) -> String {
    let home = trimmed(home)
    guard !home.isEmpty else { return path }
    if trimmed(path) == home { return "~" }
    return path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
  }

  private static func trimmed(_ path: String) -> String {
    var path = path
    while path.hasSuffix("/") { path.removeLast() }
    return path
  }

  /// The state a folded caption's dot takes from the panes under it, or nil for none.
  ///
  /// The states a folded caption says anything about, most urgent first: the three that ask for
  /// somebody in the order the core ranks them (`Alert`, in `crates/muster-core/src/attention.rs`),
  /// then working. An idle or waiting agent asks nothing of somebody looking at a folded list, so
  /// it draws no dot there.
  public static func mostUrgent(_ states: [String]) -> String? {
    ["blocked", "stuck", "done", "working"].first { states.contains($0) }
  }

  /// What hovering a helper's pill says: its name and state, where it is, and who made it.
  public static func details(of helper: HelperPill) -> String {
    // A working or idle helper's row would say nothing of its state, so the pill says the word.
    var lines = ["\(helper.name): \(helper.stateLine.isEmpty ? helper.state : helper.stateLine)"]
    if !helper.directory.isEmpty { lines.append(abbreviated(helper.directory)) }
    lines.append("made by \(helper.madeBy)")
    return lines.joined(separator: "\n")
  }

  /// What VoiceOver reads for an entry's pills, and what hovering their count says: `3 helpers`,
  /// then how many are in each state that asks for somebody.
  public static func helpersSummary(_ helpers: [HelperPill]) -> String {
    let count = helpers.count == 1 ? "1 helper" : "\(helpers.count) helpers"
    let asking = ["blocked", "stuck", "done"].compactMap { state -> String? in
      let held = helpers.filter { $0.state == state }.count
      return held == 0 ? nil : "\(held) \(state)"
    }
    return ([count] + asking).joined(separator: " · ")
  }

  /// Which of a folded caption's panes get an icon, or of an entry's helpers a pill, when only
  /// `capacity` fit, and how many do not.
  ///
  /// All of them when they fit. Otherwise the most urgent ones, in the order the roster lists them,
  /// so that an agent that is blocked inside a folded tab never drops out for want of room; the
  /// count of the rest is drawn beside them.
  public static func foldedIcons(states: [String], capacity: Int) -> (shown: [Int], more: Int) {
    guard states.count > capacity else { return (Array(states.indices), 0) }
    let capacity = max(0, capacity)
    func rank(_ state: String) -> Int {
      switch Tier(state: state) {
      case .asks: 0
      case .look: 1
      case .calm: 2
      case .absent: 3
      }
    }
    let urgent = states.indices.sorted { (rank(states[$0]), $0) < (rank(states[$1]), $1) }
    return (Array(urgent.prefix(capacity)).sorted(), states.count - capacity)
  }

  /// What hovering a Hidden caption says, and what VoiceOver reads for it: `3 hidden panes ·
  /// 1 blocked` while folded, nil while its rows say it themselves. A folded tab's caption says
  /// each pane it is folded over and that pane's state.
  public static func summary(of row: Row) -> String? {
    if row.folded {
      // One line per pane, so a folded tab can be read by hovering it.
      return row.foldedPanes.map { "\($0.name): \($0.state)" }.joined(separator: "\n")
    }
    guard case .hidden(let panes, let urgent, let collapsed) = row.kind, collapsed else {
      return nil
    }
    let count = panes == 1 ? "1 hidden pane" : "\(panes) hidden panes"
    return row.state.isEmpty ? count : "\(count) · \(urgent) \(row.state)"
  }

  /// A pane row's second line: what a waiting agent waits on, which says more than its title
  /// while it waits, and otherwise the core's - unless the pane's machine cannot be reached,
  /// when the line says so and what the pane was doing before, rather than a title that reads
  /// like a live agent's.
  public static func subtitle(_ subtitle: String, agent: PaneAgent) -> String {
    if agent.state == "unreachable" {
      return agent.lastState.isEmpty ? "unreachable" : "unreachable · was \(agent.lastState)"
    }
    return agent.state == "waiting" && !agent.waiting.isEmpty
      ? "waiting on \(agent.waiting)" : subtitle
  }

  /// A small mark at a pane row's trailing edge. Each is there only when it says something, so
  /// a row with nothing to add stays the row it always was.
  public enum Accessory: Equatable {
    /// Muster cannot read the agent's screen, so its state is only its own word.
    case unreadable
    /// A bell nobody has heard.
    case bell
    /// Sub-agents running, as a count like Mail's unread one.
    case subagents(Int)
    /// How full the agent's context is, from 0 to 100, as a ring.
    case context(Float)
    /// Nothing about the pane is raised for the person.
    case quiet
    /// The pane's agent is in the group whose conversation the window shows.
    case member
    /// The conversation with this pane's agent, as an envelope that opens it, with what is unread
    /// and how much of that is to the person.
    case conversation(unread: Int, toYou: Int)
    /// How many panes a folded Hidden caption is over.
    case hiddenPanes(Int)
    /// Whether the Hidden caption is folded, as the chevron a click on it turns.
    case disclosure(collapsed: Bool)
  }

  /// The marks a row carries, left to right. The ones that ask somebody to look come first,
  /// nearest the name; the gauges, which are read rather than noticed, sit at the edge where a
  /// column of them lines up down the list.
  public static func accessories(of row: Row) -> [Accessory] {
    if case .hidden(let panes, _, let collapsed) = row.kind {
      return (collapsed ? [.hiddenPanes(panes)] : []) + [.disclosure(collapsed: collapsed)]
    }
    guard let agent = row.agent else { return [] }
    var marks: [Accessory] = []
    if let conversation = row.conversation {
      marks.append(.conversation(unread: conversation.unread, toYou: conversation.toYou))
    }
    if row.member { marks.append(.member) }
    if row.quiet { marks.append(.quiet) }
    // The rest are about the agent as it is now, which nobody can say for a pane whose machine
    // cannot be reached: a context ring on a rebooted devenv's agent reads as an agent alive.
    if agent.state == "unreachable" { return marks }
    if agent.unreadable { marks.append(.unreadable) }
    if agent.rang { marks.append(.bell) }
    if agent.subagents > 0 { marks.append(.subagents(agent.subagents)) }
    if let used = agent.contextUsed { marks.append(.context(min(max(used, 0), 100))) }
    return marks
  }

  /// How far a context ring has moved from its empty colour toward its full one, 0 to 1, for a
  /// context `used` percent full.
  ///
  /// Continuous rather than a step at a threshold, so a glance says how close an agent is to
  /// compacting and not only whether it has passed some line. Squared so that most of the move
  /// happens in the top half: a context half full is a quarter of the way along and still
  /// reads as calm, while 80% is past halfway and 90% is nearly the full colour, which is
  /// about where harnesses compact.
  public static func contextBlend(used: Float) -> CGFloat {
    let fraction = CGFloat(min(max(used, 0), 100) / 100)
    return fraction * fraction
  }

  /// The progress a row draws, if any. A percentage or a failure is worth a bar; indeterminate
  /// progress is some agents' way of saying they are working, which the dot already says.
  public static func progress(of row: Row) -> PaneAgent.Progress? {
    guard let progress = row.agent?.progress else { return nil }
    switch progress.state {
    case "error": return progress
    case "running", "paused": return progress.percent == nil ? nil : progress
    default: return nil
    }
  }

  /// What the envelope on a row says on hover and to VoiceOver.
  public static func conversationLine(_ conversation: Conversation) -> String {
    switch (conversation.unread, conversation.toYou) {
    case (0, _): return "your conversation with it"
    case (let unread, 0): return "\(unread) unread in your conversation with it"
    case (let unread, let toYou):
      return "\(unread) unread in your conversation with it, \(toYou) to you"
    }
  }

  /// What hovering a pane row says, and what VoiceOver reads for it: the label, what its agent
  /// is doing, and everything it said about itself, one line each. The row itself is a glance;
  /// this is the look.
  public static func details(of row: Row) -> String {
    var lines = [row.label]
    // A renamed pane's label is its given name, so the callsign is said on its own.
    if !row.callsign.isEmpty, !row.givenName.isEmpty {
      lines.append("callsign \(row.callsign)")
    }
    if !row.directory.isEmpty { lines.append(abbreviated(row.directory)) }
    if let machine = row.machine { lines.append("on \(machine.daemon)") }
    guard let agent = row.agent else { return lines.joined(separator: "\n") }
    var state = agent.state
    if agent.state == "unreachable" {
      // What follows is what the agent said before, and has to read as that.
      state = "its machine cannot be reached"
      if !agent.lastState.isEmpty { state += "; it was \(agent.lastState) when last heard from" }
    } else if agent.reported {
      state += ", as the agent reports"
    }
    lines.append(state)
    if let unread = agent.unreadInput {
      let since = Date(timeIntervalSince1970: TimeInterval(unread.sinceMS) / 1000)
      let at = since.formatted(date: .omitted, time: .shortened)
      let waiting = unread.bytes == 1 ? "1 byte is" : "\(unread.bytes) bytes are"
      lines.append(
        "\(unread.program) (pid \(unread.pid)) has not read its input since \(at) - \(waiting) waiting"
      )
    }
    if agent.unreadable { lines.append("Muster cannot read this agent's screen") }
    if agent.adapter == "silent" {
      lines.append("its adapter is not reporting (muster docs harnesses)")
    }
    if !agent.waiting.isEmpty { lines.append("waiting on \(agent.waiting)") }
    // A title is a live agent's word, so a pane whose machine cannot be reached leaves it out.
    if !row.title.isEmpty && agent.state != "unreachable" { lines.append(row.title) }
    if !row.helpers.isEmpty { lines.append(helpersSummary(row.helpers)) }
    if let used = agent.contextUsed { lines.append("\(Int(used.rounded()))% of its context used") }
    switch agent.subagents {
    case 0: break
    case 1: lines.append("1 sub-agent running")
    default: lines.append("\(agent.subagents) sub-agents running")
    }
    if !agent.model.isEmpty {
      lines.append(agent.effort.isEmpty ? agent.model : "\(agent.model) · \(agent.effort)")
    }
    if let cost = agent.costUSD { lines.append(String(format: "$%.2f so far", cost)) }
    if agent.rang { lines.append("the bell rang") }
    if row.member { lines.append("in the conversation shown") }
    if let conversation = row.conversation {
      lines.append(conversationLine(conversation))
    }
    return lines.joined(separator: "\n")
  }

  /// Whether dragging one pane onto a row is a gesture Muster can carry out.
  ///
  /// Here rather than in the view so that the rule is testable: a decision inside
  /// `validateDrop` is a decision no test can reach.
  ///
  /// **A drop on a pane row or a tab caption may cross machines.** The pane stays a process on
  /// its own machine; what changes is where it sits, beside the row's pane or in the caption's
  /// tab, and a tab is one tree over every machine it spans (MIP-8, section 6).
  ///
  /// A machine's row is not a place a pane can go: it names no tab, and dropping onto a machine
  /// that already holds the pane would mean nothing at all.
  ///
  /// Dropping a row on itself is legal and does nothing, which is what an accidental drag is.
  ///
  /// A hidden pane's row is not one either: landing beside it would hide the pane dropped, which
  /// nobody dragging a row means.
  public static func canArrange(_ pane: PaneKey, onto row: Row) -> Bool {
    if row.isTab {
      return true
    }
    return row.pane != nil && !row.hidden
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

  /// The tab a dragged row takes with it into another window, or nil for none.
  ///
  /// A caption takes its own tab. So does every pane row of a list with no caption at all,
  /// which is a window holding one tab: there the list *is* the tab, and with no caption to grab
  /// it by, the only tab of a window could not be dragged anywhere (kan a_2WEHsAdQb). Drawing a
  /// caption whenever a second window is open was the other way to give it a handle, and it
  /// would have been a boundary that comes and goes with windows opened somewhere else - the
  /// thing `rows` keeps captions from being.
  ///
  /// Under a caption a pane row takes no tab, because there the caption is how to move one.
  public static func draggedTab(of row: Row, in rows: [Row]) -> String? {
    if row.isTab { return row.tab }
    // A hidden pane's tab is one no window holds, so there is nothing to take into another.
    guard row.isPane, !row.hidden, !rows.contains(where: \.isTab) else { return nil }
    return row.tab
  }

  /// What a drop onto this window's list is asking for.
  public enum Drop: Equatable {
    /// A tab from another window, to move here.
    case tab(String)
    /// A pane from this window, to arrange onto the row it was dropped on.
    case pane(PaneKey)
  }

  /// Which of the things a drag carries a drop means, or nil for nothing.
  ///
  /// A pane row of a window holding one tab carries both its pane and its tab, so where the
  /// drag came from decides: within its own list it is a pane being arranged, and in another
  /// window's list it is that window's tab arriving.
  public static func drop(tab: String?, pane: PaneKey?, fromThisWindow: Bool) -> Drop? {
    if let pane, acceptsPane(fromThisWindow: fromThisWindow) { return .pane(pane) }
    if let tab, acceptsTab(fromThisWindow: fromThisWindow) { return .tab(tab) }
    return nil
  }

  /// The dot beside a row, and whether to draw one at all.
  ///
  /// The same colors the pane borders use, because they are the same states and a
  /// sidebar that disagreed with the window beside it would be worse than no sidebar. Unlike
  /// a border, the dot is drawn for every state including idle: a border exists to be
  /// noticed against a resting default, where a list with gaps in a column reads as missing
  /// data rather than as calm.
  @MainActor
  public static func dotColor(state: String) -> NSColor {
    PaneAppearance.borderColor(state: state)
  }

  /// The context ring's colour with nothing used: the grey its row's other quiet marks use.
  public static let contextEmptyColor = NSColor.secondaryLabelColor

  /// The context ring's colour with all of it used. Red because no agent state is red, so a
  /// nearly full ring cannot be read as an agent asking for something - which the orange it
  /// replaced could, being `blocked`'s.
  public static let contextFullColor = NSColor.systemRed

  /// What a context ring `used` percent full is painted in, between `[colors] context_empty`
  /// and `context_full`, or the defaults above for whichever end the file left out.
  ///
  /// Only meaningful under a drawing appearance, because both default ends are dynamic: each is
  /// resolved to sRGB for the appearance current at the call and then mixed, alpha included,
  /// since the grey is partly transparent and the red is not.
  @MainActor
  public static func contextColor(used: Float) -> NSColor {
    let chrome = PaneAppearance.configured
    let empty = chrome.contextEmpty.flatMap(NSColor.init(hex:)) ?? contextEmptyColor
    let full = chrome.contextFull.flatMap(NSColor.init(hex:)) ?? contextFullColor
    guard let from = empty.usingColorSpace(.sRGB), let to = full.usingColorSpace(.sRGB) else {
      return empty
    }
    let blend = contextBlend(used: used)
    let mix = { (a: CGFloat, b: CGFloat) in a + (b - a) * blend }
    return NSColor(
      srgbRed: mix(from.redComponent, to.redComponent),
      green: mix(from.greenComponent, to.greenComponent),
      blue: mix(from.blueComponent, to.blueComponent),
      alpha: mix(from.alphaComponent, to.alphaComponent))
  }

  /// How tall a row is.
  ///
  /// **A few heights, and deliberately not a function of how long the text is.** A list of fifteen
  /// agents is read by scanning it, so a height that varied with what an agent happened to be
  /// writing would move every row below it each time one of them wrote a longer sentence. What
  /// moves a row is a whole line arriving or going away.
  ///
  /// A pane's entry is its callsign's line and the line saying its state and directory, with a
  /// task line between them when it has a name or a title, and a line of pills under them when it
  /// has helpers (Fencil `agent-list-rows`). A helper listed under an open
  /// entry is one line, and so is a caption; a machine's heading saying what its restart cost is
  /// two.
  public static let oneLine: CGFloat = 20
  public static let twoLines: CGFloat = 40
  public static let helperLine: CGFloat = 22
  public static let entry: CGFloat = 42
  public static let taskLine: CGFloat = 16
  public static let pillsLine: CGFloat = 15
  public static func height(of row: Row) -> CGFloat {
    guard row.isPane else { return row.subtitle.isEmpty ? oneLine : twoLines }
    if row.isHelper { return helperLine }
    return entry + (hasTaskLine(row) ? taskLine : 0) + (row.helpers.isEmpty ? 0 : pillsLine)
  }

  /// Whether a pane's entry has a task line. Its marks do not earn it one: they come and go with
  /// what the agent is doing, a bell or a sub-agent, and a row must not move for that, so an entry
  /// with no name and no title draws them at the end of its state line.
  public static func hasTaskLine(_ row: Row) -> Bool {
    row.isPane && !row.isHelper && !row.taskLine.isEmpty
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

  /// How wide the list is until the core has said otherwise. The core holds the real answer,
  /// which is whatever the list was last dragged to.
  public static let width: CGFloat = 200

  /// The window width below which the list is put away altogether, whatever it was dragged to.
  public static let minimumWindow: CGFloat = 400

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
  ///
  /// `wanted` is the width the list was dragged to. It gets at most half the window, so a list
  /// dragged wide in a big window narrows as the window does instead of taking the panes' room,
  /// and the window gives the list up below `minimumWindow`, whatever it was dragged to.
  ///
  /// The list's width is a whole number of points because the panes start where it ends. A
  /// terminal surface at a fractional x is resampled by the compositor, and every glyph in it
  /// comes out blurred.
  public static func widths(in total: CGFloat, shown: Bool = true, wanted: CGFloat = width) -> (
    sidebar: CGFloat, regions: CGFloat
  ) {
    guard shown, total >= minimumWindow else { return (0, max(0, total)) }
    let sidebar = min(wanted, total / 2).rounded(.down)
    return (sidebar, total - sidebar)
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

  /// Called when somebody clicks a tab caption's fold control, with the tab and whether they are
  /// asking to fold it. The core keeps the answer and sends it back on the roster.
  public var onTabFolded: ((String, Bool) -> Void)?

  /// Called when somebody picks a machine's row, meaning they want a pane on it.
  ///
  /// The only way into a machine holding nothing, and what makes it safe for Muster to stop
  /// giving every machine a column of its own: a machine you have finished with stays empty,
  /// and one row gets you back (kan a_2HpkpfIfq, a_2I6h18OU6).
  public var onMachinePicked: ((String) -> Void)?
  /// A group picked from an agent's envelope or the Needs you strip, by its daemon and name:
  /// shows its conversation.
  public var onGroupPicked: ((String, String) -> Void)?

  /// The human's groups, drawn on their agents' rows on the next
  /// `apply(roster:agents:keyboard:)`.
  public var groups: [SidebarModel.MessageGroup] = []

  /// The panes of the members of the group whose conversation the window shows, empty while it
  /// shows its panes. Marked on the next `apply(roster:agents:keyboard:)`, like `groups`.
  public var conversationMembers: Set<PaneKey> = []

  /// Whether the Hidden caption is folded, as the core's presentation last said. Drawn on the
  /// next `apply(roster:agents:keyboard:)`, like `groups`.
  public var hiddenCollapsed = false
  /// Whether the Needs you strip is folded to one line, as the core's presentation last said.
  public var needsYouFolded = false {
    didSet { if needsYouFolded != oldValue { redrawStrip() } }
  }

  /// Who is asking for the person, in the order the core ranks them. Drawn by the strip.
  public var asking: [AskingEntry] = [] {
    didSet { if asking != oldValue { redrawStrip() } }
  }

  /// Called when somebody clicks the strip's header, with whether they are asking to fold it.
  public var onNeedsYouToggled: ((Bool) -> Void)?

  /// Called when somebody clicks the Hidden caption, with whether they are asking to fold it.
  public var onHiddenToggled: ((Bool) -> Void)?

  /// The panes whose helpers lines are open, drawn on the next `apply(roster:agents:keyboard:)`.
  /// The window's own, unlike the Hidden caption's fold: nothing outside this window reads it,
  /// and a relaunch folding them all again is the default anyway.
  public var helpersOpen: Set<PaneKey> = []

  /// Called when somebody clicks the pills on a pane's entry, with the pane and whether they are
  /// asking to open the list of its helpers.
  public var onHelpersToggled: ((PaneKey, Bool) -> Void)?

  /// Builds the menu for a right-click on an entry's pills, which is about its helpers together.
  public var onHelpersMenu: ((SidebarModel.Row) -> NSMenu?)?

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

  /// Called when somebody clicks a problem's remedy, meaning they want it carried out.
  public var onProblemRemedied: ((Problem) -> Void)?

  /// Builds the menu for a right-clicked row, or answers nil for none. The window's to answer,
  /// because what the menu offers depends on the bindings and the machines.
  public var onRowMenu: ((SidebarModel.Row) -> NSMenu?)?

  /// A right-clicked envelope's menu. A group has no row of its own
  /// to take Leave Group and Delete Group from, so the envelope carries them.
  public var onConversationMenu: ((SidebarModel.Conversation) -> NSMenu?)?

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
  static let draggedPane = PaneDrop.type

  /// A tab row, which is dragged between windows rather than within one: dropped into another
  /// window's list, it moves the tab there.
  static let draggedTab = NSPasteboard.PasteboardType("dev.muster.tab")

  private let table = SidebarTable()
  private let strip = NeedsYouStripView()
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
    // A table left to choose its own style insets every row 16 points from each edge, which at
    // 200 points is a sixth of the list. A row keeps its own margins instead.
    table.style = .fullWidth
    // No gap between rows: the tab on screen's raised panel is drawn a row at a time and has to
    // read as one rectangle.
    table.intercellSpacing = .zero
    table.selectionHighlightStyle = .regular
    table.backgroundColor = .clear
    table.dataSource = self
    table.delegate = self
    table.target = self
    table.action = #selector(rowClicked)
    table.doubleAction = #selector(rowDoubleClicked)
    table.registerForDraggedTypes([SidebarView.draggedPane, SidebarView.draggedTab])
    table.rowMenu = { [weak self] row in
      guard let self, self.rows.indices.contains(row) else { return nil }
      return self.onRowMenu?(self.rows[row])
    }
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
    strip.onPanePicked = { [weak self] in self?.onPanePicked?($0) }
    strip.onGroupPicked = { [weak self] in self?.onGroupPicked?($0, $1) }
    strip.onFoldToggled = { [weak self] in self?.onNeedsYouToggled?($0) }
    addSubview(strip)

    problemsView.onDismiss = { [weak self] in
      guard let self, case .raised(let showing) = self.problems else { return }
      self.dismissed.formUnion(showing.map(\.key))
      self.redrawProblems()
    }
    problemsView.onRemedy = { [weak self] problem in
      self?.onProblemRemedied?(problem)
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
    // The list from the top, and the strip under it, over the problems. Under rather than over,
    // so that a card arriving or leaving does not move a single row of the list (Fencil
    // `needs-you-strip`).
    let below = min(
      strip.height(forWidth: bounds.width, limit: (bounds.height * Self.stripShare).rounded(.down)),
      max(0, bounds.height - height))
    strip.frame = CGRect(x: 0, y: height, width: bounds.width, height: below)
    scroll.frame = CGRect(
      x: 0, y: height + below, width: bounds.width,
      height: max(0, bounds.height - height - below))
  }

  /// Where the list of agents is, beside the strip and the problems.
  var listFrame: CGRect { scroll.frame }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Redraws every row against the colours in force, after the config file was read again.
  ///
  /// Whole rather than diffed, unlike everything else here: the rows have not changed, only
  /// what they are painted in, so the diff that keeps a fifteen-row list from flickering on
  /// every agent transition would correctly decide there was nothing to do.
  public func reloadColors() {
    table.reloadData()
    strip.recolor()
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
  /// A changed row is shown again in the view it already has, rather than reloaded. Reloading
  /// even one row builds its view from nothing, and that construction was most of what a focus
  /// change cost the main thread (kan a_2WEaf4xGg). A row with no view loaded - scrolled out of
  /// sight - needs nothing, because the table asks for it from `rows` when it comes back.
  ///
  /// A whole reload is still right when the shape of the list moves - a pane opened, a tab
  /// closed, rows reordered by a drag - because then the rows are not the same rows and
  /// comparing them position by position would be comparing different things. That case is
  /// rare; a state blinking is not.
  ///
  /// **Showing a row again does not re-measure it.** It draws inside the frame the row already
  /// had, and `noteHeightOfRows` is the only thing that makes a table ask again. Skipping it is
  /// how a pane whose agent titles itself came to draw two lines in a one-line frame until
  /// something unrelated reloaded the lot.
  ///
  /// Instantly rather than animated: a note animates by default, and a row growing under
  /// somebody reading the list is the movement the two heights exist to avoid.
  public func apply(roster: Roster, agents: [PaneKey: PaneAgent], keyboard: PaneKey? = nil) {
    inputs = (roster, agents, keyboard)
    redraw()
  }

  /// What the list was last given, kept so that a minute passing can draw it again.
  private var inputs: (roster: Roster, agents: [PaneKey: PaneAgent], keyboard: PaneKey?)?

  /// The time rows measure how long a state has held against. Replaceable so that a test can let
  /// a minute pass without waiting for one.
  var clock: () -> Date = { Date() }

  /// How many rows have been drawn since this list was made, counting a whole reload as every
  /// row. What the cost of a change is measured in: a state change should be one, an unchanged
  /// roster none.
  private(set) var redrawCount = 0

  /// How many of the strip's cards have been drawn, counted the way `redrawCount` counts rows.
  var stripRedrawCount: Int { strip.redrawCount }

  /// The strip, so a test can read what it draws.
  var needsYou: NeedsYouStripView { strip }

  /// Whether some row or card shows how long a state has held, which is whether a minute passing
  /// can change what the list draws.
  var showsTime: Bool {
    rows.contains(where: SidebarModel.showsTime(in:)) || NeedsYouModel.showsTime(strip.cards)
  }

  /// Called after the rows were drawn, for whoever keeps the minute counted.
  var onShowsTimeChanged: (() -> Void)?

  private func redraw() {
    guard let inputs else { return }
    let fresh = SidebarModel.rows(
      roster: inputs.roster, agents: inputs.agents, keyboard: inputs.keyboard, groups: groups,
      hiddenCollapsed: hiddenCollapsed, helpersOpen: helpersOpen, members: conversationMembers,
      now: clock())
    let previous = rows
    rows = fresh
    defer { onShowsTimeChanged?() }
    redrawStrip(notifying: false)
    guard let changed = SidebarModel.changes(from: previous, to: fresh) else {
      redrawCount += fresh.count
      table.reloadData()
      return
    }
    redrawCount += changed.redraw.count
    for index in changed.redraw {
      rowView(at: index)?.show(fresh[index])
    }
    guard !changed.remeasure.isEmpty else { return }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0
      table.noteHeightOfRows(withIndexesChanged: changed.remeasure)
    }
  }

  /// Draws the strip from the asking list and what the list was last given, and gives it the room.
  private func redrawStrip(notifying: Bool = true) {
    guard let inputs else { return }
    let cards = NeedsYouModel.cards(
      entries: asking, roster: inputs.roster, agents: inputs.agents, groups: groups, now: clock())
    strip.show(cards: cards, folded: needsYouFolded)
    needsLayout = true
    if notifying { onShowsTimeChanged?() }
  }

  /// The most of the sidebar the strip may take, so that a long list of askers leaves the agents
  /// listed under it somewhere to be seen.
  static let stripShare: CGFloat = 0.45

  /// The view drawing a row, or nil when the table has none loaded for it.
  func rowView(at row: Int) -> SidebarRowView? {
    table.view(atColumn: 0, row: row, makeIfNecessary: false) as? SidebarRowView
  }

  /// Draws the list again against the time now, which changes only the rows showing how long a
  /// state has held. Called by the window's minute ticker, and by tests that have moved `clock`.
  func tick() {
    redraw()
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
    case .hidden(_, _, let collapsed):
      onHiddenToggled?(!collapsed)
    }
  }

  /// A double-click asks to rename. Only a pane or a tab is Muster's to rename: a machine's
  /// name is the config file's, and a group's is everyone's in it.
  @objc private func rowDoubleClicked() {
    let clicked = table.clickedRow
    guard rows.indices.contains(clicked), rows[clicked].isPane || rows[clicked].isTab else {
      return
    }
    onRowRenamed?(rows[clicked])
  }
}

/// The list's table, which asks for a row's menu when somebody right-clicks it.
///
/// Asked per click rather than set once as the table's `menu`, because each row is about a
/// different pane or tab and a roster arriving can change which row is under the pointer.
final class SidebarTable: NSTableView {
  var rowMenu: ((Int) -> NSMenu?)?

  override func menu(for event: NSEvent) -> NSMenu? {
    rowMenu?(row(at: convert(event.locationInWindow, from: nil)))
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
    let view =
      tableView.makeView(withIdentifier: SidebarRowView.identifier, owner: self)
      as? SidebarRowView ?? SidebarRowView()
    view.show(rows[row])
    view.onFoldToggled = { [weak self] row in self?.onTabFolded?(row.tab, !row.folded) }
    view.onGroupPicked = { [weak self] daemon, group in self?.onGroupPicked?(daemon, group) }
    view.onHelpersToggled = { [weak self] row in
      guard let maker = row.pane else { return }
      self?.onHelpersToggled?(maker, !row.helpersOpen)
    }
    view.onHelpersMenu = { [weak self] row in self?.onHelpersMenu?(row) }
    view.onConversationMenu = { [weak self] conversation in
      self?.onConversationMenu?(conversation)
    }
    return view
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
  ///
  /// A row can carry a tab too (`SidebarModel.draggedTab`), and a pane row in a window holding
  /// one tab carries both.
  public func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int)
    -> NSPasteboardWriting?
  {
    guard rows.indices.contains(row) else { return nil }
    let item = NSPasteboardItem()
    if let tab = SidebarModel.draggedTab(of: rows[row], in: rows) {
      item.setString(tab, forType: SidebarView.draggedTab)
    }
    if let pane = rows[row].pane {
      item.setString(PaneDrop.payload(pane), forType: SidebarView.draggedPane)
    }
    return item.types.isEmpty ? nil : item
  }

  public func tableView(
    _ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
    proposedDropOperation operation: NSTableView.DropOperation
  ) -> NSDragOperation {
    switch drop(info) {
    case .tab:
      // Onto the list as a whole: a tab joins the end of this window's list, whichever row the
      // pointer happens to be over.
      tableView.setDropRow(-1, dropOperation: .on)
      return .move
    case .pane(let pane):
      // On a row rather than between two. The card's rule is that a drag exchanges two panes,
      // and an arrangement has no "between" to insert into - so the row you drop on is the
      // place you are asking for, and retargeting an above-row drop keeps the highlight honest.
      guard rows.indices.contains(row) else { return [] }
      if operation == .above {
        tableView.setDropRow(row, dropOperation: .on)
      }
      return SidebarModel.canArrange(pane, onto: rows[row]) ? .move : []
    case nil:
      return []
    }
  }

  public func tableView(
    _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
    dropOperation operation: NSTableView.DropOperation
  ) -> Bool {
    switch drop(info) {
    case .tab(let tab):
      onTabReceived?(tab)
      return true
    case .pane(let pane):
      guard rows.indices.contains(row), SidebarModel.canArrange(pane, onto: rows[row]) else {
        return false
      }
      if rows[row].isTab {
        onPaneGrouped?(pane, rows[row].tab)
        return true
      }
      guard let onto = rows[row].pane else { return false }
      onPaneArranged?(pane, onto)
      return true
    case nil:
      return false
    }
  }

  /// What a drag means here, from what it carries and where it came from: this window, or another
  /// (`cameFromThisWindow`).
  private func drop(_ info: NSDraggingInfo) -> SidebarModel.Drop? {
    SidebarModel.drop(
      tab: draggedTab(info), pane: dragged(info), fromThisWindow: cameFromThisWindow(info))
  }

  /// The tab a drag is carrying, or nil when it is carrying something else.
  private func draggedTab(_ info: NSDraggingInfo) -> String? {
    info.draggingPasteboard.string(forType: SidebarView.draggedTab).flatMap {
      $0.isEmpty ? nil : $0
    }
  }

  /// The pane a drag is carrying, or nil when it is carrying something else.
  private func dragged(_ info: NSDraggingInfo) -> PaneKey? {
    info.draggingPasteboard.string(forType: SidebarView.draggedPane)
      .flatMap(PaneDrop.pane(fromPayload:))
  }
}
