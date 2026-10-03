import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

// The pointer over a pane, and the link under it.
//
// libghostty decides both - it knows which cell is a link and whether the modifier that opens one
// is held - and reports them to the surface. What is Muster's is turning that into a cursor and a
// line of text, which is what says a link can be clicked before anybody clicks it.

@Suite("hovering a link", .ownsTheSeam)
@MainActor
struct LinkHoverTests {
  @Test("each shape gets the cursor Ghostty draws for it, and a link gets the hand")
  func theCursors() {
    #expect(SurfaceView.cursor(for: .link) == .pointingHand)
    #expect(SurfaceView.cursor(for: .text) == .iBeam)
    #expect(SurfaceView.cursor(for: .arrow) == .arrow)
    #expect(SurfaceView.cursor(for: .grabbing) == .closedHand)
  }

  @Test("the pane's cursor is the shape its surface last asked for")
  func theCursorFollowsTheSurface() {
    _ = recorder()
    let recording = RecordingSurface()
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    view.attach(recording, typeable: true)
    #expect(view.cursor == .iBeam, "a pane starts with the cursor for text")

    recording.onPointerShape?(.link)
    #expect(view.cursor == .pointingHand)

    recording.onPointerShape?(.text)
    #expect(view.cursor == .iBeam)
  }

  @Test("the pointer hides when the surface asks, and comes back when it asks again")
  func thePointerHidesWhileTyping() {
    _ = recorder()
    let recording = RecordingSurface()
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    view.attach(recording, typeable: true)
    #expect(!view.pointerHidden)

    recording.onPointerVisibility?(false)
    #expect(view.pointerHidden)

    recording.onPointerVisibility?(true)
    #expect(!view.pointerHidden)
  }

  @Test("the link under the pointer shows at the pane's bottom left, and goes when it leaves")
  func theBanner() {
    _ = recorder()
    let recording = RecordingSurface()
    let frame = NSRect(x: 0, y: 0, width: 400, height: 300)
    let chrome = PaneChrome(frame: frame, surface: SurfaceView(frame: frame))
    chrome.surface.attach(recording, typeable: true)
    #expect(chrome.shownLink == nil)

    recording.onHoverLink?("https://example.com/a")
    chrome.layoutSubtreeIfNeeded()
    #expect(chrome.shownLink == "https://example.com/a")
    // Over the terminal, and never in the way of a click on it.
    #expect(chrome.hitTest(NSPoint(x: 12, y: 10)) === chrome.surface)

    recording.onHoverLink?(nil)
    #expect(chrome.shownLink == nil)
  }
}
