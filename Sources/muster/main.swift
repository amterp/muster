import MusterCore

// The shell is a stub until the libghostty spike lands. What it prints is just proof
// that the executable target builds against the core and nothing else.
let states = AgentState.allCases.map(\.rawValue).joined(separator: ", ")
print("muster (pre-alpha) - agent states: \(states)")
