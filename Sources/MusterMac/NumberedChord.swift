import AppKit

/// When a half-typed numbered chord is over, decided from the keyboard alone.
///
/// Only under `numbered_chords = "tab_then_pane"` is there anything to decide. There ⌘2 names a
/// tab and the press after it names a pane inside it, and the core takes the first press back on
/// anything that changes something - a keystroke, a click, another chord. What that rule cannot
/// cover is somebody who does nothing at all: ⌘2, let go, walk away. Until this existed the
/// window sat waiting for a second press with its numbers moved, and the only way out was to go
/// and click on something.
///
/// Letting go of the modifier is what a hand means by "that was the whole gesture", and only a
/// shell can see it - the core is handed requests and never a keyboard. So the watching happens
/// here and what it concludes goes back in Muster's own vocabulary, as `EndNumberedChord`.
///
/// Pure, and separate from the window for the reason `SidebarModel` is separate from the
/// sidebar: a decision made inside `flagsChanged` is a decision no test can reach, and this one
/// has a rebound-keymap case nobody would find by driving the app.
public enum NumberedChord {
  /// The modifiers every numbered chord shares, and so the ones whose release ends a sequence.
  ///
  /// Read off the bindings rather than assumed to be ⌘, because `focus_pane_1 = "ctrl+1"` is a
  /// thing a config file may say. The intersection rather than the union: releasing ⇧ while
  /// still holding ⌘ has not ended anything, and only a modifier that every remaining press
  /// needs can be the one that says the hand is finished.
  ///
  /// Empty when the nine are unbound, when they carry no modifier, or when they disagree about
  /// which one - and an empty answer means [`ends`] never fires, leaving the chord to end the
  /// way it always has. That is the safe direction: a chord that outlives the gesture costs a
  /// press, and one that ends early costs the gesture.
  public static func modifiers(_ bindings: [Core.Binding]) -> NSEvent.ModifierFlags {
    let numbered = bindings.filter { $0.action.hasPrefix(place) && !$0.key.isEmpty }
    guard let first = numbered.first else { return [] }
    return numbered.dropFirst().reduce(menuModifiers(first.modifiers)) { shared, binding in
      shared.intersection(menuModifiers(binding.modifiers))
    }
  }

  /// Whether letting go of these leaves a gesture that was still being typed.
  ///
  /// `held` is what the keyboard reports now, after the release. A window not in the middle of
  /// a chord answers false whatever the hand is doing, so ⌘C costs nothing.
  public static func ends(
    numbering: Roster.Numbering, held: NSEvent.ModifierFlags, chord: NSEvent.ModifierFlags
  ) -> Bool {
    guard numbering.isHalfTyped, !chord.isEmpty else { return false }
    return !held.intersection(.deviceIndependentFlagsMask).contains(chord)
  }

  /// What the nine actions are called, which is the one place this shell reads that name.
  ///
  /// `focus_pane_1` to `focus_pane_9`. They keep those names under both schemes even though
  /// under this one a press means the Nth numbered chord rather than the Nth pane - see
  /// `docs/configuration.md`, which explains why renaming them for a prototype was the thing
  /// that would have made it expensive to take out again.
  private static let place = "focus_pane_"
}

/// The window, with the modifiers it is being held with reported as they move.
///
/// A subclass for one override. Modifier events are not key equivalents and no menu item can
/// carry one, so the only way to see ⌘ come up is to sit in the responder chain - and the
/// window is the end of every chain in it, which a view is not: the first responder here is a
/// pane's surface, the agent list, or a find field depending on where you last clicked, and
/// three overrides that had to agree would be three chances to disagree.
///
/// Deliberately *not* gated on a pane being typeable, unlike `SurfaceView`'s key handling. A
/// gesture begun in a pane that never came up still has to be able to end.
public final class KeyboardWindow: NSWindow {
  /// Called with the modifiers still held, every time the set of them changes.
  public var onModifiersChanged: ((NSEvent.ModifierFlags) -> Void)?

  public override func flagsChanged(with event: NSEvent) {
    super.flagsChanged(with: event)
    onModifiersChanged?(event.modifierFlags)
  }
}
