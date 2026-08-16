import AppKit
import GhosttyKit

/// The renderer seam: everything libghostty-shaped lives behind this module.
///
/// The contract is deliberately small - stand up a runtime, put a surface in a view,
/// tell it when things change. `architecture.md` names the eventual shape (create a
/// surface in a region, run a pane channel into it, resize it, read its grid). This is
/// the first slice of it, and enough to prove the embedding API drives from outside
/// ghostty's own app.
public enum RendererError: Error {
  case initFailed(Int32)
  case appCreationFailed
  case surfaceCreationFailed
}

// libghostty's runtime callbacks arrive on its own threads - the renderer thread, the IO
// thread - so they must be genuinely non-isolated. Written as closures inside the
// @MainActor class below they would inherit its isolation, and Swift would compile in an
// executor check that aborts the process the first time libghostty calls one from a
// thread that is not main. That failure looks like a libghostty crash and is not one.
// File scope keeps them nonisolated, which is what @convention(c) needs anyway.

private func rendererWakeup(_ userdata: UnsafeMutableRawPointer?) {
  Task { @MainActor in Renderer.current?.tick() }
}

private func rendererAction(
  _ app: ghostty_app_t?, _ target: ghostty_target_s, _ action: ghostty_action_s
) -> Bool {
  // Declining every action. The ones Muster needs - title changes, bells - arrive with
  // the features that consume them.
  false
}

private func rendererReadClipboard(
  _ userdata: UnsafeMutableRawPointer?, _ location: ghostty_clipboard_e,
  _ state: UnsafeMutableRawPointer?
) -> Bool { false }

private func rendererConfirmReadClipboard(
  _ userdata: UnsafeMutableRawPointer?, _ string: UnsafePointer<CChar>?,
  _ state: UnsafeMutableRawPointer?, _ request: ghostty_clipboard_request_e
) {}

private func rendererWriteClipboard(
  _ userdata: UnsafeMutableRawPointer?, _ location: ghostty_clipboard_e,
  _ content: UnsafePointer<ghostty_clipboard_content_s>?, _ len: Int, _ confirm: Bool
) {}

/// The surface's command has exited, which for Muster means the pane's bridge is gone.
///
/// Worth having rather than declining, because a surface whose process ended keeps rendering
/// the last thing it painted - libghostty's own "press any key to close the window" screen,
/// among others - and nothing else in the app can tell that apart from a live pane. Every
/// keystroke after this reaches a channel with nobody on the other end.
///
/// `userdata` is the token the surface was created with, resolved on the main actor rather
/// than dereferenced here: this arrives on libghostty's thread, and a pointer to a Surface
/// that has since been freed is exactly the crash this indirection avoids.
private func rendererCloseSurface(_ userdata: UnsafeMutableRawPointer?, _ processAlive: Bool) {
  let token = UInt(bitPattern: userdata)
  Task { @MainActor in Surface.reportExit(token: token, processAlive: processAlive) }
}

/// One libghostty runtime. Owns the app handle every surface hangs off.
///
/// libghostty calls back when it has work to do rather than being polled, so the host's
/// whole obligation is to forward that wakeup to the main queue.
@MainActor
public final class Renderer {
  private let app: ghostty_app_t
  /// Replaced when the config file is read again, and freed with the app.
  private var config: ghostty_config_t
  /// Where the derived config is written, kept so a reload writes to the same place.
  private let configPath: String

  /// What libghostty made of the configuration Muster handed it, if anything.
  ///
  /// Empty is the ordinary case. A line here means Muster's own translation emitted something
  /// libghostty does not accept, which is a bug in `ghosttyConfiguration` rather than in
  /// anybody's config file - the person's own file was parsed and refused by the core long
  /// before this. Held rather than logged because this module has no way to reach the log, and
  /// answering to whoever built it is the smaller of the two dependencies.
  public private(set) var diagnostics: [String] = []

  /// Stands up the runtime, painting panes the way `appearance` says.
  ///
  /// `configPath` is where the derived libghostty config is written, and is a path rather than
  /// a decision for the same reason every other path is: where a file goes is an OS question.
  /// It must be absolute - libghostty asserts that rather than refusing, so a relative one is
  /// undefined in a release build.
  public init(appearance: Appearance = Appearance(), configPath: String) throws {
    // A program name and nothing else. libghostty parses whatever argv it is handed as its own
    // configuration and as `+action` invocations, so Muster's real arguments would be offered
    // to a parser with opinions about them - `muster --pane w1:p1` has been reaching it all
    // along. argc 0 is not the fix: that exits the process before any error handling can say
    // why. Deliberately leaked, because libghostty keeps the pointer for the process's life.
    var argv: [UnsafeMutablePointer<CChar>?] = [strdup("muster")]
    let rc = argv.withUnsafeMutableBufferPointer { arguments in
      ghostty_init(UInt(arguments.count), arguments.baseAddress!)
    }
    if rc != GHOSTTY_SUCCESS { throw RendererError.initFailed(rc) }

    guard let config = ghostty_config_new() else { throw RendererError.appCreationFailed }
    // Muster's own appearance, translated. Nothing on disk belonging to another application is
    // read: there is no ghostty_config_load_default_files call here any more, so what a pane
    // looks like is decided by ~/.muster/config.toml and nothing else.
    let lines = ghosttyConfiguration(appearance)
    if !lines.isEmpty, write(lines, to: configPath) {
      configPath.withCString { ghostty_config_load_file(config, $0) }
    }
    ghostty_config_finalize(config)
    self.config = config
    self.configPath = configPath
    self.diagnostics = Renderer.complaints(about: config)

    // Six callbacks, and a spike owes real answers to none of them.
    var runtime = ghostty_runtime_config_s(
      userdata: nil,
      supports_selection_clipboard: false,
      wakeup_cb: rendererWakeup,
      action_cb: rendererAction,
      read_clipboard_cb: rendererReadClipboard,
      confirm_read_clipboard_cb: rendererConfirmReadClipboard,
      write_clipboard_cb: rendererWriteClipboard,
      close_surface_cb: rendererCloseSurface
    )

    guard let app = ghostty_app_new(&runtime, config) else {
      ghostty_config_free(config)
      throw RendererError.appCreationFailed
    }
    self.app = app
  }

  // Isolated because both handles are main-actor state: libghostty is not thread-safe,
  // and freeing them off the main actor is exactly the kind of teardown crash that only
  // shows up on quit.
  isolated deinit {
    ghostty_app_free(app)
    ghostty_config_free(config)
  }

  /// The one runtime this process has. libghostty's wakeup callback carries userdata,
  /// but tying it back through an `Unmanaged` pointer buys nothing while exactly one
  /// runtime exists, and costs a retain cycle to get wrong.
  public static var current: Renderer?

  /// What libghostty said about a config it was handed.
  ///
  /// Nothing here is fatal to it: an unknown key and an unparseable value each append one of
  /// these and leave the rest of the file applied.
  private static func complaints(about config: ghostty_config_t) -> [String] {
    (0..<ghostty_config_diagnostics_count(config)).compactMap { at in
      ghostty_config_get_diagnostic(config, at).message.map { String(cString: $0) }
    }
  }

  fileprivate func tick() {
    ghostty_app_tick(app)
  }

  public func setFocus(_ focused: Bool) {
    ghostty_app_set_focus(app, focused)
  }

  /// Repaints every surface from an appearance that has just been read again.
  ///
  /// A whole new config handle rather than a mutation, because there is no setter: the same
  /// file-and-load path a launch takes, handed to `ghostty_app_update_config`, which pushes it
  /// to every surface. Colours, cursor and font size take effect immediately; padding and
  /// scrollback are documented as reaching new surfaces only.
  ///
  /// The old handle is kept and the new one dropped on failure, so a config that will not build
  /// leaves the window looking exactly as it did rather than half repainted.
  public func apply(appearance: Appearance) {
    let lines = ghosttyConfiguration(appearance)
    guard let updated = ghostty_config_new() else { return }
    if !lines.isEmpty, write(lines, to: configPath) {
      configPath.withCString { ghostty_config_load_file(updated, $0) }
    }
    ghostty_config_finalize(updated)
    diagnostics = Renderer.complaints(about: updated)

    ghostty_app_update_config(app, updated)
    ghostty_config_free(config)
    config = updated
  }

  /// Creates a surface that renders into `view`, running `command`.
  ///
  /// The command is the only way bytes reach a surface - libghostty exposes no way to
  /// feed one directly (see docs/observations/libghostty-9f9b8d1d.md section 2), which
  /// is why the pane bridge is a subprocess rather than a function call.
  public func makeSurface(in view: NSView, command: String?) throws -> Surface {
    var config = ghostty_surface_config_new()
    let token = Surface.nextToken()
    // A token rather than a pointer to the Surface, which does not exist yet and would
    // outlive nothing if it did: libghostty hands this back on its own thread, after the
    // surface may already have been freed.
    config.userdata = UnsafeMutableRawPointer(bitPattern: token)
    config.platform_tag = GHOSTTY_PLATFORM_MACOS
    config.platform = ghostty_platform_u(
      macos: ghostty_platform_macos_s(nsview: Unmanaged.passUnretained(view).toOpaque()))
    let scale = view.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor
    config.scale_factor = Double(scale ?? 2)
    // Zero means "no per-surface override", so the size comes from the config the app was built
    // with - which is Muster's, translated. This is the knob a per-pane font size would use,
    // and Muster has no per-pane font size on purpose: a grid you read at a glance wants one.
    config.font_size = 0

    let surface: ghostty_surface_t? =
      if let command {
        command.withCString { c in
          config.command = c
          return ghostty_surface_new(app, &config)
        }
      } else {
        ghostty_surface_new(app, &config)
      }

    guard let surface else { throw RendererError.surfaceCreationFailed }
    return Surface(surface, token: token)
  }
}

/// One rendered pane. Disposable: it owns no truth, and closing it touches no session.
@MainActor
public final class Surface {
  private let surface: ghostty_surface_t
  private let token: UInt

  /// Called when the command this surface is running exits.
  ///
  /// Its argument is whether a process is somehow still alive, which libghostty reports and
  /// Muster has no use for beyond putting it in the log: either way this pane is not one
  /// anybody can type into any more.
  public var onProcessExited: (@MainActor (Bool) -> Void)?

  init(_ surface: ghostty_surface_t, token: UInt) {
    self.surface = surface
    self.token = token
    Surface.living[token] = Held(surface: self)
  }

  isolated deinit {
    Surface.living.removeValue(forKey: token)
    ghostty_surface_free(surface)
  }

  /// A way back to a surface somebody else owns.
  ///
  /// Weak, because a strong entry here would keep every pane ever opened alive for the life
  /// of the app - and worse, would stop the `deinit` that removes it from ever running.
  private struct Held {
    weak var surface: Surface?
  }

  // Surfaces that could still be told their process exited, by the token libghostty carries
  // for them.
  private static var living: [UInt: Held] = [:]
  private static var tokens: UInt = 0

  static func nextToken() -> UInt {
    // From one, because zero is what a null userdata reads as and the two must not collide.
    tokens += 1
    return tokens
  }

  static func reportExit(token: UInt, processAlive: Bool) {
    living[token]?.surface?.onProcessExited?(processAlive)
  }

  public func setSize(width: UInt32, height: UInt32) {
    ghostty_surface_set_size(surface, width, height)
  }

  public func setFocus(_ focused: Bool) {
    ghostty_surface_set_focus(surface, focused)
  }

  /// Sizes this pane's text, in points away from what the configuration asked for.
  ///
  /// An offset rather than a size, because the size it is offsetting from may be the renderer's
  /// own - `[font] size` is optional, and nothing outside this module knows what libghostty
  /// picked. Zero puts it back, which is what makes the reset action a reset rather than a
  /// number Muster would have to remember.
  ///
  /// Driven by a binding action rather than by rebuilding a config, which is what the API
  /// offers for this and costs no file. The string never escapes this module.
  ///
  /// Reset first, always. libghostty's own actions are relative - `increase_font_size:2` adds
  /// two points to whatever is there - so setting an offset twice would double it. The core
  /// republishes the whole presentation on every change and a new pane is handed the offset in
  /// force, so this is called more than once with the same number as a matter of course, and
  /// has to mean the same thing every time.
  /// Returns the actions the renderer would not carry out, which is empty in every ordinary
  /// case. These are named by string and nothing in the suite can check the names: validating
  /// one needs a live surface, which needs a GPU and a window. So the refusal is reported
  /// rather than discarded, and a pin bump that renamed an action shows up as a log line
  /// instead of as a chord that quietly does nothing.
  @discardableResult
  public func setFontSizeOffset(_ points: Int32) -> [String] {
    var refused = act("reset_font_size", [])
    if points > 0 { refused = act("increase_font_size:\(points)", refused) }
    if points < 0 { refused = act("decrease_font_size:\(-points)", refused) }
    return refused
  }

  private func act(_ action: String, _ refused: [String]) -> [String] {
    let carried = action.withCString {
      ghostty_surface_binding_action(surface, $0, UInt(strlen($0)))
    }
    return carried ? refused : refused + [action]
  }

  /// Sends committed text straight into the surface's own terminal.
  ///
  /// Only the spike uses this. Muster's panes are fed by a daemon whose VT holds the
  /// real terminal modes, so nothing that reaches a user's pane may be encoded here.
  public func sendText(_ text: String) {
    text.withCString { ghostty_surface_text(surface, $0, UInt(strlen($0))) }
  }

  /// The pane's grid dimensions, which the daemon needs in cells rather than pixels.
  public var cellSize: (columns: UInt16, rows: UInt16) {
    let size = ghostty_surface_size(surface)
    return (size.columns, size.rows)
  }

  // Selection, which is the surface's own business and nobody else's.
  //
  // Unlike a keystroke, a drag over a pane never reaches the daemon: the selection is made
  // against the grid libghostty has already painted here, so no mode has to be guessed and no
  // daemon has to agree. That is what makes copy possible while reporting mouse buttons to
  // the program in the pane is still blocked (kan a_27CTgqqdv).

  /// Reports where the pointer is, measured from this surface's top left.
  ///
  /// Which is not where AppKit measures from, and an unflipped position selects the mirror
  /// image of the drag. The conversion is the caller's because the caller is the only one a
  /// test can reach - a surface needs a GPU and a window, and forwarding is all this does.
  public func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags) {
    ghostty_surface_mouse_pos(
      surface, Double(point.x), Double(point.y), ghosttyModifiers(modifiers))
  }

  /// Presses or releases the left button, which is what starts and ends a selection.
  ///
  /// Only the left one. The others mean nothing to a selection, and a right-click that
  /// reached the surface would be a context menu Muster has not built.
  public func leftMouse(pressed: Bool, modifiers: NSEvent.ModifierFlags) {
    ghostty_surface_mouse_button(
      surface, pressed ? GHOSTTY_MOUSE_PRESS : GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT,
      ghosttyModifiers(modifiers))
  }

  /// What is selected in this pane, or nil when nothing is.
  ///
  /// Copied out rather than handed back as a pointer: libghostty owns the buffer and wants it
  /// freed before this returns, and a String is what every caller wanted anyway.
  public var selectedText: String? {
    guard ghostty_surface_has_selection(surface) else { return nil }
    var text = ghostty_text_s()
    guard ghostty_surface_read_selection(surface, &text) else { return nil }
    defer { ghostty_surface_free_text(surface, &text) }
    guard let bytes = text.text, text.text_len > 0 else { return nil }
    return String(
      decoding: UnsafeRawBufferPointer(start: bytes, count: Int(text.text_len)), as: UTF8.self)
  }
}

/// Puts the derived config where libghostty can read it, and says whether it got there.
///
/// A failure is not fatal and not even unusual - a read-only home, a directory nobody created -
/// and the consequence is a window on the renderer's own defaults rather than no window. The
/// caller skips the load, and the diagnostics stay empty because nothing was ever handed over.
private func write(_ lines: [String], to path: String) -> Bool {
  let file = URL(fileURLWithPath: path)
  do {
    try FileManager.default.createDirectory(
      at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    try (lines.joined(separator: "\n") + "\n").write(to: file, atomically: true, encoding: .utf8)
    return true
  } catch {
    return false
  }
}

/// AppKit's modifier flags, in libghostty's spelling.
///
/// Only the four that mean something to a selection. Caps lock and the left/right variants
/// exist in the enum and change nothing about dragging out a range of cells.
private func ghosttyModifiers(_ flags: NSEvent.ModifierFlags) -> ghostty_input_mods_e {
  var mods: UInt32 = GHOSTTY_MODS_NONE.rawValue
  if flags.contains(.shift) { mods |= GHOSTTY_MODS_SHIFT.rawValue }
  if flags.contains(.control) { mods |= GHOSTTY_MODS_CTRL.rawValue }
  if flags.contains(.option) { mods |= GHOSTTY_MODS_ALT.rawValue }
  if flags.contains(.command) { mods |= GHOSTTY_MODS_SUPER.rawValue }
  return ghostty_input_mods_e(mods)
}
