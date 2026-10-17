import AppKit
import Testing

@testable import MusterMac

// libghostty measures a surface's cells in the scale it was made with until it is told another.
// A surface made on a laptop's 2x display and then sized on a 1x one was given 1x pixels to fill
// with 2x cells, so its pane ran at half the columns and half the rows it had room for.
//
// No test can move a window between two real screens, so these stand a window in for one whose
// scale they set, and call what AppKit calls when a window's scale moves.

@Suite("a pane follows its window's scale from one display to another")
struct SurfaceScaleTests {
  @MainActor
  @Test("a pane made at 2x and moved to a 1x display is told the new scale and sized in its pixels")
  func aBackingChangeRescalesThePane() {
    let window = ScaledWindow(scale: 2)
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 50))
    window.contentView?.addSubview(view)
    let surface = RecordingSurface()
    view.attach(surface, typeable: true)
    #expect(surface.contentScales.last == 2)
    #expect(surface.sizes.last.map { [$0.width, $0.height] } == [200, 100])

    window.scale = 1
    view.viewDidChangeBackingProperties()
    #expect(surface.contentScales.last == 1, "the surface kept the scale it was made with")
    #expect(
      surface.sizes.last.map { [$0.width, $0.height] } == [100, 50],
      "the surface was not sized in the new display's pixels")
    #expect(view.layer?.contentsScale == 1)
  }

  @MainActor
  @Test("a window changing screen reaches every pane it holds, parked ones included")
  func aScreenChangeReachesParkedPanes() {
    let window = ScaledWindow(scale: 2)
    let store = PaneSurfaces(parkedIn: window.contentView!, startPane: { _, _, _ in })
    let (chrome, _) = store.borrow(
      daemonID: "local", daemonSocket: "/tmp/local.sock",
      leaf: .init(paneID: "p1", linkSocketPath: "/tmp/link.sock", bridgeRestarts: 0),
      focus: { _ in }, pointer: { _, _ in })
    let surface = RecordingSurface()
    chrome.surface.attach(surface, typeable: true)
    store.park(everythingBut: [])

    window.scale = 1
    store.matchBackingScale()
    #expect(surface.contentScales.last == 1, "a parked pane kept the old display's scale")
  }
}

/// A window whose backing scale a test sets, standing in for one moved between displays.
@MainActor
private final class ScaledWindow: NSWindow {
  var scale: CGFloat

  init(scale: CGFloat) {
    self.scale = scale
    super.init(
      contentRect: NSRect(x: 0, y: 0, width: 200, height: 100), styleMask: [.titled],
      backing: .buffered, defer: true)
  }

  override var backingScaleFactor: CGFloat { scale }
}
