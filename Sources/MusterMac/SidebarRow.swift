import AppKit

/// One row of the agent list, drawn as direction A (Fencil `agent-list-rows`).
///
/// A pane row is two lines. The first is its callsign and name with the small marks at the far
/// edge; the second is what its state wants you to know - the word and how long it has held, for a
/// state that asks for you - then the directory's last part, with a context bar at the far edge.
/// To the left are the chord that reaches it as two keycaps and an icon whose shape says its state.
/// A blocked or stuck row is tinted and carries a colored edge; the tab on screen sits on a raised
/// panel.
///
/// Built once and shown many times. A focus change moves the keyboard marker on two rows and an
/// agent going idle repaints one, and rebuilding a row's fields and layers for that cost about a
/// millisecond of AppKit construction each - more than anything else a focus change does on the
/// main thread. So a row changing is `show` on the view already there, and the table hands a
/// view back for reuse when a whole reload or a scroll asks for one.
@MainActor
final class SidebarRowView: NSView {
  static let identifier = NSUserInterfaceItemIdentifier("sidebar-row")

  /// Behind everything: the tab on screen's panel, then a blocked or stuck row's tint, then the
  /// keyboard marker, then the colored edge.
  private let panelLayer = CALayer()
  private let tintLayer = CALayer()
  private let highlight = CALayer()
  private let edgeBar = CALayer()
  /// A state's small dot on a folded caption or a helpers line, where an icon would be too much.
  private let dot = CALayer()
  private let icon = StateIcon()
  /// Which machine the row is on, for the rows that are not panes. A pane says it in line two.
  private let swatch = CALayer()
  private let name = NSTextField(labelWithString: "")
  /// A machine heading's note on what its restart cost.
  private let subtitle = NSTextField(labelWithString: "")
  /// A pane's second line: the state, then the directory.
  private let stateField = NSTextField(labelWithString: "")
  private let directoryField = NSTextField(labelWithString: "")
  private var machineTag: MachineTag?
  private var firstKey: KeycapView?
  private var secondKey: KeycapView?
  /// Whether each column of the chord is reserved, so labels line up down the list whether or not a
  /// row has a press in it.
  private var reservesFirst = false
  private var reservesSecond = false
  /// The marks at the trailing edge, left to right (`SidebarModel.accessories(of:)`).
  private var marks: [NSView] = []
  /// What `marks` draws, so a row whose marks did not change keeps the views it has.
  private var accessories: [SidebarModel.Accessory] = []
  private let progressTrack = CALayer()
  private let progressBar = CALayer()
  /// How much of the bar is filled, from 0 to 1.
  private var progressFilled: CGFloat = 0
  /// Every layer's colour, kept to resolve again when the appearance changes: a layer holds a
  /// resolved colour, so one set once keeps light mode's in dark.
  private var layerColors: [(layer: CALayer, color: NSColor)] = []
  /// How many steps in from the list's edge the row starts.
  private var indent = 0
  private var isPaneRow = false
  /// The row this view last drew, so a change of appearance can draw it again from the palette for
  /// the new one.
  private var shown: SidebarModel.Row?

  /// Whether this row carries the keyboard marker.
  var marksKeyboard: Bool { highlight.superlayer != nil }

  /// What a machine heading's second line says, or nil when the row has none.
  var shownSubtitle: String? { subtitle.superview == nil ? nil : subtitle.stringValue }

  /// What a pane's second line says before its directory, without the separator, or nil for none.
  var shownStateLine: String? {
    stateField.superview == nil
      ? nil : stateField.stringValue.replacingOccurrences(of: " · ", with: "")
  }

  /// The directory a pane's second line ends in, or nil when it has none.
  var shownDirectory: String? { directoryField.superview == nil ? nil : directoryField.stringValue }

  /// The state the row's icon is drawn for, or nil when the row has no icon.
  var shownIcon: String? { icon.superlayer == nil ? nil : icon.state }

  /// Whether the row is tinted, and the place it takes in the raised panel, if it has one.
  var showsTint: Bool { tintLayer.superlayer != nil }
  var shownPanel: SidebarModel.PanelPlace? { panelLayer.superlayer == nil ? nil : shown?.panel }

  /// The digits of the keycaps drawn, left to right.
  var shownKeys: [Int] { [firstKey, secondKey].compactMap { $0?.value } }

  /// How full the context bar is, or nil when the row draws none.
  var shownContext: Float? { marks.compactMap { ($0 as? ContextBar)?.used }.first }

  /// How many marks sit at the trailing edge, and whether a progress bar runs under the text.
  var shownMarks: Int { marks.count }
  var showsProgress: Bool { progressBar.superlayer != nil }

  init() {
    super.init(frame: .zero)
    identifier = SidebarRowView.identifier
    wantsLayer = true
    panelLayer.cornerRadius = 10
    tintLayer.cornerRadius = 8
    highlight.cornerRadius = 8
    highlight.borderWidth = 1.5
    edgeBar.cornerRadius = 1.5
    // A square rather than a circle, and at the far edge rather than beside the name: the dot
    // by the name is the agent's state, and a second round mark in a second color beside it
    // would read as a second state.
    swatch.cornerRadius = 2
    dot.cornerRadius = SidebarRowView.dotSize / 2
    for bar in [progressTrack, progressBar] {
      bar.cornerRadius = SidebarRowView.progressHeight / 2
    }
    subtitle.font = .systemFont(ofSize: 10, weight: .regular)
    subtitle.textColor = .secondaryLabelColor
    // Truncated rather than wrapped, and the full text on hover. Wrapping would make a row's
    // height a function of what its agent is doing, so the list would jump under somebody
    // reading it every time an agent wrote a longer sentence. In a list whose whole value is
    // being scannable, a stable row beats a complete one.
    subtitle.lineBreakMode = .byTruncatingTail
    stateField.lineBreakMode = .byTruncatingTail
    directoryField.lineBreakMode = .byTruncatingMiddle
    directoryField.font = .systemFont(ofSize: 11, weight: .regular)
    directoryField.textColor = .secondaryLabelColor
    // Long directory names truncate rather than spilling past the row, for the same reason.
    name.lineBreakMode = .byTruncatingTail
    addSubview(name)
  }

  /// Draws `row`, taking away whatever the row this view last showed had and this one does not.
  func show(_ row: SidebarModel.Row) {
    shown = row
    let palette = ChromePalette.current(for: effectiveAppearance).list
    // Panes indent under their tab caption, and stay indented in a list that draws none; a
    // helper and its maker's helpers line go further in, a step for each level.
    indent = (row.isPane || row.isHelpers ? 1 : 0) + row.depth
    isPaneRow = row.isPane
    layerColors = []
    for layer in [panelLayer, tintLayer, highlight, edgeBar, dot, swatch, icon] {
      layer.removeFromSuperlayer()
    }
    for layer in [progressTrack, progressBar] { layer.removeFromSuperlayer() }
    for field in [subtitle, stateField, directoryField] { field.removeFromSuperview() }
    for key in [firstKey, secondKey] { key?.removeFromSuperview() }
    (firstKey, secondKey) = (nil, nil)
    machineTag?.removeFromSuperview()
    machineTag = nil
    progressFilled = 0

    // Behind everything else, in the order they stack. The keyboard marker is on the one row the
    // keyboard feeds, so a list of a dozen panes beside a window of two can be read back
    // against it.
    var backgrounds: [CALayer] = []
    if let place = row.panel {
      panelLayer.maskedCorners = Self.corners(of: place)
      paint(panelLayer, palette.raised.color)
      backgrounds.append(panelLayer)
    }
    if row.isTinted, let tint = palette.tint[row.state] {
      paint(tintLayer, tint.color)
      backgrounds.append(tintLayer)
    }
    if row.hasKeyboard {
      paint(highlight, NSColor.controlAccentColor.withAlphaComponent(0.22))
      highlight.borderColor = NSColor.controlAccentColor.cgColor
      backgrounds.append(highlight)
    }
    if row.isTinted {
      paint(edgeBar, PaneAppearance.borderColor(state: row.state))
      backgrounds.append(edgeBar)
    }
    for (index, layer) in backgrounds.enumerated() {
      self.layer?.insertSublayer(layer, at: UInt32(index))
    }

    if !row.isPane, let machine = row.machine, let color = NSColor(hex: machine.color) {
      paint(swatch, color)
      layer?.addSublayer(swatch)
    }

    switch row.kind {
    case .machine, .messages, .hidden:
      name.font = .systemFont(ofSize: 10, weight: .semibold)
      name.stringValue = row.label.uppercased()
      name.textColor = .secondaryLabelColor
      // A folded Hidden caption is the only one carrying a state: the most urgent of the panes
      // it is folded over.
      if row.isHiddenCaption, !row.state.isEmpty {
        paint(dot, SidebarModel.dotColor(state: row.state))
        layer?.addSublayer(dot)
      }
      if row.isMachine, !row.subtitle.isEmpty {
        subtitle.stringValue = row.subtitle
        addSubview(subtitle)
      }
    case .helpers:
      name.font = .systemFont(ofSize: 11, weight: .regular)
      name.stringValue = row.label
      name.textColor = .secondaryLabelColor
      // Folded, it carries the most urgent state of the helpers it is folded over, as a folded
      // Hidden caption does.
      if !row.state.isEmpty {
        paint(dot, SidebarModel.dotColor(state: row.state))
        layer?.addSublayer(dot)
      }
    case .group(let unread, _):
      // Bold while something waits, as a mailbox is: the badge says how much, the weight says
      // that the row is worth reading at all.
      name.font = .systemFont(ofSize: 12, weight: unread > 0 ? .semibold : .regular)
      name.stringValue = row.label
      name.textColor = unread > 0 ? .labelColor : .secondaryLabelColor
    case .tab(let reached):
      // Its own press is the whole of its chord: a caption is somewhere you go, not a step on the
      // way to a pane. The tab on screen is named in full and the ones behind it are quieter, and
      // it sits on the raised panel - where you are looking, which in a two-region window is not
      // where you are typing and so is not the accent.
      name.font = .systemFont(ofSize: 11, weight: row.onScreen ? .semibold : .regular)
      name.stringValue = row.label
      name.textColor = row.onScreen ? .labelColor : .secondaryLabelColor
      draw(tabPress: reached, press: 0, in: row, palette: palette)
    case .pane(let first, let second):
      draw(tabPress: first, press: second, in: row, palette: palette)
      // A pane no region is showing is reachable, not absent - dimming it says "not here yet"
      // rather than "gone", which is the difference between a row worth clicking and one that
      // looks broken.
      let color: NSColor = row.onScreen ? .labelColor : .secondaryLabelColor
      name.textColor = color
      name.attributedStringValue = Self.firstLine(for: row, color: color)
      icon.show(state: row.state, color: PaneAppearance.borderColor(state: row.state))
      layer?.addSublayer(icon)
      drawSecondLine(of: row, palette: palette)
      if let progress = SidebarModel.progress(of: row) {
        let failed = progress.state == "error"
        progressFilled = failed ? 1 : CGFloat(min(max(progress.percent ?? 0, 0), 100)) / 100
        paint(progressTrack, .quaternaryLabelColor)
        paint(progressBar, failed ? .systemRed : .controlAccentColor)
        layer?.addSublayer(progressTrack)
        layer?.addSublayer(progressBar)
      }
    }
    showMarks(SidebarModel.accessories(of: row))

    if row.isPane {
      // The whole of what is known, on hover and to VoiceOver, which otherwise hears a name and
      // nothing of the icon beside it.
      let details = SidebarModel.details(of: row)
      toolTip = details
      name.toolTip = details
      stateField.toolTip = details
      subtitle.toolTip = details
      setAccessibilityElement(true)
      setAccessibilityRole(.staticText)
      setAccessibilityLabel(details)
    } else if let summary = SidebarModel.summary(of: row) {
      // A folded caption stands in for the rows it hides, so it says what they would have.
      toolTip = nil
      name.toolTip = summary
      setAccessibilityElement(true)
      setAccessibilityRole(.staticText)
      setAccessibilityLabel(summary)
    } else {
      toolTip = nil
      // A machine row's subtitle is what its restart cost, which the heading has no line for.
      name.toolTip = row.subtitle.isEmpty ? row.label : "\(row.label): \(row.subtitle)"
      setAccessibilityElement(false)
      setAccessibilityRole(nil)
      setAccessibilityLabel(nil)
    }
    resolveColors()
    needsLayout = true
  }

  /// Which corners of the raised panel a row at `place` rounds: the top ones of its first row and
  /// the bottom ones of its last, so the rows read as one rectangle.
  private static func corners(of place: SidebarModel.PanelPlace) -> CACornerMask {
    let top: CACornerMask = [.layerMinXMaxYCorner, .layerMaxXMaxYCorner]
    let bottom: CACornerMask = [.layerMinXMinYCorner, .layerMaxXMinYCorner]
    switch place {
    case .top: return top
    case .bottom: return bottom
    case .middle: return []
    case .only: return top.union(bottom)
    }
  }

  /// A pane's second line: what its state wants you to know, then the directory.
  ///
  /// The state's word is in its own color for the states that ask for somebody or want a look, at
  /// a contrast the palette has checked against the tint behind it; everything else is quiet,
  /// because a calm agent's row is there to say where it is.
  private func drawSecondLine(of row: SidebarModel.Row, palette: ChromePalette.Surface) {
    let hasWhere = !row.directoryName.isEmpty || row.machine != nil
    if !row.stateLine.isEmpty {
      let loud = row.tier == .asks || row.tier == .look
      let word: NSColor =
        loud ? (palette.stateText[row.state]?.color ?? .secondaryLabelColor) : .secondaryLabelColor
      let line = NSMutableAttributedString(
        string: row.stateLine,
        attributes: [
          .font: NSFont.systemFont(ofSize: 11, weight: loud ? .semibold : .regular),
          .foregroundColor: word,
        ])
      if hasWhere {
        line.append(
          NSAttributedString(
            string: " · ",
            attributes: [
              .font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.tertiaryLabelColor,
            ]))
      }
      stateField.attributedStringValue = line
      addSubview(stateField)
    }
    if let machine = row.machine {
      let tag = MachineTag(name: machine.daemon, color: NSColor(hex: machine.color))
      addSubview(tag)
      machineTag = tag
    }
    if !row.directoryName.isEmpty {
      directoryField.stringValue = row.directoryName
      directoryField.toolTip = SidebarModel.abbreviated(row.directory)
      addSubview(directoryField)
    }
  }

  /// A pane row's first line: its callsign, in a style of its own, then what the core puts beside
  /// it - or the whole label for a pane with no callsign.
  ///
  /// Monospaced and semibold, because the callsign is the pane's handle rather than a
  /// description of it, and a renamed pane keeps it: it reads as the same kind of thing on every
  /// row whatever follows it. Neutral in color, since the one color a row has to give is its
  /// state's (Fencil `agent-identity-colors`).
  static func firstLine(for row: SidebarModel.Row, color: NSColor) -> NSAttributedString {
    let plain: [NSAttributedString.Key: Any] = [
      .font: NSFont.systemFont(ofSize: 12, weight: .regular), .foregroundColor: color,
    ]
    guard !row.callsign.isEmpty else {
      return NSAttributedString(string: row.label, attributes: plain)
    }
    let line = NSMutableAttributedString(
      string: row.callsign,
      attributes: [
        .font: NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold),
        .foregroundColor: color,
      ])
    if !row.besideCallsign.isEmpty {
      line.append(NSAttributedString(string: "  \(row.besideCallsign)", attributes: plain))
    }
    return line
  }

  /// Replaces the trailing marks, unless they are the ones already drawn.
  private func showMarks(_ wanted: [SidebarModel.Accessory]) {
    guard wanted != accessories else { return }
    accessories = wanted
    for mark in marks {
      mark.removeFromSuperview()
    }
    marks = wanted.map(SidebarRowView.mark(for:))
    for mark in marks {
      addSubview(mark)
    }
  }

  private func paint(_ layer: CALayer, _ color: NSColor) {
    layerColors.append((layer, color))
  }

  private func resolveColors() {
    effectiveAppearance.performAsCurrentDrawingAppearance {
      for (layer, color) in layerColors {
        layer.backgroundColor = color.cgColor
      }
      if highlight.superlayer != nil {
        highlight.borderColor = NSColor.controlAccentColor.cgColor
      }
      icon.resolve()
    }
  }

  /// Drawn again rather than only repainted, because what depends on the appearance here is the
  /// palette - the tint, the raised panel and the keycaps are computed for it - and a row is cheap.
  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    if let shown { show(shown) } else { resolveColors() }
  }

  /// One trailing mark, drawn small and quiet: the icon says what the agent is doing, and these
  /// only add to it.
  private static func mark(for accessory: SidebarModel.Accessory) -> NSView {
    switch accessory {
    case .unreadable:
      return symbol("eye.slash", tint: .tertiaryLabelColor, saying: "cannot read the screen")
    case .bell:
      return symbol("bell.fill", tint: .secondaryLabelColor, saying: "the bell rang")
    case .subagents(let count):
      return CountBadge(count: count, saying: count == 1 ? "1 sub-agent" : "\(count) sub-agents")
    case .unread(let count):
      return CountBadge(count: count, saying: "\(count) unread")
    case .toYou(let count):
      return symbol("at", tint: .controlAccentColor, saying: "\(count) addressed to you")
    case .context(let used):
      return ContextBar(used: used)
    case .quiet:
      return symbol("bell.slash", tint: .tertiaryLabelColor, saying: "quiet")
    case .member:
      return symbol(
        "bubble.left.and.bubble.right", tint: .secondaryLabelColor,
        saying: "in the conversation shown")
    case .hiddenPanes(let count):
      return CountBadge(
        count: count, saying: count == 1 ? "1 hidden pane" : "\(count) hidden panes")
    case .disclosure(let collapsed):
      return symbol(
        collapsed ? "chevron.right" : "chevron.down", tint: .tertiaryLabelColor,
        saying: collapsed ? "show hidden panes" : "fold hidden panes")
    case .helpers(let open):
      return symbol(
        open ? "chevron.down" : "chevron.right", tint: .tertiaryLabelColor,
        saying: open ? "fold helpers" : "show helpers")
    }
  }

  private static func symbol(_ name: String, tint: NSColor, saying: String) -> NSView {
    let image = NSImage(systemSymbolName: name, accessibilityDescription: saying)?
      .withSymbolConfiguration(.init(pointSize: 9, weight: .regular))
    let view = NSImageView(image: image ?? NSImage())
    view.contentTintColor = tint
    return view
  }

  /// The chord that reaches this row, drawn as keycaps beside the icon rather than instead of it.
  ///
  /// The icon is what the row is for and the chord is how to get there, so a row wants both. Zero
  /// draws no key, which is what a press a row does not have carries - the tab column in a window
  /// of one tab, and both columns on a pane nothing reaches.
  ///
  /// One function for every kind of row, because a chord means one thing wherever it lands and
  /// two implementations of "draw the chord" would be two chances for them to look different
  /// depending on which row they were on.
  ///
  /// **Every press is at full contrast** (Fencil `row-whole-chord`): the tab digit repeating down a
  /// group used to recede, and was too faint to find at a glance, which is what a chord is drawn
  /// for. The second press of a chord already begun is the one the next keystroke can make, and is
  /// drawn live.
  ///
  /// A row reserving a column draws no key in it and still advances past it, which is what keeps
  /// every label in the list on one edge.
  private func draw(
    tabPress first: Int, press second: Int, in row: SidebarModel.Row,
    palette: ChromePalette.Surface
  ) {
    let hasFirst = SidebarRowView.pressable(first)
    let hasSecond = SidebarRowView.pressable(second)
    reservesFirst = hasFirst || row.reservedPresses >= 1
    reservesSecond = hasSecond || row.reservedPresses >= 2
    if hasFirst {
      let key = KeycapView(digit: first)
      key.show(digit: first, live: false, surface: palette)
      addSubview(key)
      firstKey = key
    }
    if hasSecond {
      let key = KeycapView(digit: second)
      key.show(digit: second, live: row.isSecondPress, surface: palette)
      addSubview(key)
      secondKey = key
    }
  }

  /// Whether a press is one of the nine ⌘1 to ⌘9 name.
  ///
  /// The core sends nothing else, and this is the shell agreeing rather than deciding: a digit
  /// drawn beside a row that no key can produce would be worse than an unlabelled row.
  private static func pressable(_ press: Int) -> Bool {
    (1...9).contains(press)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  static let dotSize: CGFloat = 7
  /// Between two trailing marks, and between the last of them and the text.
  static let markGap: CGFloat = 4
  static let progressHeight: CGFloat = 2
  /// A point smaller than a state dot, so the two read as different kinds of mark.
  static let swatchSize: CGFloat = 6
  static let inset: CGFloat = 8
  static let indent: CGFloat = 10
  /// Between the two keycaps of one chord: close enough to read as one address rather than as two
  /// columns that happen to be adjacent.
  static let keyGap: CGFloat = 2
  /// Where a pane row's content starts and stops, in from the list's edges: inside the raised
  /// panel and inside the tint.
  static let paneInset: CGFloat = 20
  static let paneTrailing: CGFloat = 16

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    defer { CATransaction.commit() }
    if panelLayer.superlayer != nil {
      panelLayer.frame = bounds.insetBy(dx: 6, dy: 0)
    }
    if isPaneRow {
      layoutPaneRow()
    } else {
      layoutOtherRow()
    }
  }

  /// Where a key column starts, advancing `left` past each column the row reserves.
  private func placeKeys(from left: inout CGFloat) {
    let top = (bounds.height - KeycapView.height) / 2
    func place(_ key: KeycapView?) {
      key?.frame = CGRect(x: left, y: top, width: KeycapView.width, height: KeycapView.height)
      left += KeycapView.width
    }
    if reservesFirst {
      place(firstKey)
      if reservesSecond { left += SidebarRowView.keyGap }
    }
    if reservesSecond { place(secondKey) }
  }

  private func layoutPaneRow() {
    let side: CGFloat = panelLayer.superlayer == nil ? 4 : 10
    let marked = bounds.insetBy(dx: side, dy: 1)
    tintLayer.frame = marked
    highlight.frame = marked
    edgeBar.frame = CGRect(x: marked.minX, y: marked.minY + 7, width: 3, height: marked.height - 14)

    var left = SidebarRowView.paneInset + CGFloat(indent - 1) * SidebarRowView.indent
    placeKeys(from: &left)
    if reservesFirst || reservesSecond { left += 7 }
    icon.frame = CGRect(
      x: left, y: ((bounds.height - StateIcon.size) / 2).rounded(), width: StateIcon.size,
      height: StateIcon.size)
    left += StateIcon.size + 7
    let textLeft = left
    let right = bounds.width - SidebarRowView.paneTrailing

    let nameHeight = min(bounds.height, name.fittingSize.height)
    let secondHeight = ceil(
      max(stateField.fittingSize.height, directoryField.fittingSize.height, 13))
    let top = (bounds.height - nameHeight - secondHeight - 2) / 2
    let secondY = top
    let firstY = top + secondHeight + 2

    // The marks on the first line from the trailing edge in; the context bar is the second
    // line's, so a column of them lines up down the list whatever the first line carries.
    var firstRight = right
    var secondRight = right
    for mark in marks.reversed() {
      let size = mark.fittingSize
      if mark is ContextBar {
        mark.frame = CGRect(
          x: secondRight - size.width, y: secondY + (secondHeight - size.height) / 2,
          width: size.width, height: size.height)
        secondRight -= size.width + 8
      } else {
        mark.frame = CGRect(
          x: firstRight - size.width, y: firstY + (nameHeight - size.height) / 2,
          width: size.width, height: size.height)
        firstRight -= size.width + SidebarRowView.markGap
      }
    }
    name.frame = CGRect(
      x: textLeft, y: firstY, width: max(0, firstRight - textLeft), height: nameHeight)

    // The second line: state, then the machine's tag, then the directory taking what is left. The
    // state gives way first when the row is narrow, but never to less than a word's worth.
    let room = max(0, secondRight - textLeft)
    var cursor = textLeft
    if stateField.superview != nil {
      let wanted = ceil(stateField.fittingSize.width)
      let hasMore = directoryField.superview != nil || machineTag != nil
      // With a machine tag to fit as well, the state gives up more, so the directory is not the
      // thing that goes.
      let share = machineTag == nil ? 0.65 : 0.45
      let width = hasMore ? min(wanted, room * share) : min(wanted, room)
      stateField.frame = CGRect(x: cursor, y: secondY, width: width, height: secondHeight)
      cursor += width
    }
    if let tag = machineTag {
      let size = tag.fittingSize
      let width = min(size.width, max(0, secondRight - cursor))
      tag.frame = CGRect(
        x: cursor, y: secondY + (secondHeight - size.height) / 2, width: width, height: size.height)
      cursor += width + 5
    }
    if directoryField.superview != nil {
      directoryField.frame = CGRect(
        x: cursor, y: secondY, width: max(0, secondRight - cursor), height: secondHeight)
    }

    if progressBar.superlayer != nil {
      // Along the bottom edge under the text, the way a download's bar sits under its name.
      let span = max(0, right - textLeft)
      let height = SidebarRowView.progressHeight
      progressTrack.frame = CGRect(x: textLeft, y: 2, width: span, height: height)
      progressBar.frame = CGRect(x: textLeft, y: 2, width: span * progressFilled, height: height)
    }
  }

  private func layoutOtherRow() {
    let left = SidebarRowView.inset + CGFloat(indent) * SidebarRowView.indent
    // Keys, then dot, then label, laid out from a running left edge rather than each from
    // `left`, because a row can carry both: the chord says how to reach the row and the dot says
    // why you would want to.
    var textLeft = left
    placeKeys(from: &textLeft)
    if reservesFirst || reservesSecond { textLeft += 7 }
    if dot.superlayer != nil {
      dot.frame = CGRect(
        x: textLeft, y: (bounds.height - SidebarRowView.dotSize) / 2,
        width: SidebarRowView.dotSize, height: SidebarRowView.dotSize)
      textLeft += SidebarRowView.inset + SidebarRowView.dotSize
    }
    // The marks from the trailing edge in, each centred on the row as the dot is, so a column of
    // them reads straight down the list whatever each row's second line does.
    var right = bounds.width - SidebarRowView.inset
    if swatch.superlayer != nil {
      let size = SidebarRowView.swatchSize
      swatch.frame = CGRect(
        x: right - size, y: (bounds.height - size) / 2, width: size, height: size)
      right -= size + SidebarRowView.markGap
    }
    for mark in marks.reversed() {
      let size = mark.fittingSize
      mark.frame = CGRect(
        x: right - size.width, y: (bounds.height - size.height) / 2, width: size.width,
        height: size.height)
      right -= size.width + SidebarRowView.markGap
    }
    // Sized to the text and then centred, rather than given the whole row. A label draws its
    // text at the top of whatever frame it is handed, so a full-height frame puts the words
    // above the dot beside them - which reads as the dot being wrong rather than the text.
    let width = max(0, right - textLeft)
    let textHeight = min(bounds.height, name.fittingSize.height)
    guard subtitle.superview != nil else {
      name.frame = CGRect(
        x: textLeft, y: (bounds.height - textHeight) / 2, width: width, height: textHeight)
      return
    }
    // Two lines share the row: the pair is centred together, so a one-line row and a two-line
    // row read as the same list rather than as two lists.
    let secondHeight = min(bounds.height, subtitle.fittingSize.height)
    let top = (bounds.height - textHeight - secondHeight) / 2
    name.frame = CGRect(x: textLeft, y: top + secondHeight, width: width, height: textHeight)
    subtitle.frame = CGRect(x: textLeft, y: top, width: width, height: secondHeight)
  }
}

/// The machine a pane is on, as a small outlined tag with its color in a corner of it.
@MainActor
final class MachineTag: NSView {
  private let label = NSTextField(labelWithString: "")
  private let swatch = CALayer()
  private let color: NSColor?

  init(name: String, color: NSColor?) {
    self.color = color
    super.init(frame: .zero)
    wantsLayer = true
    layer?.cornerRadius = 4
    layer?.borderWidth = 1
    swatch.cornerRadius = 1.5
    label.stringValue = name
    label.font = .systemFont(ofSize: 10.5, weight: .regular)
    label.textColor = .secondaryLabelColor
    label.lineBreakMode = .byTruncatingTail
    addSubview(label)
    toolTip = "on \(name)"
    resolveColors()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  private var showsSwatch: Bool { color != nil }

  override var fittingSize: NSSize {
    let text = label.fittingSize
    return NSSize(width: ceil(text.width) + (showsSwatch ? 17 : 10), height: 15)
  }

  override func layout() {
    super.layout()
    let text = label.fittingSize
    var left: CGFloat = 5
    if showsSwatch {
      if swatch.superlayer == nil { layer?.addSublayer(swatch) }
      swatch.frame = CGRect(x: left, y: (bounds.height - 5) / 2, width: 5, height: 5)
      left += 7
    }
    label.frame = CGRect(
      x: left, y: (bounds.height - text.height) / 2, width: max(0, bounds.width - left - 5),
      height: text.height)
  }

  private func resolveColors() {
    effectiveAppearance.performAsCurrentDrawingAppearance {
      layer?.borderColor = NSColor.separatorColor.cgColor
      swatch.backgroundColor = color?.cgColor
    }
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    resolveColors()
  }
}

/// A count drawn the way Mail draws an unread one - sub-agents running, messages unread: a number
/// in a capsule, quiet enough to skip and plain enough to read at a glance.
@MainActor
final class CountBadge: NSView {
  private let label = NSTextField(labelWithString: "")

  init(count: Int, saying: String) {
    super.init(frame: .zero)
    wantsLayer = true
    layer?.backgroundColor = NSColor.quaternaryLabelColor.cgColor
    label.stringValue = String(count)
    label.font = .monospacedDigitSystemFont(ofSize: 9, weight: .semibold)
    label.textColor = .secondaryLabelColor
    label.alignment = .center
    addSubview(label)
    setAccessibilityLabel(saying)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var fittingSize: NSSize {
    let text = label.fittingSize
    return NSSize(width: max(text.width + 8, 14), height: 13)
  }

  override func layout() {
    super.layout()
    layer?.cornerRadius = bounds.height / 2
    let text = label.fittingSize
    label.frame = CGRect(
      x: 0, y: (bounds.height - text.height) / 2, width: bounds.width, height: text.height)
  }

  // Resolved again when the appearance changes, since a layer holds a colour, not a name.
  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    effectiveAppearance.performAsCurrentDrawingAppearance {
      layer?.backgroundColor = NSColor.quaternaryLabelColor.cgColor
    }
  }
}

/// How full a row's agent's context is, as a short bar that turns red as it fills: read at a
/// glance down a column of them, where a percentage would have to be read. The percentage is on
/// hover.
@MainActor
final class ContextBar: NSView {
  static let width: CGFloat = 22
  static let height: CGFloat = 4

  /// Percent, 0 to 100.
  let used: Float
  private let track = CALayer()
  private let fill = CALayer()

  init(used: Float) {
    self.used = min(max(used, 0), 100)
    super.init(frame: .zero)
    wantsLayer = true
    track.cornerRadius = Self.height / 2
    fill.cornerRadius = Self.height / 2
    layer?.addSublayer(track)
    layer?.addSublayer(fill)
    let words = "\(Int(self.used.rounded()))% of its context used"
    setAccessibilityLabel(words)
    toolTip = words
    resolveColors()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var fittingSize: NSSize { NSSize(width: Self.width, height: Self.height) }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    track.frame = bounds
    fill.frame = CGRect(
      x: 0, y: 0, width: bounds.width * CGFloat(used / 100), height: bounds.height)
    CATransaction.commit()
  }

  /// Its colors move from `[colors] context_empty` toward `context_full` as it fills, as the ring
  /// they replace did, so a person who repainted the ends still has them.
  private func resolveColors() {
    effectiveAppearance.performAsCurrentDrawingAppearance {
      track.backgroundColor = NSColor.quaternaryLabelColor.cgColor
      fill.backgroundColor = SidebarModel.contextColor(used: used).cgColor
    }
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    resolveColors()
  }
}
