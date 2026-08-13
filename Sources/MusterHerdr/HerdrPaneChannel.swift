import Foundation
import MusterCore

/// The channel for input Muster cannot encode correctly by itself.
///
/// herdr's `pane.send_input` encodes against the pane's live terminal state - the kitty
/// flags it has negotiated, whether it turned on application cursor keys, whether
/// bracketed paste is enabled - none of which is visible to a control-stream client
/// (`docs/observations/herdr-0.8.0.md` section 5). So the keys and text where guessing is
/// known to go wrong come here instead, and the daemon gets them right.
///
/// It costs a socket round trip, which is why it carries the exceptions rather than
/// everything: arrows, whose encoding a pager rejects when guessed, and paste, which is
/// one action and so pays the round trip once.
public final class HerdrPaneChannel: PaneChannel {
  private let client: HerdrClient
  private let paneID: String

  /// Fails when no daemon socket can be found, which is a real state rather than an error
  /// - a pane still works without this channel, with a guessed encoding.
  public init?(paneID: String, client: HerdrClient? = nil) {
    guard
      let client = client ?? HerdrClient.discoverSocketPath().map({ HerdrClient(socketPath: $0) })
    else { return nil }
    self.client = client
    self.paneID = paneID
    Log.info("server_channel.ready", ["pane": paneID, "socket": client.socketPath])
  }

  public func deliver(_ intent: PaneIntent) -> Bool {
    let params: [String: Any]
    switch intent {
    case .text(let text):
      params = ["pane_id": paneID, "text": text]
    case .key(let name):
      params = ["pane_id": paneID, "keys": [name]]
    case .input, .scroll:
      // Not this channel's job. Bytes already encoded belong on the control stream, and a
      // scroll is answered there against the same live state.
      return false
    }

    let started = DispatchTime.now()
    let result = client.request(method: "pane.send_input", params: params)
    let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started.uptimeNanoseconds) / 1e6

    switch result {
    case .success:
      // Timed on every send because this sits on the input path, and the decision to route
      // a key this way rests on the number being small (card a_26BIX28HG).
      Log.debug(
        "server_channel.sent", ["intent": label(intent), "ms": String(format: "%.2f", elapsed)])
      return true
    case .failure(let failure):
      Log.warn(
        "server_channel.failed",
        [
          "intent": label(intent), "error": "\(failure)",
          "ms": String(format: "%.2f", elapsed),
          "impact": "falls back to a locally guessed encoding, which pagers reject",
        ])
      return false
    }
  }

  public var encodesServerSide: Bool { true }

  public var channelDescription: String { "herdr \(client.socketPath) (\(paneID))" }

  private func label(_ intent: PaneIntent) -> String {
    switch intent {
    case .key(let name): "key:\(name)"
    case .text: "text"
    case .input: "input"
    case .scroll: "scroll"
    }
  }
}
