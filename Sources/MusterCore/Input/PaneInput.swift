import Foundation

/// One pane's input path: keymap first, then encode, then out.
///
/// The whole of "what happens when you type" in one place, so the shell above only has to
/// decide *that* a key was pressed and this decides what it means. It lives in the core
/// rather than beside the window because every decision here is testable and none of it is
/// about macOS - the two things it needs from the outside, an encoder and a channel,
/// arrive as protocols.
public final class PaneInput {
  private let channel: PaneChannel
  private let serverChannel: PaneChannel?
  private let encoder: KeyEncoding
  private let keymap: Keymap
  private var warnedAboutDroppedInput = false

  /// Everything leaves through here, in order.
  ///
  /// Two channels reach the same PTY by different routes: control-stream bytes travel
  /// app → bridge → daemon, while a server-encoded key goes app → daemon directly and
  /// skips a hop. Left concurrent, `abc<up>def` can deliver the arrow out of place. So
  /// sends are queued and a server-encoded intent completes its round trip before the next
  /// item goes out - which costs nothing at typing speed and is what makes mixing the two
  /// routes safe at all.
  private let outbound = DispatchQueue(label: "muster.pane-input")

  public init(
    channel: PaneChannel,
    serverChannel: PaneChannel? = nil,
    encoder: KeyEncoding,
    keymap: Keymap = Keymap()
  ) {
    self.channel = channel
    self.serverChannel = serverChannel
    self.encoder = encoder
    self.keymap = keymap
  }

  public func send(_ key: KeyEvent) {
    // Precedence: the keymap gets first refusal, and the encoder only sees what it
    // declines (architecture.md, input precedence).
    switch keymap.resolve(key) {
    case .text(let bytes):
      Log.debug(
        "input.bound.text",
        ["key": "\(key.key)", "mods": "\(key.modifiers.rawValue)", "bytes": String(bytes.count)])
      deliver(.input(bytes))
      return
    case .serverEncoded(let name):
      sendServerEncoded(name: name, key: key)
      return
    case .action:
      Log.debug("input.bound", ["key": "\(key.key)", "mods": "\(key.modifiers.rawValue)"])
      return
    case .unbound:
      break
    }

    guard let bytes = try? encoder.encode(key) else {
      Log.warn(
        "input.encode.failed",
        [
          "key": "\(key.key)", "mods": "\(key.modifiers.rawValue)",
          "impact": "this keystroke reaches the pane as nothing at all",
        ])
      return
    }
    // An empty encoding is normal and frequent - modifiers alone, and every key while an
    // input method is composing - so it is not a warning, but a silence worth being able
    // to tell apart from a dropped one.
    guard !bytes.isEmpty else {
      Log.trace("input.key.empty", ["key": "\(key.key)", "action": "\(key.action)"])
      return
    }
    Log.debug(
      "input.key",
      [
        "key": "\(key.key)", "mods": "\(key.modifiers.rawValue)", "action": "\(key.action)",
        "bytes": String(bytes.count),
        "encoded": Log.includesInput ? String(decoding: bytes, as: UTF8.self).debugDescription : "",
      ])
    deliver(.input(bytes))
  }

  public func send(text: String) {
    Log.debug(
      "input.text",
      ["characters": String(text.count), "text": Log.includesInput ? text.debugDescription : ""])
    deliver(.input(Array(text.utf8)))
  }

  /// Sends the clipboard to the pane.
  ///
  /// Server-encoded when there is a channel that can: a program which enabled DEC 2004
  /// wants the text fenced by paste markers so it can tell pasting from very fast typing,
  /// and a shell uses the same fence to stop a multi-line paste running as it arrives.
  /// Only the daemon knows whether that mode is on. A paste is one action rather than one
  /// per keystroke, so the round trip it costs is free.
  ///
  /// Without such a channel the text goes raw and unfenced, which is right for a single
  /// line and wrong for several. Guessing the fence on would be worse: markers sent to a
  /// program that never asked arrive as literal `[200~` on its input.
  public func paste(text: String) {
    guard !text.isEmpty else { return }
    Log.info(
      "input.paste",
      [
        "characters": String(text.count),
        "server_encoded": String(serverChannel != nil),
        "text": Log.includesInput ? text.debugDescription : "",
      ])
    guard let serverChannel else {
      deliver(.input(Array(text.utf8)))
      return
    }
    deliver(.text(text), over: serverChannel, fallback: .input(Array(text.utf8)))
  }

  public func scroll(direction: PaneIntent.ScrollDirection, lines: UInt16) {
    deliver(.scroll(direction: direction, lines: lines))
  }

  /// Hands a key to the daemon to encode, because we would get it wrong.
  ///
  /// Falls back to local encoding rather than dropping the key: a guessed arrow beats no
  /// arrow, and a daemon that has gone away must not take the keyboard with it.
  private func sendServerEncoded(name: String, key: KeyEvent) {
    guard let serverChannel else {
      sendLocallyEncoded(key)
      return
    }
    Log.debug("input.key.server", ["key": "\(key.key)", "name": name])
    let local = (try? encoder.encode(key)) ?? []
    deliver(.key(name: name), over: serverChannel, fallback: .input(local))
  }

  private func sendLocallyEncoded(_ key: KeyEvent) {
    guard let bytes = try? encoder.encode(key), !bytes.isEmpty else { return }
    deliver(.input(bytes))
  }

  private func deliver(_ intent: PaneIntent) {
    deliver(intent, over: channel, fallback: nil)
  }

  private func deliver(_ intent: PaneIntent, over target: PaneChannel, fallback: PaneIntent?) {
    outbound.sync {
      guard !target.deliver(intent) else { return }
      if let fallback, fallback != intent {
        Log.warn(
          "input.fallback",
          [
            "channel": target.channelDescription,
            "impact": "sent with a guessed encoding instead, which may be wrong for this pane",
          ])
        if channel.deliver(fallback) { return }
      }
      reportDropped(target)
    }
  }

  private func reportDropped(_ target: PaneChannel) {
    Log.warn(
      "input.dropped",
      [
        "channel": target.channelDescription,
        "impact": "the pane looks frozen but is fine; nothing typed here reached it",
      ])
    // Once on stderr, not per keystroke: a pane that swallows input produces a lot of
    // them, and a log that scrolls is a log nobody reads. The record above is per event.
    guard !warnedAboutDroppedInput else { return }
    warnedAboutDroppedInput = true
    FileHandle.standardError.write(
      Data(
        """
        muster: the pane bridge is not connected, so input is going nowhere.
        The pane keeps rendering, which makes this look like a frozen program rather \
        than a broken channel. Usual causes: muster-bridge failed to start (its own \
        error is above), or it could not reach \(target.channelDescription).

        """.utf8))
  }
}
