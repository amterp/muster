import AppKit
import Testing

@testable import MusterMac

// What the list of shortcuts says, and the one property that keeps it honest.
//
// The interesting failure here is not a wrong chord - the core publishes those and a rebind
// moves them - it is a row that is missing. A list that quietly omits something is worse than
// no list, because it reads as a complete answer, and nobody searches for a line they do not
// know should be there.

@Suite("the shortcuts list")
struct ShortcutsTests {
  @MainActor
  @Test("everything the menu bar installs is also in the list")
  func nothingInstalledCanMissTheList() {
    // Driven from every action this shell knows how to install rather than from a live core,
    // which in a test has published nothing. That is also the stronger source: it is the
    // table somebody adds a line to, so this fails the moment a new action reaches the menu
    // bar and not the list - a new group, or a section filter that stopped matching.
    let everything = MenuActions.byName.keys.map {
      Core.Binding(action: $0, key: "KeyA", modifiers: ["super"])
    }
    let installed = AppMenu.paneItems(everything).map(\.title)
    let listed = Set(Shortcuts.sections(everything).flatMap { $0.rows.map(\.title) })

    #expect(installed.count == MenuActions.byName.count, "the menu dropped an action it knows")
    for title in installed {
      #expect(listed.contains(title), "\(title) is in the menu bar and not in the list")
    }
  }

  @MainActor
  @Test("the platform's own chords are listed, though the core never names them")
  func theListCoversWhatTheCoreDoesNotOwn() {
    // Copy, paste and quit go through the responder chain and the application, so none is in
    // the core's table and none can be rebound by Muster's config. A person looking for "how
    // do I copy" does not care which layer answers.
    let listed = Set(Shortcuts.sections([]).flatMap { $0.rows.map(\.title) })
    #expect(listed.contains("Copy"))
    #expect(listed.contains("Paste"))
    #expect(listed.contains("Quit muster"))
  }

  @MainActor
  @Test("what has no chord at all is listed too")
  func theListCoversWhatHasNoChord() {
    // The half a list built only from bindings would leave out, and the half a new window
    // most needs: nothing on screen says the divider between two panes can be dragged.
    let rows = Shortcuts.sections([]).flatMap(\.rows)
    let chordless = rows.filter { $0.chord.isEmpty }
    #expect(!chordless.isEmpty, "nothing without a chord was listed")
    for row in chordless {
      #expect(!row.note.isEmpty, "\(row.title) has neither a chord nor a note, so it says nothing")
    }
  }

  @MainActor
  @Test("an unbound action keeps its row and loses its chord")
  func unbindingLeavesTheRow() {
    // Same rule the menu already follows: somebody who unbound ⌘W did it to get the shortcut
    // back, not to lose the action - and this is also where you look to confirm it is gone.
    let unbound = Core.Binding(action: "close_pane", key: "", modifiers: [])
    let rows = Shortcuts.sections([unbound]).flatMap(\.rows)
    let row = rows.first { $0.title == "Close Pane" }

    #expect(row != nil, "an unbound action lost its row")
    #expect(row?.chord == "")
  }

  @MainActor
  @Test("an action newer than this shell is listed under its own name")
  func anUnknownActionIsStillFindable() {
    // The menu skips one of these, because it has no title and no selector to dispatch. The
    // list can do better: the chord works, so somebody who pressed it deserves to be able to
    // look it up rather than meet a gap.
    let ahead = Core.Binding(action: "teleport", key: "KeyJ", modifiers: ["super"])
    let rows = Shortcuts.sections([ahead]).flatMap(\.rows)

    #expect(rows.contains { $0.title == "teleport" && $0.chord == "⌘J" })
  }

  @MainActor
  @Test("chords are spelled the way this platform prints them")
  func chordsReadLikeAMenu() {
    // Modifiers in the platform's own order, so a column of them can be scanned rather than
    // translated. Spelled here rather than taken from AppKit because AppKit renders these
    // into a menu item and gives no string back.
    #expect(
      Shortcuts.spell(
        Core.Binding(action: "x", key: "ArrowLeft", modifiers: ["super", "shift", "control"]))
        == "⌃⇧⌘←")
    #expect(Shortcuts.spell(Core.Binding(action: "x", key: "KeyD", modifiers: ["super"])) == "⌘D")
    #expect(Shortcuts.spell(Core.Binding(action: "x", key: "Enter", modifiers: [])) == "↩")
    #expect(Shortcuts.spell(Core.Binding(action: "x", key: "", modifiers: ["super"])) == "")
  }
}
