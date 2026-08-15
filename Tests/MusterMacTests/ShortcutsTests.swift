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
  @Test("the window is as big as its contents, so nothing truncates and nothing scrolls")
  func theWindowFitsWhatItHolds() {
    // The rule for a window of this kind rather than a detail of this one: a list you have
    // to scroll to find a shortcut in is a list you go back to the README instead of.
    let sections = Shortcuts.sections(
      MenuActions.byName.keys.map { Core.Binding(action: $0, key: "KeyA", modifiers: ["super"]) })
    let roomy = CGSize(width: 4000, height: 4000)
    let size = Shortcuts.windowSize(sections, limit: roomy)

    let rows = sections.reduce(0) { $0 + $1.rows.count }
    let wanted =
      CGFloat(rows) * Shortcuts.Metrics.rowHeight
      + CGFloat(sections.count) * Shortcuts.Metrics.headerHeight
    #expect(size.height >= wanted, "the window opens too short to hold its own list")

    let columns = Shortcuts.columnWidths(sections)
    #expect(
      size.width >= columns.title + columns.detail + Shortcuts.Metrics.gap,
      "the columns do not fit, so the longest row truncates")
  }

  @MainActor
  @Test("a screen smaller than the list is what the scroller is for")
  func aSmallScreenClampsRatherThanOverflows() {
    // The one case a scroll bar is right, and the reason it is not simply removed. A window
    // taller than the display is worse than scrolling.
    let sections = Shortcuts.sections([Core.Binding(action: "zoom", key: "KeyA", modifiers: [])])
    let cramped = CGSize(width: 200, height: 80)
    let size = Shortcuts.windowSize(sections, limit: cramped)

    #expect(size.height <= cramped.height)
    #expect(size.width <= cramped.width)
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
    // The punctuation keys, which the core names and a keyboard prints. Left out, the row for
    // this very panel read `⌘Slash` - the wire's spelling showing through to the one person
    // who opened a list of chords to find out which key to press.
    #expect(Shortcuts.spell(Core.Binding(action: "x", key: "Slash", modifiers: ["super"])) == "⌘/")
    #expect(
      Shortcuts.spell(Core.Binding(action: "x", key: "BracketLeft", modifiers: ["super"])) == "⌘[")
  }
}
