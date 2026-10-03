import AppKit
import MusterRenderer
import SwiftUI

// The bar itself is ported from ghostty's own macOS app - `SurfaceSearchOverlay` and
// `BackportSelectionTextField` in macos/Sources/Ghostty/Surface View/SurfaceView.swift and
// macos/Sources/Helpers/Backport.swift, MIT, Mitchell Hashimoto and Ghostty contributors, see
// NOTICE. The layout, the corner-snapping drag, the button style, the escape and return
// behaviour and the counter are theirs. So is the search: each pane's surface holds the pane's
// own stream and history, so libghostty searches, marks and scrolls to a match exactly as it
// does in Ghostty, and reports its counts back through the same actions. What is Muster's is
// that the bar is opened and stepped by Muster's own keymap, and follows the keyboard between
// panes.
//
// **This is the only file in `Sources/` that imports SwiftUI, and here is what that cost** -
// worth reading before deciding a second one should. The hosting view's layer background has
// to be forced clear, because a GPU surface sits underneath it. The text field is a backport
// rather than the stock one, because the stock API crashes on macOS 15 when text is deleted.
// And the bar reaches its own field through a `NotificationCenter` hop, because `@FocusState`
// cannot be read from outside a view body. None of that is an argument against SwiftUI - it
// is as native as AppKit and sometimes the better answer (`docs/architecture.md`) - but it is
// the bill, and it comes due per hosted view rather than once.

/// What the find bar is showing.
///
/// The needle is the only thing here somebody types; everything else is the renderer's answer
/// to it. Kept apart from the view so that what is on screen is a function of what the
/// renderer said, which is the same rule the rest of the window follows.
@MainActor
final class FindState: ObservableObject {
  @Published var needle: String = ""

  /// What is selected in the field, so that asking to find again selects what is there and
  /// typing replaces it. Only honoured on macOS 26 and up - see `SelectableTextField`.
  @Published var selection: Range<String.Index>?

  /// How many matches there are, and which one is selected counting from zero. Nil is not
  /// known yet, which is what every needle starts as.
  @Published var total: Int?
  @Published var selected: Int?

  /// "3/47" on a match, "-/47" before one is selected, and nothing before anything is counted.
  /// Ghostty's own counter, and a "?" where it has a place but no total yet.
  var counter: String {
    if let selected { return "\(selected + 1)/\(total.map(String.init) ?? "?")" }
    if let total { return "-/\(total)" }
    return ""
  }
}

/// The bar drawn over a pane.
struct FindBarView: View {
  @ObservedObject var state: FindState

  /// Called whenever the needle changes, which is once per keystroke.
  let onNeedle: (String) -> Void
  let onStep: (_ forward: Bool) -> Void
  let onClose: () -> Void

  /// Puts the keyboard back in the pane, leaving the bar up. What escape means the first time.
  let onReturnToPane: () -> Void

  @State private var corner: Corner = .topRight
  @State private var dragOffset: CGSize = .zero
  @State private var barSize: CGSize = .zero
  @FocusState private var isFieldFocused: Bool

  private let padding: CGFloat = 8

  var body: some View {
    GeometryReader { geometry in
      HStack(spacing: 4) {
        SelectableTextField("Find", text: $state.needle, selection: $state.selection)
          .textFieldStyle(.plain)
          .frame(width: 180)
          .padding(.leading, 8)
          .padding(.trailing, 50)
          .padding(.vertical, 6)
          .background(Color.primary.opacity(0.1))
          .cornerRadius(6)
          .focused($isFieldFocused)
          .overlay(alignment: .trailing) {
            Text(state.counter)
              .font(.caption)
              .foregroundColor(.secondary)
              .monospacedDigit()
              .padding(.trailing, 8)
          }
          .onChange(of: state.needle) { _, needle in onNeedle(needle) }
          .onSubmit { onStep(true) }
          .onExitCommand {
            // Escape gives the pane the keyboard back before it closes the bar, so the way
            // out of typing is not also the way out of the results. A second escape, with
            // the field empty or unfocused, is what closes it.
            if state.needle.isEmpty {
              onClose()
            } else {
              onReturnToPane()
            }
          }
          .onKeyPress(.return, phases: .down) { press in
            guard press.modifiers.contains(.shift) else { return .ignored }
            onStep(false)
            return .handled
          }

        Button(action: { onStep(true) }) { Image(systemName: "chevron.up") }
          .buttonStyle(FindButtonStyle())
        Button(action: { onStep(false) }) { Image(systemName: "chevron.down") }
          .buttonStyle(FindButtonStyle())
        Button(action: onClose) { Image(systemName: "xmark") }
          .buttonStyle(FindButtonStyle())
      }
      .padding(8)
      .background(.background)
      .clipShape(RoundedRectangle(cornerRadius: 8))
      .shadow(radius: 4)
      .onAppear { isFieldFocused = true }
      // Only this bar's own: every window has one, and another window's find is not this one's.
      .onReceive(NotificationCenter.default.publisher(for: .musterFindFocus, object: state)) { _ in
        // Asking to find while the bar is already up means "let me type a new one", so the
        // field takes the keyboard and selects what is in it. Deferred because the request
        // arrives from a menu item, which is still unwinding its own event.
        DispatchQueue.main.async {
          isFieldFocused = true
          state.selection = state.needle.startIndex..<state.needle.endIndex
        }
      }
      .background(
        GeometryReader { barGeometry in
          Color.clear.onAppear { barSize = barGeometry.size }
        }
      )
      .padding(padding)
      .offset(dragOffset)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: corner.alignment)
      .gesture(
        DragGesture()
          .onChanged { drag in dragOffset = drag.translation }
          .onEnded { drag in
            let centre = centre(of: corner, in: geometry.size)
            let dropped = CGPoint(
              x: centre.x + drag.translation.width, y: centre.y + drag.translation.height)
            withAnimation(.easeOut(duration: 0.2)) {
              corner = closestCorner(to: dropped, in: geometry.size)
              dragOffset = .zero
            }
          }
      )
    }
  }

  enum Corner {
    case topLeft, topRight, bottomLeft, bottomRight

    var alignment: Alignment {
      switch self {
      case .topLeft: return .topLeading
      case .topRight: return .topTrailing
      case .bottomLeft: return .bottomLeading
      case .bottomRight: return .bottomTrailing
      }
    }
  }

  private func centre(of corner: Corner, in container: CGSize) -> CGPoint {
    let halfWidth = barSize.width / 2 + padding
    let halfHeight = barSize.height / 2 + padding
    switch corner {
    case .topLeft: return CGPoint(x: halfWidth, y: halfHeight)
    case .topRight: return CGPoint(x: container.width - halfWidth, y: halfHeight)
    case .bottomLeft: return CGPoint(x: halfWidth, y: container.height - halfHeight)
    case .bottomRight:
      return CGPoint(x: container.width - halfWidth, y: container.height - halfHeight)
    }
  }

  private func closestCorner(to point: CGPoint, in container: CGSize) -> Corner {
    if point.x < container.width / 2 {
      return point.y < container.height / 2 ? .topLeft : .bottomLeft
    }
    return point.y < container.height / 2 ? .topRight : .bottomRight
  }
}

/// Ghostty's search button: secondary until the pointer is on it.
struct FindButtonStyle: ButtonStyle {
  @State private var isHovered = false

  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .foregroundStyle(isHovered || configuration.isPressed ? .primary : .secondary)
      .padding(.horizontal, 2)
      .frame(height: 26)
      .background(
        RoundedRectangle(cornerRadius: 6).fill(background(pressed: configuration.isPressed))
      )
      .onHover { hovering in isHovered = hovering }
  }

  private func background(pressed: Bool) -> Color {
    if pressed { return Color.primary.opacity(0.2) }
    if isHovered { return Color.primary.opacity(0.1) }
    return Color.clear
  }
}

/// A `TextField` whose selection can be set, where the platform allows it.
///
/// Ported from ghostty's `BackportSelectionTextField`, including its version gate and the
/// reason for it: the API exists from macOS 15 and SwiftUI crashes there when text is deleted,
/// so 26 is the floor. Below that the selection binding does nothing and the field is an
/// ordinary one - which costs asking to find twice in a row not selecting what is already
/// typed, and nothing else.
struct SelectableTextField: View {
  private let title: LocalizedStringKey
  @Binding private var text: String
  @Binding private var selection: Range<String.Index>?

  init(
    _ title: LocalizedStringKey, text: Binding<String>, selection: Binding<Range<String.Index>?>
  ) {
    self.title = title
    self._text = text
    self._selection = selection
  }

  var body: some View {
    if #available(macOS 26, *) {
      TextField(
        title, text: _text,
        selection: Binding(
          get: { selection.map { TextSelection(range: $0) } },
          set: { chosen in
            if let chosen, case .selection(let range) = chosen.indices {
              selection = range
            } else {
              selection = nil
            }
          }
        ))
    } else {
      TextField(title, text: _text)
    }
  }
}

extension Notification.Name {
  /// Asks whichever find bar is up to take the keyboard and select what is in it.
  ///
  /// A notification rather than a call because the bar is a SwiftUI view inside a hosting
  /// view, and its focus is `@FocusState` - which nothing outside the view body can set.
  static let musterFindFocus = Notification.Name("dev.muster.findFocus")
}

/// The find bar, and the only thing outside this file that knows one exists.
///
/// Owns the hosted view and the state it draws, and drives the search of the pane it is over.
/// Everything AppKit is here and everything SwiftUI is above, so the window deals with a
/// hosting view exactly nowhere.
@MainActor
public final class FindBar {
  let state = FindState()
  private lazy var hosting: NSHostingView<FindBarView> = {
    let view = NSHostingView(
      rootView: FindBarView(
        state: state,
        onNeedle: { [weak self] needle in self?.typed(needle) },
        onStep: { [weak self] forward in self?.step(forward: forward) },
        onClose: { [weak self] in self?.close() },
        onReturnToPane: { [weak self] in self?.onReturnToPane?() }
      ))
    // The pane draws itself with a GPU layer underneath this, so anything opaque here would
    // be a rectangle of window colour over a terminal. Only the bar itself paints.
    view.layer?.backgroundColor = .clear
    return view
  }()

  /// Puts the keyboard back in the pane, leaving the bar up. The window's, because which view
  /// is the pane is a thing only it knows.
  public var onReturnToPane: (@MainActor () -> Void)?

  /// Says so when the renderer would not search for a needle.
  ///
  /// The same shape as the window's report about font sizing, and reported the same way,
  /// because it fails the same way: the action is named by a string libghostty parses, so a
  /// version that renamed it is a find that quietly counts nothing.
  public var onRefused: (@MainActor ([String]) -> Void)?

  /// How long a needle shorter than `shortNeedle` waits before it is searched for.
  ///
  /// Ghostty's own figures. One or two characters match nearly everything in a pane's history,
  /// and typing a longer needle passes through them on the way - so each one waits to see
  /// whether another character follows before costing a search of the whole history.
  static let debounce: Duration = .milliseconds(300)
  static let shortNeedle = 3

  /// The pane the bar is over, which is also the one being searched.
  ///
  /// Weak because a pane can close under an open find bar, and a bar holding the last view of
  /// a dead pane alive would keep a libghostty surface alive with it.
  private weak var chrome: PaneChrome?

  /// A short needle waiting out the debounce, cancelled by whatever is typed next.
  private var waiting: Task<Void, Never>?

  public init() {}

  /// Shows the bar over a pane, moving it there if it was over another one.
  ///
  /// The needle survives the move and is searched for again, because a find bar that follows
  /// the keyboard to a second pane is being asked the same question about a different pane -
  /// and a counter left over from the first would be a count of matches that are not there.
  public func show(over chrome: PaneChrome) {
    guard self.chrome !== chrome else { return }
    // The pane being left keeps its marks otherwise, which would be two panes looking searched
    // and one of them counted.
    leave()
    hosting.removeFromSuperview()
    chrome.addSubview(hosting)
    hosting.frame = chrome.bounds
    hosting.autoresizingMask = [.width, .height]
    self.chrome = chrome
    chrome.surface.onSearch = { [weak self] report in self?.apply(report) }
    if !state.needle.isEmpty {
      search(state.needle)
    }
  }

  /// Takes a needle as it is typed, once per keystroke.
  func typed(_ needle: String) {
    waiting?.cancel()
    waiting = nil
    // The counts belong to the needle before this one, and would be drawn against this one
    // until the renderer answered.
    state.total = nil
    state.selected = nil
    guard !needle.isEmpty, needle.count < FindBar.shortNeedle else {
      search(needle)
      return
    }
    waiting = Task { [weak self] in
      try? await Task.sleep(for: FindBar.debounce)
      guard !Task.isCancelled else { return }
      self?.search(needle)
    }
  }

  /// Goes to the next match, or the previous one.
  ///
  /// Nothing with no bar up: the chords work with the keyboard back in the pane, which means
  /// they can be pressed with no bar at all.
  public func step(forward: Bool) {
    guard let chrome else { return }
    chrome.surface.navigateSearch(next: forward)
  }

  /// Takes the bar down, and ends the search with it.
  public func close() {
    leave()
    hosting.removeFromSuperview()
  }

  /// Whether the bar is on screen. For the window and for a test; nothing else asks.
  public var isShown: Bool { chrome != nil }

  private func search(_ needle: String) {
    guard let chrome else { return }
    let refused = chrome.surface.search(needle)
    if !refused.isEmpty { onRefused?(refused) }
  }

  private func apply(_ report: SearchReport) {
    switch report {
    case .total(let total): state.total = total
    case .selected(let selected): state.selected = selected
    }
  }

  /// Ends the search of the pane the bar is over, and stops listening to it.
  private func leave() {
    waiting?.cancel()
    waiting = nil
    chrome?.surface.onSearch = nil
    chrome?.surface.search(nil)
    chrome = nil
    state.total = nil
    state.selected = nil
  }
}
