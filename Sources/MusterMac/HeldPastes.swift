import Foundation

/// A paste a pane's daemon would not send without somebody saying yes.
///
/// The daemon holds a paste with a line break in it, going to a program that did not ask for
/// pasted text to be marked as such: each line would land as though typed, Return included, and
/// run. Nothing of it has reached the program.
public struct HeldPaste: Equatable {
  public let pane: PaneKey
  public let text: String

  public init(pane: PaneKey, text: String) {
    self.pane = pane
    self.text = text
  }
}

/// Asks about each held paste, one at a time, and sends the ones somebody says yes to.
///
/// **Queued rather than replaced.** Each held paste is a paste somebody made, possibly into
/// different panes, and each wants its own answer: replacing the one on screen would drop a
/// paste without asking, which is the one thing a held paste promises not to do.
///
/// The asking is injected, because the real one is a sheet on a window and what is decided here -
/// what is sent on a yes, that nothing is on a no, and the order - is worth testing without one.
@MainActor
public final class HeldPastes {
  /// Shows a held paste to somebody, and calls back once with whether they said to paste it.
  public typealias Ask = @MainActor (_ held: HeldPaste, _ answer: @escaping (Bool) -> Void) -> Void

  private let ask: Ask
  private var waiting: [HeldPaste] = []
  private var asking = false

  public init(ask: @escaping Ask) {
    self.ask = ask
  }

  /// Takes a paste the daemon held, and asks about it once nothing else is being asked.
  public func hold(_ held: HeldPaste) {
    waiting.append(held)
    askNext()
  }

  private func askNext() {
    guard !asking, !waiting.isEmpty else { return }
    asking = true
    let held = waiting.removeFirst()
    ask(held) { [weak self] confirmed in
      // Named rather than sent to the keyboard's pane, because the keyboard may have moved
      // while somebody read the question.
      if confirmed { Core.paste(text: held.text, confirmedFor: held.pane) }
      self?.asking = false
      self?.askNext()
    }
  }
}
