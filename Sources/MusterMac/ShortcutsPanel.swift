import AppKit

/// The window that shows what Muster does and which key does it.
///
/// A panel rather than a sheet: it is something to leave open beside the panes while you try
/// the chords in it, and a sheet would block the window it is describing.
///
/// Built from `Shortcuts.sections` every time it opens, not once at launch. Bindings come
/// from the core, and a list cached at launch would be right until somebody edited their
/// config - which is exactly the moment they open this.
@MainActor
public final class ShortcutsPanel: NSObject {
  private var panel: NSPanel?
  private let table = NSTableView()
  private var rows: [Entry] = []

  /// One line, flattened out of the sections so a table can index it.
  fileprivate enum Entry {
    case header(String)
    case row(Shortcuts.Row)
  }

  /// Shows the list, or brings it forward if it is already up.
  ///
  /// Rebuilt on the way in, so a rebind made since it was last opened is what it shows.
  public func show(bindings: [Core.Binding]) {
    rows = ShortcutsPanel.entries(Shortcuts.sections(bindings))
    table.reloadData()

    if let panel {
      panel.makeKeyAndOrderFront(nil)
      return
    }
    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 420, height: 520),
      styleMask: [.titled, .closable, .resizable, .utilityWindow],
      backing: .buffered,
      defer: false)
    panel.title = "muster Shortcuts"
    panel.isFloatingPanel = true
    // So it does not take the keyboard away from the pane somebody is about to type the
    // shortcut into. Reading a list of chords while unable to press one would be a poor joke.
    panel.becomesKeyOnlyIfNeeded = true
    panel.hidesOnDeactivate = false

    let scroll = NSScrollView(frame: panel.contentLayoutRect)
    scroll.documentView = table
    scroll.hasVerticalScroller = true
    scroll.autoresizingMask = [.width, .height]
    panel.contentView?.addSubview(scroll)

    self.panel = panel
    panel.center()
    panel.makeKeyAndOrderFront(nil)
  }

  public override init() {
    super.init()
    let name = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("what"))
    name.resizingMask = .autoresizingMask
    table.addTableColumn(name)
    table.headerView = nil
    table.rowSizeStyle = .default
    // Nothing here is a destination, so nothing is selectable: a highlight on a row would
    // suggest pressing return does something.
    table.selectionHighlightStyle = .none
    table.backgroundColor = .clear
    table.dataSource = self
    table.delegate = self
  }

  fileprivate static func entries(_ sections: [Shortcuts.Section]) -> [Entry] {
    sections.flatMap { section in
      [Entry.header(section.title)] + section.rows.map(Entry.row)
    }
  }
}

extension ShortcutsPanel: NSTableViewDataSource, NSTableViewDelegate {
  public func numberOfRows(in tableView: NSTableView) -> Int {
    rows.count
  }

  public func tableView(
    _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
  ) -> NSView? {
    guard rows.indices.contains(row) else { return nil }
    switch rows[row] {
    case .header(let title): return ShortcutsHeaderView(title: title)
    case .row(let entry): return ShortcutsRowView(row: entry)
    }
  }

  public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
    false
  }
}

@MainActor
private final class ShortcutsHeaderView: NSView {
  init(title: String) {
    super.init(frame: .zero)
    let label = NSTextField(labelWithString: title.uppercased())
    label.font = .systemFont(ofSize: 10, weight: .semibold)
    label.textColor = .secondaryLabelColor
    label.frame = NSRect(x: 12, y: 2, width: 320, height: 16)
    label.autoresizingMask = [.width]
    addSubview(label)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }
}

/// What it does on the left, the chord on the right.
///
/// The chord is monospaced and right-aligned so a column of them can be scanned down rather
/// than read across - which is the whole reason somebody opened this.
@MainActor
private final class ShortcutsRowView: NSView {
  init(row: Shortcuts.Row) {
    super.init(frame: .zero)
    let what = NSTextField(labelWithString: row.title)
    what.font = .systemFont(ofSize: 12)
    what.frame = NSRect(x: 12, y: 2, width: 200, height: 16)
    addSubview(what)

    // The note takes the chord's place when there is no chord, because a row saying only
    // "Focus a pane" with an empty column looks like something that failed to load.
    let trailing = row.chord.isEmpty ? row.note : row.chord
    let detail = NSTextField(labelWithString: trailing)
    detail.font =
      row.chord.isEmpty
      ? .systemFont(ofSize: 11) : .monospacedSystemFont(ofSize: 12, weight: .regular)
    detail.textColor = row.chord.isEmpty ? .secondaryLabelColor : .labelColor
    detail.alignment = .right
    detail.frame = NSRect(x: 216, y: 2, width: 188, height: 16)
    detail.autoresizingMask = [.width]
    addSubview(detail)

    if !row.chord.isEmpty && !row.note.isEmpty {
      what.toolTip = row.note
    }
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }
}
