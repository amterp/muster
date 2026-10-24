import AppKit
import Testing

@testable import MusterMac

// What the tab view does with a view it is handed. No GPU, no libghostty, no daemon: the surface
// factory is a closure and this injects one that records rather than one that allocates.
//
// The property under test throughout is that identity is the pane. The core publishes the whole
// arrangement every time, so a view that rebuilt what it was handed would tear down and respawn
// every surface and bridge in the window each time any pane in it changed - which is invisible in
// a screenshot and obvious the moment an agent's output flickers away.

/// Records the panes the view asked to have stood up, and what socket each was pointed at.
@MainActor
private final class Started {
  var panes: [String] = []
  /// Which daemon socket each pane's bridge was to dial, in the order they were started.
  var daemonSockets: [String?] = []
  /// Whether each pane's daemon was on another machine.
  var remote: [Bool] = []
}

/// A window's worth of surfaces, recorded rather than allocated.
@MainActor
private func surfaces(_ started: Started) -> PaneSurfaces {
  paneSurfaces { machine, chrome, pane in
    started.panes.append(
      "\(pane.daemon):\(chrome.paneID ?? "")@\(pane.linkSocketPath ?? "-")")
    started.daemonSockets.append(machine?.socket)
    started.remote.append(machine?.remote ?? false)
  }
}

/// A tab view whose surfaces are recorded rather than allocated.
@MainActor
private func tabView(width: CGFloat = 800, height: CGFloat = 600) -> (TabView, Started) {
  let (view, started, _) = tabViewAndStore(width: width, height: height)
  return (view, started)
}

@MainActor
private func tabViewAndStore(
  width: CGFloat = 800, height: CGFloat = 600
) -> (TabView, Started, PaneSurfaces) {
  let started = Started()
  let store = surfaces(started)
  return (
    TabView(frame: NSRect(x: 0, y: 0, width: width, height: height), surfaces: store),
    started, store
  )
}

/// A view, applied the way the window applies one: what it stops showing is parked first.
@MainActor
private func show(_ view: TabView, _ described: WindowContents, in store: PaneSurfaces) {
  store.park(everythingBut: described.panes ?? view.onScreen)
  view.apply(described)
}

private func local(_ pane: String) -> PaneKey { PaneKey(daemon: "local", pane: pane) }

private func leaf(
  _ id: String, on daemon: String = "local", socket: String? = "/tmp/\(0).sock",
  restarts: UInt32 = 0
) -> PaneTree {
  .pane(.init(daemon: daemon, paneID: id, linkSocketPath: socket, bridgeRestarts: restarts))
}

private func contents(
  _ tree: PaneTree?, keyboard: String? = nil, daemonSocket: String? = "/tmp/muster/daemon.sock",
  remote: Bool = false
) -> WindowContents {
  WindowContents(
    tab: "w1:t1", tree: tree, keyboard: keyboard.map(local),
    machines: ["local": .init(socket: daemonSocket, remote: remote)])
}

@Suite("a tab view renders a tree", .ownsTheSeam)
struct TabViewTests {
  @MainActor
  @Test("a pane that survives a change keeps its surface")
  func survivingPanesAreNotRebuilt() {
    // The whole reason this class diffs rather than rebuilds. A split adds one pane; the
    // other one must not blink.
    let (view, started) = tabView()
    view.apply(contents(leaf("w1:p1", socket: "/tmp/a.sock")))
    let before = view.chrome(for: local("w1:p1"))

    view.apply(
      contents(
        .split(
          axis: .columns, ratio: 0.5,
          first: leaf("w1:p1", socket: "/tmp/a.sock"),
          second: leaf("w1:p2", socket: "/tmp/b.sock"))))

    #expect(view.chrome(for: local("w1:p1")) === before)
    #expect(started.panes == ["local:w1:p1@/tmp/a.sock", "local:w1:p2@/tmp/b.sock"])
  }

  @MainActor
  @Test("a pane the tree no longer names leaves the tab, and keeps its bridge")
  func departedPanesAreParked() {
    // Parked rather than torn down, which is the whole of a_2HzbwzO32. A remote pane's bridge
    // is an ssh exec, and the far machine takes about 440ms to open a session for one - so a
    // surface thrown away on every tab switch is half a second of blank pane on every switch
    // back. What replaces the teardown is the roster: a pane stops costing anything when its
    // daemon stops holding it.
    let (view, _, store) = tabViewAndStore()
    show(
      view, contents(.split(axis: .rows, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))),
      in: store)

    show(view, contents(leaf("w1:p1")), in: store)

    #expect(view.chrome(for: local("w1:p2")) == nil)
    #expect(view.panes.map(\.pane) == ["w1:p1"])
    #expect(view.subviews.compactMap { ($0 as? PaneChrome)?.paneID } == ["w1:p1"])
    #expect(store.parked == [PaneKey(daemon: "local", pane: "w1:p2")])
  }

  @MainActor
  @Test("a pane that comes back on screen starts no second bridge")
  func aParkedPaneIsReused() {
    // The measurement the card asked for, at the level a test can reach it: switching to a
    // tab and back used to spawn a bridge each time, and the log held seventeen bridge.start
    // records for three devenv panes in one session.
    let (view, started, store) = tabViewAndStore()
    let both = PaneTree.split(
      axis: .rows, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))
    show(view, contents(both), in: store)
    let parked = store.chrome(for: PaneKey(daemon: "local", pane: "w1:p2"))

    show(view, contents(leaf("w1:p1")), in: store)
    show(view, contents(both), in: store)

    #expect(started.panes.count == 2)
    #expect(view.chrome(for: local("w1:p2")) === parked)
  }

  @MainActor
  @Test("two views showing one pane share its surface")
  func onePanePerWindowHasOneSurface() {
    // A daemon lets one bridge draw a pane, so a pane drawn twice is one surface printing a
    // refusal and a panel nobody can close (kan a_2Ht74jTXV). The core
    // no longer opens two regions onto one tab; a store keyed by pane cannot draw one twice
    // even if it does again.
    let started = Started()
    let store = surfaces(started)
    let first = TabView(frame: NSRect(x: 0, y: 0, width: 800, height: 600), surfaces: store)
    let second = TabView(frame: NSRect(x: 0, y: 0, width: 800, height: 600), surfaces: store)

    first.apply(contents(leaf("w1:p1")))
    second.apply(contents(leaf("w1:p1")))

    #expect(started.panes == ["local:w1:p1@/tmp/0.sock"])
    #expect(store.count == 1)
    // And the second one is the one showing it, because it took it last.
    #expect(
      second.chrome(for: local("w1:p1"))
        === store.chrome(for: PaneKey(daemon: "local", pane: "w1:p1")))
  }

  @MainActor
  @Test("a parked pane the daemons no longer hold is let go")
  func closedPanesAreReleased() {
    // What stops the held set growing with switches. A window that visited fifteen tabs would
    // otherwise hold fifteen tabs' worth of bridges and sockets until it quit.
    let (view, _, store) = tabViewAndStore()
    show(
      view, contents(.split(axis: .rows, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))),
      in: store)
    show(view, contents(leaf("w1:p1")), in: store)

    store.release(everythingBut: [PaneKey(daemon: "local", pane: "w1:p1")])

    #expect(store.count == 1)
    #expect(store.parked.isEmpty)
  }

  @MainActor
  @Test("a pane parked after the roster that left it out is let go")
  func aPaneParkedAfterItsLastRosterIsReleased() {
    // The order a window meets when it skips a view that has a newer one behind it: the roster
    // published with the skipped view arrives while the pane is still on screen, and the view
    // that parks it comes after. No later roster need follow, so this park is the last chance
    // to let the pane go before it holds its surface and threads for as long as the window is
    // quiet.
    let (view, _, store) = tabViewAndStore()
    show(
      view, contents(.split(axis: .rows, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))),
      in: store)
    store.release(everythingBut: [PaneKey(daemon: "local", pane: "w1:p1")])

    show(view, contents(leaf("w1:p1")), in: store)

    #expect(store.parked.isEmpty)
    #expect(store.count == 1)
  }

  @MainActor
  @Test("a pane on screen survives a roster that does not name it")
  func onScreenPanesAreNeverReleased() {
    // A roster arrives empty for reasons that are not "every pane closed" - a daemon
    // mid-reconnect, a publish between arrangements - and tearing the window down for one
    // would be the worst answer available to a transient.
    let (view, _, store) = tabViewAndStore()
    show(view, contents(leaf("w1:p1")), in: store)

    store.release(everythingBut: [])

    #expect(view.chrome(for: local("w1:p1")) != nil)
    #expect(store.count == 1)
  }

  @MainActor
  @Test("a pane whose socket moved gets a new bridge")
  func aChangedSocketRebuildsTheSurface() {
    // A bridge is spawned by its surface's command, so a pane whose socket changed is a pane
    // dialing a listener that is gone. It would keep painting and swallow every keystroke,
    // which is the failure that has cost this project the most time.
    let (view, started) = tabView()
    view.apply(contents(leaf("w1:p1", socket: nil)))

    view.apply(contents(leaf("w1:p1", socket: "/tmp/opened.sock")))

    #expect(started.panes == ["local:w1:p1@-", "local:w1:p1@/tmp/opened.sock"])
  }

  @MainActor
  @Test("a pane whose bridge the core replaced gets a new surface")
  func aReplacedBridgeRebuildsTheSurface() {
    // The other half of kan a_2HrmSyRAQ. A bridge is a surface's command, so the core saying
    // "this pane's bridge is gone" can only be acted on by building the surface again - and
    // this pane's socket has not moved, so nothing else here would notice.
    let (view, started, store) = tabViewAndStore()
    show(view, contents(leaf("w1:p1", socket: "/tmp/a.sock")), in: store)
    let before = view.chrome(for: local("w1:p1"))

    show(view, contents(leaf("w1:p1", socket: "/tmp/a.sock", restarts: 1)), in: store)

    #expect(started.panes == ["local:w1:p1@/tmp/a.sock", "local:w1:p1@/tmp/a.sock"])
    #expect(view.chrome(for: local("w1:p1")) !== before)
    #expect(store.count == 1, "the surface it replaced is still being held somewhere")
  }

  @MainActor
  @Test("a tree that has not arrived leaves what is on screen alone")
  func anAbsentTreeChangesNothing() {
    // A tab's tree can arrive after the panes it names, so this is an ordinary moment rather
    // than a failure. Tearing surfaces down for it is a flicker on every split.
    let (view, started) = tabView()
    view.apply(contents(leaf("w1:p1")))

    view.apply(contents(nil))

    #expect(view.panes.map(\.pane) == ["w1:p1"])
    #expect(started.panes.count == 1)
  }

  @MainActor
  @Test("only the pane with the keyboard is marked as having it")
  func focusFollowsTheView() {
    let (view, _) = tabView()
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))

    view.apply(contents(tree, keyboard: "w1:p2"))

    #expect(view.chrome(for: local("w1:p2"))?.isFocused == true)
    #expect(view.chrome(for: local("w1:p1"))?.isFocused == false)
  }

  @MainActor
  @Test("a tab whose keyboard is on no pane marks none of them")
  func noKeyboardMarksNoPane() {
    // A focus ring on a pane nothing is typing into is a "type here" that is lying.
    let (view, _) = tabView()

    view.apply(contents(leaf("w1:p1"), keyboard: nil))

    #expect(view.chrome(for: local("w1:p1"))?.isFocused == false)
  }

  @MainActor
  @Test("a tree that has not arrived for another tab shows nothing of the last one")
  func anAbsentTreeForANewTabClearsTheView() {
    // Leaving the last tab's panes up would draw one tab's panes under another tab's name, and
    // feed the keyboard into a pane the sidebar says is somewhere else.
    let (view, _, store) = tabViewAndStore()
    show(view, contents(leaf("w1:p1")), in: store)

    show(view, WindowContents(tab: "w1:t2", tree: nil, keyboard: nil), in: store)

    #expect(view.panes.isEmpty)
    #expect(view.chrome(for: local("w1:p1")) == nil)
  }

  @MainActor
  @Test("each pane in a tab spanning machines is started on its own machine")
  func eachLeafIsStartedOnItsMachine() {
    // One tree, two machines: the leaf says which machine runs its pane, and the bridge has to
    // dial that machine's daemon, or a devenv pane is drawn from the laptop's.
    let (view, started) = tabView()
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5, first: leaf("w1:p1", socket: "/tmp/a.sock"),
      second: leaf("w1:p2", on: "devenv", socket: "/tmp/b.sock"))

    view.apply(
      WindowContents(
        tab: "w1:t1", tree: tree, keyboard: PaneKey(daemon: "devenv", pane: "w1:p2"),
        machines: [
          "local": .init(socket: "/tmp/muster/daemon.sock"),
          "devenv": .init(socket: "/tmp/muster/devenv.sock", remote: true),
        ]))

    #expect(started.panes == ["local:w1:p1@/tmp/a.sock", "devenv:w1:p2@/tmp/b.sock"])
    #expect(started.daemonSockets == ["/tmp/muster/daemon.sock", "/tmp/muster/devenv.sock"])
    #expect(started.remote == [false, true])
    #expect(view.chrome(for: PaneKey(daemon: "devenv", pane: "w1:p2"))?.isFocused == true)
  }

  @MainActor
  @Test("every pane and divider is laid out inside the tab")
  func layoutFillsTheTab() {
    let (view, _) = tabView(width: 900, height: 500)
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5,
      first: leaf("w1:p1"),
      second: .split(axis: .rows, ratio: 0.5, first: leaf("w1:p2"), second: leaf("w1:p3")))

    view.apply(contents(tree))
    view.layoutSubtreeIfNeeded()

    let chromes = view.subviews.compactMap { $0 as? PaneChrome }
    #expect(chromes.count == 3)
    #expect(chromes.allSatisfy { view.bounds.contains($0.frame) })
    #expect(chromes.allSatisfy { $0.frame.width > 0 && $0.frame.height > 0 })
    // Three panes need two dividers, and every one of them is a grab handle: a missing one
    // is a split nobody can resize.
    #expect(view.subviews.filter { $0 is DividerView }.count == 2)
  }

  @MainActor
  @Test("the wheel moves the pane it is over, not the one with the keyboard")
  func aWheelMovesThePaneUnderIt() {
    // The card's whole case: reading one agent's output while typing into another is the
    // ordinary thing to do in a window of fifteen, and a wheel that moved the focused pane
    // made it impossible without clicking away from what you were typing in first.
    let recorder = recorder()
    let (view, _) = tabView()
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))
    view.apply(contents(tree, keyboard: "w1:p1"))
    // The real path does this when the pane's bridge starts; here the surface factory only
    // records, so nothing else would make the view willing to report input.
    guard let scrolled = view.chrome(for: local("w1:p2")) else { return }
    scrolled.surface.attach(typeable: true)
    let mark = recorder.requests.count
    guard let event = wheel(deltaY: 3) else { return }

    scrolled.surface.scrollWheel(with: event)

    let wheels = recorder.sent(since: mark) {
      if case .wheel = $0.payload { true } else { false }
    }
    #expect(wheels.map { $0.wheel.paneID } == ["w1:p2"])
    #expect(wheels.map { $0.wheel.daemonID } == ["local"])
    #expect(wheels.map { $0.wheel.dy } == [3])
    // And the keyboard stayed where it was: nothing asked to focus the pane it moved over.
    let focuses = recorder.sent(since: mark) {
      if case .focusPane = $0.payload { true } else { false }
    }
    #expect(focuses.isEmpty)
  }

  @MainActor
  @Test("a click reaches the daemon of the pane it landed on")
  func aClickReachesThePaneUnderIt() {
    // The same path the wheel takes, so a program that asked for the mouse in a pane without
    // the keyboard is told about a click in it - which is also the click that moves the
    // keyboard there.
    let recorder = recorder()
    let (view, _) = tabView()
    let tree = PaneTree.split(
      axis: .columns, ratio: 0.5, first: leaf("w1:p1"), second: leaf("w1:p2"))
    view.apply(contents(tree, keyboard: "w1:p1"))
    guard let clicked = view.chrome(for: local("w1:p2")) else { return }
    clicked.surface.attach(typeable: true)
    let mark = recorder.requests.count

    clicked.surface.mouseDown(
      with: NSEvent.mouseEvent(
        with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)

    let mice = recorder.sent(since: mark) {
      if case .mouse = $0.payload { true } else { false }
    }
    #expect(mice.map { $0.mouse.paneID } == ["w1:p2"])
    #expect(mice.map { $0.mouse.daemonID } == ["local"])
    #expect(mice.map { $0.mouse.action } == ["press"])
  }
}

@Suite("a tab view says which daemon its panes come from")
struct TabFrameSourceTests {
  @MainActor
  @Test("the daemon's socket reaches the pane that is about to be started")
  func theDaemonSocketReachesTheBridge() {
    // A bridge has no way to find a daemon for itself, so dropping this value between the view
    // and the bridge's command line is invisible until a pane renders nothing - which is
    // exactly how it was found.
    let (view, started) = tabView()

    view.apply(contents(leaf("w1:p1"), daemonSocket: "/tmp/muster/daemon.sock"))

    #expect(started.daemonSockets == ["/tmp/muster/daemon.sock"])
    #expect(started.remote == [false])
  }

  @MainActor
  @Test("a remote machine says so, beside the near end of its forward")
  func aRemoteMachineSaysSo() {
    let (view, started) = tabView()

    view.apply(
      contents(leaf("w1:p1"), daemonSocket: "/tmp/muster/devenv.sock", remote: true))

    #expect(started.daemonSockets == ["/tmp/muster/devenv.sock"])
    #expect(started.remote == [true])
  }

  @MainActor
  @Test("a pane started before its daemon had a socket is started again once it has one")
  func aDaemonSocketArrivingRebuildsTheSurface() {
    // The window gives a pane no bridge while its daemon has no socket, because a bridge with
    // nothing to dial renders nothing. The pane's own sockets have not moved when the daemon's
    // arrives, so without this the pane would stay blank until something else about it changed.
    let (view, started, store) = tabViewAndStore()
    show(view, contents(leaf("w1:p1"), daemonSocket: nil), in: store)

    show(view, contents(leaf("w1:p1"), daemonSocket: "/tmp/muster/daemon.sock"), in: store)

    #expect(started.daemonSockets == [nil, "/tmp/muster/daemon.sock"])
    #expect(store.count == 1)
  }

  @MainActor
  @Test("a daemon's socket going away does not take its panes down")
  func aDaemonSocketLeavingKeepsTheSurface() {
    // A surface holds the pane's history, and its bridge's connection outlives whatever made the
    // view briefly name no socket. The core replaces a bridge that actually died.
    let (view, started, store) = tabViewAndStore()
    show(view, contents(leaf("w1:p1")), in: store)
    let before = view.chrome(for: local("w1:p1"))

    show(view, contents(leaf("w1:p1"), daemonSocket: nil), in: store)

    #expect(started.panes.count == 1)
    #expect(view.chrome(for: local("w1:p1")) === before)
  }
}
