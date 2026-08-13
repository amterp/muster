import MusterCore

/// One ordered account of everything the input path sent, across every channel.
///
/// Shared rather than per-channel because the interesting property spans them: bytes go
/// out over a control stream while named keys go straight to the daemon, and the question
/// is whether `abc<up>def` still arrives in that order. Two separate logs could not say.
public final class SendRecorder: @unchecked Sendable {
  public private(set) var sends: [(channel: String, intent: PaneIntent)] = []

  public init() {}

  public func record(_ channel: String, _ intent: PaneIntent) {
    sends.append((channel, intent))
  }

  /// Just the intents, in order, for the common assertion.
  public var intents: [PaneIntent] { sends.map(\.intent) }

  /// Which channel carried each send, in order.
  public var channels: [String] { sends.map(\.channel) }
}

/// A pane channel that writes to a recorder instead of a daemon.
public final class FakeChannel: PaneChannel {
  private let recorder: SendRecorder
  private let name: String
  private let accepts: (PaneIntent) -> Bool

  /// - Parameter accepts: whether a given intent succeeds. Refusal is a real state worth
  ///   faking: the daemon channel refuses anything it cannot encode, and any channel can
  ///   refuse when the far end has gone away.
  public init(
    name: String,
    recorder: SendRecorder,
    encodesServerSide: Bool = false,
    accepts: @escaping (PaneIntent) -> Bool = { _ in true }
  ) {
    self.name = name
    self.recorder = recorder
    self.encodesServerSide = encodesServerSide
    self.accepts = accepts
  }

  public func deliver(_ intent: PaneIntent) -> Bool {
    guard accepts(intent) else { return false }
    recorder.record(name, intent)
    return true
  }

  public let encodesServerSide: Bool

  public var channelDescription: String { name }
}

/// An encoder that spells a keystroke the obvious way.
///
/// Enough for testing what the pipeline *does* with bytes. What the bytes should be is a
/// separate question, answered against the real encoder in `MusterVTTests` where the
/// oracle is libghostty's own output rather than a fixture written from memory.
public final class FakeEncoder: KeyEncoding {
  public init() {}

  public func encode(_ key: KeyEvent) throws -> [UInt8] {
    Array(key.text.utf8)
  }
}
