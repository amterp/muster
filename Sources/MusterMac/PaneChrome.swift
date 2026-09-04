import AppKit

/// What a pane's agent state looks like, and what the window says about the daemon.
///
/// Pure and separate from the view, because these are the decisions - which state gets
/// which color, what a stale backend does to a title - and a decision inside `draw` is a
/// decision no test can reach. The views below only apply what this returns.
public enum PaneAppearance {
  /// The color for an agent state, by the backend's own spelling.
  ///
  /// Anything unrecognized falls to `unknown` rather than to `idle`. A state we could not
  /// read is not an agent that finished, and coloring it as one is how a user learns to stop
  /// trusting the colors.
  ///
  /// `unknown` is fainter than `idle` rather than another hue. The two used to share a gray,
  /// which cost nothing while the only thing painted was a border - neither state draws one
  /// - and became wrong the moment the sidebar drew a dot for every row, where a pane whose
  /// harness could not be read would have been indistinguishable from one with nothing to
  /// do. Faint rather than colorful because `unknown` is an absence of information, and a
  /// hue of its own would read as a fifth thing an agent can be doing.
  ///
  /// `working` is cyan rather than the blue it was, for two reasons that arrived separately.
  /// It collided with the focus ring, which follows the macOS accent and is blue on the
  /// default one - four points of blue saying two different things. And plain ANSI blue is the
  /// least legible of the sixteen on a dark background, which matters more for this row than
  /// any other because `working` is the state a window spends most of its time in. Cyan is
  /// legible in both mediums, distinct from green and from orange at a glance, and calm, which
  /// is what the resting-but-busy state should be while `blocked` is the loud one.
  ///
  /// This table is the legend *by default*, and `muster window` paints the same one in the
  /// terminal's own sixteen (`crates/muster-cli/src/render.rs`, `agent_style`) - the window is
  /// canonical because that is where attention lives, and the surface with the smaller audience
  /// is the one that moves. That medium has no orange, so blocked is yellow there, and it
  /// leaves idle and unknown bare because no color is what resting looks like in a list of
  /// words. Nothing can check the two across the language line, so a row changed here is
  /// changed in that file and in `docs/architecture.md` too.
  ///
  /// A person may repaint these (`[colors] agent_*`), and only here: the CLI keeps its fixed
  /// sixteen so that `muster window` reads the same on anybody's machine. So this is the
  /// default rather than the whole answer, and it stays the canonical *default* - the file
  /// overrides a row, it does not move the row the CLI is pinned against.
  @MainActor
  public static func borderColor(state: String) -> NSColor {
    if let chosen = chosen(state: state) { return chosen }
    return defaultBorderColor(state: state)
  }

  /// The legend Muster ships, with nothing configured. What the tripwire pins, and what
  /// `docs/architecture.md` states.
  public static func defaultBorderColor(state: String) -> NSColor {
    switch state {
    case "working": return NSColor.systemCyan
    case "blocked": return NSColor.systemOrange
    case "done": return NSColor.systemGreen
    case "idle": return NSColor.systemGray
    default: return NSColor.tertiaryLabelColor
    }
  }

  /// Which pane a keystroke would reach.
  ///
  /// Unset follows `controlAccentColor`, and that is a decision rather than a fallback: the
  /// accent is the platform's own answer to "which thing has focus", and it already tracks a
  /// choice the person made in System Settings. Ignoring that would be worse than the collision
  /// this ring was redrawn for.
  @MainActor
  public static var focusColor: NSColor {
    configured.focusRing.flatMap(NSColor.init(hex:)) ?? NSColor.controlAccentColor
  }

  /// What the config file said about Muster's own chrome, or nothing.
  ///
  /// Held rather than asked for per paint, the way `DividerView.color` is: a border is applied
  /// on every state change on every pane, and a round trip to the core per repaint would put
  /// the config file on the render path. Set from the one event that carries it, which is also
  /// what makes a saved file take effect without a relaunch.
  @MainActor private(set) static var configured: Core.Chrome = .none

  /// Takes a new answer. The caller repaints; nothing here reaches a view.
  @MainActor
  public static func adopt(chrome: Core.Chrome) {
    configured = chrome
  }

  /// Parsed rather than trusted blindly, on the same terms as the divider: the core already
  /// refused anything malformed when it read the file, so a value that fails here means the two
  /// sides disagree about the format - and the shipped colour is a better answer than black.
  @MainActor
  private static func chosen(state: String) -> NSColor? {
    let named: String?
    switch state {
    case "working": named = configured.agents.working
    case "blocked": named = configured.agents.blocked
    case "done": named = configured.agents.done
    case "idle": named = configured.agents.idle
    case "unknown": named = configured.agents.unknown
    // Every other spelling is a state herdr invented since this was written, and it is drawn
    // as unknown rather than as itself - the same rule the default table follows, and for the
    // same reason: a state we could not read is not a fifth thing an agent can be doing.
    default: named = configured.agents.unknown
    }
    return named.flatMap(NSColor.init(hex:))
  }

  /// **How thick each ring is, and how far apart.** Here rather than on the view for the
  /// reason the colours are: this is the decision the two rings rest on, and a number inside
  /// `layout` is a number no test can reach.
  ///
  /// The two used to be equal weights side by side, which read as one four-point edge
  /// whatever colours they carried. They differ in kind now because they have to: the focus
  /// ring follows `controlAccentColor`, the accent a person picks in System Settings, so any
  /// fixed state palette collides with somebody's - green with `done`, orange with `blocked`.
  /// Weight and a gap are legible whatever the accent turns out to be.
  ///
  /// The state ring keeps the whole of what it had, because it is what the product is about.
  /// The focus ring is the one that gives way: it says one small thing about the window rather
  /// than anything about an agent.
  public static let stateWidth: CGFloat = 2
  public static let focusGap: CGFloat = 2
  public static let focusWidth: CGFloat = 1

  /// How much of a pane's edge is chrome rather than terminal - the sum, so a surface cannot
  /// end up under a ring by arithmetic that drifted.
  public static let inset: CGFloat = stateWidth + focusGap + focusWidth

  /// Whether a state deserves a visible border at all.
  ///
  /// Idle and unknown do not. Every pane carrying a colored edge all the time is every
  /// pane carrying none: the border exists to be noticed, and it is only noticeable if
  /// the resting state is bare.
  public static func isHighlighted(state: String) -> Bool {
    state == "working" || state == "blocked" || state == "done"
  }

  /// How big the number drawn over a pane should be, for a pane this size.
  ///
  /// A share of the pane's shorter side, so one number fills a tall narrow split and a wide
  /// short one alike rather than being sized for whatever arrangement it was designed against.
  /// Clamped at both ends: past the ceiling it stops being readable as a digit and starts being
  /// a shape, and under the floor a pane split four ways would draw something too small to find
  /// - which is exactly the window where finding it matters.
  ///
  /// The share and both clamps doubled together, which is what keeps a big pane and a small one
  /// changing by the same amount: on anything roomy the ceiling is what governs, so moving the
  /// share alone would have left those panes exactly where they were.
  public static func badgeSize(in bounds: NSSize) -> CGFloat {
    min(max(min(bounds.width, bounds.height) * 0.8, 56), 264)
  }

  /// The window title: which pane the keyboard feeds, and whether the daemon is still there.
  ///
  /// Agent state used to be here and is not any more, because a window with fifteen panes has
  /// fifteen agent states and one title - the per-pane borders carry that now. What survives
  /// is the part that is genuinely per-window: a window rendering an hour-old session as
  /// though it were live is the worst failure available to this product, and it is
  /// indistinguishable from a working one without saying so.
  ///
  /// Zoom is here for a different reason: a zoomed tab and a tab with one pane look identical
  /// on screen, so without this a user has no way to learn why their other panes vanished.
  ///
  /// Three different windows have no pane at all, they look identical on screen, and they
  /// want opposite reactions - so each says which it is. `rendererCheck` is the one with no
  /// daemon behind it by design. `problem` is the one that asked for `w9:p99` and got
  /// nothing. Neither is a window whose panes were all closed, which is an ordinary state to
  /// be in and recoverable from, and titling that one as the renderer check told a user their
  /// window was a diagnostic mode they had never asked for.
  ///
  /// `daemon` names whose health this is, because a window can show more than one and only
  /// the unhealthiest reaches the title. Empty leaves it out, which is what a window with
  /// nothing attached has to say.
  /// `unseenProblems` is how many problems have nowhere else to appear, which is the count
  /// outstanding whenever the roster is off screen and zero whenever it is on. The roster says
  /// this properly, with the sentence and what to do about it; the title only has room to say
  /// that there is something, and a window too narrow for a roster would otherwise be a window
  /// that says nothing at all.
  public static func title(
    paneID: String?, zoomed: Bool, health: String, detail: String, daemon: String = "",
    problem: String? = nil, rendererCheck: Bool = false, unseenProblems: Int = 0
  ) -> String {
    guard let paneID, !paneID.isEmpty else {
      if rendererCheck { return "muster (renderer check - keyboard not connected)" }
      if let problem { return "muster - \(problem)" }
      // The health too, because the commonest reason a window has no panes and did not ask
      // to is that the daemon holding them went away.
      return "muster - no panes" + reported(health: health, detail: detail, daemon: daemon)
        + counted(problems: unseenProblems)
    }
    var title = "muster - \(paneID)"
    if zoomed {
      title += " · zoomed"
    }
    return title + reported(health: health, detail: detail, daemon: daemon)
      + counted(problems: unseenProblems)
  }

  /// What a title says about problems it is the only place to mention.
  private static func counted(problems: Int) -> String {
    switch problems {
    case ..<1: return ""
    case 1: return " · 1 problem"
    default: return " · \(problems) problems"
    }
  }

  /// What a title says about a daemon, which is nothing at all while it is answering.
  private static func reported(health: String, detail: String, daemon: String) -> String {
    let named = daemon.isEmpty ? "" : " \(daemon)"
    switch health {
    case "connected", "":
      return ""
    case "stale":
      return detail.isEmpty ? " · stale\(named)" : " · stale\(named) (\(detail))"
    default:
      return " · \(health)\(named)"
    }
  }
}

/// Holds one surface, draws the agent state around it, and says whether it has the keyboard.
///
/// A container rather than the surface itself, because libghostty attaches its own Metal
/// layer to the view it is handed and a border on that layer is a fight over ownership -
/// not a thing to discover at render time.
///
/// Two edges, not one, and they mean different things. The outer is the agent's state, which
/// is the whole point of the product; the inner says which pane a keystroke would go to. One
/// edge carrying both would make a working pane and a focused pane indistinguishable, which
/// is exactly the confusion these are for clearing up.
///
/// **They differ in kind, not only in hue, and they have to.** The focus ring follows
/// `controlAccentColor` - the accent a person chooses in System Settings, which Muster does
/// not pick and cannot know. So any fixed state palette collides with *some* accent: green
/// with `done`, orange with `blocked`, blue with `working` on the default. Painting the two
/// different colours fixes whichever collision you happen to have and leaves the same bug for
/// the next person. A thick outer ring, a gap, and a thin inner one is legible whatever the
/// accent turns out to be.
@MainActor
public final class PaneChrome: NSView {
  /// The pane this view is showing, or nil for the renderer check.
  public private(set) var paneID: String?
  public private(set) var state: String = "unknown"
  public private(set) var isFocused = false

  public let surface: SurfaceView

  /// Called when somebody clicks this pane, meaning they want the keyboard here.
  public var onFocusRequested: ((String) -> Void)?

  /// Called when the wheel moves over this pane. Never moves the keyboard: a wheel scrolls
  /// what the pointer is over and a click is what asks for the keyboard, and keeping the two
  /// apart is what lets you read one agent while typing into another.
  public var onScrollRequested: ((_ paneID: String, _ direction: String, _ delta: Double) -> Void)?

  private let focusRing = CALayer()

  /// The number a numbered chord would reach this pane by, drawn over it while one is being
  /// typed. Added over the surface rather than beside it, the way the find bar is.
  private let badge = PaneBadge(frame: .zero)

  /// Asks the core where this pane is looking, one request at a time.
  ///
  /// Coalesced for the reason a divider drag and a find needle are: a wheel produces events
  /// faster than a daemon answers, and a selection that lags the scroll by a frame is fine
  /// while a scroll that waits for a round trip is not. Only the newest answer is used, which
  /// is exactly what placing a selection wants.
  private let viewports: LatestRequestSender<Core.Viewport>

  /// How many scrolls this pane has been asked for.
  ///
  /// Compared against what it was when a drag ended, which is the one thing that can make a
  /// pinned selection wrong: the viewport that arrives has to be the one the drag ended under,
  /// and a wheel touched in between makes it a different pane position.
  private var scrolls: UInt64 = 0
  private var scrollsWhenSelected: UInt64 = 0

  public init(frame: NSRect, surface: SurfaceView, dispatcher: Dispatcher = Core.dispatcher) {
    self.surface = surface
    viewports = LatestRequestSender(
      what: "viewport", queue: "muster.viewport", dispatcher: dispatcher,
      read: { response in
        readResponse(response).flatMap { decoded in
          guard case .paneViewport(let viewport) = decoded.payload else {
            return .failure(
              Refused("the core answered a viewport read with something other than a viewport"))
          }
          return .success(Core.read(viewport))
        }
      })
    super.init(frame: frame)
    wantsLayer = true
    layer?.addSublayer(focusRing)
    // Never animated. A frame set from a layout pass would otherwise slide into place over a
    // quarter second, and a surface whose bounds lag its host by that long renders the
    // previous arrangement while the daemon has already moved on.
    focusRing.actions = ["position": NSNull(), "bounds": NSNull(), "borderColor": NSNull()]
    addSubview(surface)
    surface.onClick = { [weak self] in
      guard let self, let paneID = self.paneID else { return }
      self.onFocusRequested?(paneID)
    }
    surface.onScroll = { [weak self] direction, delta in
      guard let self, let paneID = self.paneID else { return }
      self.scrolls += 1
      self.onScrollRequested?(paneID, direction, delta)
      // Only while something is selected, so an ordinary scroll costs the round trip it
      // always cost and nothing more.
      if self.surface.isTrackingSelection { self.askWhereThePaneIsLooking() }
    }
    // A drag has ended, and the cells it covered are screen cells until they are counted from
    // the bottom of the pane instead. That needs the pane's own position, which is a round
    // trip - so the view reports and this asks.
    surface.onSelectionMade = { [weak self] in
      guard let self else { return }
      self.scrollsWhenSelected = self.scrolls
      self.askWhereThePaneIsLooking()
    }
    viewports.onAnswer = { [weak self] answer, _ in
      guard let self else { return }
      self.surface.applyViewport(
        try? answer.get(), movedSince: self.scrolls != self.scrollsWhenSelected)
    }
    // After the surface, so it composites over libghostty's own layer rather than under it.
    addSubview(badge)
    badge.isHidden = true
    applyAppearance()
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  private func askWhereThePaneIsLooking() {
    guard let paneID else { return }
    var read = Muster_ReadViewport()
    read.paneID = paneID
    var request = Muster_Request()
    request.readViewport = read
    viewports.send(request)
  }

  public func attach(paneID: String?) {
    self.paneID = paneID
    applyAppearance()
  }

  /// Takes a state change, if it is about this pane.
  ///
  /// The core sends every pane's transitions, because the sidebar wants them and the log
  /// wants them all. A window showing several panes has to filter, and one that forgot to
  /// would show its neighbor's agent as its own.
  public func apply(paneID: String, state: String) {
    guard paneID == self.paneID else { return }
    self.state = state
    applyAppearance()
  }

  /// Takes the colours in force again, for a pane that was already drawn when they changed.
  ///
  /// The state has not moved, so nothing else here would repaint: a border is applied when a
  /// transition arrives, and saving a config file is not one.
  public func recolor() {
    applyAppearance()
  }

  public func apply(focused: Bool) {
    guard focused != isFocused else { return }
    isFocused = focused
    applyAppearance()
  }

  /// Draws the number a chord would reach this pane by, or nothing for zero.
  ///
  /// Only ever non-zero under `numbered_chords = "tab_then_pane"` while a press has named this
  /// pane's tab, so the resting window carries nothing extra. What number that is comes off the
  /// roster - the same field the agent list draws - rather than being counted here, so the
  /// digit on the pane and the digit on its row cannot come apart.
  public func apply(badge reached: Int) {
    badge.apply(number: reached)
  }

  /// Whether a number is drawn over this pane right now.
  public var badgeShown: Bool { !badge.isHidden }

  /// The badge itself, so a test can check that a click did not land on it.
  var badgeView: NSView { badge }

  public override func layout() {
    super.layout()
    surface.frame = bounds.insetBy(dx: PaneAppearance.inset, dy: PaneAppearance.inset)
    // Inside the state ring and the gap, so the two never touch. A layer's border is drawn
    // inward from its frame, which is why this is the outer edge of the focus ring rather
    // than its inner one.
    let focusInset = PaneAppearance.stateWidth + PaneAppearance.focusGap
    focusRing.frame = bounds.insetBy(dx: focusInset, dy: focusInset)
    badge.frame = bounds
  }

  private func applyAppearance() {
    let highlighted = PaneAppearance.isHighlighted(state: state)
    layer?.borderWidth = highlighted ? PaneAppearance.stateWidth : 0
    layer?.borderColor = PaneAppearance.borderColor(state: state).cgColor
    focusRing.borderWidth = PaneAppearance.focusWidth
    focusRing.borderColor =
      isFocused ? PaneAppearance.focusColor.cgColor : NSColor.clear.cgColor
    needsLayout = true
  }
}

/// One large number over a pane, while a two-stage numbered chord is being typed.
///
/// The agent list already draws these, and drawing them again here is not redundancy: under
/// `numbered_chords = "tab_then_pane"` the second press picks between panes, and the panes are
/// what somebody is looking at while deciding. Reading a number off a list at the edge of the
/// window and mapping it back onto a split is the work this saves - and it is the only
/// indicator at all when the list is closed.
///
/// **Transparent to the mouse.** A click on a pane already asks for the keyboard, and a badge
/// that swallowed one would make the numbers look pressable and not be. Returning nil from
/// `hitTest` sends the click to the surface underneath, which is what makes "click the number
/// you can see" work without a second way to focus a pane.
@MainActor
final class PaneBadge: NSView {
  private var number = 0

  func apply(number reached: Int) {
    guard reached != number else { return }
    number = reached
    isHidden = reached < 1
    needsDisplay = true
  }

  /// Never the mouse's. See the note above.
  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  override func draw(_ dirty: NSRect) {
    guard number >= 1 else { return }
    // A halo in the page's own background rather than a plate behind the digit: a terminal is
    // mostly background with text scattered over it, so a rectangle would cover what somebody
    // is reading while a glow only separates the digit from whatever it happens to land on.
    let halo = NSShadow()
    halo.shadowColor = NSColor.textBackgroundColor.withAlphaComponent(0.85)
    halo.shadowBlurRadius = 16
    halo.shadowOffset = .zero

    let drawn = NSAttributedString(
      string: String(number),
      attributes: [
        .font: NSFont.monospacedDigitSystemFont(
          ofSize: PaneAppearance.badgeSize(in: bounds.size), weight: .bold),
        // Faint enough to read the pane through, strong enough to find at a glance. The pane
        // underneath is still the thing being chosen between, so the number sits over it
        // rather than replacing it.
        .foregroundColor: NSColor.labelColor.withAlphaComponent(0.38),
        .shadow: halo,
      ])
    let size = drawn.size()
    drawn.draw(
      at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
  }

  /// Kept even though it only calls up: defining `init?(coder:)` below stops `NSView`'s own
  /// designated initialisers being inherited, and this is the one every caller uses.
  override init(frame: NSRect) {
    super.init(frame: frame)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }
}
