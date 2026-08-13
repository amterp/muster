import AppKit

// The one place an NSEvent becomes something the core understands. Everything below the
// shell speaks the seam's vocabulary, so this is the whole surface area of "macOS" in
// Muster's input path (architecture.md: the shell wires OS events in and nothing more).
//
// Names rather than numbers, because that vocabulary has to survive leaving the process:
// the same `KeyA` and `shift` appear in the conformance corpus and in the run log, and a
// GTK shell would write its own table producing them (docs/testing.md, what stays native).
//
// The heuristics here are ported from ghostty's own macOS app
// (macos/Sources/Ghostty/NSEvent+Extension.swift, MIT, Mitchell Hashimoto and Ghostty
// contributors - see NOTICE). They are not obvious and they are load-bearing: macOS does
// not tell an application which modifiers a layout spent producing a character, and the
// answer ghostty settled on has years of keyboard layouts behind it. Reinventing it would
// mean rediscovering the same edge cases through bug reports.

extension NSEvent {
  /// This event as a Muster keystroke.
  ///
  /// A key macOS names but libghostty does not - the JIS kana and eisu keys are the only
  /// two at this pin - still types its character, so it becomes an unidentified key with
  /// text rather than a dropped keystroke.
  func musterKeyEvent(action: String, isComposing: Bool) -> Muster_KeyEvent {
    var event = Muster_KeyEvent()
    event.action = action
    event.key = KeyNames.name(forMacOSKeycode: UInt16(keyCode)) ?? "unidentified"
    event.modifiers = modifierFlags.musterNames
    // macOS offers no way to ask which modifiers the layout consumed. ghostty's heuristic,
    // unchanged for years: control and command never contribute to text, everything else is
    // assumed to have.
    event.consumedModifiers = event.modifiers.filter { $0 != "control" && $0 != "super" }
    event.text = musterText ?? ""
    if let unshifted = unshiftedCodepoint {
      event.unshiftedCodepoint = unshifted.value
    }
    event.isComposing = isComposing
    return event
  }

  /// The text this keystroke produced, with the two cases a terminal must not see.
  private var musterText: String? {
    guard let characters else { return nil }
    guard characters.count == 1, let scalar = characters.unicodeScalars.first else {
      return characters
    }

    // A control character means macOS already applied ctrl. The encoder does its own
    // control mapping, and would otherwise apply it twice.
    if scalar.value < 0x20 {
      return self.characters(byApplyingModifiers: modifierFlags.subtracting(.control))
    }

    // Function keys arrive as private-use codepoints. They are keys, not text, and sending
    // them would type a glyph nobody has.
    if scalar.value >= 0xF700 && scalar.value <= 0xF8FF {
      return nil
    }

    return characters
  }

  /// What this key produces with no modifiers at all.
  ///
  /// `characters(byApplyingModifiers: [])` rather than `charactersIgnoringModifiers`, which
  /// changes behavior when control is held and would report the wrong key.
  private var unshiftedCodepoint: Unicode.Scalar? {
    guard type == .keyDown || type == .keyUp else { return nil }
    return characters(byApplyingModifiers: [])?.unicodeScalars.first
  }
}

extension NSEvent.ModifierFlags {
  /// The modifiers that are down, named as the core names them, in a stable order.
  var musterNames: [String] {
    var names: [String] = []
    if contains(.shift) { names.append("shift") }
    if contains(.control) { names.append("control") }
    if contains(.option) { names.append("alt") }
    if contains(.command) { names.append("super") }
    if contains(.capsLock) { names.append("capsLock") }

    // Which side, from the device-dependent bits AppKit does not expose by name. Only the
    // right-hand bits are checked because the encoding treats left as the default, and a
    // chord with both sides down is not something the protocol can express anyway.
    if rawValue & UInt(NX_DEVICERSHIFTKEYMASK) != 0 { names.append("shiftIsRight") }
    if rawValue & UInt(NX_DEVICERCTLKEYMASK) != 0 { names.append("controlIsRight") }
    if rawValue & UInt(NX_DEVICERALTKEYMASK) != 0 { names.append("altIsRight") }
    if rawValue & UInt(NX_DEVICERCMDKEYMASK) != 0 { names.append("superIsRight") }

    return names
  }
}
