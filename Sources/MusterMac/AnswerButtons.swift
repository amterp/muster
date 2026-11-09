import SwiftUI

/// The answers a question offers, one button each (mip/0009-group-conversation.md, section 6),
/// drawn the same under a banner and in a conversation. Picking one fills the reply rather than
/// sending it. Space or Return picks the button holding the keyboard, and Tab and Shift-Tab move
/// between them whatever the system's keyboard navigation setting, since a question is answered
/// from the keyboard as often as with the pointer; past either end the keyboard goes back to
/// whatever the owner says. No digit picks one, since a reply may start with "1.": the numbers on
/// the buttons name them, as a chord's keys name a pane, and are not themselves a chord.
struct AnswerButtons<Focus: Hashable>: View {
  let answers: [String]
  /// The answer picked, by its place from 0.
  let picked: Int?
  /// The answer its author recommends, by its place from 0: labeled, and never picked by it.
  let recommended: Int?
  let enabled: Bool
  var focus: FocusState<Focus?>.Binding
  /// What holds the keyboard on each answer's button.
  let focusing: (Int) -> Focus
  let onPick: (Int) -> Void
  /// Tab past the last answer, or Shift-Tab (`back`) before the first.
  let onTabOut: (_ back: Bool) -> Void

  @Environment(\.paneLook) private var look

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(Array(answers.enumerated()), id: \.offset) { index, answer in
        let isPicked = picked == index
        Button(action: { onPick(index) }) {
          HStack(spacing: 8) {
            Text("\(index + 1)")
              .font(.system(size: 10.5, weight: .semibold).monospacedDigit())
              .frame(width: KeycapView.width, height: KeycapView.height)
              .background(
                RoundedRectangle(cornerRadius: 4).fill(look.keycapFill)
              )
              .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(look.rule))
            Text(answer)
            if recommended == index { RecommendedLabel() }
            Spacer(minLength: 0)
            if isPicked {
              Image(systemName: "checkmark").foregroundStyle(look.accent)
            }
          }
          .padding(.horizontal, 6)
          .padding(.vertical, 4)
          .background(RoundedRectangle(cornerRadius: 6).fill(isPicked ? look.accentTint : .clear))
          .overlay(
            RoundedRectangle(cornerRadius: 6).strokeBorder(isPicked ? look.accent : look.rule)
          )
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .focusable()
        .focused(focus, equals: focusing(index))
        .disabled(!enabled)
        .help(
          isPicked ? "Take this answer back" : "Pick this answer; send it with or without a note"
        )
        .onKeyPress(keys: [.space, .return], phases: .down) { _ in
          onPick(index)
          return .handled
        }
        .onKeyPress(keys: [.tab], phases: .down) { press in
          let back = press.modifiers.contains(.shift)
          let next = back ? index - 1 : index + 1
          if answers.indices.contains(next) {
            focus.wrappedValue = focusing(next)
          } else {
            onTabOut(back)
          }
          return .handled
        }
      }
    }
  }
}

/// Says which answer a question's author recommends, beside it, in words rather than by a color
/// alone, which a picked answer's checkmark already uses.
struct RecommendedLabel: View {
  @Environment(\.paneLook) private var look

  var body: some View {
    Text("Recommended")
      .font(.caption)
      .foregroundStyle(look.dimText)
      .padding(.horizontal, 5)
      .padding(.vertical, 1)
      .overlay(Capsule().strokeBorder(look.rule))
  }
}
