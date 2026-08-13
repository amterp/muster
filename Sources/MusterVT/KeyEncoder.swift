import CGhosttyVt
import MusterCore

/// Turns a keystroke into the bytes a terminal program expects.
///
/// This is libghostty-vt's own key encoder, which matters more than it sounds: it is the
/// same code the pane's terminal would have used if the pane's terminal were doing the
/// encoding, and the same code herdr's TUI path runs (`src/ghostty/mod.rs:2552`). Muster
/// is not writing a second implementation that has to agree with a first one.
///
/// What it cannot supply is the state to encode against. See `TerminalModeProfile`.
public final class KeyEncoder {
  private let encoder: GhosttyKeyEncoder
  private let event: GhosttyKeyEvent

  public enum Failure: Error {
    case creationFailed(GhosttyResult)
    case encodingFailed(GhosttyResult)
  }

  /// Creates an encoder fixed to one set of pane modes.
  ///
  /// Fixed rather than per-call because a pane's modes change rarely and a keystroke
  /// happens on every keypress: this is on the input-to-glyph path the perf budget is
  /// written against, so the per-key work is one struct fill and one encode.
  public init(profile: TerminalModeProfile) throws {
    var encoder: GhosttyKeyEncoder?
    let created = ghostty_key_encoder_new(nil, &encoder)
    guard created == GHOSTTY_SUCCESS, let encoder else { throw Failure.creationFailed(created) }
    self.encoder = encoder

    // The event is reused across calls for the same reason.
    var event: GhosttyKeyEvent?
    let eventCreated = ghostty_key_event_new(nil, &event)
    guard eventCreated == GHOSTTY_SUCCESS, let event else {
      ghostty_key_encoder_free(encoder)
      throw Failure.creationFailed(eventCreated)
    }
    self.event = event

    apply(profile)
  }

  deinit {
    ghostty_key_event_free(event)
    ghostty_key_encoder_free(encoder)
  }

  private func apply(_ profile: TerminalModeProfile) {
    var kittyFlags = profile.kittyFlags
    var cursorKeys = profile.applicationCursorKeys
    var keypad = profile.applicationKeypad
    var altEscape = profile.altSendsEscapePrefix
    var modifyOtherKeys = profile.modifyOtherKeys
    var optionAsAlt = profile.optionActsAsAlt.ghosttyValue

    ghostty_key_encoder_setopt(encoder, GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS, &kittyFlags)
    ghostty_key_encoder_setopt(
      encoder, GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION, &cursorKeys)
    ghostty_key_encoder_setopt(encoder, GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION, &keypad)
    ghostty_key_encoder_setopt(encoder, GHOSTTY_KEY_ENCODER_OPT_ALT_ESC_PREFIX, &altEscape)
    ghostty_key_encoder_setopt(
      encoder, GHOSTTY_KEY_ENCODER_OPT_MODIFY_OTHER_KEYS_STATE_2, &modifyOtherKeys)
    // This one is an enum rather than a bool, and getting that wrong reads four bytes
    // from a one-byte Bool: the encoder then sees whatever followed it on the stack.
    ghostty_key_encoder_setopt(
      encoder, GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT, &optionAsAlt)
  }

  /// The bytes this keystroke should put on the pane's input.
  ///
  /// Empty means the key produces nothing - a bare modifier press under a profile that
  /// does not report them, or a keystroke the input method has claimed. Empty is a
  /// normal answer, not a failure, and callers must not send anything for it.
  public func encode(_ key: KeyEvent) throws -> [UInt8] {
    // A composing keystroke belongs to the input method. Encoding it would deliver the
    // romaji as well as the characters it composes into.
    guard !key.isComposing else { return [] }

    ghostty_key_event_set_action(event, key.action.ghosttyAction)
    ghostty_key_event_set_key(event, key.key.ghosttyKey)
    ghostty_key_event_set_mods(event, key.modifiers.rawValue)
    ghostty_key_event_set_consumed_mods(event, key.consumedModifiers.rawValue)
    ghostty_key_event_set_composing(event, false)
    ghostty_key_event_set_unshifted_codepoint(event, key.unshiftedCodepoint?.value ?? 0)

    // 128 covers every sequence the protocol can produce for a keystroke. The retry is
    // not dead code: with associated-text reporting on, a key can carry arbitrary text.
    var buffer = [CChar](repeating: 0, count: 128)
    var length = 0

    let result = key.text.withCString { text -> GhosttyResult in
      ghostty_key_event_set_utf8(event, text, strlen(text))
      var result = buffer.withUnsafeMutableBufferPointer {
        ghostty_key_encoder_encode(encoder, event, $0.baseAddress, $0.count, &length)
      }
      if result == GHOSTTY_OUT_OF_SPACE {
        buffer = [CChar](repeating: 0, count: length)
        result = buffer.withUnsafeMutableBufferPointer {
          ghostty_key_encoder_encode(encoder, event, $0.baseAddress, $0.count, &length)
        }
      }
      return result
    }

    guard result == GHOSTTY_SUCCESS else { throw Failure.encodingFailed(result) }
    return buffer.prefix(length).map { UInt8(bitPattern: $0) }
  }
}

extension TerminalModeProfile.OptionAsAlt {
  fileprivate var ghosttyValue: GhosttyOptionAsAlt {
    switch self {
    case .never: GHOSTTY_OPTION_AS_ALT_FALSE
    case .always: GHOSTTY_OPTION_AS_ALT_TRUE
    case .leftOnly: GHOSTTY_OPTION_AS_ALT_LEFT
    case .rightOnly: GHOSTTY_OPTION_AS_ALT_RIGHT
    }
  }
}

extension KeyEvent.Action {
  fileprivate var ghosttyAction: GhosttyKeyAction {
    switch self {
    case .press: GHOSTTY_KEY_ACTION_PRESS
    case .release: GHOSTTY_KEY_ACTION_RELEASE
    case .repeated: GHOSTTY_KEY_ACTION_REPEAT
    }
  }
}
