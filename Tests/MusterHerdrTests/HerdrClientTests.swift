import TestSupport
import Testing

@testable import MusterHerdr

// A driver. The cases live in corpus/conformance/socket-discovery.json.

@Test("herdr socket discovery")
func socketDiscoveryConformance() throws {
  let corpus = try Conformance.load("socket-discovery.json")

  let ran = corpus.run { given in
    var environment: [String: String] = [:]
    if case .object(let fields)? = given["env"] {
      for (name, value) in fields { environment[name] = value.stringValue ?? "" }
    }
    let path = HerdrClient.discoverSocketPath(environment: environment)
    return .fields(["path": path.map(JSONValue.string) ?? .null])
  }

  #expect(ran == corpus.cases.count)
  #expect(ran > 0)
}
