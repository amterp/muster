import AppKit

/// Something wrong with this window, as the core reported it.
///
/// The shell's mirror of `ProblemsChanged`, on the same terms as `Roster` and `Presentation`:
/// it arrives whole and is drawn, and nothing on this side decides what is wrong or how to
/// say it.
public struct Problem: Equatable {
  /// How much this asks of the person reading it. Spelled by the core; an unknown spelling is
  /// read as the more serious of the two, because under-reporting a problem is the failure
  /// this whole surface exists to fix.
  public enum Severity: Equatable {
    case error
    case warning

    public init(_ spelled: String) {
      self = spelled == "warning" ? .warning : .error
    }
  }

  /// Names the condition, so the same thing reported twice is one problem. Also what a
  /// dismissal remembers.
  public let key: String
  public let severity: Severity

  /// What to tell the person, in the words of whatever found it.
  public let detail: String

  public init(key: String, severity: Severity, detail: String) {
    self.key = key
    self.severity = severity
    self.detail = detail
  }
}

/// What the problems area at the foot of the roster draws.
///
/// Pure, and apart from the view for the reason `SidebarModel` is: whether a dismissed
/// problem reappears is a decision, and a decision inside `layout()` is a decision no test
/// can reach.
public enum ProblemsModel {
  public enum Display: Equatable {
    /// Nothing is wrong, so nothing is drawn - no empty box and no zero. The roster looks
    /// exactly as it did before this feature existed, which is the common case and the one
    /// worth protecting.
    case nothing

    /// Problems worth reading, worst first.
    case raised([Problem])

    /// Everything outstanding has been dismissed, so it collapses to a count somebody can
    /// click. The severity is the worst one outstanding, because that decides the colour and
    /// a red dot behind a yellow one would be a lie about what is waiting.
    case collapsed(count: Int, severity: Problem.Severity)
  }

  /// What to draw, given what is wrong and what somebody has already waved away.
  ///
  /// A dismissal hides a problem and never clears it: the condition is still true, so the
  /// count stays until whatever caused it is fixed. That is also why there is no way to
  /// dismiss something permanently - the alternative is a window that agreed to stop
  /// mentioning a broken config, which is the state this feature was built to end.
  public static func display(problems: [Problem], dismissed: Set<String>) -> Display {
    guard !problems.isEmpty else { return .nothing }
    let unread = problems.filter { !dismissed.contains($0.key) }
    guard unread.isEmpty else { return .raised(unread) }
    let worst: Problem.Severity = problems.contains { $0.severity == .error } ? .error : .warning
    return .collapsed(count: problems.count, severity: worst)
  }

  /// Which dismissals are still about something.
  ///
  /// Called whenever the list changes, so that a problem which cleared and came back is shown
  /// again rather than staying hidden behind a dismissal of the last time it happened. Fixing
  /// a config and breaking it a second time should look like the second time it happened.
  public static func retained(dismissed: Set<String>, outstanding: [Problem]) -> Set<String> {
    dismissed.intersection(outstanding.map(\.key))
  }
}

/// The problems area at the foot of the roster.
///
/// Here rather than over a pane because what it reports is about the window: a config file is
/// not any pane's fault, and a box drawn over one would say it was. The roster is also where
/// somebody already looks to see what needs them, and it has the vertical room a title bar
/// does not.
@MainActor public final class ProblemsView: NSView {
  /// Called when somebody waves the box away.
  public var onDismiss: (() -> Void)?

  /// Called when somebody clicks the collapsed count, wanting it back.
  public var onReveal: (() -> Void)?

  /// How many lines of a message this will draw before ending in an ellipsis.
  ///
  /// A limit rather than the whole text at any length, because the roster's job is listing
  /// agents and a pathological message must not take the window. Counted in lines rather than
  /// points so that `truncatesLastVisibleLine` can do its job - it needs a line limit, and
  /// against an unlimited field a height cap clips mid-sentence instead, which reads as a
  /// broken box rather than a long message. `toolTip` carries the rest either way.
  ///
  /// The refusals this carries today run to about eight lines at a roster's width, so this is
  /// rarely what decides.
  private static let messageLines = 14

  private static let inset: CGFloat = 8
  private static let dotSize: CGFloat = 8
  private static let collapsedHeight: CGFloat = 24

  /// The row along the top carrying the dot, how many others are waiting, and the way out.
  /// Kept off the message's own lines so a box with two problems in it cannot draw the count
  /// over the sentence.
  private static let headerHeight: CGFloat = 18

  private let dot = ProblemDot()
  private let message = NSTextField(wrappingLabelWithString: "")
  private let dismiss = NSButton()
  private let count = NSTextField(labelWithString: "")
  private var display: ProblemsModel.Display = .nothing

  public override init(frame: NSRect) {
    super.init(frame: frame)
    // Selectable so a refusal naming a value can be copied out of the window rather than
    // retyped from it.
    message.isSelectable = true
    message.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    message.maximumNumberOfLines = ProblemsView.messageLines
    message.cell?.truncatesLastVisibleLine = true
    addSubview(message)

    count.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
    addSubview(count)
    addSubview(dot)

    dismiss.title = "✕"
    dismiss.isBordered = false
    dismiss.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    dismiss.target = self
    dismiss.action = #selector(dismissClicked)
    dismiss.toolTip = "Hide this until it changes. It stays counted until the cause is fixed."
    addSubview(dismiss)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Draws what the model decided, and says nothing when there is nothing to say.
  public func show(_ display: ProblemsModel.Display) {
    self.display = display
    switch display {
    case .nothing:
      isHidden = true
    case .raised(let problems):
      isHidden = false
      // The worst one, whole. A stack of boxes would compete with the list for a window that
      // has one thing wrong with it almost every time; the rest are reachable by fixing this
      // one, which is the order somebody would work through them anyway.
      let worst = problems[0]
      message.stringValue = worst.detail
      message.toolTip = worst.detail
      message.isHidden = false
      dismiss.isHidden = false
      dot.severity = worst.severity
      // Only the count, because the others are reachable by fixing this one - which is the
      // order somebody would work through them in anyway.
      count.stringValue = problems.count > 1 ? "+\(problems.count - 1) more" : ""
      count.isHidden = problems.count < 2
    case .collapsed(let outstanding, let severity):
      isHidden = false
      message.isHidden = true
      dismiss.isHidden = true
      dot.severity = severity
      count.stringValue = String(outstanding)
      count.isHidden = false
      toolTip = "\(outstanding) still outstanding. Click to read."
    }
    needsLayout = true
  }

  /// How much of the roster this wants, which is none when nothing is wrong.
  public func height(forWidth width: CGFloat) -> CGFloat {
    switch display {
    case .nothing:
      return 0
    case .collapsed:
      return ProblemsView.collapsedHeight
    case .raised:
      // The field's own line limit is what bounds this, so there is no second cap to keep in
      // step with it.
      let fits = message.sizeThatFits(
        NSSize(
          width: max(0, width - ProblemsView.inset * 2), height: .greatestFiniteMagnitude))
      return fits.height + ProblemsView.headerHeight + ProblemsView.inset
    }
  }

  public override func layout() {
    super.layout()
    let inset = ProblemsView.inset
    let dotSize = ProblemsView.dotSize
    switch display {
    case .nothing:
      return
    case .collapsed:
      dot.frame = CGRect(
        x: inset, y: (bounds.height - dotSize) / 2, width: dotSize, height: dotSize)
      let size = count.fittingSize
      count.frame = CGRect(
        x: inset * 2 + dotSize, y: (bounds.height - size.height) / 2,
        width: max(0, bounds.width - inset * 3 - dotSize), height: size.height)
    case .raised:
      // Unflipped, so the header is at the top of the box and the message hangs below it.
      let header = ProblemsView.headerHeight
      let top = bounds.height - header
      dot.frame = CGRect(
        x: inset, y: top + (header - dotSize) / 2, width: dotSize, height: dotSize)
      let closeSize = dismiss.fittingSize
      dismiss.frame = CGRect(
        x: bounds.width - closeSize.width - inset / 2, y: top + (header - closeSize.height) / 2,
        width: closeSize.width, height: closeSize.height)
      let countLeft = inset * 2 + dotSize
      let countSize = count.fittingSize
      count.frame = CGRect(
        x: countLeft, y: top + (header - countSize.height) / 2,
        width: max(0, bounds.width - countLeft - closeSize.width - inset),
        height: countSize.height)
      message.frame = CGRect(
        x: inset, y: inset, width: max(0, bounds.width - inset * 2),
        height: max(0, bounds.height - header - inset))
    }
  }

  public override func draw(_ dirty: NSRect) {
    guard display != .nothing else { return }
    // A tinted ground rather than a border, so the area reads as part of the roster it sits in
    // rather than as a dialog that landed on top of it.
    dot.severity.tint.withAlphaComponent(0.12).setFill()
    bounds.fill()
    NSColor.separatorColor.setStroke()
    let top = NSBezierPath()
    top.move(to: CGPoint(x: 0, y: bounds.maxY - 0.5))
    top.line(to: CGPoint(x: bounds.maxX, y: bounds.maxY - 0.5))
    top.stroke()
  }

  public override func mouseDown(with event: NSEvent) {
    guard case .collapsed = display else {
      super.mouseDown(with: event)
      return
    }
    onReveal?()
  }

  @objc private func dismissClicked() {
    onDismiss?()
  }
}

/// The coloured dot that says how much a problem is asking for.
@MainActor final class ProblemDot: NSView {
  var severity: Problem.Severity = .error {
    didSet { needsDisplay = true }
  }

  override func draw(_ dirty: NSRect) {
    severity.tint.setFill()
    NSBezierPath(ovalIn: bounds).fill()
  }
}

extension Problem.Severity {
  /// System colours rather than Muster's own, because these two have platform meanings and
  /// `[colors]` names what the panes look like. It also gets dark mode for free, which a pair
  /// of hard-coded hexes would not.
  var tint: NSColor {
    switch self {
    case .error: return .systemRed
    case .warning: return .systemYellow
    }
  }
}
