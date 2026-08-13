import TestSupport
import Testing

@testable import MusterCore

// A driver. The cases live in corpus/conformance/agent-state.json.

@Test("agent state from a backend's spelling")
func agentStateConformance() throws {
  let corpus = try Conformance.load("agent-state.json")

  let ran = corpus.run { given in
    let state = AgentState(backendValue: given["backendValue"]?.stringValue ?? "")
    return .fields(["state": .string(state.rawValue)])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}

@Test("every state Muster knows has a case in the corpus")
func everyStateIsCovered() throws {
  // The corpus lists names one per case so the set is pinned as data. That only holds if
  // adding a sixth state to the enum fails something - otherwise the new one ships
  // untested and the corpus quietly describes a smaller vocabulary than the code has.
  let corpus = try Conformance.load("agent-state.json")
  let covered = Set(corpus.cases.compactMap { $0.expect["state"]?.stringValue })

  #expect(covered == Set(AgentState.allCases.map(\.rawValue)))
}
