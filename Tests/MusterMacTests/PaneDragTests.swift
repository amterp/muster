import AppKit
import Testing

@testable import MusterMac

// Dragging a pane by its handle onto another pane's side.
//
// The rules are Ghostty's and live in `PaneDrop`, so most of this is arithmetic. What needs a
// view is that the handle is where a hand looks for it and takes the press there from the
// terminal, and that a drop becomes the one request a script can send too, naming both panes
// and the side.

@MainActor private let laptop = PaneKey(daemon: "laptop", pane: "p1")
@MainActor private let neighbor = PaneKey(daemon: "laptop", pane: "p2")
@MainActor private let devenv = PaneKey(daemon: "devenv", pane: "p9")

@Suite("dragging a pane", .ownsTheSeam)
@MainActor
struct PaneDragTests {
  /// The diagonals cut a pane into four triangles, corner to corner whatever its shape, so a
  /// point is nearest the edge whose triangle it is in.
  @Test("a drop goes to the side of the pane the pointer is nearest")
  func theFourTriangles() {
    let wide = CGSize(width: 400, height: 100)
    #expect(PaneDrop.zone(at: CGPoint(x: 10, y: 50), in: wide) == .left)
    #expect(PaneDrop.zone(at: CGPoint(x: 390, y: 50), in: wide) == .right)
    #expect(PaneDrop.zone(at: CGPoint(x: 200, y: 95), in: wide) == .up)
    #expect(PaneDrop.zone(at: CGPoint(x: 200, y: 5), in: wide) == .down)
    // A quarter of the way across and most of the way up: a share of each side rather than a
    // distance in points, so the top wins in a wide pane where points would say left.
    #expect(PaneDrop.zone(at: CGPoint(x: 100, y: 90), in: wide) == .up)

    let tall = CGSize(width: 100, height: 400)
    #expect(PaneDrop.zone(at: CGPoint(x: 10, y: 200), in: tall) == .left)
    #expect(PaneDrop.zone(at: CGPoint(x: 50, y: 390), in: tall) == .up)
  }

  @Test("a point equally near two sides goes left, right, up, down in that order")
  func ties() {
    let square = CGSize(width: 100, height: 100)
    #expect(PaneDrop.zone(at: CGPoint(x: 0, y: 100), in: square) == .left)
    #expect(PaneDrop.zone(at: CGPoint(x: 100, y: 100), in: square) == .right)
    #expect(PaneDrop.zone(at: CGPoint(x: 50, y: 50), in: square) == .left)
  }

  @Test("the overlay fills the half of the pane the dropped pane would take")
  func theOverlay() {
    let bounds = CGRect(x: 0, y: 0, width: 400, height: 200)
    #expect(PaneDrop.overlay(for: .left, in: bounds) == CGRect(x: 0, y: 0, width: 200, height: 200))
    #expect(
      PaneDrop.overlay(for: .right, in: bounds) == CGRect(x: 200, y: 0, width: 200, height: 200))
    #expect(PaneDrop.overlay(for: .up, in: bounds) == CGRect(x: 0, y: 100, width: 400, height: 100))
    #expect(PaneDrop.overlay(for: .down, in: bounds) == CGRect(x: 0, y: 0, width: 400, height: 100))
  }

  /// A pane is a process its daemon owns, and another window is another process: neither drop
  /// has a move behind it, so neither lights anything.
  @Test("a pane drops onto another pane on its machine, in its window, and nowhere else")
  func whatIsAccepted() {
    #expect(PaneDrop.accepts(laptop, onto: neighbor, fromThisWindow: true))
    #expect(!PaneDrop.accepts(laptop, onto: laptop, fromThisWindow: true))
    #expect(!PaneDrop.accepts(laptop, onto: devenv, fromThisWindow: true))
    #expect(!PaneDrop.accepts(laptop, onto: neighbor, fromThisWindow: false))
  }

  /// The agent list takes this payload already, so a pane dragged by its handle can land on a
  /// row or a tab caption there, and a row dragged out of the list can land on a pane.
  @Test("a dragged pane is carried as the agent list carries one")
  func thePayload() {
    #expect(PaneDrop.type == SidebarView.draggedPane)
    #expect(PaneDrop.pane(fromPayload: PaneDrop.payload(laptop)) == laptop)
    #expect(PaneDrop.pane(fromPayload: "laptop") == nil)
    #expect(PaneDrop.pane(fromPayload: "\tp1") == nil)
  }

  @Test("the handle sits at the top middle of the pane, as Ghostty's does")
  func whereTheHandleIs() {
    let chrome = chrome(width: 400, height: 300)
    #expect(chrome.grabHandle.frame == CGRect(x: 160, y: 288, width: 80, height: 12))
    #expect(chrome.grabHandle.isHidden == false)
  }

  /// The handle is the one place a drag in a pane is Muster's rather than the program's, so it
  /// has to win the press there and lose it everywhere else.
  @Test("a press on the handle is the handle's, and a point beside it is the terminal's")
  func theHandleTakesThePress() {
    let chrome = chrome(width: 400, height: 300)
    #expect(chrome.hitTest(NSPoint(x: 200, y: 294)) === chrome.grabHandle)
    #expect(chrome.hitTest(NSPoint(x: 150, y: 294)) === chrome.surface)
    #expect(chrome.hitTest(NSPoint(x: 200, y: 280)) === chrome.surface)
  }

  @Test("a pane with nothing behind it has no handle")
  func noPaneNoHandle() {
    let frame = NSRect(x: 0, y: 0, width: 400, height: 300)
    let bare = PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
    bare.layoutSubtreeIfNeeded()
    #expect(bare.grabHandle.isHidden)
  }

  @Test("the symbol shows across the top of the pane and not below it")
  func theBand() {
    let bounds = CGRect(x: 0, y: 0, width: 400, height: 300)
    #expect(PaneDrop.handleBand(in: bounds) == CGRect(x: 0, y: 240, width: 400, height: 60))
    // Never thinner than the handle, so a short pane still shows what it is grabbing.
    let short = CGRect(x: 0, y: 0, width: 400, height: 40)
    #expect(PaneDrop.handleBand(in: short).height == PaneDrop.handleSize.height)

    let handle = chrome(width: 400, height: 300).grabHandle
    #expect(!handle.iconShown)
    handle.pointerInBand = true
    #expect(handle.iconShown)
  }

  @Test("a press on the handle asks for the keyboard, as a click on the pane does")
  func aPressFocuses() {
    let chrome = chrome(width: 400, height: 300)
    var asked: [String] = []
    chrome.onFocusRequested = { asked.append($0) }
    chrome.grabHandle.mouseDown(with: mouse())
    #expect(asked == ["p2"])
  }

  @Test("a drop on a side asks the core to put the pane there, naming both and the side")
  func aDropIsOneRequest() {
    let recorder = recorder()
    let chrome = chrome(width: 400, height: 300)
    chrome.onPaneDropped = { dragged, side in
      Core.arrange(pane: dragged, onto: neighbor, side: side)
    }
    let mark = recorder.requests.count

    #expect(chrome.drop(laptop, at: NSPoint(x: 200, y: 10), fromThisWindow: true))

    let sent = recorder.sent(since: mark) {
      if case .arrangePane = $0.payload { true } else { false }
    }
    #expect(sent.count == 1)
    #expect(sent.first?.arrangePane.daemonID == "laptop")
    #expect(sent.first?.arrangePane.paneID == "p1")
    #expect(sent.first?.arrangePane.ontoPaneID == "p2")
    #expect(sent.first?.arrangePane.side == "down")
  }

  @Test("a drop the pane may not take sends nothing")
  func aRefusedDrop() {
    let recorder = recorder()
    let chrome = chrome(width: 400, height: 300)
    chrome.onPaneDropped = { dragged, side in
      Core.arrange(pane: dragged, onto: neighbor, side: side)
    }
    let mark = recorder.requests.count

    #expect(!chrome.drop(devenv, at: NSPoint(x: 200, y: 10), fromThisWindow: true))
    #expect(!chrome.drop(neighbor, at: NSPoint(x: 200, y: 10), fromThisWindow: true))
    #expect(!chrome.drop(laptop, at: NSPoint(x: 200, y: 10), fromThisWindow: false))

    #expect(
      recorder.sent(since: mark) {
        if case .arrangePane = $0.payload { true } else { false }
      }.isEmpty)
  }
}

/// `neighbor`'s chrome, laid out.
@MainActor
private func chrome(width: CGFloat, height: CGFloat) -> PaneChrome {
  let frame = NSRect(x: 0, y: 0, width: width, height: height)
  let chrome = PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
  chrome.attach(paneID: neighbor.pane)
  chrome.key = neighbor
  chrome.layoutSubtreeIfNeeded()
  return chrome
}

private func mouse() -> NSEvent {
  NSEvent.mouseEvent(
    with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
    context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}
