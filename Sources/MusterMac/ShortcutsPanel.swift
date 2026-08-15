import AppKit

/// The window that shows what Muster does and which key does it.
///
/// A panel rather than a sheet: it is something to leave open beside the panes while you try
/// the chords in it, and a sheet would block the window it is describing.
///
/// It opens at the size that fits its contents, so nothing truncates and there is nothing to
/// scroll. That is the rule for a window of this kind rather than a detail of this one - a
/// list you have to scroll to find a shortcut in is a list you go back to the README instead
/// of. The scroller survives for the one case it is for, a screen smaller than the list, and
/// hides itself otherwise.
///
/// Built from `Shortcuts.sections` every time it opens, not once at launch. Bindings come
/// from the core, and a list cached at launch would be right until somebody edited their
/// config - which is exactly the moment they open this.
@MainActor
public final class ShortcutsPanel: NSObject {
  private var panel: NSPanel?
  private let table = NSTableView()
  private let scroll = NSScrollView()
  private var rows: [Entry] = []

  /// How wide each column ended up, measured from the longest row rather than fixed.
  private var columns: (title: CGFloat, detail: CGFloat) = (0, 0)

  /// One line, flattened out of the sections so a table can index it.
  fileprivate enum Entry {
    case header(String)
    case row(Shortcuts.Row)
  }

  /// Shows the list, or brings it forward if it is already up.
  ///
  /// Rebuilt and re-measured on the way in, so a rebind made since it was last opened is what
  /// it shows and the window still fits it.
  public func show(bindings: [Core.Binding]) {
    let sections = Shortcuts.sections(bindings)
    columns = Shortcuts.columnWidths(sections)
    rows = ShortcutsPanel.entries(sections)

    let panel = panelForShowing()
    let limit =
      (panel.screen ?? NSScreen.main)?.visibleFrame.size
      ?? CGSize(width: 900, height: 900)
    panel.setContentSize(Shortcuts.windowSize(sections, limit: limit))
    table.frame = NSRect(origin: .zero, size: scroll.contentSize)
    table.sizeLastColumnToFit()
    table.reloadData()
    panel.center()
    panel.makeKeyAndOrderFront(nil)
  }

  private func panelForShowing() -> NSPanel {
    if let panel { return panel }
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

    scroll.frame = panel.contentLayoutRect
    scroll.documentView = table
    scroll.hasVerticalScroller = true
    // Present for a screen too small to hold the list, absent every other time. The window
    // sizes itself to fit, so a visible scroller here means the display was the constraint.
    scroll.autohidesScrollers = true
    scroll.drawsBackground = false
    scroll.autoresizingMask = [.width, .height]
    // Without this the table keeps whatever width it was born with and every row is laid out
    // against a bounds narrower than the panel - which puts the chord column in the middle
    // and squeezes the titles to nothing. A document view does not follow its clip view
    // unless it is told to.
    table.autoresizingMask = [.width]
    panel.contentView?.addSubview(scroll)

    self.panel = panel
    return panel
  }

  public override init() {
    super.init()
    let name = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("what"))
    name.resizingMask = .autoresizingMask
    table.addTableColumn(name)
    table.headerView = nil
    table.backgroundColor = .clear
    // Nothing here is a destination, so nothing is selectable: a highlight on a row would
    // suggest pressing return does something.
    table.selectionHighlightStyle = .none
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

  /// Per row, because a heading needs the space above it that separates one group from the
  /// last, and because the window's height is the sum of these.
  public func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
    guard rows.indices.contains(row) else { return Shortcuts.Metrics.rowHeight }
    switch rows[row] {
    case .header: return Shortcuts.Metrics.headerHeight
    case .row: return Shortcuts.Metrics.rowHeight
    }
  }

  public func tableView(
    _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
  ) -> NSView? {
    guard rows.indices.contains(row) else { return nil }
    switch rows[row] {
    case .header(let title): return ShortcutsHeaderView(title: title)
    case .row(let entry): return ShortcutsRowView(row: entry, detailWidth: columns.detail)
    }
  }

  public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
    false
  }
}

@MainActor
private final class ShortcutsHeaderView: NSView {
  private let label = NSTextField(labelWithString: "")

  init(title: String) {
    super.init(frame: .zero)
    label.stringValue = title.uppercased()
    label.font = Shortcuts.Metrics.header
    label.textColor = .secondaryLabelColor
    addSubview(label)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Sat on the bottom of its row rather than centred in it, so the extra height a heading
  /// carries becomes space above the heading and not a gap between it and its first row.
  override func layout() {
    super.layout()
    let height = min(bounds.height, label.fittingSize.height)
    label.frame = CGRect(
      x: Shortcuts.Metrics.inset, y: 0,
      width: max(0, bounds.width - Shortcuts.Metrics.inset * 2), height: height)
  }
}

/// What it does on the left, the chord on the right.
///
/// The chord is right-aligned so a column of them can be scanned down rather than read
/// across - which is the whole reason somebody opened this.
@MainActor
private final class ShortcutsRowView: NSView {
  /// Measured from the longest row and handed down, rather than chosen here. A fixed width
  /// is what truncated "Go to a pane nothing is showing" into something unreadable.
  private let detailWidth: CGFloat

  private let what = NSTextField(labelWithString: "")
  private let detail = NSTextField(labelWithString: "")

  init(row: Shortcuts.Row, detailWidth: CGFloat) {
    self.detailWidth = detailWidth
    super.init(frame: .zero)
    what.stringValue = row.title
    what.font = Shortcuts.Metrics.title
    addSubview(what)

    // The note takes the chord's place when there is no chord, because a row saying only
    // "Focus a pane" with an empty column looks like something that failed to load.
    detail.stringValue = row.chord.isEmpty ? row.note : row.chord
    detail.font = row.chord.isEmpty ? Shortcuts.Metrics.title : Shortcuts.Metrics.chord
    detail.textColor = row.chord.isEmpty ? .secondaryLabelColor : .labelColor
    detail.alignment = .right
    addSubview(detail)

    if !row.chord.isEmpty && !row.note.isEmpty {
      what.toolTip = row.note
    }
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Positioned from `bounds` rather than by a frame set at init.
  ///
  /// A table hands a row view its size after building it, so a frame chosen against the zero
  /// one it was born with lands wherever the resize leaves it - which put every chord off the
  /// right-hand edge and made the whole column look empty. The same arithmetic-in-`layout`
  /// the sidebar rows already do.
  override func layout() {
    super.layout()
    let detailLeft = max(0, bounds.width - Shortcuts.Metrics.inset - detailWidth)
    // Each field sized to its own text and centred. A label draws at the top of its frame, so
    // a full-height one puts the words above the middle of the row - and the two here are
    // different sizes, so they would sit at different heights as well as the wrong one.
    what.frame = centred(
      what, x: Shortcuts.Metrics.inset,
      width: max(0, detailLeft - Shortcuts.Metrics.inset - Shortcuts.Metrics.gap / 2))
    detail.frame = centred(detail, x: detailLeft, width: detailWidth)
  }

  private func centred(_ field: NSTextField, x: CGFloat, width: CGFloat) -> CGRect {
    let height = min(bounds.height, field.fittingSize.height)
    return CGRect(x: x, y: (bounds.height - height) / 2, width: width, height: height)
  }
}
