import AppKit
import GhosttyKit

// Keystrokes, handed to a surface the way Ghostty's own macOS app hands them over - ported from
// macos/Sources/Ghostty/NSEvent+Extension.swift (`ghosttyKeyEvent`, `ghosttyCharacters`) and
// Surface View/SurfaceView_AppKit.swift (`keyDown`, `flagsChanged`, `keyAction`), MIT, Mitchell
// Hashimoto and Ghostty contributors, see NOTICE.
//
// A surface is given only the keys its pane's program already got from the daemon, and only for
// what libghostty does on a keystroke by itself: scroll to the bottom, clear a selection, hide
// the pointer. What it encodes and writes to its PTY is read and dropped by the bridge, so no key
// reaches a program twice.

extension Surface {
  /// A key pressed or held down.
  ///
  /// `committed` is what an input method committed for this keystroke, if anything; otherwise
  /// the text is the event's own, as the layout translates it. `composing` is whether an input
  /// method was or still is composing, which libghostty must not encode.
  public func pressKey(_ event: NSEvent, committed: String?, composing: Bool) {
    let translated = translationEvent(for: event)
    let action = event.isARepeat ? GHOSTTY_ACTION_REPEAT : GHOSTTY_ACTION_PRESS
    if let committed {
      key(event, action, translationMods: translated.modifierFlags, text: committed)
    } else {
      key(
        event, action, translationMods: translated.modifierFlags,
        text: translated.ghosttyCharacters, composing: composing)
    }
  }

  public func releaseKey(_ event: NSEvent) {
    key(event, GHOSTTY_ACTION_RELEASE)
  }

  /// A modifier pressed or released on its own.
  ///
  /// AppKit reports only the flags that are down now, so which way this went is read off whether
  /// the key's own flag - and for a right-hand key, its side - is among them.
  public func changeModifiers(_ event: NSEvent) {
    let mod: UInt32
    switch event.keyCode {
    case 0x39: mod = GHOSTTY_MODS_CAPS.rawValue
    case 0x38, 0x3C: mod = GHOSTTY_MODS_SHIFT.rawValue
    case 0x3B, 0x3E: mod = GHOSTTY_MODS_CTRL.rawValue
    case 0x3A, 0x3D: mod = GHOSTTY_MODS_ALT.rawValue
    case 0x37, 0x36: mod = GHOSTTY_MODS_SUPER.rawValue
    default: return
    }

    var action = GHOSTTY_ACTION_RELEASE
    if ghosttyMods(event.modifierFlags).rawValue & mod != 0 {
      // Down, but a right-hand key is only pressed if its own side is: otherwise this is the
      // right one coming up while the left is still held.
      let raw = event.modifierFlags.rawValue
      let sidePressed =
        switch event.keyCode {
        case 0x3C: raw & UInt(NX_DEVICERSHIFTKEYMASK) != 0
        case 0x3E: raw & UInt(NX_DEVICERCTLKEYMASK) != 0
        case 0x3D: raw & UInt(NX_DEVICERALTKEYMASK) != 0
        case 0x36: raw & UInt(NX_DEVICERCMDKEYMASK) != 0
        default: true
        }
      if sidePressed { action = GHOSTTY_ACTION_PRESS }
    }
    key(event, action)
  }

  /// The event with the modifiers this surface translates characters with, which differ from
  /// the event's own when the surface's config spends option on alt.
  ///
  /// Built only when they differ, because reusing the original event is what keeps some input
  /// methods working - Ghostty's app notes Korean.
  private func translationEvent(for event: NSEvent) -> NSEvent {
    let wanted = eventModifierFlags(
      ghostty_surface_key_translation_mods(surface, ghosttyMods(event.modifierFlags)))
    // Exact states for the four rather than `wanted` as it is, because the event carries hidden
    // bits that certain dead keys depend on.
    var mods = event.modifierFlags
    for flag in [NSEvent.ModifierFlags.shift, .control, .option, .command] {
      if wanted.contains(flag) { mods.insert(flag) } else { mods.remove(flag) }
    }
    guard mods != event.modifierFlags else { return event }
    return NSEvent.keyEvent(
      with: event.type, location: event.locationInWindow, modifierFlags: mods,
      timestamp: event.timestamp, windowNumber: event.windowNumber, context: nil,
      characters: event.characters(byApplyingModifiers: mods) ?? "",
      charactersIgnoringModifiers: event.charactersIgnoringModifiers ?? "",
      isARepeat: event.isARepeat, keyCode: event.keyCode) ?? event
  }

  private func key(
    _ event: NSEvent, _ action: ghostty_input_action_e,
    translationMods: NSEvent.ModifierFlags? = nil, text: String? = nil, composing: Bool = false
  ) {
    var input = ghostty_input_key_s()
    input.action = action
    input.keycode = UInt32(event.keyCode)
    input.mods = ghosttyMods(event.modifierFlags)
    // macOS will not say which modifiers a layout spent on a character. Ghostty's heuristic:
    // control and command never do, and everything else is assumed to have.
    input.consumed_mods = ghosttyMods(
      (translationMods ?? event.modifierFlags).subtracting([.control, .command]))
    input.unshifted_codepoint = 0
    if event.type == .keyDown || event.type == .keyUp,
      let codepoint = event.characters(byApplyingModifiers: [])?.unicodeScalars.first
    {
      input.unshifted_codepoint = codepoint.value
    }
    input.composing = composing

    // libghostty encodes control characters itself, so text that starts with one goes as no
    // text at all: otherwise ctrl+enter does the wrong thing.
    guard let text, let first = text.utf8.first, first >= 0x20 else {
      _ = ghostty_surface_key(surface, input)
      return
    }
    text.withCString { pointer in
      input.text = pointer
      _ = ghostty_surface_key(surface, input)
    }
  }
}

extension NSEvent {
  /// The text a surface is given for this event: a control character as the key without
  /// control, since libghostty maps control itself, and nothing for a function key's
  /// private-use codepoint.
  fileprivate var ghosttyCharacters: String? {
    guard let characters else { return nil }
    if characters.count == 1, let scalar = characters.unicodeScalars.first {
      if scalar.value < 0x20 {
        return self.characters(byApplyingModifiers: modifierFlags.subtracting(.control))
      }
      if scalar.value >= 0xF700 && scalar.value <= 0xF8FF {
        return nil
      }
    }
    return characters
  }
}

/// AppKit's modifier flags, in libghostty's spelling, sides included.
func ghosttyMods(_ flags: NSEvent.ModifierFlags) -> ghostty_input_mods_e {
  var mods: UInt32 = GHOSTTY_MODS_NONE.rawValue
  if flags.contains(.shift) { mods |= GHOSTTY_MODS_SHIFT.rawValue }
  if flags.contains(.control) { mods |= GHOSTTY_MODS_CTRL.rawValue }
  if flags.contains(.option) { mods |= GHOSTTY_MODS_ALT.rawValue }
  if flags.contains(.command) { mods |= GHOSTTY_MODS_SUPER.rawValue }
  if flags.contains(.capsLock) { mods |= GHOSTTY_MODS_CAPS.rawValue }
  let raw = flags.rawValue
  if raw & UInt(NX_DEVICERSHIFTKEYMASK) != 0 { mods |= GHOSTTY_MODS_SHIFT_RIGHT.rawValue }
  if raw & UInt(NX_DEVICERCTLKEYMASK) != 0 { mods |= GHOSTTY_MODS_CTRL_RIGHT.rawValue }
  if raw & UInt(NX_DEVICERALTKEYMASK) != 0 { mods |= GHOSTTY_MODS_ALT_RIGHT.rawValue }
  if raw & UInt(NX_DEVICERCMDKEYMASK) != 0 { mods |= GHOSTTY_MODS_SUPER_RIGHT.rawValue }
  return ghostty_input_mods_e(mods)
}

private func eventModifierFlags(_ mods: ghostty_input_mods_e) -> NSEvent.ModifierFlags {
  var flags = NSEvent.ModifierFlags(rawValue: 0)
  if mods.rawValue & GHOSTTY_MODS_SHIFT.rawValue != 0 { flags.insert(.shift) }
  if mods.rawValue & GHOSTTY_MODS_CTRL.rawValue != 0 { flags.insert(.control) }
  if mods.rawValue & GHOSTTY_MODS_ALT.rawValue != 0 { flags.insert(.option) }
  if mods.rawValue & GHOSTTY_MODS_SUPER.rawValue != 0 { flags.insert(.command) }
  return flags
}
