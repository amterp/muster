/// Ghostty's binding actions that belong to a surface alone: it holds the pane's scrollback and
/// selection, so these write nothing to the program. What Muster calls each is the core's; what
/// libghostty calls it stays in this module, like every other action string here.
public enum SurfaceAction: CaseIterable, Sendable {
  case scrollToTop
  case scrollToBottom
  case scrollPageUp
  case scrollPageDown
  case jumpToPreviousPrompt
  case jumpToNextPrompt
  case selectAll

  var ghosttyName: String {
    switch self {
    case .scrollToTop: "scroll_to_top"
    case .scrollToBottom: "scroll_to_bottom"
    case .scrollPageUp: "scroll_page_up"
    case .scrollPageDown: "scroll_page_down"
    case .jumpToPreviousPrompt: "jump_to_prompt:-1"
    case .jumpToNextPrompt: "jump_to_prompt:1"
    case .selectAll: "select_all"
    }
  }
}
