import AppKit
import CoreGraphics
import Testing

@testable import MusterMac

// The mouse's back and forward buttons, and the request they and the menu items send.
//
// Where the history goes is the core's to decide and its suite's to pin. What is the shell's is
// that the two buttons are recognized wherever they land in the window, that no other button is
// mistaken for them, and that what they ask for is the request the chord and the CLI send.

@Suite("the focus history", .ownsTheSeam)
@MainActor
struct FocusHistoryTests {
  @Test("the back and forward buttons are AppKit's buttons 3 and 4, pressed or released")
  func theTwoButtons() {
    #expect(KeyboardWindow.historyDirection(for: button(3, .otherMouseDown)) == false)
    #expect(KeyboardWindow.historyDirection(for: button(4, .otherMouseDown)) == true)
    #expect(KeyboardWindow.historyDirection(for: button(3, .otherMouseUp)) == false)
    #expect(KeyboardWindow.historyDirection(for: button(2, .otherMouseDown)) == nil)
    #expect(KeyboardWindow.historyDirection(for: button(0, .leftMouseDown)) == nil)
  }

  /// The window rather than a view, so the buttons work over the agent list and the dividers
  /// as well as over a pane. The release is swallowed and asks for nothing, so one click is
  /// one step.
  @Test("the window walks the history on the press, and only on the press")
  func theWindowTakesThem() {
    let window = KeyboardWindow(
      contentRect: NSRect(x: 0, y: 0, width: 100, height: 100), styleMask: [.titled],
      backing: .buffered, defer: true)
    var walked: [Bool] = []
    window.onHistoryButton = { walked.append($0) }

    window.sendEvent(button(3, .otherMouseDown))
    window.sendEvent(button(3, .otherMouseUp))
    window.sendEvent(button(4, .otherMouseDown))
    window.sendEvent(button(4, .otherMouseUp))

    #expect(walked == [false, true])
  }

  @Test("going back and forward asks the core, which keeps the history")
  func theRequest() {
    let recorder = recorder()
    let mark = recorder.requests.count

    Core.focusHistory(forward: false)
    Core.focusHistory(forward: true)

    let sent = recorder.sent(since: mark) {
      if case .focusHistory = $0.payload { true } else { false }
    }
    #expect(sent.map(\.focusHistory.forward) == [false, true])
  }

  @Test("the menu items and the shortcuts list carry both directions")
  func theMenuItems() {
    #expect(MenuActions.byName["focus_back"]?.title == "Go Back")
    #expect(MenuActions.byName["focus_forward"]?.title == "Go Forward")
    #expect(Shortcuts.pointing.contains { $0.note == "the mouse's back and forward buttons" })
  }
}

/// A press or release of one mouse button, built through Core Graphics because
/// `NSEvent.mouseEvent` has no way to say which of the other buttons it was.
private func button(_ number: Int64, _ type: NSEvent.EventType) -> NSEvent {
  let cgType: CGEventType =
    switch type {
    case .leftMouseDown: .leftMouseDown
    case .otherMouseUp: .otherMouseUp
    default: .otherMouseDown
    }
  let event = CGEvent(
    mouseEventSource: nil, mouseType: cgType, mouseCursorPosition: .zero,
    mouseButton: CGMouseButton(rawValue: UInt32(number)) ?? .center)!
  event.setIntegerValueField(.mouseEventButtonNumber, value: number)
  return NSEvent(cgEvent: event)!
}
