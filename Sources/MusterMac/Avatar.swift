import AppKit
import SwiftUI

// The small parts the group conversation and the message banner share: an avatar for who wrote a
// message, the keycaps and chord caps that name how to reach or send, and a tag.

/// An author's avatar: their initial on their identity color, and where their agent's state is
/// known, that state's icon on the corner.
struct AvatarView: View {
  let name: String
  let color: RGB
  /// The state of the agent behind it, when it is an agent this window knows.
  var state: String?
  var size: CGFloat = 24

  @Environment(\.paneLook) private var look

  var body: some View {
    ZStack(alignment: .bottomTrailing) {
      RoundedRectangle(cornerRadius: size * 0.25)
        .fill(Color(nsColor: color.color))
        .frame(width: size, height: size)
        .overlay(
          Text(Self.initial(of: name))
            .font(.system(size: size * 0.5, weight: .semibold))
            .foregroundStyle(Color.white))
      if let state {
        StateIconRepresentable.icon(state)
          .frame(width: StateIcon.size, height: StateIcon.size)
          .scaleEffect(0.7)
          .background(Circle().fill(look.background).scaleEffect(0.8))
          .offset(x: 4, y: 4)
      }
    }
    .frame(width: size + 4, height: size + 4, alignment: .topLeading)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(state.map { "\(name), \($0)" } ?? name)
  }

  /// The letter an avatar shows: the name's first letter or digit, upper case, past a leading @.
  static func initial(of name: String) -> String {
    let first = name.drop(while: { !$0.isLetter && !$0.isNumber }).first ?? name.first
    return first.map { String($0).uppercased() } ?? "?"
  }
}

/// A `StateIcon` for SwiftUI. Made again when the state changes, which is all a state icon does.
struct StateIconRepresentable: NSViewRepresentable {
  let state: String

  func makeNSView(context: Context) -> StateIconView {
    StateIconView(state: state)
  }

  func updateNSView(_ view: StateIconView, context: Context) {
    // A `StateIconView` shows the state it was made with; SwiftUI replaces it through `id`.
  }
}

extension StateIconRepresentable {
  /// Draws the icon for `state`, replacing the view when the state changes.
  static func icon(_ state: String) -> some View {
    StateIconRepresentable(state: state).id(state)
  }
}

/// One digit of a chord as the agent list draws it, so a member's keys look like its row's.
struct KeycapRepresentable: NSViewRepresentable {
  let digit: Int
  let surface: ChromePalette.Surface

  func makeNSView(context: Context) -> KeycapView {
    let key = KeycapView(digit: digit)
    key.show(digit: digit, live: false, surface: surface)
    return key
  }

  func updateNSView(_ key: KeycapView, context: Context) {
    key.show(digit: digit, live: false, surface: surface)
  }

  func sizeThatFits(_ proposal: ProposedViewSize, nsView: KeycapView, context: Context) -> CGSize? {
    CGSize(width: KeycapView.width, height: KeycapView.height)
  }
}

/// The chord that reaches a pane, as the keys the agent list draws: the tab's press, then the
/// pane's. A press of 0 is no key.
struct ChordKeys: View {
  let first: Int
  let second: Int

  @Environment(\.paneLook) private var look

  var body: some View {
    HStack(spacing: 2) {
      if SidebarRowView.pressable(first) {
        KeycapRepresentable(digit: first, surface: look.surface)
          .frame(width: KeycapView.width, height: KeycapView.height)
      }
      if SidebarRowView.pressable(second) {
        KeycapRepresentable(digit: second, surface: look.surface)
          .frame(width: KeycapView.width, height: KeycapView.height)
      }
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("keys \(first) \(second)")
  }
}

/// A chord to press, spelled with the glyphs of a menu, on a cap.
struct ChordCap: View {
  let text: String
  var onAccent = false

  @Environment(\.paneLook) private var look

  var body: some View {
    Text(text)
      .font(.system(size: 11, weight: .semibold))
      .padding(.horizontal, 5)
      .frame(minWidth: 20, minHeight: 17)
      .foregroundStyle(onAccent ? Color.white : look.text)
      .background(
        RoundedRectangle(cornerRadius: 4)
          .fill(onAccent ? Color.white.opacity(0.18) : look.keycapFill)
      )
      .overlay(
        RoundedRectangle(cornerRadius: 4)
          .strokeBorder(onAccent ? Color.white.opacity(0.4) : look.rule)
      )
      .accessibilityLabel("key \(text)")
  }
}

/// A short label in a capsule: "To you", "Urgent".
struct TagView: View {
  let text: String
  let foreground: Color
  let fill: Color

  var body: some View {
    Text(text)
      .font(.caption.weight(.semibold))
      .lineLimit(1)
      .fixedSize()
      .foregroundStyle(foreground)
      .padding(.horizontal, 6)
      .padding(.vertical, 1)
      .background(Capsule().fill(fill))
  }
}

extension PaneLook {
  /// Text that says something needs doing: a warning on this surface.
  var warning: Color { stateText("blocked") }
  /// Text that says something is wrong or urgent on this surface.
  var alert: Color { stateText("stuck") }
}
