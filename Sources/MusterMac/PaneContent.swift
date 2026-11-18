import AppKit

@MainActor
public protocol PaneContent: AnyObject {
  var view: NSView { get }
  var onClick: (@MainActor () -> Void)? { get set }
  var onMenu: (@MainActor () -> NSMenu?)? { get set }

  func focus(in window: NSWindow)
  func apply(windowIsKey: Bool)
  func apply(windowIsVisible: Bool)
  func apply(parked: Bool)
  func apply(covered: Bool)
  func matchBackingScale()
  func setFontSizeOffset(_ points: Int32) -> [String]
  func tearDown()
}

extension PaneContent {
  public func focus(in window: NSWindow) {
    if window.firstResponder !== view {
      window.makeFirstResponder(view)
    }
  }

  func routeKeyDown(
    _ event: NSEvent, wasComposing: Bool = false, committed: String? = nil,
    stillComposing: Bool = false
  ) -> Bool {
    Core.send(
      keyDown: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "press", isComposing: stillComposing),
      wasComposing: wasComposing, committed: committed, stillComposing: stillComposing)
  }

  func routeKeyUp(_ event: NSEvent, isComposing: Bool) {
    Core.send(
      keyUp: event.musterKeyEvent(
        action: event.isARepeat ? "repeated" : "release", isComposing: isComposing))
  }
}
