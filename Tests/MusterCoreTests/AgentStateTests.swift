import Testing

@testable import MusterCore

@Test("every state a backend can name survives the round trip")
func backendSpellingsRoundTrip() {
  for state in AgentState.allCases {
    #expect(AgentState(backendValue: state.rawValue) == state)
  }
}

@Test("a state we have never heard of reads as unknown, not as idle")
func unrecognizedStateFailsClosed() {
  #expect(AgentState(backendValue: "hibernating") == .unknown)
  #expect(AgentState(backendValue: "") == .unknown)
}
