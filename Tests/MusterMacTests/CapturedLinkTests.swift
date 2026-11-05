import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

@Suite("web links while the program has the mouse", .ownsTheSeam)
@MainActor
struct CapturedLinkTests {
  private let link = OpenedLink(kind: .hyperlink, url: "https://example.com/muster")

  private func pane() -> (SurfaceView, RecordingSurface) {
    _ = recorder()
    let surface = RecordingSurface()
    surface.mouseCaptured = true
    surface.cellPixelSize = (20, 40)
    surface.linkAtPoint = { [link] point in point.x < 50 ? link : nil }
    let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    view.attach(surface, typeable: true)
    return (view, surface)
  }

  private func event(
    _ type: NSEvent.EventType, x: CGFloat = 10, flags: NSEvent.ModifierFlags = .command
  ) -> NSEvent {
    NSEvent.mouseEvent(
      with: type, location: NSPoint(x: x, y: 90), modifierFlags: flags, timestamp: 0,
      windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
  }

  @Test("Cmd and Cmd+Shift own both button events", arguments: [false, true])
  func aWebLinkOpensOnce(shift: Bool) {
    let (view, surface) = pane()
    var opened: [OpenedLink] = []
    var reports: [Core.Pointer] = []
    var focused = false
    view.onOpenLink = { opened.append($0) }
    view.onPointer = { reports.append($0) }
    view.onClick = { focused = true }
    let flags: NSEvent.ModifierFlags = shift ? [.command, .shift] : .command

    view.mouseDown(with: event(.leftMouseDown, flags: flags))
    view.mouseUp(with: event(.leftMouseUp, flags: flags))

    #expect(opened == [link])
    #expect(focused)
    #expect(reports.isEmpty)
    #expect(surface.buttons.isEmpty)
  }

  @Test("a drag starting on a link sends no unmatched motion or release")
  func aLinkDragIsSwallowed() {
    let (view, surface) = pane()
    var opened = false
    var reports: [Core.Pointer] = []
    view.onOpenLink = { _ in opened = true }
    view.onPointer = { reports.append($0) }

    view.mouseDown(with: event(.leftMouseDown))
    view.mouseDragged(with: event(.leftMouseDragged, x: 60))
    view.mouseUp(with: event(.leftMouseUp, x: 60))

    #expect(!opened)
    #expect(reports.isEmpty)
    #expect(surface.buttons.isEmpty)
    #expect(surface.positions == [NSPoint(x: 60, y: 10)])
  }

  @Test("a small movement within the pressed cell still opens the link")
  func clickJitterStillOpens() {
    let (view, _) = pane()
    var opened: [OpenedLink] = []
    view.onOpenLink = { opened.append($0) }
    view.mouseDown(with: event(.leftMouseDown))
    view.mouseDragged(with: event(.leftMouseDragged, x: 11))
    view.mouseUp(with: event(.leftMouseUp, x: 11))
    #expect(opened == [link])
  }

  @Test(
    "releasing Cmd first or ending capture still swallows the release", arguments: [false, true])
  func theReleaseRemainsOurs(endCapture: Bool) {
    let (view, surface) = pane()
    var reports: [Core.Pointer] = []
    view.onPointer = { reports.append($0) }
    view.mouseDown(with: event(.leftMouseDown))
    surface.mouseCaptured = !endCapture
    view.mouseUp(with: event(.leftMouseUp, flags: []))
    #expect(reports.isEmpty)
    #expect(surface.buttons.isEmpty)
  }

  @Test(
    "ordinary clicks and presses outside a link remain the program's", arguments: [false, true])
  func anApplicationPressKeepsItsRelease(outside: Bool) {
    let (view, surface) = pane()
    var reports: [Core.Pointer] = []
    var opened = false
    view.onPointer = { reports.append($0) }
    view.onOpenLink = { _ in opened = true }

    view.mouseDown(
      with: event(.leftMouseDown, x: outside ? 70 : 10, flags: outside ? .command : []))
    view.mouseUp(with: event(.leftMouseUp))

    #expect(!opened)
    #expect(surface.buttons.count == 2)
    #expect(reports.count == 2)
  }

  @Test("a delayed renderer report cannot hide the current web link")
  func hoverOverridesRendererReports() {
    let (view, surface) = pane()
    var hovered: String?
    view.onHoverLink = { hovered = $0 }
    view.mouseMoved(with: event(.mouseMoved))
    surface.onPointerShape?(.text)
    surface.onHoverLink?(nil)
    #expect(view.cursor == .pointingHand)
    #expect(hovered == link.url)

    surface.mouseCaptured = false
    view.mouseMoved(with: event(.mouseMoved, x: 70))
    #expect(view.cursor == .iBeam)
    #expect(hovered == nil)
  }

  @Test("surface replacement clears the hover and remembered press")
  func replacementClearsState() {
    let (view, _) = pane()
    view.mouseDown(with: event(.leftMouseDown))
    view.attach(RecordingSurface(), typeable: true)
    #expect(view.cursor == .iBeam)
  }

  @Test("captured non-web links do not acquire the host's click")
  func otherSchemesStayWithTheProgram() {
    let (view, surface) = pane()
    surface.linkAtPoint = { _ in OpenedLink(kind: .hyperlink, url: "file:///tmp/example") }
    view.mouseDown(with: event(.leftMouseDown))
    view.mouseUp(with: event(.leftMouseUp))
    #expect(surface.buttons.count == 2)
  }
}
