import AppKit
import Testing

@testable import MusterMac

// What a pane's chrome says that is not the header's: the focus ring, where the terminal sits under
// the header, and the window's title. Small, and worth pinning anyway - the core sends every pane's
// transitions, so a window that forgets to filter shows its neighbor's agent as its own, and that
// failure is invisible until there are two panes to confuse.

@Suite("pane chrome")
struct PaneChromeTests {
  @Test("an unrecognized state is unknown, never idle")
  func unknownStateIsNotIdle() {
    // Not the same assertion twice: the point is that a state the daemon invents next week
    // gets the resting appearance of something unreadable rather than being colored as an
    // agent that finished. A user who learns the colors lie stops reading them.
    #expect(
      PaneAppearance.defaultBorderColor(state: "compacting")
        == PaneAppearance.defaultBorderColor(state: "unknown"))
    #expect(
      PaneAppearance.defaultBorderColor(state: "compacting")
        != PaneAppearance.defaultBorderColor(state: "done"))
  }

  @Test("the window's palette is the canonical default legend")
  func theLegendIsCanonical() {
    // A change-detector on purpose. `muster window` paints these same rows in the
    // terminal's own sixteen, and nothing checks the two across the language line - so this
    // exists to make changing the canon a decision that fails a build and hands you the other
    // two places it has to change: `crates/muster-cli/src/render.rs` and
    // `docs/architecture.md`.
    //
    // The *default* legend, since a person may repaint these. That does not weaken what this
    // pins: the CLI is fixed at the sixteen and honours no configuration, so what the two
    // surfaces have to agree on is exactly this table.
    #expect(PaneAppearance.defaultBorderColor(state: "working") == NSColor.systemCyan)
    #expect(PaneAppearance.defaultBorderColor(state: "waiting") == NSColor.systemIndigo)
    #expect(PaneAppearance.defaultBorderColor(state: "blocked") == NSColor.systemOrange)
    #expect(PaneAppearance.defaultBorderColor(state: "stuck") == NSColor.systemPurple)
    #expect(PaneAppearance.defaultBorderColor(state: "done") == NSColor.systemGreen)
    #expect(PaneAppearance.defaultBorderColor(state: "idle") == NSColor.systemGray)
    #expect(PaneAppearance.defaultBorderColor(state: "unknown") == NSColor.tertiaryLabelColor)
  }

  @MainActor
  @Test("a colour the file names is what the window paints, and one it leaves out is not")
  func configuredColoursWin() {
    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(
      chrome: Core.Chrome(
        divider: nil, focusRing: nil,
        agents: Core.AgentColors(blocked: "#ff0000")))

    #expect(PaneAppearance.borderColor(state: "blocked") == NSColor(hex: "#ff0000"))
    // Partial the way `[keymap]` is: somebody fixing one row they cannot see is not adopting
    // a theme, and the four they said nothing about are exactly what ships.
    #expect(
      PaneAppearance.borderColor(state: "working")
        == PaneAppearance.defaultBorderColor(state: "working"))
  }

  @MainActor
  @Test("an unset focus ring follows the platform's accent")
  func theFocusRingFollowsTheAccent() {
    // Not a fallback but the design. The accent is the platform's own answer to which thing
    // has focus and already tracks a choice made in System Settings, so a colour of Muster's
    // own here would ignore it - which is worse than the collision the ring was redrawn for.
    PaneAppearance.adopt(chrome: .none)
    #expect(PaneAppearance.focusColor == NSColor.controlAccentColor)

    defer { PaneAppearance.adopt(chrome: .none) }
    PaneAppearance.adopt(
      chrome: Core.Chrome(divider: nil, focusRing: "#bb9af7", agents: Core.AgentColors()))
    #expect(PaneAppearance.focusColor == NSColor(hex: "#bb9af7"))
  }

  @MainActor
  @Test("a pane has a border only while the keyboard is in it, whatever its agent is doing")
  func focusIsTheOnlyBorder() {
    // The border that said what an agent was doing went to the header (Fencil `pane-headers`).
    // What is left is one ring that says which pane a keystroke reaches, and that follows the
    // accent rather than a state's color - so no state can collide with somebody's accent.
    let chrome = pane()
    chrome.attach(paneID: "w1:p1")
    for state in ["working", "blocked", "stuck", "done", "idle", "unknown"] {
      chrome.apply(header: testHeaderRow("w1:p1", state: state))
      #expect(chrome.layer?.borderWidth == 0, "a \(state) pane carries a border")
    }
    chrome.apply(focused: true)
    #expect(chrome.layer?.borderWidth == PaneAppearance.focusWidth)
    chrome.apply(focused: false)
    #expect(chrome.layer?.borderWidth == 0)
  }

  @MainActor
  @Test("the terminal starts below the header and inside the ring, focused or not")
  func theSurfaceSitsBelowTheHeader() {
    // The ring's room is reserved whether or not it is drawn, so moving the keyboard from one
    // pane to another never resizes a terminal and reflows what is in it.
    let chrome = pane(width: 400, height: 300)
    chrome.attach(paneID: "w1:p1")
    let inset = PaneAppearance.inset
    let expected = CGRect(
      x: inset, y: inset, width: 400 - 2 * inset,
      height: 300 - 2 * inset - PaneHeaderModel.height)

    chrome.layoutSubtreeIfNeeded()
    #expect(chrome.surface.frame == expected)
    #expect(
      chrome.header.frame
        == CGRect(
          x: inset, y: 300 - inset - PaneHeaderModel.height, width: 400 - 2 * inset,
          height: PaneHeaderModel.height))
    #expect(chrome.header.frame.minY == chrome.surface.frame.maxY, "flush: nothing between")

    chrome.apply(focused: true)
    chrome.layoutSubtreeIfNeeded()
    #expect(chrome.surface.frame == expected)
  }

  @MainActor
  @Test(
    "a pane gains its header at 60 points, and a pane at the tree's minimum has no negative size")
  func theHeaderNeedsRoom() {
    let inset = PaneAppearance.inset
    let short = pane(width: 400, height: 59)
    short.attach(paneID: "w1:p1")
    short.layoutSubtreeIfNeeded()
    #expect(short.header.isHidden)
    #expect(short.surface.frame.height == 59 - 2 * inset, "a pane too short to carry a header")

    let enough = pane(width: 400, height: 60)
    enough.attach(paneID: "w1:p1")
    enough.layoutSubtreeIfNeeded()
    #expect(!enough.header.isHidden)
    #expect(enough.surface.frame.height == 60 - 2 * inset - PaneHeaderModel.height)

    // `PaneTree.minimumPaneSize`: a pane this small must not be given a header it has no room for.
    let smallest = pane(width: 400, height: 8)
    smallest.attach(paneID: "w1:p1")
    smallest.layoutSubtreeIfNeeded()
    #expect(smallest.surface.frame.height >= 0)
  }

  @MainActor
  @Test("a click over the terminal is still measured from the terminal's own top")
  func theMouseStillLandsOnItsCell() throws {
    // The header pushes the surface down inside the pane. A position reported from the pane's top
    // would be a cell too low for every row of the header, so this clicks a few points under the
    // surface's top edge and checks the position did not pick up the header's height.
    let chrome = pane(width: 400, height: 300)
    chrome.attach(paneID: "w1:p1")
    chrome.layoutSubtreeIfNeeded()
    chrome.surface.attach(typeable: true)
    var reported: [Core.Mouse] = []
    chrome.onPointerRequested = { _, pointer in
      if case .mouse(let mouse) = pointer { reported.append(mouse) }
    }
    let surface = chrome.surface.frame
    // Five points in and three down, in the pane's own coordinates: an event in a view with no
    // window is positioned in its ancestors' coordinates.
    let event = try #require(
      NSEvent.mouseEvent(
        with: .leftMouseDown, location: NSPoint(x: surface.minX + 5, y: surface.maxY - 3),
        modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0,
        clickCount: 1, pressure: 1))

    chrome.surface.mouseDown(with: event)

    let landed = try #require(reported.first)
    // Pixels are points times the backing scale, whatever that is here, so the two are in the
    // ratio of the offsets, and the vertical one is nowhere near a header's height.
    #expect(landed.x > 0)
    #expect(abs(landed.y / landed.x - 3.0 / 5.0) < 0.01, "x \(landed.x), y \(landed.y)")
  }

  @Test("a stale backend is admitted in the title")
  func staleIsVisible() {
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "stale",
      detail: "the daemon closed the connection")
    #expect(title.contains("w1:p1"))
    // The failure this exists to prevent: a window rendering an hour-old session as though
    // it were live, which is indistinguishable from a working one without saying so.
    #expect(title.contains("stale"))
  }

  @Test("a title says which daemon went stale")
  func staleNamesItsDaemon() {
    // A window showing a laptop and a devenv has two answers and one title bar. Saying only
    // "stale" leaves the reader to guess whether their local shell or their VPN is the
    // problem, and those are very different next actions.
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "stale", detail: "the connection dropped",
      daemon: "devenv")
    #expect(title.contains("stale devenv"))
    #expect(title.contains("the connection dropped"))
  }

  @Test("a healthy window says nothing about its health")
  func connectedIsQuiet() {
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "connected", detail: "")
    #expect(title == "muster - w1:p1")
  }

  @Test("a title counts the problems that have nowhere else to appear")
  func problemsWithNoRosterReachTheTitle() {
    // The narrow-window hole. A roster is where a problem is actually readable, and a window
    // too narrow for one used to report a broken config exactly the way Muster used to: not at
    // all. The title has no room for the sentence, so it says there is one.
    let one = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "connected", detail: "", unseenProblems: 1)
    #expect(one == "muster - w1:p1 · 1 problem")

    let several = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "connected", detail: "", unseenProblems: 3)
    #expect(several == "muster - w1:p1 · 3 problems")
  }

  @Test("a title says nothing about problems the roster is already showing")
  func problemsOnScreenStayOutOfTheTitle() {
    // Zero rather than a count, because the window passes zero whenever the roster is on
    // screen. Saying it twice would spend the title's one line on something already readable.
    let title = PaneAppearance.title(
      paneID: "w1:p1", zoomed: false, health: "connected", detail: "", unseenProblems: 0)
    #expect(title == "muster - w1:p1")
  }

  @Test("a window with no panes still counts its problems")
  func problemsReachAPanelessTitle() {
    // The launch case. A config refused before any pane exists is the worst moment to be
    // quiet, and that branch of the title is a different one.
    let title = PaneAppearance.title(
      paneID: nil, zoomed: false, health: "connected", detail: "", unseenProblems: 1)
    #expect(title.contains("1 problem"))
  }

  @Test("a zoomed tab says so, because it looks like a tab with one pane")
  func zoomIsAdmitted() {
    // The other panes are still there and still running. Without this the user has no way to
    // learn why they vanished, and nothing to undo.
    let title = PaneAppearance.title(paneID: "w1:p1", zoomed: true, health: "connected", detail: "")
    #expect(title.contains("zoomed"))
  }

  @MainActor
  @Test("a state for another pane is not this pane's state")
  func otherPanesAreIgnored() {
    let chrome = pane()
    chrome.attach(paneID: "w1:p1")

    chrome.apply(header: testHeaderRow("w1:p2", state: "blocked"))
    #expect(chrome.state == "unknown")

    chrome.apply(header: testHeaderRow("w1:p1", state: "blocked"))
    #expect(chrome.state == "blocked")
  }

  @MainActor
  @Test("the keyboard and the agent are two different edges")
  func focusAndStateDoNotShareABorder() {
    // One edge carrying both would make a working pane and the focused pane
    // indistinguishable, which is exactly the confusion these are for clearing up. Fifteen
    // panes make that difference the whole product.
    let chrome = pane()
    chrome.attach(paneID: "w1:p1")

    chrome.apply(header: testHeaderRow("w1:p1", state: "working"))
    chrome.apply(focused: false)
    #expect(chrome.isFocused == false)
    #expect(chrome.state == "working")

    chrome.apply(focused: true)
    #expect(chrome.isFocused)
    #expect(chrome.state == "working")
  }

  @MainActor
  @Test("a click asks for the keyboard, and names the pane it is about")
  func aClickCarriesThePane() {
    let chrome = pane()
    chrome.attach(paneID: "w1:p3")
    var asked: [String] = []
    chrome.onFocusRequested = { asked.append($0) }

    chrome.surface.mouseDown(
      with: NSEvent.mouseEvent(
        with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)

    #expect(asked == ["w1:p3"])
  }

  @MainActor
  @Test("a wheel names the pane it moved over, and asks for no keyboard")
  func aWheelCarriesThePane() {
    let chrome = pane()
    chrome.attach(paneID: "w1:p3")
    var scrolled: [String] = []
    var focused: [String] = []
    chrome.onPointerRequested = { paneID, _ in scrolled.append(paneID) }
    chrome.onFocusRequested = { focused.append($0) }
    // A view with no pane behind it reports nothing at all, which is a separate rule and one
    // the renderer check relies on.
    chrome.surface.attach(typeable: true)
    guard let event = wheel(deltaY: 3) else { return }

    chrome.surface.scrollWheel(with: event)

    #expect(scrolled == ["w1:p3"])
    // Two edges mean two things in this window, and so do two gestures: a click asks for the
    // keyboard and a wheel does not.
    #expect(focused.isEmpty)
  }

  @Test("the renderer check has no pane and says so")
  func rendererCheckIsLabeled() {
    #expect(
      PaneAppearance.title(
        paneID: nil, zoomed: false, health: "disconnected", detail: "", rendererCheck: true
      ).contains("renderer check"))
  }

  @Test("a window whose panes all closed is not the renderer check")
  func emptyIsNotADiagnosticMode() {
    // The two look identical on screen and want opposite reactions: one is a diagnostic mode
    // nobody asked for, the other is a window a keystroke refills. Titling the second as the
    // first told a user their ordinary window had turned into something else.
    let title = PaneAppearance.title(
      paneID: nil, zoomed: false, health: "connected", detail: "", daemon: "local")
    #expect(title == "muster - no panes")
  }

  @Test("an empty window still says its daemon went away")
  func emptyReportsHealth() {
    // The likeliest reason a window has no panes and nobody closed any. Without this the
    // window that lost its devenv and the window somebody emptied read the same.
    let title = PaneAppearance.title(
      paneID: nil, zoomed: false, health: "disconnected", detail: "", daemon: "devenv")
    #expect(title.contains("no panes"))
    #expect(title.contains("disconnected devenv"))
  }

  @Test("a number is drawn over a pane only while a chord is reaching for it")
  @MainActor
  func theBadgeAppearsOnlyMidChord() {
    let chrome = pane()
    // Zero is what every pane carries at rest and what they go back to the
    // moment a gesture ends, so a badge visible at rest would be one visible almost always.
    #expect(chrome.badgeShown == false)
    chrome.apply(badge: 2)
    #expect(chrome.badgeShown)
    chrome.apply(badge: 0)
    #expect(chrome.badgeShown == false)
  }

  @Test("the number is transparent to the mouse")
  @MainActor
  func theBadgeDoesNotSwallowClicks() {
    // Clicking a pane already asks for the keyboard, which is what makes "click the number you
    // can see" work with no second way to focus a pane. A badge that took the click would make
    // the numbers look pressable and not be, and only while they were on screen - a bug that
    // appears for a tenth of a second at a time and is gone before anybody can point at it.
    let chrome = pane()
    chrome.layoutSubtreeIfNeeded()
    chrome.apply(badge: 2)
    #expect(chrome.hitTest(NSPoint(x: 50, y: 50)) !== chrome.badgeView)
  }

  @Test("the number is sized off the pane, so a narrow split still draws a legible one")
  func theBadgeFitsThePane() {
    // A digit sized for one arrangement is a digit that overflows another. What matters is
    // that it stays a readable digit rather than a shape, at both ends of a real window.
    let wide = PaneAppearance.badgeSize(in: NSSize(width: 1600, height: 900))
    let sliver = PaneAppearance.badgeSize(in: NSSize(width: 1600, height: 90))
    #expect(sliver < wide, "a short pane should draw a smaller number than a tall one")
    #expect(sliver >= 56, "a number too small to find is worst in the window that needs it most")
    #expect(wide <= 264, "past a point a digit stops reading as a digit")
  }
}

@MainActor
private func pane(width: CGFloat = 100, height: CGFloat = 100) -> PaneChrome {
  let frame = NSRect(x: 0, y: 0, width: width, height: height)
  return PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
}
