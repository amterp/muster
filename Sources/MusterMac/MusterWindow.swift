import AppKit
import MusterRenderer

/// The window, and everything hanging under it.
///
/// The shell's whole contribution to what is on screen. It holds no truth: every region,
/// every pane and which one has the keyboard arrive from the core as a whole view, and this
/// applies it. What it does own is what only an OS layer can - a real window, real surfaces,
/// the responder chain, and a title.
///
/// A class rather than top-level code in the executable, because everything here has been
/// wrong at least once and an executable's entry point is not reachable from a test
/// (`docs/testing.md`, thin shell).
@MainActor
public final class MusterWindow: NSObject {
  public let window: NSWindow
  private let renderer: Renderer
  private let executable: String
  private let strip = RegionStrip(frame: NSRect(x: 0, y: 0, width: 960, height: 600))
  private let sidebar = SidebarView(frame: .zero)
  private let shortcuts = ShortcutsPanel()
  private let split = WindowLayout(frame: NSRect(x: 0, y: 0, width: 960, height: 600))
  private let empty = EmptyWindowView(frame: .zero)
  private var regions: [String: RegionView] = [:]

  /// Every pane's surface, held for as long as its daemon holds the pane rather than for as
  /// long as a region is showing it.
  ///
  /// Implicitly unwrapped because it cannot be built any earlier: it parks panes inside the
  /// content view and starts them through this window, so it needs both a view hierarchy and
  /// a `self` to capture - and neither exists until `super.init` has run. Non-nil from the end
  /// of `init` onwards, and every region is made through `make(regionID:)` after that.
  private var surfaces: PaneSurfaces!

  /// Whether this window was opened with no daemon behind it on purpose.
  ///
  /// The one empty window that is not worth explaining a way out of, because there is none:
  /// `--renderer-check` proves the renderer paints and nothing here can make it a pane.
  private var rendererCheck = false

  /// Everything the attached daemons hold, whether or not this window is showing it.
  ///
  /// Held here rather than only in the sidebar because it is half of what the sidebar draws
  /// and the other half arrives separately: a roster and an agent state land in either
  /// order, and whichever is second has to be able to redraw with both.
  private var roster = Roster(tabs: [])

  /// The modifiers a numbered chord is held with, so that letting go of them can end one.
  ///
  /// Cached from the bindings rather than asked for per event: this is read on every modifier
  /// the keyboard reports, which is several a second while somebody types, and the answer only
  /// moves when the config file does.
  private var chordModifiers: NSEvent.ModifierFlags = []

  /// Every pane's last known agent state, whether or not it is on screen.
  ///
  /// Held here rather than only in the chrome because the two arrive in either order: a pane
  /// created by a split is described by the daemon before its surface exists here, and a
  /// chrome built afterwards would otherwise render as `unknown` until that agent next moved.
  ///
  /// Keyed by daemon as well as pane, because this map spans regions and two daemons hand out
  /// the same pane ids - one keyed by pane alone would let a devenv agent paint a border on
  /// the laptop.
  private var agents: [PaneKey: PaneAgent] = [:]

  /// What each daemon last said about itself.
  ///
  /// Per daemon, because health is per connection: a devenv behind a dropped VPN says nothing
  /// about the laptop beside it, and one window-wide state would report the loss of both.
  private var health: [String: DaemonHealth] = [:]
  /// The chords this window was last given, which the empty window's hint names.
  private var bindings: [Core.Binding] = []

  private struct DaemonHealth {
    let state: String
    let detail: String
  }

  private var keyboardPane: String?

  /// The pane the keyboard feeds, with the daemon that hands out its id.
  ///
  /// Held beside `keyboardPane` because the list spans daemons and a bare pane id does not:
  /// two daemons hand out the same ones, so a sidebar keyed on the id alone would light up a
  /// devenv row for a laptop pane.
  private var keyboardKey: PaneKey?

  /// Which region the keyboard is in, kept so that a chord can reach the surface it happened
  /// on. Held beside `keyboardKey` rather than searched for, because the pane id in that key
  /// is only unique within its own daemon.
  private var focusedRegionID: String?
  private var zoomed = false
  private var problem: String?

  /// Everything the core says is wrong with this window. Held so the title can count them when
  /// the roster - which is where they are actually readable - is not on screen.
  private var outstanding: [Problem] = []

  /// The surface the keyboard is on, or nil when nothing is rendering yet.
  ///
  /// Only a live surface can say how big a cell is, and only the pane a chord happened on has
  /// the right answer: the font size is one window-wide setting, but a pane whose surface has
  /// not been sized yet has no measurement at all.
  private var keyboardSurface: SurfaceView? {
    guard let region = focusedRegionID, let pane = keyboardKey?.pane else { return nil }
    return regions[region]?.chrome(for: pane)?.surface
  }

  /// How big the region holding the keyboard is, or nil before it has been laid out.
  ///
  /// The region rather than the pane, because a resize moves a divider by a share of what that
  /// divider splits, and with one split on an axis the thing being split is the whole region.
  /// Measuring the focused pane instead would report about half the extent and so ask for
  /// about twice the distance.
  private var keyboardRegionSize: (width: Float, height: Float)? {
    guard let region = focusedRegionID, let view = regions[region] else { return nil }
    let size = view.bounds.size
    guard size.width > 0, size.height > 0 else { return nil }
    return (width: Float(size.width), height: Float(size.height))
  }

  /// The find bar, once somebody has asked for one. Built on demand and kept afterwards, so
  /// that closing it and opening it again returns to the needle it was last used with.
  private var findBar: FindBar?

  /// Where the window is when it is not full-screen, which is the only rectangle worth writing
  /// down. macOS reports a full-screen window's frame as the whole display, so a window that
  /// went full-screen and quit would otherwise come back the size of somebody's monitor with no
  /// way to get its old size back.
  private var settledFrame: NSRect?

  /// Whether the window is on its way into, or already in, the platform's full-screen.
  ///
  /// Set in `windowWillEnterFullScreen` rather than read from the style mask, because the
  /// resize that grows the window to the whole display arrives before the mask says so - and
  /// that resize is exactly the one whose rectangle must not be kept.
  private var fullScreen = false

  /// Reports where the window has settled, one request at a time.
  private lazy var frames = WindowFrameSender()

  /// Asks for the agent list's width while its edge is dragged, one request at a time: a drag
  /// reports a width per frame of motion, and each one ends in a file write.
  private lazy var sidebarWidths = LatestRequestSender<Void>(
    what: "set_sidebar_width", queue: "muster.sidebar-width", dispatcher: Core.dispatcher,
    read: { response in readResponse(response).map { _ in () } })

  /// Pastes a daemon held, asked about on this window one at a time.
  ///
  /// This window rather than whichever shows the pane, because it is the window the paste was
  /// made in: a paste goes from a window to its own core, and the daemon answers the core that
  /// sent it. The pane may be on another tab by now, and a yes still lands in it, since the
  /// confirmed paste names it.
  private lazy var heldPastes = HeldPastes { [weak self] held, answer in
    guard let self else { return answer(false) }
    ConfirmSheet.ask(
      on: self.window,
      question: "Paste text that may run commands?",
      body: "It has a line break, and the program in this pane did not ask for pasted text to "
        + "be marked as a paste, so each line may run as a command as it arrives.",
      confirm: "Paste", preview: held.text,
      then: { answer(true) }, otherwise: { answer(false) })
  }

  public init(renderer: Renderer, executable: String) {
    self.renderer = renderer
    self.executable = executable
    let keyboard = KeyboardWindow(
      contentRect: strip.frame,
      styleMask: [.titled, .closable, .resizable, .miniaturizable],
      backing: .buffered,
      defer: false)
    window = keyboard
    super.init()
    keyboard.onModifiersChanged = { [weak self] held in self?.apply(held: held) }
    // Not through `focusBack` and `focusForward`, which beep at either end of the history: a
    // mouse button there does nothing, as a browser's does.
    keyboard.onHistoryButton = { forward in
      Core.debug(
        BoundAction.event, BoundAction.record(mouse: forward ? "focus_forward" : "focus_back"))
      Core.focusHistory(forward: forward)
    }
    split.attach(sidebar: sidebar, strip: strip)
    // A window narrowed until the roster will not fit takes the problems area with it, so the
    // title has to pick them up at exactly that moment.
    split.onSidebarVisibilityChanged = { [weak self] in self?.applyTitle() }
    split.onSidebarDragged = { [weak self] width in
      self?.sidebarWidths.send(Core.setSidebarWidth(width))
    }
    strip.attach(empty: empty)
    let bindings = Core.bindings()
    self.bindings = bindings
    applyEmptyMessage()
    chordModifiers = NumberedChord.modifiers(bindings)
    window.contentView = split
    window.delegate = self
    // After the content view, because a parked pane waits inside the window rather than
    // outside every hierarchy - a surface is handed to libghostty as a view, and keeping that
    // view in a window for its whole life is the only state this has ever run in.
    surfaces = PaneSurfaces(parkedIn: split) { [weak self] region, chrome, pane in
      self?.start(chrome, in: region, pane: pane)
    }
    surfaces.window(key: window.isKeyWindow)
    // Named rather than left to the default, because `show` toggles into full-screen for a
    // window that quit from it and a default is a thing that can move.
    window.collectionBehavior.insert(.fullScreenPrimary)
    // Where a window with nothing written down about it opens. `show` puts back a saved
    // rectangle over the top of this.
    window.center()
    sidebar.onPanePicked = { pane in
      Core.focus(daemonID: pane.daemon, paneID: pane.pane)
    }
    sidebar.onTabPicked = { tab in
      Core.focus(tab: tab)
    }
    // A machine's row is the way into one holding nothing: it asks for a tab there, which on a
    // machine with none is a workspace with a pane in it. The same request `muster tab new
    // --daemon <id>` sends.
    sidebar.onMachinePicked = { daemon in
      Core.createTab(daemonID: daemon)
    }
    sidebar.onPaneArranged = { pane, onto in
      Core.arrange(pane: pane, onto: onto)
    }
    sidebar.onPaneGrouped = { pane, tab in
      Core.arrange(pane: pane, intoTab: tab)
    }
    // The same request `muster tab move` and the menu send, naming this window by naming none.
    sidebar.onTabReceived = { tab in
      Core.info("tab.dropped", ["tab": tab])
      Core.moveTab(tab, to: "")
    }
    // Both halves name their subject outright rather than leaving it to whatever has the
    // keyboard: the row somebody double-clicked is very often a pane no region is showing,
    // which is what the list is for.
    sidebar.onRowRenamed = { [weak self] row in
      guard let self else { return }
      if let pane = row.pane {
        rename(pane: pane)
      } else if row.isTab {
        rename(tab: row.tab)
      }
    }
    sidebar.onRowMenu = { [weak self] row in self?.menu(for: row) }
    surfaces.menu = { [weak self] pane in self?.menu(forPane: pane) }
    applyTitle()
  }

  /// Opens the window where it was left.
  ///
  /// The frame is applied before the window is on screen rather than after, because the core's
  /// events arrive a run-loop turn later and a window that jumps from the middle of the display
  /// to its real size is a jump somebody sees on every launch. That is the same split
  /// `Core.appearance()` makes against `AppearanceChanged`.
  ///
  /// The screens go with the question because only this layer can ask the platform for them.
  /// Where the window lands is the core's answer: a rectangle saved on a display that is gone
  /// comes back fitted to one that is here.
  public func show() {
    let screens = NSScreen.screens.map(\.visibleFrame)
    let restoring = Core.windowFrame(screens: screens)
    if let rect = restoring.rect {
      window.setFrame(rect, display: false)
      settledFrame = rect
    }
    // A window opening somewhere unexpected is a thing people report and cannot otherwise
    // evidence: what was saved, what the screens were, and what came back are three answers a
    // screenshot cannot give. The full-screen half is here because nothing else records it -
    // a space that fails to open leaves a window at its ordinary size and says nothing.
    Core.info(
      "window.frame.restored",
      [
        "frame": restoring.rect.map(described) ?? "(none)",
        "full_screen": String(restoring.fullScreen),
        "screens": screens.map(described).joined(separator: " "),
      ])
    window.makeKeyAndOrderFront(nil)
    // Where it actually opened, rather than where it wished to. A first launch has no saved
    // rectangle at all and a restored one may have been fitted to a different display, so
    // without this the core would hold neither until somebody moved the window.
    reportFrame()
    // After that report, so the rectangle the window comes back out to is on the record before
    // the space takes it. The delegate reports the full-screen itself.
    if restoring.fullScreen {
      window.toggleFullScreen(nil)
    }
  }

  /// Renders what the core says this window is showing.
  ///
  /// Whole-view and idempotent, so applying the same one twice is applying it once. Regions
  /// that survive keep their surfaces, and a region that goes hands its panes back rather than
  /// tearing them down - a surface belongs to its pane, and outlives every region that shows
  /// it (`architecture.md`, a surface belongs to its pane).
  public func apply(_ contents: WindowContents) {
    // Before any region applies, and it has to be before: a pane that moved from one region
    // to another is named by the second, and a pass that ran afterwards would take it back
    // from whichever of the two happened to apply first. What is left over waits off screen
    // with its bridge running, which is what makes returning to a tab free rather than the
    // half-second its machine takes to open another ssh session.
    surfaces.park(
      everythingBut: contents.regions.reduce(into: Set<PaneKey>()) { claimed, described in
        claimed.formUnion(described.panes ?? regions[described.id]?.onScreen ?? [])
      })

    var order: [(id: String, weight: CGFloat, view: NSView)] = []
    for described in contents.regions {
      let region = regions[described.id] ?? make(regionID: described.id)
      order.append((id: described.id, weight: described.weight, view: region))
      region.apply(described, focused: described.id == contents.focusedRegion)
      // After the surfaces exist, because a surface is what carries a size. Every pane every
      // time: the core sends the whole view, so this is idempotent by the same argument the
      // rest of this function is - and the surface itself skips a number it already has, which
      // is what stops a publish per agent transition from reflowing the window.
      for leaf in described.tree?.leaves ?? [] {
        report(
          region.chrome(for: leaf.paneID)?.surface.setFontSizeOffset(leaf.fontSizeOffset) ?? [])
      }
    }

    let named = Set(contents.regions.map(\.id))
    for (id, gone) in regions where !named.contains(id) {
      gone.removeFromSuperview()
      regions.removeValue(forKey: id)
    }
    strip.arrange(order)

    keyboardPane = contents.keyboardPane
    let focused = contents.regions.first { $0.id == contents.focusedRegion }
    focusedRegionID = focused?.id
    keyboardKey = focused.flatMap { region in
      region.keyboardPane.map { PaneKey(daemon: region.daemon, pane: $0) }
    }
    sidebar.apply(roster: roster, agents: agents, keyboard: keyboardKey)
    zoomed = focused?.zoomed ?? false
    empty.apply(showing: !contents.regions.isEmpty)
    applyTitle()
  }

  public func apply(pane: PaneKey, agent: PaneAgent) {
    let rangBefore = agents[pane]?.rang ?? false
    agents[pane] = agent
    // Whether or not a region is showing it. A pane parked off screen keeps its border
    // painted, so the tab somebody switches back to is right on its first frame rather than
    // on the agent's next transition.
    surfaces.chrome(for: pane)?.apply(paneID: pane.pane, state: agent.state)
    // One Dock bounce per bell nobody heard, as Terminal and Ghostty do for a bell in the
    // background: informational, so it stops by itself rather than until Muster is clicked.
    if agent.rang && !rangBefore && !NSApp.isActive {
      NSApp.requestUserAttention(.informationalRequest)
    }
    sidebar.apply(roster: roster, agents: agents, keyboard: keyboardKey)
  }

  /// Everything the daemons hold, whether or not this window is showing it.
  ///
  /// The list is the half of the founding desideratum the window cannot carry on its own: a
  /// pane no region shows has no border to colour, and it is the one most likely to have
  /// finished while nobody was looking.
  public func apply(_ roster: Roster) {
    self.roster = roster
    // The one message naming every pane on every attached daemon, which makes it the only
    // thing that can say a parked pane has closed. Without this a window that visited fifteen
    // tabs would hold fifteen tabs' worth of bridges until it quit.
    surfaces.release(everythingBut: Set(roster.panes.map(\.key)))
    sidebar.apply(roster: roster, agents: agents, keyboard: keyboardKey)
    applyBadges()
  }

  /// How long a numbered chord has to be held before the numbers appear over the panes.
  ///
  /// It exists to stop a tab jump you make and finish in one motion from flashing the numbers
  /// on the way past. Settled by driving it rather than by argument, and zero and 50ms were
  /// both tried and rejected on the way here: below about a tenth of a second the flash is
  /// still there, and a flash is worse than a wait nobody notices.
  ///
  /// The reveal goes through `asyncAfter` whatever this is, so it lands a runloop turn after
  /// the roster that caused it rather than inside it. That stays true at zero, which is what
  /// makes taking the delay away again a change of one constant rather than of a code path.
  private static let badgeDelay: TimeInterval = 0.12

  /// Whether a chord is being typed right now, which is not yet whether the badges are drawn.
  private var badgesWanted = false
  private var badgesShown = false

  /// Which arming the pending reveal belongs to, so a chord that ends inside the delay cancels
  /// it rather than flashing a moment later over a window that has moved on.
  private var badgeGeneration = 0

  /// Reveals or hides the numbers over the panes, following the chord being typed.
  ///
  /// One timer for the window rather than one per pane: fifteen panes revealing themselves on
  /// fifteen independent deadlines is fifteen chances to disagree about what moment it is.
  private func applyBadges() {
    let wanted = roster.numbering.isHalfTyped
    guard wanted != badgesWanted else {
      // The chord has not started or ended, but the panes under it may have. Redrawn so a pane
      // that opened mid-gesture carries its number.
      drawBadges()
      return
    }
    badgesWanted = wanted
    badgeGeneration += 1
    guard wanted else {
      badgesShown = false
      drawBadges()
      return
    }
    drawBadges()
    let arming = badgeGeneration
    DispatchQueue.main.asyncAfter(deadline: .now() + MusterWindow.badgeDelay) { [weak self] in
      guard let self, self.badgeGeneration == arming else { return }
      self.badgesShown = true
      self.drawBadges()
    }
  }

  /// Hands every pane the press that would reach it now, or zero while none would.
  ///
  /// The press comes off the roster, which is the same field the agent list draws - so the
  /// digit over a pane and the second digit beside its row are one answer rather than two.
  ///
  /// Gated on the tab being armed rather than on the press being non-zero, because a pane row
  /// carries its whole chord whether or not anything has been pressed. A first press does put
  /// the tab it named on screen, so in practice the panes with badges are the ones visible -
  /// but a window that drew badges because a pane happened to carry a number would be one
  /// closed tab away from drawing them for a gesture nobody made.
  private func drawBadges() {
    for tab in roster.tabs {
      let reached = badgesShown && tab.armed
      for pane in tab.panes {
        surfaces.chrome(for: pane.key)?.apply(badge: reached ? pane.press : 0)
      }
    }
  }

  /// Takes what the keyboard is holding, and ends a numbered chord that was still being typed.
  ///
  /// Only while a press has named a tab -
  /// so an idle window makes no round trip for the ⌘ every other shortcut is held with.
  private func apply(held: NSEvent.ModifierFlags) {
    guard NumberedChord.ends(numbering: roster.numbering, held: held, chord: chordModifiers)
    else { return }
    Core.endNumberedChord()
  }

  /// What the window should be showing of itself, as the core decided it.
  ///
  /// Applied rather than toggled: the core holds the answer and sends it whole, including
  /// once at startup, so this window never has a default of its own to disagree with.
  public func apply(presentation: Presentation) {
    split.sidebarShown = presentation.sidebar
    split.sidebarWidth = presentation.sidebarWidth
  }

  /// Says so when the renderer would not size a pane's text.
  ///
  /// Nothing in the suite can catch this: the actions are named by string, and validating one
  /// needs a live surface. So a version bump that renamed them shows up here rather than as a
  /// chord that quietly does nothing.
  private func report(_ refused: [String]) {
    guard !refused.isEmpty else { return }
    Core.warn(
      "renderer.action.refused",
      [
        "actions": refused.joined(separator: ", "),
        "impact": "the text in this pane is whatever size it already was, and the chord that "
          + "asked will keep doing nothing",
        "check": "whether libghostty renamed these between deps/ghostty.pin bumps; Muster "
          + "names them as strings and nothing else can tell",
      ])
  }

  /// Repaints the window after the config file was read again.
  ///
  /// The panes are libghostty's to repaint and the chrome is Muster's, which is the same split
  /// the launch path makes - the core sends one answer and the shell hands each half to whoever
  /// draws it. `adoptChrome` is that launch path's own call, used here rather than repeated, so
  /// the two cannot end up applying different subsets of it.
  public func apply(appearance: Core.Appearance) {
    renderer.apply(appearance: appearance.pane)
    adoptChrome(appearance)
    for divider in dividers() {
      divider.recolor()
    }
    // The rings and the roster's dots are drawn from the same table, so both have to be told.
    // Nothing else repaints them: a border is applied when a state changes, and a saved file
    // changes no state.
    for chrome in surfaces.chromes {
      chrome.recolor()
    }
    sidebar.reloadColors()
  }

  /// Rebuilds the menu after the config file was read again.
  ///
  /// On macOS this is the whole of rebinding: a key equivalent on a menu item is where the
  /// platform dispatches a chord from, so a menu that still carries the old ones is a config
  /// that did not reload.
  public func apply(bindings: [Core.Binding]) {
    AppMenu.install(target: self, bindings: bindings)
    // The empty window names a chord, so a rebind has to reach it too. A window sitting empty
    // while somebody edits the config file is exactly when a stale hint would be read.
    self.bindings = bindings
    applyEmptyMessage()
    // Rebinding the nine onto another modifier moves which release ends a two-stage chord, and
    // a stale answer here is a gesture that either never ends or ends on the wrong key.
    chordModifiers = NumberedChord.modifiers(bindings)
  }

  /// Every divider on screen, region boundaries and pane splits alike.
  private func dividers() -> [DividerView] {
    var found: [DividerView] = []
    var pending: [NSView] = [split]
    while let view = pending.popLast() {
      if let divider = view as? DividerView { found.append(divider) }
      pending.append(contentsOf: view.subviews)
    }
    return found
  }

  public func apply(daemon: String, health state: String, detail: String) {
    health[daemon] = DaemonHealth(state: state, detail: detail)
    applyTitle()
    applyEmptyMessage()
  }

  /// What the empty window says, which names the daemons still being attached.
  private func applyEmptyMessage() {
    let connecting = health.filter { $0.value.state == "connecting" }.keys.sorted()
    empty.apply(EmptyWindow.message(bindings: bindings, connecting: connecting))
  }

  /// A window with no daemon behind it: one surface running the user's shell.
  ///
  /// Not a degenerate case of the above, and deliberately not made to look like one. There is
  /// no pane, no core-published view and no control stream to put keystrokes on, so this
  /// renders and swallows the keyboard - which the title says, because a terminal that
  /// ignores you and does not explain itself is the worst thing this app could ship.
  public func showRendererCheck() {
    rendererCheck = true
    let region = make(regionID: "renderer-check")
    strip.arrange([(id: "renderer-check", weight: 1, view: region)])
    let chrome = PaneChrome(frame: strip.bounds, surface: SurfaceView(frame: strip.bounds))
    region.addSubview(chrome)
    chrome.autoresizingMask = [.width, .height]
    region.layoutSubtreeIfNeeded()
    start(chrome, command: ProcessInfo.processInfo.environment["SHELL"], typeable: false)
    applyTitle()
  }

  private func make(regionID: String) -> RegionView {
    let region = RegionView(frame: strip.bounds, surfaces: surfaces)
    strip.addSubview(region)
    regions[regionID] = region
    return region
  }

  /// Opens a link somebody cmd-clicked in a pane, on this Mac, if `LinkPolicy` allows it.
  ///
  /// A web link in a devenv pane opens here like one in a laptop pane: the browser is on this
  /// machine, and the address means the same from either.
  private func open(_ link: OpenedLink, onThisMachine: Bool, daemon: String) {
    switch LinkPolicy.decide(link, onThisMachine: onThisMachine) {
    case .open(let url):
      NSWorkspace.shared.open(url)
    case .confirm(let url):
      let handler =
        NSWorkspace.shared.urlForApplication(toOpen: url)
        .map { FileManager.default.displayName(atPath: $0.path) } ?? "whichever app handles it"
      ConfirmSheet.ask(
        on: window, question: "Open this link?",
        body: "A program in this pane linked some text to this address, which the screen does "
          + "not show. It opens in \(handler).",
        confirm: "Open Link", preview: url.absoluteString,
        then: { NSWorkspace.shared.open(url) })
    case .refuse(let reason):
      NSSound.beep()
      // The link itself is left out: it is text a program printed, and may carry a token.
      Core.warn(
        "link.refused",
        [
          "daemon": daemon,
          "reason": reason,
          "impact": "a cmd-clicked link in this pane did not open",
          "check": "whether the link is a path on another machine, or a file that would run "
            + "code; copy it and open it by hand if it is what you meant",
        ])
    }
  }

  /// Gives a pane's chrome a surface, and starts the bridge that paints it.
  private func start(
    _ chrome: PaneChrome, in region: WindowContents.Region, pane: PaneTree.Leaf
  ) {
    guard let paneID = chrome.paneID else { return }
    let daemonID = region.daemon
    if let agent = agents[PaneKey(daemon: daemonID, pane: paneID)] {
      chrome.apply(paneID: paneID, state: agent.state)
    }
    guard let linkSocketPath = pane.linkSocketPath else {
      // No link is bound for this pane yet, and a bridge the window cannot hear from is one
      // whose death nobody notices. The core binds one and republishes, and the surface is
      // built on that pass instead.
      Core.warn(
        "pane.surface.deferred",
        [
          "daemon": daemonID,
          "pane": paneID,
          "impact": "this pane is blank until the core binds its link and republishes",
          "check": "a record above this saying why the pane's link socket could not be bound",
        ])
      return
    }
    guard let daemonSocket = region.daemonSocket else {
      // Rebuilt once the view names one: a surface that was never given a bridge for want of a
      // socket is not reused (PaneSurfaces.borrow).
      Core.warn(
        "pane.surface.deferred",
        [
          "daemon": daemonID,
          "pane": paneID,
          "impact": "this pane is blank until the core reaches its daemon and republishes",
          "check": "the daemon's attach records above this, which say why it was not reached",
        ])
      return
    }
    // Reported rather than acted on here: the core is what can find out whether a bridge
    // ending means the daemon has dropped the pane, which is the commonest reason.
    chrome.surface.onProcessExited = { processAlive in
      Core.bridgeExited(daemonID: daemonID, paneID: paneID, processAlive: processAlive)
    }
    let onThisMachine = !region.remote
    chrome.surface.onOpenLink = { [weak self] link in
      self?.open(link, onThisMachine: onThisMachine, daemon: daemonID)
    }
    start(
      chrome,
      command: PaneCommand.bridge(
        executable: executable, paneID: paneID, daemonSocket: daemonSocket,
        linkSocketPath: linkSocketPath, remote: region.remote,
        reattaching: pane.bridgeRestarts > 0),
      typeable: true)
    Core.bridgeStarted(daemonID: daemonID, paneID: paneID, bridgeRestarts: pane.bridgeRestarts)
  }

  private func start(_ chrome: PaneChrome, command: String?, typeable: Bool) {
    Core.info(
      "surface.create",
      ["pane": chrome.paneID ?? "(none)", "command": command ?? "(none)"])
    do {
      chrome.surface.attach(
        try renderer.makeSurface(in: chrome.surface, command: command), typeable: typeable)
    } catch {
      // One pane, not the window: the rest keep rendering, and a bug report needs to say
      // which one went missing rather than that something failed.
      Core.error(
        "pane.surface.failed",
        [
          "pane": chrome.paneID ?? "(none)",
          "error": "\(error)",
          "impact": "this pane renders nothing; every other pane in the window is unaffected",
        ])
    }
  }

  /// Says why this window is showing nothing, when it was asked to show something.
  ///
  /// The core has already explained itself on stderr and in the log. This is the part a user
  /// who is looking at the window can see, and without it an empty window titled as the
  /// renderer check reads as though the pane they named was fine.
  public func report(problem: String) {
    self.problem = problem
    applyTitle()
  }

  /// Asks whether to send a paste the pane's daemon held, and sends it on a yes.
  public func hold(_ held: HeldPaste) {
    heldPastes.hold(held)
  }

  /// Comes to the front, with its window made key.
  public func raise() {
    if window.isMiniaturized { window.deminiaturize(nil) }
    window.makeKeyAndOrderFront(nil)
  }

  /// Everything wrong with the window, for the roster to say properly and the title to count.
  ///
  /// Both, because the roster is the only one that can carry a sentence and it is not always on
  /// screen. A window narrowed below the width a list needs would otherwise report a broken
  /// config exactly the way Muster used to: not at all.
  public func apply(problems: [Problem]) {
    outstanding = problems
    sidebar.apply(problems: problems)
    applyTitle()
  }

  /// The daemon whose health the title should report, which is the unhappiest one.
  ///
  /// A window showing two daemons has two answers and one title bar. Reporting the worst
  /// names a session that is not being kept up to date; reporting anything else would let a
  /// stale devenv sit behind a title that says everything is fine, which is the failure the
  /// health state exists to prevent.
  ///
  /// Nothing attached is disconnected rather than fine, because a window with no daemon
  /// behind it is not a healthy window.
  private var worstHealth: (daemon: String, state: String, detail: String) {
    let ranked = ["connected": 0, "": 0, "connecting": 1, "stale": 1, "disconnected": 2]
    guard
      let worst = health.max(by: { (ranked[$0.value.state] ?? 3) < (ranked[$1.value.state] ?? 3) })
    else {
      return ("", "disconnected", "")
    }
    return (worst.key, worst.value.state, worst.value.detail)
  }

  private func applyTitle() {
    let (daemon, state, detail) = worstHealth
    window.title = PaneAppearance.title(
      paneID: keyboardPane, zoomed: zoomed, health: state, detail: detail, daemon: daemon,
      problem: problem, rendererCheck: rendererCheck,
      unseenProblems: split.sidebarVisible ? 0 : outstanding.count)
  }
}

/// Whether anybody is looking at this window.
///
/// The only input to agent state that no daemon can supply. `done` is `idle` on a pane nobody
/// looked at, and no daemon can see a window's focus - so without this, a window sitting behind
/// a browser while an agent finishes would report `idle`: "nothing needs you", at the one
/// moment something does. The core decides what this means; this only says it happened.
///
/// Key window rather than app activation, because the question is whether this window was
/// being looked at. An app can be frontmost with this window behind its own settings sheet,
/// and the agent that finished underneath was no more seen than if the app were hidden.
extension MusterWindow: NSWindowDelegate {
  public func windowDidBecomeKey(_ notification: Notification) {
    Core.windowFocused(true)
    surfaces.window(key: true)
  }

  public func windowDidResignKey(_ notification: Notification) {
    Core.windowFocused(false)
    surfaces.window(key: false)
  }

  /// Hidden, minimized, covered, or on a locked screen. What Ghostty's own window does, so that a
  /// window nobody can see stops drawing.
  public func windowDidChangeOcclusionState(_ notification: Notification) {
    surfaces.window(visible: window.occlusionState.contains(.visible))
  }

  public func windowDidMove(_ notification: Notification) {
    reportFrame()
  }

  public func windowDidResize(_ notification: Notification) {
    reportFrame()
  }

  public func windowWillEnterFullScreen(_ notification: Notification) {
    fullScreen = true
    reportFrame()
  }

  public func windowDidExitFullScreen(_ notification: Notification) {
    fullScreen = false
    reportFrame()
  }

  /// A full-screen transition that was started and did not happen - a space macOS declined to
  /// make, or one somebody escaped out of. Without this the window would be remembered as
  /// full-screen and come back into a space it never entered.
  public func windowDidFailToEnterFullScreen(_ window: NSWindow) {
    fullScreen = false
    reportFrame()
  }
}

extension MusterWindow {
  /// Tells the core where the window is, keeping the last size it had outside full-screen.
  ///
  /// Called from four delegate methods and from `show`, all of which are the same fact arriving
  /// by different doors. The sender behind it coalesces, so a drag costs one round trip at a
  /// time and always ends on the position the gesture finished at.
  private func reportFrame() {
    if !fullScreen {
      settledFrame = window.frame
    }
    frames.send(rect: settledFrame, fullScreen: fullScreen)
  }
}

/// A rectangle as one field of a log line.
private func described(_ rect: NSRect) -> String {
  "\(Int(rect.origin.x)),\(Int(rect.origin.y)) \(Int(rect.width))x\(Int(rect.height))"
}

// What the menu's items do. Every one of them is a request to the core and nothing else:
// the shell asks, the daemon answers, and the window changes when the view that comes back
// says it did (architecture.md, one action path).
extension MusterWindow {
  @objc public func splitRight(_ sender: Any?) {
    Core.split(side: "right")
  }

  @objc public func splitDown(_ sender: Any?) {
    Core.split(side: "down")
  }

  @objc public func splitLeft(_ sender: Any?) {
    Core.split(side: "left")
  }

  @objc public func splitUp(_ sender: Any?) {
    Core.split(side: "up")
  }

  @objc public func newTab(_ sender: Any?) {
    Core.createTab()
  }

  /// Closes the tab the keyboard is in, and every pane in it.
  ///
  /// No confirmation, on the same terms as Close Pane: Muster does not ask before closing a
  /// pane either, and a sheet in front of one of the two would say the other is safe.
  @objc public func closeTab(_ sender: Any?) {
    Core.closeTab()
  }

  /// Pulls the pane the keyboard is on out of its split and into a tab of its own.
  ///
  /// Which pane that is is the core's, like every other item here. The pane keeps running - this
  /// is one `pane.move`, not a tab made and a pane started and one of them thrown away.
  @objc public func movePaneToNewTab(_ sender: Any?) {
    Core.movePaneToNewTab()
  }

  /// Names the pane the keyboard is on, having asked what to call it.
  ///
  /// The core decides which pane that is, as it does for every other item here - the roster is
  /// read only to start the field off with the name this pane already has, and a window whose
  /// roster has not caught up asks for a name from empty rather than refusing.
  @objc public func renamePane(_ sender: Any?) {
    askToName(subject: "pane", current: namedPane(keyboardKey)?.givenName ?? "") { name in
      Core.renamePane(name: name)
    }
  }

  /// Names the tab the keyboard's pane is in, having asked what to call it.
  @objc public func renameTab(_ sender: Any?) {
    askToName(subject: "tab", current: tabHolding(keyboardKey)?.givenName ?? "") { name in
      Core.renameTab(name: name)
    }
  }

  /// Opens the find bar over the pane the keyboard is on, or refocuses the one already up.
  ///
  /// The bar follows the keyboard rather than staying where it was opened: a find is about a
  /// pane, and one hanging over a pane nobody is searching would count matches in another.
  @objc public func find(_ sender: Any?) {
    guard let chrome = keyboardChrome() else {
      // The renderer check, and a window whose every pane closed. Nothing to search and
      // nothing worth interrupting somebody about - the menu item is there because the menu
      // is built from the core's whole action list, not from what is currently possible.
      Core.warn(
        "find.noPane",
        [
          "impact": "the find bar did not open, because no pane has this window's keyboard.",
          "check": "whether this is the renderer check, or a window whose panes have all "
            + "exited - the attach records above say which",
        ])
      return
    }
    let bar = findBar ?? makeFindBar()
    bar.show(over: chrome)
    NotificationCenter.default.post(name: .musterFindFocus, object: nil)
  }

  @objc public func findNext(_ sender: Any?) {
    findBar?.step(forward: true)
  }

  @objc public func findPrevious(_ sender: Any?) {
    findBar?.step(forward: false)
  }

  private func makeFindBar() -> FindBar {
    let bar = FindBar()
    bar.onReturnToPane = { [weak self] in
      guard let chrome = self?.keyboardChrome() else { return }
      self?.window.makeFirstResponder(chrome.surface)
    }
    bar.onRefused = { [weak self] refused in self?.reportUnsearched(refused) }
    findBar = bar
    return bar
  }

  /// Says so when the renderer would not search for what was typed.
  ///
  /// Separate from `report` because the consequence is different and so is what to check: the
  /// pane is fine, and the find bar will count nothing in it however much there is to find.
  private func reportUnsearched(_ refused: [String]) {
    guard !refused.isEmpty else { return }
    Core.warn(
      "renderer.action.refused",
      [
        "actions": refused.joined(separator: ", "),
        "impact": "find searched nothing, so the bar shows no count and nothing is marked, "
          + "whatever the pane holds",
        "check": "whether libghostty renamed its search actions between deps/ghostty.pin "
          + "bumps; Muster names them as strings and nothing else can tell",
      ])
  }

  /// The chrome of the pane this window's keyboard feeds.
  func keyboardChrome() -> PaneChrome? {
    guard let key = keyboardKey else { return nil }
    return surfaces.chrome(for: key)
  }

  /// Runs the sheet, and sends what came back.
  ///
  /// Shared by the menu items and by a double-click in the list, so that all three are one
  /// path rather than three that drift. Nothing is drawn optimistically: the request goes out
  /// and the name appears when the daemon's answer comes back as the next roster.
  func askToName(subject: String, current: String, then send: @escaping (String) -> Void) {
    RenameSheet.ask(on: window, subject: subject, current: current, then: send)
  }

  /// Names a pane, having asked what to call it - the pane named rather than the keyboard's,
  /// for a row double-clicked or a menu opened on a pane somebody is not typing into.
  func rename(pane: PaneKey) {
    askToName(subject: "pane", current: namedPane(pane)?.givenName ?? "") { name in
      Core.renamePane(name: name, daemonID: pane.daemon, paneID: pane.pane)
    }
  }

  func rename(tab: String) {
    let current = roster.tabs.first { $0.id == tab }?.givenName ?? ""
    askToName(subject: "tab", current: current) { name in
      Core.renameTab(name: name, tabID: tab)
    }
  }

  /// The machines are asked for as the menu opens, because one can attach or drop between one
  /// right-click and the next.
  func menu(forPane pane: PaneKey) -> NSMenu? {
    ContextMenus.pane(
      pane, surface: surfaces.chrome(for: pane)?.surface,
      machines: Core.machines().map(\.daemon), rename: { [weak self] in self?.rename(pane: $0) })
  }

  /// A right-clicked row's menu. A machine's row has none: clicking it already makes a tab
  /// there, and its name is the config file's to change.
  func menu(for row: SidebarModel.Row) -> NSMenu? {
    switch row.kind {
    case .pane:
      guard let pane = row.pane else { return nil }
      return ContextMenus.agentRow(
        pane, onScreen: row.onScreen, rename: { [weak self] in self?.rename(pane: $0) })
    case .tab:
      let first = roster.tabs.first { $0.id == row.tab }?.panes.first?.key
      return ContextMenus.tab(
        row.tab, firstPane: first, machines: Core.machines().map(\.daemon),
        rename: { [weak self] in self?.rename(tab: $0) })
    case .machine:
      return nil
    }
  }

  /// This window's roster entry for a pane, if it has one.
  func namedPane(_ key: PaneKey?) -> Roster.Pane? {
    guard let key else { return nil }
    return roster.panes.first { $0.key == key }
  }

  /// The tab holding a pane, as the roster listed it.
  func tabHolding(_ key: PaneKey?) -> Roster.Tab? {
    guard let key else { return nil }
    return roster.tabs.first { tab in tab.panes.contains { $0.key == key } }
  }

  @objc public func closePane(_ sender: Any?) {
    Core.closePane()
  }

  // Each of the four reports the cell it is resizing by and the region it is resizing inside,
  // because `resize_step` is a distance and what the daemon moves is a share of the region.
  // Only a live surface can say how many points a cell is, and only the window can say how
  // wide the region it drew is.
  @objc public func resizePaneLeft(_ sender: Any?) {
    Core.resize(direction: "left", cell: keyboardSurface?.cellPointSize, region: keyboardRegionSize)
  }

  @objc public func resizePaneRight(_ sender: Any?) {
    Core.resize(
      direction: "right", cell: keyboardSurface?.cellPointSize, region: keyboardRegionSize)
  }

  @objc public func resizePaneUp(_ sender: Any?) {
    Core.resize(direction: "up", cell: keyboardSurface?.cellPointSize, region: keyboardRegionSize)
  }

  @objc public func resizePaneDown(_ sender: Any?) {
    Core.resize(direction: "down", cell: keyboardSurface?.cellPointSize, region: keyboardRegionSize)
  }

  @objc public func zoomPane(_ sender: Any?) {
    Core.zoom()
  }

  // Ghostty's surface-local binding actions, carried out on the surface with the keyboard.
  @objc public func scrollToTop(_ sender: Any?) { performOnSurface(.scrollToTop) }
  @objc public func scrollToBottom(_ sender: Any?) { performOnSurface(.scrollToBottom) }
  @objc public func scrollPageUp(_ sender: Any?) { performOnSurface(.scrollPageUp) }
  @objc public func scrollPageDown(_ sender: Any?) { performOnSurface(.scrollPageDown) }
  @objc public func jumpToPreviousPrompt(_ sender: Any?) {
    performOnSurface(.jumpToPreviousPrompt)
  }
  @objc public func jumpToNextPrompt(_ sender: Any?) { performOnSurface(.jumpToNextPrompt) }
  /// The pane's text, unless a field the person is typing in has the keyboard - the rename
  /// sheet, the find bar - in which case that field's. The menu item targets this window
  /// directly, so without this a field never hears cmd+A.
  @objc public func selectAllInPane(_ sender: Any?) {
    if let field = MusterWindow.textBeingEdited(by: NSApp.keyWindow?.firstResponder) {
      field.selectAll(sender)
      return
    }
    performOnSurface(.selectAll)
  }

  /// The text a field holds while the person types in it, given what has the keyboard: the field
  /// editor AppKit makes first responder for a text field. A pane's surface is never one.
  static func textBeingEdited(by firstResponder: NSResponder?) -> NSText? {
    firstResponder as? NSText
  }

  // The two the pane's daemon carries out, handed the keystroke that asked when one did: a menu
  // item's action runs inside the key event it was chosen by.
  @objc public func clearScreen(_ sender: Any?) {
    Core.perform(.clearScreen, key: NSApp.currentEvent)
  }
  @objc public func resetTerminal(_ sender: Any?) {
    Core.perform(.resetTerminal, key: NSApp.currentEvent)
  }

  private func performOnSurface(_ action: SurfaceAction) {
    keyboardChrome()?.surface.perform(action)
  }

  @objc public func increaseFontSize(_ sender: Any?) {
    Core.adjustFontSize("larger")
  }

  @objc public func decreaseFontSize(_ sender: Any?) {
    Core.adjustFontSize("smaller")
  }

  @objc public func resetFontSize(_ sender: Any?) {
    Core.adjustFontSize("reset")
  }

  @objc public func reloadConfig(_ sender: Any?) {
    Core.reloadConfig()
  }

  @objc public func toggleSidebar(_ sender: Any?) {
    Core.toggleSidebar()
  }

  /// Opens the list of what this window does. Answered here rather than by the core: the
  /// list is built from what the core already publishes, and a window is a shell's to open.
  /// Opens another Muster, which is what another window is.
  ///
  /// Nothing is asked of the core, on the same terms as `showShortcuts` below: a window is a
  /// process - the core holds one session per process - so making one is starting an app, and
  /// starting an app is an OS act. There is no request that could carry it, and a core that
  /// grew one would be a core that has to be running before a window can exist.
  ///
  /// Through Launch Services rather than by spawning the executable, because that is what makes
  /// a GUI app: activation, the Dock, and which application macOS charges a permission prompt
  /// to. A new app is given launchd's environment rather than this one's, so where Muster keeps
  /// its files travels as an argument - the same thing `muster window new` does, which reaches
  /// Launch Services through `open` and has to clear that command's own environment to get the
  /// same answer. Read back by `launchHome`.
  @objc public func newWindow(_ sender: Any?) {
    openAnother(fresh: true)
  }

  /// Starts another Muster, and says whether it is a window somebody asked for.
  ///
  /// Fresh means it starts on tabs of its own rather than on the ones this window is showing -
  /// which it could not render anyway, because a daemon lets one bridge draw a pane - and takes
  /// an arrangement nothing has ever held. Not fresh means it takes the most recent arrangement
  /// no live window is holding, which is the window that was closed.
  /// Opens a closed window again, onto a pane or tab it holds.
  ///
  /// What going to that pane or tab from here comes to: the window is closed, its tabs are still
  /// its own, and a window is a process - so this starts one, told which record to take and where
  /// to go once it is open.
  public func reopen(named name: String, showing show: String) {
    openAnother(
      fresh: false, reopening: [windowFlag, name] + (show.isEmpty ? [] : [showFlag, show]))
  }

  private func openAnother(fresh: Bool, reopening: [String] = []) {
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.createsNewApplicationInstance = true
    var arguments = (fresh ? [freshFlag] : []) + reopening
    if let home = ProcessInfo.processInfo.environment["MUSTER_HOME"], !home.isEmpty {
      arguments += ["--home", home]
    }
    configuration.arguments = arguments
    NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) {
      _, error in
      guard let error else { return }
      Core.warn(
        "window.open.failed",
        [
          "detail": error.localizedDescription,
          "bundle": Bundle.main.bundleURL.path,
          "impact": "no second window opened, and the one you are looking at is unaffected",
          "check":
            "whether this build is a real bundle - a window opened from a build tree has no "
            + "app to make a second copy of",
        ])
    }
  }

  /// Brings back the window that was closed, which is another Muster with one flag off.
  ///
  /// Nothing is asked of the core, on the same terms as `newWindow`. The difference between the
  /// two is which arrangement the launched window takes, and the launched window works that out
  /// for itself from the flag - so this is `newWindow` without it.
  @objc public func reopenWindow(_ sender: Any?) {
    openAnother(fresh: false)
  }

  @objc public func showShortcuts(_ sender: Any?) {
    shortcuts.show(bindings: Core.bindings())
  }

  /// Quits, ending the sessions this window is attached to rather than leaving them running.
  ///
  /// Asks first, and the asking is the feature. Leaving sessions running is the default, the
  /// promise, and what every other way out of Muster does; this is the one action in the app
  /// that ends processes holding somebody's work, and until now the only way to do it was
  /// `pgrep` and a signal - which cost a working agent once, because the daemon holding it
  /// looked exactly like the scratch ones beside it.
  ///
  /// What the sheet is told comes from the core rather than from anything accumulated here, so
  /// the machines it names are the ones `muster window` would name.
  @objc public func quitAndCloseSessions(_ sender: Any?) {
    let machines = Core.machines()
    ConfirmSheet.ask(
      on: window,
      question: QuitSummary.question(machines: machines),
      body: QuitSummary.body(machines: machines),
      confirm: "Quit and Close Sessions"
    ) {
      // Set, then quit through the one path out. The core does the stopping while this
      // window's bridges are still alive, which is the same reason quitting is synchronous
      // at all.
      Core.closeSessionsOnQuit()
      NSApp.terminate(nil)
    }
  }

  @objc public func focusNextTab(_ sender: Any?) {
    Core.focus(tabStep: "next")
  }

  @objc public func focusPreviousTab(_ sender: Any?) {
    Core.focus(tabStep: "previous")
  }

  /// Presses the numbered chord this item was made for.
  ///
  /// One method for all nine, reading the digit off the item's tag. Nine methods differing by
  /// a digit is nine places for one of them to drift, and the number is data anyway.
  @objc public func pressNumberedChord(_ sender: Any?) {
    guard let item = sender as? NSMenuItem, item.tag > 0 else { return }
    Core.pressNumberedChord(item.tag)
  }

  @objc public func focusPaneAsking(_ sender: Any?) {
    // Nothing asking is a press with nothing to act on, which the platform answers with a beep.
    if !Core.focusAsking() {
      NSSound.beep()
    }
  }

  // A beep at either end of the history, for the same reason. The mouse buttons skip these and
  // stay silent there, as a browser's do.
  @objc public func focusBack(_ sender: Any?) {
    if !Core.focusHistory(forward: false) {
      NSSound.beep()
    }
  }

  @objc public func focusForward(_ sender: Any?) {
    if !Core.focusHistory(forward: true) {
      NSSound.beep()
    }
  }

  @objc public func focusNextPane(_ sender: Any?) {
    Core.focus(step: "next")
  }

  @objc public func focusPreviousPane(_ sender: Any?) {
    Core.focus(step: "previous")
  }

  @objc public func focusPaneLeft(_ sender: Any?) {
    Core.focus(step: "left")
  }

  @objc public func focusPaneRight(_ sender: Any?) {
    Core.focus(step: "right")
  }

  @objc public func focusPaneUp(_ sender: Any?) {
    Core.focus(step: "up")
  }

  @objc public func focusPaneDown(_ sender: Any?) {
    Core.focus(step: "down")
  }
}

/// The sidebar down the left, and everything else to the right of it.
///
/// Its own view rather than arithmetic inside the window, so that the one number here - how
/// much width the list takes - is a pure function a test can call. The list is as wide as it was
/// last dragged, and the regions get what is left.
@MainActor
final class WindowLayout: NSView {
  private var sidebar: NSView?
  private var strip: NSView?
  private let edge = SidebarEdgeView()

  /// How wide the core says the list is. Mirrored like `sidebarShown`: a drag asks for a width
  /// and this changes when the answer arrives.
  var sidebarWidth = SidebarModel.width {
    didSet { needsLayout = true }
  }

  /// Called while the list's edge is dragged, with the width being asked for.
  var onSidebarDragged: ((CGFloat) -> Void)? {
    get { edge.onDrag }
    set { edge.onDrag = newValue }
  }

  /// Whether the core says the list belongs on screen. Mirrored, never decided: the answer
  /// is written down beside the arrangement and comes back on the next launch.
  var sidebarShown = true {
    didSet { needsLayout = true }
  }

  /// Whether the list is actually on screen, which is not the same question as `sidebarShown`.
  ///
  /// A window narrowed until the list will not fit hides it without anybody deciding to, and
  /// something that can only be reported in the list needs to know the difference - otherwise a
  /// small window is a window that says nothing, which is the bug the problems area exists to
  /// fix.
  private(set) var sidebarVisible = true

  /// Called when the list appears or disappears, whoever caused it.
  var onSidebarVisibilityChanged: (() -> Void)?

  override var isFlipped: Bool { true }

  func attach(sidebar: NSView, strip: NSView) {
    self.sidebar = sidebar
    self.strip = strip
    addSubview(sidebar)
    addSubview(strip)
    // Last, so it is on top of both: it straddles the line between them.
    addSubview(edge)
    needsLayout = true
  }

  override func layout() {
    super.layout()
    let (listWidth, regionWidth) = SidebarModel.widths(
      in: bounds.width, shown: sidebarShown, wanted: sidebarWidth)
    sidebar?.frame = CGRect(x: 0, y: 0, width: listWidth, height: bounds.height)
    sidebar?.isHidden = listWidth == 0
    edge.frame = CGRect(
      x: listWidth - SidebarEdgeView.reach, y: 0, width: SidebarEdgeView.reach * 2,
      height: bounds.height)
    edge.isHidden = listWidth == 0
    window?.invalidateCursorRects(for: edge)
    strip?.frame = CGRect(x: listWidth, y: 0, width: regionWidth, height: bounds.height)
    strip?.needsLayout = true
    let visible = listWidth > 0
    if visible != sidebarVisible {
      sidebarVisible = visible
      onSidebarVisibilityChanged?()
    }
  }
}

/// The line between the agent list and the panes, as something to drag.
///
/// Invisible: the list and the panes already meet at a line, and this only gives it a grip. It
/// reaches a few points either side of that line so the pointer does not have to find a single
/// pixel, and it is laid out over both views so it wins the cursor there.
///
/// Like `DividerView`, a drag asks rather than moves: it reports the width the pointer is asking
/// for, and the list moves when the core answers with the width it settled on.
@MainActor
final class SidebarEdgeView: NSView {
  /// How far the grip reaches either side of the line, in points.
  static let reach: CGFloat = 3

  var onDrag: ((CGFloat) -> Void)?

  override func resetCursorRects() {
    addCursorRect(bounds, cursor: .resizeLeftRight)
  }

  override func mouseDragged(with event: NSEvent) {
    guard let superview else { return }
    onDrag?(superview.convert(event.locationInWindow, from: nil).x)
  }
}

/// Where each region sits, and where the lines between them are.
///
/// A pure function, in the same spirit as `PaneTree.frames`: this is the arithmetic, and
/// arithmetic inside `layout` is arithmetic no test can call. A wrong frame here looks like a
/// rendering problem and is a division.
///
/// Weights rather than a tree, because Muster owns no tree over regions - owning one is what
/// would make it a multiplexer, a non-goal - so the whole arrangement is a list and a
/// division by the sum of it.
enum RegionStripLayout {
  struct Placement: Equatable {
    let frame: CGRect
    /// The line on this region's right, and the area it shares with its neighbour. Absent on
    /// the last region, which has nothing to its right to divide against.
    let divider: CGRect?
    let area: CGRect?
  }

  /// The same thickness a pane divider has. Two lines of different weights in one window
  /// would read as two different kinds of thing, and they are the same thing one level apart.
  static let dividerThickness = PaneTree.dividerThickness

  static func place(weights: [CGFloat], in bounds: CGRect) -> [Placement] {
    guard !weights.isEmpty else { return [] }
    // A weight that is not a positive number is not this window's to validate - it came
    // across the seam - so it is read as an equal share rather than collapsing the region to
    // nothing or poisoning the total with a NaN.
    let sane = weights.map { $0.isFinite && $0 > 0 ? $0 : 1 }
    let total = sane.reduce(0, +)
    let lines = CGFloat(weights.count - 1) * dividerThickness
    let usable = max(0, bounds.width - lines)

    var placements: [Placement] = []
    var x = bounds.minX
    for (index, weight) in sane.enumerated() {
      let width = usable * weight / total
      let frame = CGRect(x: x, y: bounds.minY, width: width, height: bounds.height)
      x += width
      guard index < sane.count - 1 else {
        placements.append(Placement(frame: frame, divider: nil, area: nil))
        continue
      }
      let divider = CGRect(
        x: x, y: bounds.minY, width: dividerThickness, height: bounds.height)
      // What the pair shares, which is what a drag's ratio is measured against - the two
      // regions plus the line between them, and nothing further along the window.
      let next = usable * sane[index + 1] / total
      let area = CGRect(
        x: frame.minX, y: bounds.minY, width: width + dividerThickness + next,
        height: bounds.height)
      x += dividerThickness
      placements.append(Placement(frame: frame, divider: divider, area: area))
    }
    return placements
  }
}

/// Regions, side by side, in the order the core listed them, at the widths it gave them.
///
/// Every region starts at the same weight, so equal shares are what a window that has never
/// been dragged looks like - the floor this used to hardcode, now falling out of the
/// arrangement rather than being it.
///
/// A drag moves nothing locally. It asks the core, which owns this arrangement outright, and
/// the strip lands where the next published view puts it - the same discipline as a pane
/// divider, for the same reason.
@MainActor
final class RegionStrip: NSView {
  private var order: [NSView] = []
  private var identities: [String] = []
  private var weights: [CGFloat] = []
  private var dividers: [DividerView] = []

  /// What fills the strip when no region does. Behind the regions rather than beside them,
  /// so it needs no share of the arrangement and a region drawn over it hides it.
  private var empty: NSView?

  override var isFlipped: Bool { true }

  func attach(empty: NSView) {
    self.empty = empty
    addSubview(empty, positioned: .below, relativeTo: nil)
    needsLayout = true
  }

  func arrange(_ regions: [(id: String, weight: CGFloat, view: NSView)]) {
    order = regions.map(\.view)
    identities = regions.map(\.id)
    weights = regions.map(\.weight)
    needsLayout = true
  }

  override func layout() {
    super.layout()
    empty?.frame = bounds
    guard !order.isEmpty else { return }
    let placements = RegionStripLayout.place(weights: weights, in: bounds)

    let wanted = placements.filter { $0.divider != nil }.count
    while dividers.count < wanted {
      let divider = DividerView(frame: .zero)
      addSubview(divider)
      dividers.append(divider)
    }
    while dividers.count > wanted {
      dividers.removeLast().removeFromSuperview()
    }

    var line = 0
    for (index, placement) in placements.enumerated() {
      order[index].frame = placement.frame
      order[index].needsLayout = true
      guard let frame = placement.divider, let area = placement.area else { continue }
      let divider = dividers[line]
      line += 1
      let region = identities[index]
      divider.onDrag = { ratio in Core.setRegionBoundary(region: region, ratio: ratio) }
      divider.axis = .columns
      divider.area = area
      divider.frame = frame
      // Cursor rectangles are cached against the frame they were set from, so a divider that
      // moved shows the wrong resize cursor - or none - until this is asked for.
      window?.invalidateCursorRects(for: divider)
    }
  }
}
