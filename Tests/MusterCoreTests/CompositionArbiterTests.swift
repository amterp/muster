import TestSupport
import Testing

@testable import MusterCore

// A driver, not a suite. The cases live in corpus/conformance/composition-arbiter.json so
// that a core rewritten in another language is judged by the same ones (MIP-1). What is
// here is only the translation between this language's types and the corpus's vocabulary.

@Test("composition arbitration")
func compositionConformance() throws {
  let corpus = try Conformance.load("composition-arbiter.json")

  let ran = corpus.run { given in
    let outcome = CompositionArbiter.outcome(
      wasComposing: given["wasComposing"]?.boolValue ?? false,
      committed: given["committed"]?.stringValue,
      stillComposing: given["stillComposing"]?.boolValue ?? false)

    return switch outcome {
    case .sendNothing: .fields(["outcome": "sendNothing"])
    case .sendKey: .fields(["outcome": "sendKey"])
    case .sendText(let text): .fields(["outcome": "sendText", "text": .string(text)])
    }
  }

  // Asserted rather than assumed: a corpus that stopped being read passes every driver.
  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}
