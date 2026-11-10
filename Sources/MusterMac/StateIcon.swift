import AppKit

/// An agent's state as a shape, so that a state reads without its color.
///
/// One `CAShapeLayer` path per state, drawn from the 16-unit artwork the redesign was mocked up
/// in. Not SF Symbols: those have no filled octagon, their weights do not match one another at
/// 14 points, and what they look like changes with the OS.
///
/// The working ring turns slowly, so an agent at work reads as one without drawing the eye, and is
/// held still for somebody who asked macOS to reduce motion.
///
/// Filled for the states that want the person (blocked, stuck, done), outline for the calm ones
/// (working, waiting, idle) and a dashed or slashed ring for the two that say nothing about the
/// agent (unknown, unreachable). Marks inside a filled shape are cut out of it, so whatever is
/// behind the icon - a tinted row, a header - shows through them.
final class StateIcon: CAShapeLayer {
  static let size: CGFloat = 14

  /// The state this draws, by the backend's own spelling. An unrecognized one draws as unknown,
  /// the rule `PaneAppearance` follows for its colors.
  private(set) var state = "unknown"

  /// What it is painted in. Held as an `NSColor` because a layer keeps a resolved `CGColor`, so one
  /// set once would keep light mode's in dark; `resolve()` paints it again under the drawing
  /// appearance in force when it is called.
  private(set) var color = NSColor.secondaryLabelColor

  override init() {
    super.init()
    actions = ["path": NSNull(), "fillColor": NSNull(), "bounds": NSNull(), "position": NSNull()]
    bounds = CGRect(x: 0, y: 0, width: Self.size, height: Self.size)
  }

  override init(layer: Any) {
    super.init(layer: layer)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  func show(state: String, color: NSColor) {
    self.state = Self.known.contains(state) ? state : "unknown"
    self.color = color
    let drawn = Self.drawing(for: self.state, size: Self.size)
    path = drawn.path
    fillRule = drawn.rule
    resolve()
    turnIfWorking()
  }

  /// The share of the working ring left open, and how long one turn of it takes (Fencil
  /// `working-icon`).
  static let workingGap = 0.2
  static let turnSeconds: CFTimeInterval = 4
  private static let workingRadius = 5.6
  private static let workingWidth = 2.2
  private static let turnKey = "turn"

  /// Whether the person asked macOS for less motion. Replaceable so a test can answer either way.
  nonisolated(unsafe) static var reducesMotion: () -> Bool = {
    NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
  }

  /// Whether an icon showing `state` turns: the working one, unless the person asked for less
  /// motion, when it is the same ring held still.
  static func turns(state: String) -> Bool {
    state == "working" && !reducesMotion()
  }

  /// Whether this icon is turning now.
  var isTurning: Bool { animation(forKey: Self.turnKey) != nil }

  private func turnIfWorking() {
    let turns = Self.turns(state: state)
    guard turns else {
      removeAnimation(forKey: Self.turnKey)
      return
    }
    guard !isTurning else { return }
    let turn = CABasicAnimation(keyPath: "transform.rotation.z")
    turn.fromValue = 0
    // Clockwise, in a layer whose y runs up.
    turn.toValue = -2 * Double.pi
    turn.duration = Self.turnSeconds
    turn.repeatCount = .infinity
    turn.isRemovedOnCompletion = false
    add(turn, forKey: Self.turnKey)
  }

  /// A layer leaving the tree loses its animations, and a row takes its icon out and puts it back
  /// each time it is shown, so the turn starts again whenever the icon comes back in.
  override func action(forKey event: String) -> CAAction? {
    if event == kCAOnOrderIn { turnIfWorking() }
    return super.action(forKey: event)
  }

  func resolve() {
    fillColor = color.cgColor
  }

  static let known: Set<String> = [
    "working", "blocked", "stuck", "waiting", "done", "idle", "unknown", "unreachable",
  ]

  /// Whether a state is drawn as a solid shape rather than an outline.
  static func isFilled(state: String) -> Bool {
    ["blocked", "stuck", "done"].contains(state)
  }

  /// A state's shape at `size` points, and the fill rule that makes its cut-outs cut out.
  static func drawing(for state: String, size: CGFloat) -> (
    path: CGPath, rule: CAShapeLayerFillRule
  ) {
    let scale = size / artwork
    // The artwork is y-down, as an SVG is, and a layer's geometry is y-up.
    let flip = CGAffineTransform(translationX: 0, y: size).scaledBy(x: scale, y: -scale)
    let drawn = artworkPath(for: state)
    return (drawn.path.copy(using: [flip]) ?? drawn.path, drawn.rule)
  }

  /// The side of the square the shapes below are drawn in.
  private static let artwork: CGFloat = 16

  private static func artworkPath(for state: String) -> (
    path: CGPath, rule: CAShapeLayerFillRule
  ) {
    switch state {
    case "working":
      // An open ring, which turns while the agent works (`turns(state:)`) and held still reads as
      // a turn in progress.
      let ring = 2 * Double.pi * workingRadius
      // A round end reaches half the line's width into the gap, so the arc stops that much short
      // of the share of the ring the gap shows.
      let drawn = ring * (1 - workingGap) - workingWidth
      let arc = CGMutablePath()
      arc.addArc(
        center: CGPoint(x: 8, y: 8), radius: workingRadius, startAngle: -.pi / 2,
        endAngle: -.pi / 2 + drawn / workingRadius, clockwise: false)
      return (outline(of: arc, width: workingWidth), .nonZero)
    case "blocked":
      let path = CGMutablePath()
      path.addEllipse(in: circle(radius: 7.5))
      path.addPath(
        CGPath(
          roundedRect: CGRect(x: 6.9, y: 3.3, width: 2.2, height: 6.1),
          cornerWidth: 1.1, cornerHeight: 1.1, transform: nil))
      path.addEllipse(in: CGRect(x: 8 - 1.3, y: 12 - 1.3, width: 2.6, height: 2.6))
      return (path, .evenOdd)
    case "stuck":
      let path = CGMutablePath()
      path.move(to: CGPoint(x: 5.1, y: 0.6))
      for point in [
        (10.9, 0.6), (15.4, 5.1), (15.4, 10.9), (10.9, 15.4), (5.1, 15.4),
        (0.6, 10.9), (0.6, 5.1),
      ] {
        path.addLine(to: CGPoint(x: point.0, y: point.1))
      }
      path.closeSubpath()
      path.addPath(
        CGPath(
          roundedRect: CGRect(x: 3.9, y: 6.9, width: 8.2, height: 2.2),
          cornerWidth: 1.1, cornerHeight: 1.1, transform: nil))
      return (path, .evenOdd)
    case "waiting":
      // An hourglass: two caps and the two curves between them.
      let glass = CGMutablePath()
      glass.move(to: CGPoint(x: 4.2, y: 2.2))
      glass.addLine(to: CGPoint(x: 11.8, y: 2.2))
      glass.move(to: CGPoint(x: 4.2, y: 13.8))
      glass.addLine(to: CGPoint(x: 11.8, y: 13.8))
      glass.move(to: CGPoint(x: 5, y: 2.4))
      glass.addCurve(
        to: CGPoint(x: 11, y: 8), control1: CGPoint(x: 5, y: 5.4), control2: CGPoint(x: 11, y: 6))
      glass.addCurve(
        to: CGPoint(x: 5, y: 13.6), control1: CGPoint(x: 11, y: 10),
        control2: CGPoint(x: 5, y: 10.6))
      glass.move(to: CGPoint(x: 11, y: 2.4))
      glass.addCurve(
        to: CGPoint(x: 5, y: 8), control1: CGPoint(x: 11, y: 5.4), control2: CGPoint(x: 5, y: 6))
      glass.addCurve(
        to: CGPoint(x: 11, y: 13.6), control1: CGPoint(x: 5, y: 10),
        control2: CGPoint(x: 11, y: 10.6))
      return (outline(of: glass, width: 1.7), .nonZero)
    case "done":
      let path = CGMutablePath()
      path.addEllipse(in: circle(radius: 7.5))
      // The check is a polygon rather than a stroke, because a stroked polyline overlaps itself
      // at its corner and an even-odd fill would show the overlap as a speck of the circle.
      path.addPath(
        thickPolyline(
          [CGPoint(x: 4.6, y: 8.3), CGPoint(x: 7, y: 10.7), CGPoint(x: 11.5, y: 5.7)], width: 2))
      return (path, .evenOdd)
    case "idle":
      let ring = CGMutablePath()
      ring.addEllipse(in: circle(radius: 5.6))
      return (outline(of: ring, width: 2), .nonZero)
    case "unreachable":
      let path = CGMutablePath()
      let ring = CGMutablePath()
      ring.addEllipse(in: circle(radius: 5.6))
      path.addPath(outline(of: ring, width: 1.8))
      let slash = CGMutablePath()
      slash.move(to: CGPoint(x: 2.6, y: 13.4))
      slash.addLine(to: CGPoint(x: 13.4, y: 2.6))
      path.addPath(outline(of: slash, width: 1.8))
      return (path, .nonZero)
    default:
      // Unknown: the idle ring, broken into dashes.
      let ring = CGMutablePath()
      ring.addEllipse(in: circle(radius: 5.6))
      let dashed = ring.copy(dashingWithPhase: 0, lengths: [2.3, 2.1])
      return (outline(of: dashed, width: 2, cap: .butt), .nonZero)
    }
  }

  /// A line through three points, `width` thick, with a mitered corner and square ends, as one
  /// polygon that does not cross itself.
  private static func thickPolyline(_ points: [CGPoint], width: CGFloat) -> CGPath {
    let (start, corner, end) = (points[0], points[1], points[2])
    func unit(_ from: CGPoint, _ to: CGPoint) -> CGPoint {
      let length = hypot(to.x - from.x, to.y - from.y)
      return CGPoint(x: (to.x - from.x) / length, y: (to.y - from.y) / length)
    }
    let first = unit(start, corner)
    let second = unit(corner, end)
    let half = width / 2
    let firstNormal = CGPoint(x: -first.y, y: first.x)
    let secondNormal = CGPoint(x: -second.y, y: second.x)
    let sum = CGPoint(x: firstNormal.x + secondNormal.x, y: firstNormal.y + secondNormal.y)
    let sumLength = hypot(sum.x, sum.y)
    let bisector = CGPoint(x: sum.x / sumLength, y: sum.y / sumLength)
    let reach = half / (bisector.x * firstNormal.x + bisector.y * firstNormal.y)
    let miter = CGPoint(x: bisector.x * reach, y: bisector.y * reach)
    func offset(_ point: CGPoint, _ by: CGPoint, _ sign: CGFloat) -> CGPoint {
      CGPoint(x: point.x + by.x * sign, y: point.y + by.y * sign)
    }
    let path = CGMutablePath()
    path.move(to: offset(start, firstNormal, half))
    path.addLine(to: offset(corner, miter, 1))
    path.addLine(to: offset(end, secondNormal, half))
    path.addLine(to: offset(end, secondNormal, -half))
    path.addLine(to: offset(corner, miter, -1))
    path.addLine(to: offset(start, firstNormal, -half))
    path.closeSubpath()
    return path
  }

  private static func circle(radius: CGFloat) -> CGRect {
    CGRect(x: 8 - radius, y: 8 - radius, width: radius * 2, height: radius * 2)
  }

  /// A stroke as a shape to fill, so every state is one filled path whatever it was drawn with.
  private static func outline(of path: CGPath, width: CGFloat, cap: CGLineCap = .round) -> CGPath {
    path.copy(strokingWithWidth: width, lineCap: cap, lineJoin: .round, miterLimit: 10)
  }
}
