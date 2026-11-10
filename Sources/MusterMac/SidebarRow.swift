import AppKit

/// One row of the agent list (Fencil `agent-list-rows`).
///
/// A pane's entry is up to four lines, so that it reads at 200 points. The first is an icon whose
/// shape says its state, its callsign and its machine's tag, with the chord that reaches it as
/// keycaps at the far edge. The task line is the name somebody gave it, or else its harness's
/// title, with the small marks and the context bar at the far edge. Then what its state wants you
/// to know - the word and how long it has held, for a state that asks for you - and the
/// directory's last part. Last, a pill for each of its helpers. An entry with no name and no title
/// leaves the task line out and draws its marks at the end of the state line, and one with no
/// helpers has no pills line. A blocked or stuck entry is tinted and carries a
/// colored edge; the tab on screen sits on a raised panel.
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
  /// A state's small dot on a folded Hidden caption, where an icon would be too much.
  private let dot = CALayer()
  private let icon = StateIcon()
  /// Which machine the row is on, for the rows that are not panes. A pane says it with a tag.
  private let swatch = CALayer()
  private let name = NSTextField(labelWithString: "")
  /// A machine heading's note on what its restart cost.
  private let subtitle = NSTextField(labelWithString: "")
  /// A pane's task line.
  private let taskField = NSTextField(labelWithString: "")
  /// The line under it: the state, then the directory.
  private let stateField = NSTextField(labelWithString: "")
  private let directoryField = NSTextField(labelWithString: "")
  private var machineTag: MachineTag?
  private var pills: HelperPillsView?
  /// A tab caption's fold control, and while the tab is folded the icons of the panes it is over
  /// and the count of those that did not fit.
  private var foldControl: FoldControl?
  private var foldIcons: [StateIcon] = []
  private let foldMore = NSTextField(labelWithString: "")
  private var firstKey: KeycapView?
  private var secondKey: KeycapView?
  /// Whether each column of a caption's chord is reserved, so captions' labels line up down the
  /// list whether or not one has a press in it. A pane's keycaps sit at the far edge of its first
  /// line and reserve nothing.
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

  /// Called when somebody clicks the caption's fold control, with the row it is on.
  var onFoldToggled: ((SidebarModel.Row) -> Void)?

  /// Called when somebody presses the envelope on a pane's row, with its group's daemon and name.
  var onGroupPicked: ((String, String) -> Void)?

  /// Asked for the menu of the envelope on this row's pane, which is the group's menu.
  var onConversationMenu: ((SidebarModel.Conversation) -> NSMenu?)?

  /// Called when somebody clicks the pills on a pane's entry, with the row they are on.
  var onHelpersToggled: ((SidebarModel.Row) -> Void)?

  /// Asked for the menu of the pills on this row's pane, which is its helpers' together.
  var onHelpersMenu: ((SidebarModel.Row) -> NSMenu?)?

  /// The envelope on this row, for the tests to press.
  var conversationBadge: ConversationBadge? {
    marks.compactMap { $0 as? ConversationBadge }.first
  }

  /// The pills on this row, or nil when its pane has no helpers.
  var helperPills: HelperPillsView? { pills }

  /// Whether this row carries the keyboard marker.
  var marksKeyboard: Bool { highlight.superlayer != nil }

  /// What a machine heading's second line says, or nil when the row has none.
  var shownSubtitle: String? { subtitle.superview == nil ? nil : subtitle.stringValue }

  /// What a pane's first line says.
  var shownFirstLine: String { name.stringValue }

  /// What a pane's task line says, or nil when the entry has none.
  var shownTaskLine: String? { taskField.superview == nil ? nil : taskField.stringValue }

  /// What a pane says of its state before its directory, without the separator, or nil for none.
  var shownStateLine: String? {
    stateField.superview == nil
      ? nil : stateField.stringValue.replacingOccurrences(of: " · ", with: "")
  }

  /// The first line's hover text.
  var firstLineToolTip: String? { name.toolTip }

  /// What the state field says, with its separator.
  var shownStateText: String? { stateField.superview == nil ? nil : stateField.stringValue }

  /// Whether the machine's tag is drawn, and where.
  var showsMachineTag: Bool { machineTag.map { $0.superview != nil && !$0.isHidden } ?? false }
  var machineTagFrame: CGRect? { showsMachineTag ? machineTag?.frame : nil }

  /// Where each of a pane's lines was laid out. Meaningful after layout.
  var firstLineFrame: CGRect { name.frame }
  var taskLineFrame: CGRect? { taskField.superview == nil ? nil : taskField.frame }
  var stateLineFrame: CGRect? { stateField.superview == nil ? nil : stateField.frame }
  var directoryFrame: CGRect? { directoryField.superview == nil ? nil : directoryField.frame }
  var iconFrame: CGRect? { icon.superlayer == nil ? nil : icon.frame }
  var keyFrames: [CGRect] { [firstKey, secondKey].compactMap { $0?.frame } }
  var markFrames: [CGRect] { marks.map(\.frame) }

  /// Whether the first line gives the callsign all the room it asks for.
  var firstLineFits: Bool { name.frame.width >= ceil(name.fittingSize.width) }

  /// The directory a pane's entry ends in, or nil when it has none.
  var shownDirectory: String? { directoryField.superview == nil ? nil : directoryField.stringValue }

  /// Whether a pane's entry gives its directory all the room it asks for, or nil when it has
  /// none. Meaningful after layout.
  var directoryFits: Bool? {
    directoryField.superview == nil
      ? nil : directoryField.frame.width >= ceil(directoryField.fittingSize.width)
  }

  /// The width the state phrase was given, or nil when the row has none.
  var stateFieldWidth: CGFloat? { stateField.superview == nil ? nil : stateField.frame.width }

  /// How the first line ends when it does not fit.
  var firstLineBreak: NSLineBreakMode? {
    let style = name.attributedStringValue.attribute(.paragraphStyle, at: 0, effectiveRange: nil)
    return (style as? NSParagraphStyle)?.lineBreakMode
  }

  /// How the state phrase ends when it does not fit, or nil when the row has none.
  var stateLineBreak: NSLineBreakMode? {
    guard stateField.superview != nil else { return nil }
    let style = stateField.attributedStringValue.attribute(
      .paragraphStyle, at: 0, effectiveRange: nil)
    return (style as? NSParagraphStyle)?.lineBreakMode
  }

  /// The width the context bar was given, or nil when the row draws none.
  var contextBarWidth: CGFloat? { marks.compactMap { $0 as? ContextBar }.first?.frame.width }

  /// The state the row's icon is drawn for, or nil when the row has no icon.
  var shownIcon: String? { icon.superlayer == nil ? nil : icon.state }

  /// Whether the row is tinted, and the place it takes in the raised panel, if it has one.
  var showsTint: Bool { tintLayer.superlayer != nil }
  var shownPanel: SidebarModel.PanelPlace? { panelLayer.superlayer == nil ? nil : shown?.panel }

  /// The digits of the keycaps drawn, left to right.
  var shownKeys: [Int] { [firstKey, secondKey].compactMap { $0?.value } }

  /// How full the context bar is, or nil when the row draws none.
  var shownContext: Float? { marks.compactMap { ($0 as? ContextBar)?.used }.first }

  /// The color the context bar is filled with, or nil when the row draws none.
  var contextBarFill: CGColor? { marks.compactMap { ($0 as? ContextBar)?.fillColor }.first }

  /// Whether the caption has a fold control, and whether it shows the tab as folded.
  var showsFoldControl: Bool { foldControl != nil }
  var foldControlIsFolded: Bool? { foldControl?.isFolded }
  var foldControlFrame: CGRect? { foldControl?.frame }
  func pressFoldControl() { foldControl?.onPressed?() }

  /// The states of the icons a folded caption draws now, in order, and how many panes did not fit
  /// (nil when all did). Meaningful after layout, which is what decides the room.
  var shownFoldIcons: [String] { foldIcons.filter { !$0.isHidden }.map(\.state) }
  var foldedMore: String? { foldMore.superview == nil ? nil : foldMore.stringValue }

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
    taskField.lineBreakMode = .byTruncatingTail
    taskField.font = .systemFont(ofSize: 12, weight: .regular)
    // Long directory names truncate rather than spilling past the row, for the same reason.
    name.lineBreakMode = .byTruncatingTail
    addSubview(name)
  }

  /// Draws `row`, taking away whatever the row this view last showed had and this one does not.
  func show(_ row: SidebarModel.Row) {
    shown = row
    let palette = ChromePalette.current(for: effectiveAppearance).list
    // Panes indent under their tab caption, and stay indented in a list that draws none; a
    // helper listed under its maker's entry goes a step further in.
    indent = (row.isPane ? 1 : 0) + row.depth
    isPaneRow = row.isPane
    layerColors = []
    for layer in [panelLayer, tintLayer, highlight, edgeBar, dot, swatch, icon] {
      layer.removeFromSuperlayer()
    }
    for layer in [progressTrack, progressBar] { layer.removeFromSuperlayer() }
    for field in [subtitle, taskField, stateField, directoryField] { field.removeFromSuperview() }
    for key in [firstKey, secondKey] { key?.removeFromSuperview() }
    (firstKey, secondKey) = (nil, nil)
    machineTag?.removeFromSuperview()
    machineTag = nil
    pills?.removeFromSuperview()
    pills = nil
    foldControl?.removeFromSuperview()
    foldControl = nil
    for icon in foldIcons { icon.removeFromSuperlayer() }
    foldIcons = []
    foldMore.removeFromSuperview()
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
      if row.foldable {
        let control = FoldControl(folded: row.folded)
        control.onPressed = { [weak self] in
          guard let self, let shown else { return }
          onFoldToggled?(shown)
        }
        addSubview(control)
        foldControl = control
      }
      if row.folded { drawFoldedPanes(row.foldedPanes) }
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
      if let machine = row.machine {
        let tag = MachineTag(name: machine.daemon, color: NSColor(hex: machine.color))
        addSubview(tag)
        machineTag = tag
      }
      if SidebarModel.hasTaskLine(row) {
        taskField.stringValue = row.taskLine
        taskField.textColor = color
        addSubview(taskField)
      }
      drawStateLine(of: row, palette: palette)
      if !row.isHelper, !row.helpers.isEmpty {
        let pills = HelperPillsView(helpers: row.helpers, open: row.helpersOpen)
        pills.onPressed = { [weak self] in
          guard let self, let shown else { return }
          onHelpersToggled?(shown)
        }
        pills.onMenu = { [weak self] in
          guard let self, let shown else { return nil }
          return onHelpersMenu?(shown)
        }
        addSubview(pills)
        self.pills = pills
      }
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
      taskField.toolTip = details
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

  /// The icons of the panes a folded caption stands in for, so a blocked agent inside still shows.
  /// Laid out later, when the room is known.
  private func drawFoldedPanes(_ panes: [SidebarModel.FoldedPane]) {
    foldIcons = panes.map { pane in
      let icon = StateIcon()
      icon.show(state: pane.state, color: PaneAppearance.borderColor(state: pane.state))
      layer?.addSublayer(icon)
      return icon
    }
    foldMore.font = .systemFont(ofSize: 10, weight: .semibold)
    foldMore.textColor = .secondaryLabelColor
    foldMore.isHidden = true
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

  /// The line saying what a pane's state wants you to know, then its directory.
  ///
  /// The state's word is in its own color for the states that ask for somebody or want a look, at
  /// a contrast the palette has checked against the tint behind it; everything else is quiet,
  /// because a calm agent's row is there to say where it is.
  private func drawStateLine(of row: SidebarModel.Row, palette: ChromePalette.Surface) {
    let hasWhere = !row.directoryName.isEmpty
    if !row.stateLine.isEmpty {
      let loud = row.tier == .asks || row.tier == .look
      let word: NSColor =
        loud ? (palette.stateText[row.state]?.color ?? .secondaryLabelColor) : .secondaryLabelColor
      let line = NSMutableAttributedString(
        string: row.stateLine,
        attributes: [
          .font: NSFont.systemFont(ofSize: 11, weight: loud ? .semibold : .regular),
          .foregroundColor: word, .paragraphStyle: Self.truncating(.byTruncatingTail),
        ])
      if hasWhere {
        line.append(
          NSAttributedString(
            string: " · ",
            attributes: [
              .font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.tertiaryLabelColor,
              .paragraphStyle: Self.truncating(.byTruncatingTail),
            ]))
      }
      stateField.attributedStringValue = line
      addSubview(stateField)
    }
    if !row.directoryName.isEmpty {
      directoryField.stringValue = row.directoryName
      directoryField.toolTip = SidebarModel.abbreviated(row.directory)
      addSubview(directoryField)
    }
  }

  /// A pane's first line: its callsign, in a style of its own - or the whole label for a pane
  /// with no callsign. `withBeside` adds what the core puts beside the callsign, the pane's name or
  /// its directory and harness, for a pane's header, which has the one line; an entry in the list
  /// says those on lines of their own.
  ///
  /// Monospaced and semibold, because the callsign is the pane's handle rather than a
  /// description of it, and a renamed pane keeps it: it reads as the same kind of thing on every
  /// row whatever follows it. Neutral in color, since the one color a row has to give is its
  /// state's (Fencil `agent-identity-colors`).
  static func firstLine(
    for row: SidebarModel.Row, color: NSColor, withBeside: Bool = false
  ) -> NSAttributedString {
    let plain: [NSAttributedString.Key: Any] = [
      .font: NSFont.systemFont(ofSize: 12, weight: .regular), .foregroundColor: color,
      .paragraphStyle: truncating(.byTruncatingTail),
    ]
    guard !row.callsign.isEmpty else {
      return NSAttributedString(string: row.label, attributes: plain)
    }
    let line = NSMutableAttributedString(
      string: row.callsign,
      attributes: [
        .font: NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold),
        .foregroundColor: color, .paragraphStyle: truncating(.byTruncatingTail),
      ])
    if withBeside, !row.besideCallsign.isEmpty {
      line.append(NSAttributedString(string: "  \(row.besideCallsign)", attributes: plain))
    }
    return line
  }

  /// A paragraph style that ends a line that does not fit with an ellipsis.
  ///
  /// An attributed string carries its own paragraph style, whose default wraps by word and so
  /// overrides the field's `lineBreakMode`: in a one-line field the cut text then simply stops,
  /// with nothing to say it was cut.
  static func truncating(_ mode: NSLineBreakMode) -> NSParagraphStyle {
    let style = NSMutableParagraphStyle()
    style.lineBreakMode = mode
    return style
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
      if let badge = mark as? ConversationBadge {
        badge.onPressed = { [weak self] in
          guard let conversation = self?.shown?.conversation else { return }
          self?.onGroupPicked?(conversation.daemon, conversation.group)
        }
        badge.onMenu = { [weak self] in
          guard let conversation = self?.shown?.conversation else { return nil }
          return self?.onConversationMenu?(conversation)
        }
      }
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
      for folded in foldIcons { folded.resolve() }
      // A bar kept across a reload of the config file keeps the colors it was made with otherwise.
      for case let bar as ContextBar in marks { bar.resolveColors() }
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
    case .conversation(let unread, let toYou):
      return ConversationBadge(unread: unread, toYou: toYou)
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
    }
  }

  static func symbol(_ name: String, tint: NSColor, saying: String) -> NSView {
    let image = NSImage(systemSymbolName: name, accessibilityDescription: saying)?
      .withSymbolConfiguration(.init(pointSize: 9, weight: .regular))
    let view = NSImageView(image: image ?? NSImage())
    view.contentTintColor = tint
    return view
  }

  /// The chord that reaches this row, drawn as keycaps as well as the icon rather than instead of
  /// it: before a caption's name, and at the far edge of a pane's first line.
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
  /// A caption reserving a column draws no key in it and still advances past it, which is what
  /// keeps every caption's label on one edge.
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
  nonisolated static func pressable(_ press: Int) -> Bool {
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
  static let paneInset: CGFloat = 16
  static let paneTrailing: CGFloat = 14
  /// Where a tab caption's keys start, beside its fold control.
  static let captionInset: CGFloat = 20
  /// A pane entry's lines, top to bottom, the space between two of them and above and below them
  /// all. `SidebarModel.height(of:)` is their sum.
  static let firstLineHeight: CGFloat = 16
  static let textLineHeight: CGFloat = 15
  static let pillsLineHeight: CGFloat = 14
  static let lineGap: CGFloat = 1
  static let entryPadding: CGFloat = 5
  /// Between a line's text and what sits at its far edge.
  static let edgeGap: CGFloat = 6
  /// The least a pane's state phrase keeps of its line when the directory takes room: a word and
  /// its time, "Blocked 2m".
  static let stateFloor: CGFloat = 56
  /// The least of a machine's tag worth drawing: a row too short for that much leaves it out, and
  /// its hover text still names the machine.
  static let tagFloor: CGFloat = 28

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
    // A helper's one line is too short for the inset an entry's edge takes.
    let edgeInset: CGFloat = shown?.isHelper == true ? 3 : 7
    edgeBar.frame = CGRect(
      x: marked.minX, y: marked.minY + edgeInset, width: 3, height: marked.height - edgeInset * 2)

    let iconLeft = SidebarRowView.paneInset + CGFloat(indent - 1) * SidebarRowView.indent
    let textLeft = iconLeft + StateIcon.size + 7
    let right = bounds.width - SidebarRowView.paneTrailing

    // Lines are handed out from the top, each a slot its text is centered in.
    var top = bounds.height - SidebarRowView.entryPadding
    func slot(_ height: CGFloat) -> CGRect {
      top -= height
      defer { top -= SidebarRowView.lineGap }
      return CGRect(x: textLeft, y: top, width: max(0, right - textLeft), height: height)
    }
    func centered(_ size: NSSize, in line: CGRect, x: CGFloat, width: CGFloat) -> CGRect {
      CGRect(
        x: x, y: line.minY + ((line.height - size.height) / 2).rounded(), width: max(0, width),
        height: size.height)
    }
    let isHelper = shown?.isHelper ?? false
    // A helper's one line is as tall as its row, so its text sits in the middle of it.
    let first =
      isHelper
      ? CGRect(x: textLeft, y: 0, width: max(0, right - textLeft), height: bounds.height)
      : slot(SidebarRowView.firstLineHeight)
    icon.frame = CGRect(
      x: iconLeft, y: (first.midY - StateIcon.size / 2).rounded(), width: StateIcon.size,
      height: StateIcon.size)

    // The chord at the far edge of the first line, where a column of them reads down the list
    // and costs the text nothing on the lines below.
    var firstRight = right
    for key in [secondKey, firstKey] {
      guard let key else { continue }
      key.frame = CGRect(
        x: firstRight - KeycapView.width, y: (first.midY - KeycapView.height / 2).rounded(),
        width: KeycapView.width, height: KeycapView.height)
      firstRight -= KeycapView.width + SidebarRowView.keyGap
    }
    if firstKey != nil || secondKey != nil {
      firstRight -= SidebarRowView.edgeGap - SidebarRowView.keyGap
    }

    if isHelper {
      layoutHelperLine(in: first, right: firstRight)
      return
    }

    // The callsign whole, then the machine's tag in what is left: the tag is cut before the
    // callsign is, and left out when too little of it would show to read.
    let nameSize = name.fittingSize
    let firstRoom = max(0, firstRight - textLeft)
    let nameWidth = min(ceil(nameSize.width), firstRoom)
    name.frame = centered(nameSize, in: first, x: textLeft, width: nameWidth)
    if let tag = machineTag {
      let size = tag.fittingSize
      let spare = firstRoom - nameWidth - SidebarRowView.edgeGap
      let width = spare >= SidebarRowView.tagFloor ? min(size.width, spare) : 0
      tag.isHidden = width == 0
      tag.frame = centered(
        size, in: first, x: textLeft + nameWidth + SidebarRowView.edgeGap, width: width)
    }

    // Every mark at the far edge of a line, the context bar at the very edge so a column of bars
    // lines up down the list: the task line's when the entry has one, whose name or title ends in
    // an ellipsis and says all of itself on hover, and otherwise the state line's.
    func placeMarks(in line: CGRect) -> CGFloat {
      var lineRight = right
      for mark in marks.reversed() {
        let size = mark.fittingSize
        mark.frame = centered(size, in: line, x: lineRight - size.width, width: size.width)
        lineRight -= size.width + (mark is ContextBar ? 8 : SidebarRowView.markGap)
      }
      return lineRight
    }
    let hasTask = taskField.superview != nil
    if hasTask {
      let task = slot(SidebarRowView.textLineHeight)
      let taskRight = placeMarks(in: task)
      taskField.frame = centered(
        taskField.fittingSize, in: task, x: textLeft, width: taskRight - textLeft)
    }

    // The state, then the directory. The directory's last part is where the agent is, and cut to
    // "le…rn" it says nothing, so it is given its whole width first. The state gives way to it,
    // never to less than a word's worth, and ends in an ellipsis when it does.
    var state = slot(SidebarRowView.textLineHeight)
    if !hasTask { state.size.width = max(0, placeMarks(in: state) - textLeft) }
    let stateSize = stateField.superview != nil ? stateField.fittingSize : .zero
    let directorySize = directoryField.superview != nil ? directoryField.fittingSize : .zero
    let stateWanted = ceil(stateSize.width)
    let directoryWidth = min(
      ceil(directorySize.width),
      max(0, state.width - min(stateWanted, SidebarRowView.stateFloor)))
    let stateWidth = min(stateWanted, max(0, state.width - directoryWidth))
    if stateField.superview != nil {
      stateField.frame = centered(stateSize, in: state, x: textLeft, width: stateWidth)
    }
    if directoryField.superview != nil {
      directoryField.frame = centered(
        directorySize, in: state, x: textLeft + stateWidth,
        width: min(ceil(directorySize.width), state.width - stateWidth))
    }

    if let pills {
      pills.frame = slot(SidebarRowView.pillsLineHeight)
    }

    if progressBar.superlayer != nil {
      // Along the bottom edge under the text, the way a download's bar sits under its name.
      let span = max(0, right - textLeft)
      let height = SidebarRowView.progressHeight
      progressTrack.frame = CGRect(x: textLeft, y: 2, width: span, height: height)
      progressBar.frame = CGRect(x: textLeft, y: 2, width: span * progressFilled, height: height)
    }
  }

  /// A helper listed under its maker's entry: its callsign, then its state or else its directory,
  /// with its marks at the far edge, all on the one line.
  private func layoutHelperLine(in line: CGRect, right: CGFloat) {
    func centered(_ size: NSSize, x: CGFloat, width: CGFloat) -> CGRect {
      CGRect(
        x: x, y: line.minY + ((line.height - size.height) / 2).rounded(), width: max(0, width),
        height: size.height)
    }
    var lineRight = right
    for mark in marks.reversed() {
      let size = mark.fittingSize
      mark.frame = centered(size, x: lineRight - size.width, width: size.width)
      lineRight -= size.width + (mark is ContextBar ? 8 : SidebarRowView.markGap)
    }
    machineTag?.isHidden = true
    let nameSize = name.fittingSize
    let nameWidth = min(ceil(nameSize.width), max(0, lineRight - line.minX))
    name.frame = centered(nameSize, x: line.minX, width: nameWidth)
    var cursor = line.minX + nameWidth + SidebarRowView.edgeGap
    // The state when it has one to say, and the directory in whatever is left after it.
    if stateField.superview != nil {
      let size = stateField.fittingSize
      let width = min(ceil(size.width), max(0, lineRight - cursor))
      stateField.frame = centered(size, x: cursor, width: width)
      cursor += width
    }
    if directoryField.superview != nil {
      let size = directoryField.fittingSize
      directoryField.frame = centered(
        size, x: cursor, width: min(ceil(size.width), lineRight - cursor))
    }
  }

  private func layoutOtherRow() {
    var left = SidebarRowView.inset + CGFloat(indent) * SidebarRowView.indent
    if let control = foldControl {
      control.frame = CGRect(
        x: 4, y: (bounds.height - FoldControl.height) / 2, width: FoldControl.width,
        height: FoldControl.height)
      left = SidebarRowView.captionInset
    }
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
    if !foldIcons.isEmpty { layoutFoldedIcons(from: textLeft, right: &right) }
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

extension SidebarRowView {
  /// Puts as many of a folded caption's icons at its trailing edge as leave the name room, and a
  /// count of the rest.
  fileprivate func layoutFoldedIcons(from textLeft: CGFloat, right: inout CGFloat) {
    let step = StateIcon.size + 4
    let nameRoom: CGFloat = 60
    let states = foldIcons.map(\.state)
    let available = max(0, right - textLeft - nameRoom)
    var fit = SidebarModel.foldedIcons(states: states, capacity: Int(available / step))
    var moreWidth: CGFloat = 0
    if fit.more > 0 {
      foldMore.stringValue = "+\(fit.more)"
      moreWidth = ceil(foldMore.fittingSize.width) + 4
      fit = SidebarModel.foldedIcons(
        states: states, capacity: Int(max(0, available - moreWidth) / step))
      foldMore.stringValue = "+\(fit.more)"
      moreWidth = ceil(foldMore.fittingSize.width) + 4
    }
    for (index, icon) in foldIcons.enumerated() { icon.isHidden = !fit.shown.contains(index) }
    if fit.more == 0 { foldMore.removeFromSuperview() }
    if fit.more > 0 {
      if foldMore.superview == nil { addSubview(foldMore) }
      foldMore.isHidden = false
      let size = foldMore.fittingSize
      foldMore.frame = CGRect(
        x: right - size.width, y: (bounds.height - size.height) / 2, width: size.width,
        height: size.height)
      right -= moreWidth
    }
    var x = right - step * CGFloat(fit.shown.count) + 4
    for index in fit.shown {
      foldIcons[index].frame = CGRect(
        x: x, y: ((bounds.height - StateIcon.size) / 2).rounded(), width: StateIcon.size,
        height: StateIcon.size)
      x += step
    }
    right -= step * CGFloat(fit.shown.count) + 4
  }
}

/// A pane's helpers on its entry: a pill each, and at the far edge how many there are with a
/// chevron that says whether they are listed under it (Fencil `helper-pills`).
///
/// A pill's height and fill say its helper's state without its color. Tall for a helper that is
/// blocked or stuck, short for one that is done, between them for the calm states; solid for the
/// three that want somebody, an outline for the calm ones and dashed for the two Muster cannot
/// read, as `StateIcon` draws them. They stay in the order the roster lists them, so a pill keeps
/// its place while its state changes, and when they do not all fit the most urgent are the ones
/// kept (`SidebarModel.foldedIcons`).
@MainActor
final class HelperPillsView: NSView {
  static let pillWidth: CGFloat = 6
  static let pillGap: CGFloat = 2

  /// How tall a helper's pill is.
  static func pillHeight(state: String) -> CGFloat {
    switch SidebarModel.Tier(state: state) {
    case .asks: 14
    case .look: 6
    case .calm, .absent: 10
    }
  }

  let helpers: [SidebarModel.HelperPill]
  let isOpen: Bool
  var onPressed: (() -> Void)?
  var onMenu: (() -> NSMenu?)?

  private let bars: [CAShapeLayer]
  private let more = NSTextField(labelWithString: "")
  private let count = NSTextField(labelWithString: "")
  private let chevron: NSView

  /// The states of the pills drawn now, in order, and the count of those that did not fit (nil
  /// when all did). Meaningful after layout, which is what decides the room.
  var shownStates: [String] { zip(helpers, bars).filter { !$0.1.isHidden }.map(\.0.state) }
  var shownMore: String? { more.isHidden ? nil : more.stringValue }
  var pillFrames: [CGRect] { bars.filter { !$0.isHidden }.map(\.frame) }
  var shownCount: String { count.stringValue }

  init(helpers: [SidebarModel.HelperPill], open: Bool) {
    self.helpers = helpers
    isOpen = open
    bars = helpers.map { _ in CAShapeLayer() }
    chevron = SidebarRowView.symbol(
      open ? "chevron.down" : "chevron.right", tint: .secondaryLabelColor,
      saying: open ? "fold helpers" : "show helpers")
    super.init(frame: .zero)
    wantsLayer = true
    for bar in bars {
      bar.actions = ["path": NSNull(), "position": NSNull(), "bounds": NSNull()]
      layer?.addSublayer(bar)
    }
    for label in [more, count] {
      label.font = .monospacedDigitSystemFont(ofSize: 10, weight: .semibold)
      label.textColor = .secondaryLabelColor
      addSubview(label)
    }
    more.isHidden = true
    count.stringValue = String(helpers.count)
    addSubview(chevron)
    let words = SidebarModel.helpersSummary(helpers)
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel("\(words), \(open ? "listed" : "folded")")
    resolveColors()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    defer { CATransaction.commit() }

    let chevronSize = chevron.fittingSize
    chevron.frame = CGRect(
      x: bounds.width - chevronSize.width, y: (bounds.height - chevronSize.height) / 2,
      width: chevronSize.width, height: chevronSize.height)
    let countSize = count.fittingSize
    count.frame = CGRect(
      x: chevron.frame.minX - 1 - ceil(countSize.width),
      y: (bounds.height - countSize.height) / 2, width: ceil(countSize.width),
      height: countSize.height)

    let step = Self.pillWidth + Self.pillGap
    let states = helpers.map(\.state)
    let room = max(0, count.frame.minX - SidebarRowView.edgeGap)
    func capacity(leaving taken: CGFloat) -> Int { Int(max(0, room - taken + Self.pillGap) / step) }
    var fit = SidebarModel.foldedIcons(states: states, capacity: capacity(leaving: 0))
    var moreWidth: CGFloat = 0
    if fit.more > 0 {
      // The count's own width decides how many fit beside it, and how many fit decides the count,
      // so it is measured at its widest first.
      more.stringValue = "+\(helpers.count)"
      moreWidth = ceil(more.fittingSize.width) + 3
      fit = SidebarModel.foldedIcons(states: states, capacity: capacity(leaving: moreWidth))
      more.stringValue = "+\(fit.more)"
    }
    more.isHidden = fit.more == 0

    removeAllToolTips()
    var x: CGFloat = 0
    for (index, bar) in bars.enumerated() {
      bar.isHidden = !fit.shown.contains(index)
      guard !bar.isHidden else { continue }
      let height = Self.pillHeight(state: states[index])
      bar.frame = CGRect(
        x: x, y: ((bounds.height - height) / 2).rounded(), width: Self.pillWidth, height: height)
      draw(bar, state: states[index])
      addToolTip(
        CGRect(x: x - 1, y: 0, width: step, height: bounds.height), owner: self,
        userData: UnsafeMutableRawPointer(bitPattern: index + 1))
      x += step
    }
    if fit.more > 0 {
      let size = more.fittingSize
      more.frame = CGRect(
        x: x + 1, y: (bounds.height - size.height) / 2, width: ceil(size.width),
        height: size.height)
    }
  }

  /// One pill's shape for its state: solid, an outline, or a dashed outline.
  private func draw(_ bar: CAShapeLayer, state: String) {
    let line: CGFloat = 1.5
    switch SidebarModel.Tier(state: state) {
    case .asks, .look:
      bar.path = CGPath(
        roundedRect: bar.bounds, cornerWidth: 2, cornerHeight: 2, transform: nil)
      bar.lineWidth = 0
      bar.lineDashPattern = nil
    case .calm, .absent:
      bar.path = CGPath(
        roundedRect: bar.bounds.insetBy(dx: line / 2, dy: line / 2), cornerWidth: 1.5,
        cornerHeight: 1.5, transform: nil)
      bar.lineWidth = line
      bar.lineDashPattern = SidebarModel.Tier(state: state) == .absent ? [2, 1.5] : nil
    }
  }

  private func resolveColors() {
    effectiveAppearance.performAsCurrentDrawingAppearance {
      for (helper, bar) in zip(helpers, bars) {
        let color = PaneAppearance.borderColor(state: helper.state).cgColor
        let solid = StateIcon.isFilled(state: helper.state)
        bar.fillColor = solid ? color : nil
        bar.strokeColor = solid ? nil : color
      }
    }
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    resolveColors()
  }

  /// Every point of it is the pills', so a press on them is not a press on the row.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }

  override func menu(for event: NSEvent) -> NSMenu? {
    onMenu?()
  }

  /// What VoiceOver's press does, since it does not send the view a mouse event.
  override func accessibilityPerformPress() -> Bool {
    guard let onPressed else { return false }
    onPressed()
    return true
  }

  /// What hovering the pill at `index` says.
  func toolTip(forPill index: Int) -> String? {
    helpers.indices.contains(index) ? SidebarModel.details(of: helpers[index]) : nil
  }
}

extension HelperPillsView: NSViewToolTipOwner {
  nonisolated func view(
    _ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint,
    userData data: UnsafeMutableRawPointer?
  ) -> String {
    let index = Int(bitPattern: data) - 1
    return MainActor.assumeIsolated { toolTip(forPill: index) ?? "" }
  }
}

/// A tab caption's fold control: a chevron that turns, in a view of its own so that a click on it
/// folds the tab and a click anywhere else on the caption still shows it.
@MainActor
final class FoldControl: NSView {
  static let width: CGFloat = 16
  static let height: CGFloat = 16

  let isFolded: Bool
  var onPressed: (() -> Void)?

  init(folded: Bool) {
    isFolded = folded
    super.init(frame: .zero)
    let words = folded ? "unfold tab" : "fold tab"
    let image = NSImage(
      systemSymbolName: folded ? "chevron.right" : "chevron.down", accessibilityDescription: words)?
      .withSymbolConfiguration(.init(pointSize: 9, weight: .semibold))
    let symbol = NSImageView(image: image ?? NSImage())
    symbol.contentTintColor = .secondaryLabelColor
    symbol.frame = CGRect(x: 2, y: 2, width: 12, height: 12)
    addSubview(symbol)
    toolTip = words
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel(words)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var fittingSize: NSSize { NSSize(width: Self.width, height: Self.height) }

  /// Every point of it is the control's, so a press on the chevron is not a press on the caption.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }
}

/// The conversation with a row's agent, as an envelope with what is unread beside it, which opens
/// the conversation when pressed (Fencil `one-to-one-on-agent-row`).
///
/// Always there on a row that has one, because the row is now the only place that conversation is
/// listed: quiet with nothing unread, and in the accent color once something waits.
@MainActor
final class ConversationBadge: NSView {
  let unread: Int
  let toYou: Int
  var onPressed: (() -> Void)?

  /// Builds the menu for a right-click on the envelope: the conversation's own, Leave Group and
  /// Delete Group, which the row it sits on has no other place for.
  var onMenu: (() -> NSMenu?)?

  private let envelope = NSImageView()
  private let count = NSTextField(labelWithString: "")

  /// What the pointer and VoiceOver are told.
  static func words(unread: Int, toYou: Int) -> String {
    switch (unread, toYou) {
    case (0, _): return "Open your conversation"
    case (let unread, 0): return "Open your conversation: \(unread) unread"
    case (let unread, let toYou):
      return "Open your conversation: \(unread) unread, \(toYou) to you"
    }
  }

  init(unread: Int, toYou: Int) {
    self.unread = unread
    self.toYou = toYou
    super.init(frame: .zero)
    let tint: NSColor = unread > 0 ? .controlAccentColor : .tertiaryLabelColor
    let image = NSImage(systemSymbolName: "envelope.fill", accessibilityDescription: nil)?
      .withSymbolConfiguration(.init(pointSize: 10, weight: .regular))
    envelope.image = image
    envelope.contentTintColor = tint
    addSubview(envelope)
    if unread > 0 {
      count.stringValue = String(unread)
      count.font = .monospacedDigitSystemFont(ofSize: 10, weight: .semibold)
      count.textColor = tint
      addSubview(count)
    }
    let words = Self.words(unread: unread, toYou: toYou)
    toolTip = words
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel(words)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  static let envelopeWidth: CGFloat = 13
  static let height: CGFloat = 14

  override var fittingSize: NSSize {
    NSSize(
      width: Self.envelopeWidth + (unread > 0 ? 2 + ceil(count.fittingSize.width) : 0),
      height: Self.height)
  }

  override func layout() {
    super.layout()
    envelope.frame = CGRect(x: 0, y: 0, width: Self.envelopeWidth, height: bounds.height)
    let text = count.fittingSize
    count.frame = CGRect(
      x: Self.envelopeWidth + 2, y: (bounds.height - text.height) / 2, width: ceil(text.width),
      height: text.height)
  }

  /// Every point of it is the badge's, so a press on it is not a press on the row.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }

  override func menu(for event: NSEvent) -> NSMenu? {
    onMenu?()
  }

  /// What VoiceOver's press does, since it does not send the view a mouse event.
  override func accessibilityPerformPress() -> Bool {
    guard let onPressed else { return false }
    onPressed()
    return true
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

  /// The number it shows.
  let count: Int

  init(count: Int, saying: String) {
    self.count = count
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
  /// Long enough to read a fill against its track without waiting for it to turn red.
  static let width: CGFloat = 32
  static let height: CGFloat = 4

  /// Percent, 0 to 100.
  let used: Float
  private let track = CALayer()
  private let fill = CALayer()

  /// What the fill is painted in now, for the tests.
  var fillColor: CGColor? { fill.backgroundColor }

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
  func resolveColors() {
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
