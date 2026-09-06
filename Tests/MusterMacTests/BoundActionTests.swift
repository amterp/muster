import AppKit
import Testing

@testable import MusterMac

// What a bound action leaves in the run log, and whether a reader can tell a held key from a
// menu somebody clicked eight times.
//
// The record exists because its absence cost an evening: eight `pane.font_size` lines 100 ms
// apart with nothing in front of them read as a replay bug, and were one press of a chord
// (kan a_2KHGYh7xD). So what these assert is not the wording - it is that the cause is written,
// that it says which of the two ways it arrived, and that it lands *before* the effect.

@Suite("a bound action's press", .ownsTheSeam)
struct BoundActionTests {
  @MainActor
  @Test("a chord says which keystroke asked, and that it was a chord")
  func aChordNamesItself() throws {
    let item = boundItem(name: "split_right", keyEquivalent: "d", modifiers: .command)

    let fields = try #require(
      BoundAction.record(item: item, event: key("d", keyCode: 0x02, flags: .command)))

    #expect(fields["action"] == "split_right")
    #expect(fields["source"] == "chord")
    #expect(fields["key"] == "KeyD")
    #expect(fields["mods"] == "super")
    #expect(fields["repeat"] == "false")
  }

  @MainActor
  @Test("a held chord says so, which is the whole point of the record")
  func aHeldChordSaysSo() throws {
    // The incident this record was added for: eight effects 100 ms apart. With this field a
    // reader sees seven repeats and stops looking for a bug.
    let item = boundItem(name: "increase_font_size", keyEquivalent: "=", modifiers: .command)

    let fields = try #require(
      BoundAction.record(
        item: item, event: key("=", keyCode: 0x18, flags: .command, isARepeat: true)))

    #expect(fields["repeat"] == "true")
    #expect(fields["source"] == "chord")
  }

  @MainActor
  @Test("shift rides in the modifiers while the key equivalent stays lowercase")
  func shiftIsAModifierRatherThanACapital() throws {
    // AppKit spells shift in the mask and keeps the equivalent lowercase, so the characters
    // the event carries are the capital. A match that compared them literally would call this
    // a menu pick and lose the chord it was pressed with.
    let item = boundItem(
      name: "next_tab", keyEquivalent: "]", modifiers: [.command, .shift])

    let fields = try #require(
      BoundAction.record(
        item: item, event: key("}", unshifted: "]", keyCode: 0x1E, flags: [.command, .shift])))

    #expect(fields["source"] == "chord")
    #expect(fields["mods"] == "shift+super")
  }

  @MainActor
  @Test("picking the item with the mouse reports no keystroke")
  func aMousePickCarriesNoChord() throws {
    let item = boundItem(name: "close_pane", keyEquivalent: "w", modifiers: .command)

    let fields = try #require(BoundAction.record(item: item, event: click()))

    #expect(fields["source"] == "menu")
    #expect(fields["key"] == "")
    #expect(fields["mods"] == "")
    #expect(fields["repeat"] == "false")
  }

  @MainActor
  @Test("Return on a highlighted item is a menu pick, not the item's chord")
  func returnIsNotTheChord() throws {
    // Walking the menu with the arrow keys and pressing Return dispatches the item while the
    // current event is a keyDown. Reading the event's type alone would file that as a chord and
    // report ⌘D as Return.
    let item = boundItem(name: "split_right", keyEquivalent: "d", modifiers: .command)

    let fields = try #require(
      BoundAction.record(item: item, event: key("\r", keyCode: 0x24)))

    #expect(fields["source"] == "menu")
    #expect(fields["key"] == "")
  }

  @MainActor
  @Test("the platform's own items write nothing")
  func copyAndPasteStaySilent() {
    // Copy, Paste and the plain Quit item are not the core's actions and carry no name. A
    // record for them would be a line claiming Muster dispatched something it did not.
    let item = NSMenuItem(
      title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    item.keyEquivalentModifierMask = .command

    #expect(BoundAction.record(item: item, event: key("c", keyCode: 0x08, flags: .command)) == nil)
  }

  @MainActor
  @Test("the cause is recorded before the effect is asked for")
  func theCauseComesFirst() {
    // The ordering is the card's complaint stated as an assertion. A record written after the
    // dispatch would sit under its own effect in the timeline, which is where a reader stops
    // being able to tell one from the other.
    //
    // The target stands in for the window - a real one wants a libghostty runtime - and does
    // what the window's item does, which is one call to the core. That the window itself sends
    // a split is `PaneActionTests`.
    // An application has to exist for a menu to dispatch at all: `performActionForItem` sends
    // the item's action through `NSApp`, and without one the target is never reached.
    _ = NSApplication.shared
    let recorder = recorder()
    let target = SplittingTarget()
    let menu = AppMenu.build(
      target: target,
      bindings: [Core.Binding(action: "split_right", key: "KeyD", modifiers: ["super"])])
    guard let pane = submenu(of: menu, holding: "Split Right"),
      let at = pane.items.firstIndex(where: { $0.title == "Split Right" })
    else {
      Issue.record("the menu bar had no Split Right item to dispatch")
      return
    }
    let before = recorder.requests.count

    pane.performActionForItem(at: at)

    let sent = recorder.sent(since: before) { request in
      switch request.payload {
      case .logRecord(let record): record.event == BoundAction.event
      case .splitPane: true
      default: false
      }
    }
    guard sent.count == 2, case .logRecord(let record) = sent[0].payload,
      case .splitPane = sent[1].payload
    else {
      Issue.record("expected the press then the split, got \(sent.map(\.payload))")
      return
    }
    #expect(record.level == "debug")
    #expect(record.fields["action"] == "split_right")
    #expect(record.fields["source"] == "menu")
  }

  @MainActor
  @Test("every action the menu installs can name itself")
  func nothingInstalledCanMissItsName() {
    // Driven from every action this shell knows how to install, so it fails the moment one
    // reaches the menu bar without a name to log - which is the property the card wanted from
    // `Action::ALL`, held on this side of the seam.
    let everything = MenuActions.byName.keys.map {
      Core.Binding(action: $0, key: "KeyA", modifiers: ["super"])
    }

    for item in AppMenu.paneItems(everything) {
      #expect(!item.name.isEmpty, "\(item.title) reaches the menu bar with no name")
    }
  }

  @MainActor
  @Test("the name in the record is the core's, not one the shell keeps")
  func theNameIsTheCoresOwn() throws {
    // A rename in the core's action table has to move this log line with it. It does if the
    // name travels from the binding rather than from a table here.
    let items = AppMenu.paneItems([
      Core.Binding(action: "split_right", key: "KeyD", modifiers: ["super"])
    ])
    let item = try #require(items.first)

    #expect(item.name == "split_right")
  }
}

/// Answers `split_right` the way the window's item does, without a libghostty runtime behind it.
@MainActor
private final class SplittingTarget: NSObject {
  @objc func splitRight(_ sender: Any?) {
    Core.split(side: "right")
  }
}

/// A menu item as `AppMenu` builds one: bound to a chord and carrying the core's name for it.
@MainActor
private func boundItem(
  name: String, keyEquivalent: String, modifiers: NSEvent.ModifierFlags
) -> NSMenuItem {
  let item = NSMenuItem(
    title: name, action: #selector(MusterWindow.splitRight(_:)), keyEquivalent: keyEquivalent)
  item.keyEquivalentModifierMask = modifiers
  item.representedObject = name
  return item
}

/// The submenu holding an item with this title, wherever in the bar it landed.
@MainActor
private func submenu(of menu: NSMenu, holding title: String) -> NSMenu? {
  menu.items.compactMap(\.submenu).first { $0.items.contains { $0.title == title } }
}

/// Builds the keystroke AppKit would have delivered.
private func key(
  _ characters: String,
  unshifted: String? = nil,
  keyCode: UInt16,
  flags: NSEvent.ModifierFlags = [],
  isARepeat: Bool = false
) -> NSEvent? {
  NSEvent.keyEvent(
    with: .keyDown,
    location: .zero,
    modifierFlags: flags,
    timestamp: 0,
    windowNumber: 0,
    context: nil,
    characters: characters,
    charactersIgnoringModifiers: unshifted ?? characters,
    isARepeat: isARepeat,
    keyCode: keyCode)
}

/// A press, for the case where a menu was picked rather than a chord held.
private func click() -> NSEvent? {
  NSEvent.mouseEvent(
    with: .leftMouseUp,
    location: .zero,
    modifierFlags: [],
    timestamp: 0,
    windowNumber: 0,
    context: nil,
    eventNumber: 0,
    clickCount: 1,
    pressure: 1)
}
