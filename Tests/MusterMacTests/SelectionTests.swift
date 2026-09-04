import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// Keeping a selection on the text it was made on.
//
// A pane is scrolled somewhere else. The daemon holds the history and answers a scroll by
// repainting the screen in place, so the surface's own buffer never moves and libghostty's
// selection - which is pinned to rows of that buffer - stays where it was drawn while the text
// under it is rewritten (`observations/libghostty-9f9b8d1d.md` section 12). So Muster remembers
// where a selection is in the pane and asks for it again wherever the pane has travelled to.
//
// What is assertable is the arithmetic and the decisions: which cells were asked for, and when
// the answer is that there is nothing to draw. Whether libghostty then paints the right cells
// is a claim about a renderer that needs a GPU and a window, and is out of reach here on the
// same terms as marking a find.

/// A view whose grid is ten columns of five rows, so a cell is 10 by 20 points.
///
/// The numbers are chosen to make the assertions readable rather than to resemble a font: a
/// cell's centre lands on a whole number in both directions.
@MainActor
private func pane(_ surface: RecordingSurface) -> SurfaceView {
  Core.dispatcher = RecordingDispatcher()
  surface.cellPixelSize = (width: 20, height: 40)
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.attach(surface, typeable: true)
  return view
}

@MainActor
private func dragged(_ view: SurfaceView, from: NSPoint, to: NSPoint) {
  view.mouseDown(with: mouse(.leftMouseDown, at: from))
  view.mouseDragged(with: mouse(.leftMouseDragged, at: to))
  view.mouseUp(with: mouse(.leftMouseUp, at: to))
}

private func mouse(_ type: NSEvent.EventType, at point: NSPoint) -> NSEvent {
  NSEvent.mouseEvent(
    with: type, location: point, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
    eventNumber: 0, clickCount: 1, pressure: 1)!
}

private func looking(at rowsFromBottom: UInt32) -> Core.Viewport {
  Core.Viewport(rowsFromBottom: rowsFromBottom, rows: 5, deepest: 100)
}

@Suite("selection follows its text")
struct SelectionTests {
  @MainActor
  @Test("a drag asks where the pane is looking, so its cells can be counted from the bottom")
  func aDragAsksForTheViewport() {
    let surface = RecordingSurface()
    let view = pane(surface)
    var asked = 0
    view.onSelectionMade = { asked += 1 }

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))

    #expect(asked == 1)
    #expect(view.isTrackingSelection)
  }

  @MainActor
  @Test("a click that never drags pins nothing")
  func aClickPinsNothing() {
    // libghostty may still have selected something - a double click takes the word under it -
    // and there is no way to read back which cells that covered. Pinning the one cell that was
    // clicked would replace a word selection with a single cell on the first scroll.
    let surface = RecordingSurface()
    let view = pane(surface)
    var asked = 0
    view.onSelectionMade = { asked += 1 }

    view.mouseDown(with: mouse(.leftMouseDown, at: NSPoint(x: 10, y: 90)))
    view.mouseUp(with: mouse(.leftMouseUp, at: NSPoint(x: 10, y: 90)))

    #expect(asked == 0)
    #expect(!view.isTrackingSelection)
  }

  @MainActor
  @Test("a scrolled pane is asked to select the cells the text moved to")
  func aScrollMovesTheSelection() {
    // The bug, in one assertion. The drag covers the top row down to the fourth; scrolling the
    // pane up two rows moves that text two rows down the screen, and the selection has to be
    // asked for there rather than left where it was drawn.
    let surface = RecordingSurface()
    let view = pane(surface)

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.applyViewport(looking(at: 0))
    #expect(surface.selections.isEmpty, "pinning a selection should not redraw it")

    view.applyViewport(looking(at: 2))

    // Column 1 of the top row, which was 4 rows above the bottom, is now 2 rows above it - the
    // middle of a 10 by 20 cell in the third row down. The other end has gone off the bottom
    // and is asked for past it, which a renderer clamps to the last row.
    #expect(surface.selections.count == 1)
    #expect(surface.selections.last??.from == CGPoint(x: 15, y: 50))
    #expect(surface.selections.last??.to == CGPoint(x: 55, y: 110))
  }

  @MainActor
  @Test("a selection scrolled off the screen comes off, and comes back")
  func aSelectionOffScreenComesOffAndBack() {
    // Taken off rather than clamped to an edge, because a highlight pressed against the top of
    // a screen it is not on is the same lie in a smaller font. The pin survives, so scrolling
    // back puts it on the same cells.
    let surface = RecordingSurface()
    let view = pane(surface)

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.applyViewport(looking(at: 0))
    view.applyViewport(looking(at: 20))

    #expect(surface.selections.last == .some(nil), "a selection nowhere on screen was drawn")

    view.applyViewport(looking(at: 0))

    #expect(surface.selections.last??.from == CGPoint(x: 15, y: 10))
    #expect(surface.selections.last??.to == CGPoint(x: 55, y: 70))
  }

  @MainActor
  @Test("a pane scrolled while the answer was in flight forgets the selection")
  func aScrollDuringTheRoundTripForgetsIt() {
    // The one thing that can pin a selection to the wrong rows: the viewport that arrives has
    // to be the one in force when the drag ended. Forgetting it and taking it off screen is
    // the honest answer - a selection nobody can place is better gone than left lying.
    let surface = RecordingSurface()
    let view = pane(surface)

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.applyViewport(looking(at: 3), movedSince: true)

    #expect(surface.selections.last == .some(nil))
    #expect(!view.isTrackingSelection)
  }

  @MainActor
  @Test("a core that will not say where the pane is looking takes the selection off")
  func anUnansweredViewportTakesItOff() {
    let surface = RecordingSurface()
    let view = pane(surface)

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.applyViewport(looking(at: 0))
    view.applyViewport(nil)

    #expect(surface.selections.last == .some(nil))
  }

  @MainActor
  @Test("a new press drops what was pinned, because it is about to be replaced")
  func aNewPressDropsThePin() {
    let surface = RecordingSurface()
    let view = pane(surface)

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.applyViewport(looking(at: 0))
    view.mouseDown(with: mouse(.leftMouseDown, at: NSPoint(x: 10, y: 90)))

    #expect(!view.isTrackingSelection)
  }
}

@Suite("a scrolled pane asks where it is looking")
struct SelectionViewportTests {
  // Which pane a selection belongs to is the chrome's to know, so the round trip is the
  // chrome's to make. What is worth pinning is when it makes one: a wheel over a pane with
  // nothing selected has to cost exactly what it always cost.

  @MainActor
  @Test("a scroll over a pane with a selection asks, and one without does not")
  func onlyASelectionCostsARoundTrip() async {
    let core = RecordingDispatcher()
    Core.dispatcher = core
    let surface = RecordingSurface()
    surface.cellPixelSize = (width: 20, height: 40)
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    view.attach(surface, typeable: true)
    let chrome = PaneChrome(
      frame: NSRect(x: 0, y: 0, width: 100, height: 100), surface: view, dispatcher: core)
    chrome.attach(paneID: "p1")

    guard let notch = wheel(deltaY: 3) else { return }
    view.scrollWheel(with: notch)
    #expect(viewportReads(core).isEmpty, "an ordinary scroll asked the core where a pane was")

    dragged(view, from: NSPoint(x: 10, y: 90), to: NSPoint(x: 50, y: 40))
    view.scrollWheel(with: notch)

    await until("the chrome to ask where its pane is looking") { !viewportReads(core).isEmpty }
    #expect(viewportReads(core).allSatisfy { $0 == "p1" }, "the read named another pane")
  }

  private func viewportReads(_ core: RecordingDispatcher) -> [String] {
    core.requests.compactMap {
      if case .readViewport(let read) = $0.payload { read.paneID } else { nil }
    }
  }
}
