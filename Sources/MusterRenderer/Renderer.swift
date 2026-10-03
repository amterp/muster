import AppKit
import GhosttyKit
import os

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

/// Takes the actions Muster's find bar and links are built on, and declines the rest.
///
/// The counts are what the bar draws. Starting and ending a search are claimed without doing
/// anything, because Muster's own bar is what starts and ends one - a surface never opens
/// Ghostty's. A link is claimed and handed to the surface it was clicked in, which is the one
/// place that knows which machine the pane is on. Everything else arrives with the feature that
/// consumes it.
private func rendererAction(
  _ app: ghostty_app_t?, _ target: ghostty_target_s, _ action: ghostty_action_s
) -> Bool {
  switch action.tag {
  case GHOSTTY_ACTION_START_SEARCH, GHOSTTY_ACTION_END_SEARCH:
    return true
  case GHOSTTY_ACTION_SEARCH_TOTAL:
    let total = action.action.search_total.total
    report(.total(total >= 0 ? Int(total) : nil), to: target)
    return true
  case GHOSTTY_ACTION_SEARCH_SELECTED:
    let selected = action.action.search_selected.selected
    report(.selected(selected >= 0 ? Int(selected) : nil), to: target)
    return true
  case GHOSTTY_ACTION_OPEN_URL:
    // Copied here, on libghostty's thread: the bytes are only libghostty's until this returns.
    let open = action.action.open_url
    let url =
      open.url.map {
        String(decoding: UnsafeRawBufferPointer(start: $0, count: Int(open.len)), as: UTF8.self)
      }
      ?? ""
    let kind: OpenedLink.Kind = open.kind == GHOSTTY_ACTION_OPEN_URL_KIND_OSC8 ? .hyperlink : .text
    guard target.tag == GHOSTTY_TARGET_SURFACE, let surface = target.target.surface else {
      return false
    }
    let token = UInt(bitPattern: ghostty_surface_userdata(surface))
    Task { @MainActor in Surface.report(OpenedLink(kind: kind, url: url), token: token) }
    return true
  default:
    return false
  }
}

/// Hands a search's news to the surface it is about, on the main actor.
///
/// By the token the surface was created with, for the reason `rendererCloseSurface` uses it: this
/// arrives on libghostty's thread, and the surface may be freed by the time the hop lands.
private func report(_ search: SearchReport, to target: ghostty_target_s) {
  guard target.tag == GHOSTTY_TARGET_SURFACE, let surface = target.target.surface else { return }
  let token = UInt(bitPattern: ghostty_surface_userdata(surface))
  Task { @MainActor in Surface.report(search, token: token) }
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

/// A surface freed with `ghostty_surface_free_detached` has stopped its threads and released
/// its memory. Arrives on the thread that did that, which is not the main thread.
///
/// `userdata` is the count the free was entered in, retained for the trip.
private func rendererSurfaceFreed(_ userdata: UnsafeMutableRawPointer?) {
  guard let userdata else { return }
  Unmanaged<FreesInFlight>.fromOpaque(userdata).takeRetainedValue().ended()
}

/// Surfaces freed and still stopping their threads.
///
/// Counted under a lock rather than on the main actor, because each ends on the thread that
/// freed it, and the one reader that must not miss an end is a deinit that cannot suspend.
final class FreesInFlight: Sendable {
  private let count = OSAllocatedUnfairLock(initialState: 0)

  var value: Int { count.withLock { $0 } }

  func began() { count.withLock { $0 += 1 } }

  func ended() { count.withLock { $0 -= 1 } }
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
  /// What a surface made now is padded by, in points. Held beside the config because a
  /// surface keeps the padding it was made with, and the window has to measure a click from
  /// where that surface's text starts.
  private var padding: Double
  /// Where the derived config is written, kept so a reload writes to the same place.
  private let configPath: String

  /// Surfaces freed and still stopping their threads, which the app must outlive.
  private let frees = FreesInFlight()

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
    self.padding = surfacePadding(appearance)
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
  //
  // Not while a freed surface is still stopping its threads: that runs on a thread of its own
  // and reaches the app until it ends, so the app is ticked until every one has. Past the bound
  // the app is left rather than freed under one, which is a leak in a process that is going
  // away rather than a crash in it. Unreachable today, because `current` holds the one
  // renderer for the life of the process, so this is here for whatever frees one first.
  isolated deinit {
    let deadline = ContinuousClock.now + .seconds(5)
    while frees.value > 0, ContinuousClock.now < deadline {
      ghostty_app_tick(app)
      usleep(1_000)
    }
    guard frees.value == 0 else { return }
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

  /// How many freed surfaces are still stopping their threads.
  public var freesInFlight: Int { frees.value }

  public func setFocus(_ focused: Bool) {
    ghostty_app_set_focus(app, focused)
  }

  /// Repaints every surface from an appearance that has just been read again.
  ///
  /// A whole new config handle rather than a mutation, because there is no setter: the same
  /// file-and-load path a launch takes, handed to `ghostty_app_update_config`, which pushes it
  /// to every surface. Colours, cursor, font size and the wheel's multiplier take effect
  /// immediately; padding and scrollback are documented as reaching new surfaces only.
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
    padding = surfacePadding(appearance)
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
    return Surface(surface, token: token, padding: padding, frees: frees)
  }
}

/// One rendered pane. Disposable: it owns no truth, and closing it touches no session.
@MainActor
public final class Surface {
  let surface: ghostty_surface_t
  private let token: UInt

  /// Where this surface's free is counted until its threads have stopped.
  private let frees: FreesInFlight

  /// Called when the command this surface is running exits.
  ///
  /// Its argument is whether a process is somehow still alive, which libghostty reports and
  /// Muster has no use for beyond putting it in the log: either way this pane is not one
  /// anybody can type into any more.
  public var onProcessExited: (@MainActor (Bool) -> Void)?

  /// Called when a search this surface is running has counted its matches or moved to one.
  public var onSearch: (@MainActor (SearchReport) -> Void)?

  /// Called when somebody cmd-clicks a link in this surface, asking for it to be opened.
  ///
  /// Only reported: whether to open it, and with what, is the shell's to decide, because a link
  /// is text a program printed and may point anywhere.
  public var onOpenLink: (@MainActor (OpenedLink) -> Void)?

  /// The offset this surface is already drawn at.
  ///
  /// Not a second home for the answer - the core owns it - but a memo of what was last pushed
  /// through, so that being told the same number again costs nothing. It has to be here rather
  /// than beside the pane: a surface is thrown away and built again when a bridge dies, and a
  /// fresh one starts at the size the configuration named, which is exactly zero.
  private var fontSizeOffset: Int32 = 0

  /// The space between the text and the surface's top and left edges, in points.
  public let padding: Double

  init(_ surface: ghostty_surface_t, token: UInt, padding: Double, frees: FreesInFlight) {
    self.surface = surface
    self.token = token
    self.padding = padding
    self.frees = frees
    Surface.living[token] = Held(surface: self)
  }

  /// Freed without waiting for its threads, which is not a nicety: they can be waiting for this
  /// thread. They hand it messages through a mailbox of 64 that only a tick drains, and block
  /// when it is full, so a main thread that fell behind and then joined them waited on itself
  /// for good - one window, for three hours (docs/observations/libghostty-9f9b8d1d.md,
  /// section 15). They stop on a thread of their own while this one goes back to ticking.
  isolated deinit {
    Surface.living.removeValue(forKey: token)
    frees.began()
    ghostty_surface_free_detached(
      surface, rendererSurfaceFreed, Unmanaged.passRetained(frees).toOpaque())
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

  static func report(_ search: SearchReport, token: UInt) {
    living[token]?.surface?.onSearch?(search)
  }

  static func report(_ link: OpenedLink, token: UInt) {
    living[token]?.surface?.onOpenLink?(link)
  }

  public func setSize(width: UInt32, height: UInt32) {
    ghostty_surface_set_size(surface, width, height)
  }

  public func setFocus(_ focused: Bool) {
    ghostty_surface_set_focus(surface, focused)
  }

  public func setOcclusion(visible: Bool) {
    ghostty_surface_set_occlusion(surface, visible)
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
  /// two points to whatever is there - so setting an offset twice would double it.
  ///
  /// A number that has not moved does nothing at all, which is not an optimisation. The core
  /// publishes the whole view on every change and a view follows every agent transition, so
  /// this is called with the number already in force many times a second - and the reset above
  /// resizes the grid, so carrying it out each time would reflow every pane in the window
  /// whenever any agent blinked.
  ///
  /// Returns the actions the renderer would not carry out, which is empty in every ordinary
  /// case. These are named by string and nothing in the suite can check the names: validating
  /// one needs a live surface, which needs a GPU and a window. So the refusal is reported
  /// rather than discarded, and a pin bump that renamed an action shows up as a log line
  /// instead of as a chord that quietly does nothing.
  @discardableResult
  public func setFontSizeOffset(_ points: Int32) -> [String] {
    guard points != fontSizeOffset else { return [] }
    fontSizeOffset = points
    var refused = act("reset_font_size", [])
    if points > 0 { refused = act("increase_font_size:\(points)", refused) }
    if points < 0 { refused = act("decrease_font_size:\(-points)", refused) }
    return refused
  }

  /// Searches this pane's screen and history for a needle, replacing any search already
  /// running, and ends the search for nil.
  ///
  /// libghostty searches and marks by itself once told what to look for, on a thread of its
  /// own, and reports its counts through `onSearch`. An empty needle stops searching, which is
  /// what Ghostty's own app sends while its field is empty.
  ///
  /// Answers with the action the renderer would not carry out, and only for a needle. libghostty
  /// reports ending a search that was not running, and emptying one, as not performed, which is
  /// no failure - while a needle refused is the action renamed under a pin bump, and the search
  /// silently finding nothing.
  @discardableResult
  public func search(_ needle: String?) -> [String] {
    let refused = act(needle.map { "search:\($0)" } ?? "end_search", [])
    return needle?.isEmpty == false ? refused : []
  }

  /// Moves to the next match of the running search, or the previous one, scrolling to it.
  ///
  /// Nothing to report either way: libghostty answers a step with no search running as not
  /// performed, which is an ordinary state rather than a refusal.
  public func navigateSearch(next: Bool) {
    _ = act(next ? "navigate_search:next" : "navigate_search:previous", [])
  }

  /// Carries out one of Ghostty's own binding actions on this surface, and answers with it if
  /// libghostty did not.
  ///
  /// Not doing one is often ordinary - there is no prompt above the first one to jump to - so
  /// the caller logs it rather than warns.
  @discardableResult
  public func perform(_ action: SurfaceAction) -> [String] {
    act(action.ghosttyName, [])
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

  /// Draws what an input method is composing at the pane's cursor, or clears it with nil.
  ///
  /// Drawn only: libghostty never writes a preedit to its terminal, so a composition that is
  /// abandoned reaches nobody, and what the method commits goes to the pane the way any typed
  /// text does.
  public func setPreedit(_ text: String?) {
    guard let text, !text.isEmpty else {
      ghostty_surface_preedit(surface, nil, 0)
      return
    }
    text.withCString { ghostty_surface_preedit(surface, $0, UInt(strlen($0))) }
  }

  /// The cell under the pane's cursor, where an input method puts its candidate window: in
  /// points from the surface's top left, with `y` at the cell's bottom edge, as libghostty
  /// answers.
  public var cursorCell: NSRect {
    var x = 0.0
    var y = 0.0
    var width = 0.0
    var height = 0.0
    ghostty_surface_ime_point(surface, &x, &y, &width, &height)
    return NSRect(x: x, y: y, width: width, height: height)
  }

  /// The pane's grid dimensions, which the daemon needs in cells rather than pixels.
  public var cellSize: (columns: UInt16, rows: UInt16) {
    let size = ghostty_surface_size(surface)
    return (size.columns, size.rows)
  }

  /// How big one cell is, in backing pixels.
  ///
  /// The core needs a cell's size to read a `resize_step` written in points, and only a live
  /// surface knows it: it is the font's own measurement and it moves whenever the text is
  /// resized. Reported in the unit libghostty answers in, on the same terms as `setSize` -
  /// whoever owns the view owns the scale factor, because that is where AppKit keeps it.
  ///
  /// `nil` before the surface has been sized, which the caller reports as "could not measure"
  /// rather than papering over with a guess.
  public var cellPixelSize: (width: UInt32, height: UInt32)? {
    let size = ghostty_surface_size(surface)
    guard size.cell_width_px > 0, size.cell_height_px > 0 else { return nil }
    return (size.cell_width_px, size.cell_height_px)
  }

  /// How much of the surface its cells cover, in backing pixels: the size the pane's terminal
  /// is told it has, which is the surface less its padding and less the part of a cell that
  /// does not fit. `nil` before the surface has been sized.
  public var textPixelSize: (width: UInt32, height: UInt32)? {
    let size = ghostty_surface_size(surface)
    guard size.columns > 0, size.rows > 0 else { return nil }
    return (UInt32(size.columns) * size.cell_width_px, UInt32(size.rows) * size.cell_height_px)
  }

  // The pointer and the wheel, which libghostty turns into a selection or a scroll of its own
  // history.
  //
  // The surface holds the pane's real stream and history, so a selection it makes stays on its
  // text however the pane scrolls. What the program in the pane is owed - a mouse report, or
  // arrow keys for a wheel - is the daemon's to send, and what this surface writes back for the
  // same gesture is dropped by the bridge.

  /// Reports where the pointer is, measured from this surface's top left.
  ///
  /// Which is not where AppKit measures from, and an unflipped position selects the mirror
  /// image of the drag. The conversion is the caller's because the caller is the only one a
  /// test can reach - a surface needs a GPU and a window, and forwarding is all this does.
  public func mouseMoved(to point: NSPoint, modifiers: NSEvent.ModifierFlags) {
    ghostty_surface_mouse_pos(
      surface, Double(point.x), Double(point.y), ghosttyMods(modifiers))
  }

  /// Presses or releases a button, named by AppKit's `buttonNumber`, and says whether libghostty
  /// consumed it - which for the right button decides whether AppKit may still treat it as
  /// asking for a context menu.
  @discardableResult
  public func mouseButton(
    _ number: Int, pressed: Bool, modifiers: NSEvent.ModifierFlags
  ) -> Bool {
    ghostty_surface_mouse_button(
      surface, pressed ? GHOSTTY_MOUSE_PRESS : GHOSTTY_MOUSE_RELEASE, ghosttyButton(number),
      ghosttyMods(modifiers))
  }

  /// Whether the program in this pane has asked for the mouse, which is what decides whether a
  /// ctrl-click is the program's or a context menu.
  public var mouseCaptured: Bool {
    ghostty_surface_mouse_captured(surface)
  }

  /// Scrolls this surface's own history, or lets it answer as the pane's modes say.
  ///
  /// `momentum` is already in libghostty's numbering (`scrollMomentum`), because the same
  /// number goes to the daemon.
  public func scroll(dx: Double, dy: Double, precise: Bool, momentum: UInt32) {
    // ghostty_input_scroll_mods_t: precision in the low bit, the momentum phase in the three
    // above it (Ghostty.Input.swift, ScrollMods).
    let mods = ghostty_input_scroll_mods_t((precise ? 1 : 0) | (Int32(momentum) << 1))
    ghostty_surface_mouse_scroll(surface, dx, dy, mods)
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

/// What a surface's search has found, as libghostty reports it: one fact at a time.
/// A link somebody asked to open, as libghostty reported it.
public struct OpenedLink: Equatable, Sendable {
  public enum Kind: Equatable, Sendable {
    /// Text on the screen that looked like a URL or a path, so the target is what was shown.
    case text
    /// An OSC 8 hyperlink, whose target the program chose and the screen does not show.
    case hyperlink
  }

  public let kind: Kind
  public let url: String

  public init(kind: Kind, url: String) {
    self.kind = kind
    self.url = url
  }
}

public enum SearchReport: Equatable, Sendable {
  /// How many matches there are. Nil while libghostty does not know.
  case total(Int?)

  /// Which match is selected, counting from zero. Nil when none is.
  case selected(Int?)
}

/// A scroll's momentum phase, in libghostty's numbering.
///
/// Public because the same number goes to the pane's daemon as well as to the surface, and the
/// shell is what builds both from one event.
public func scrollMomentum(_ phase: NSEvent.Phase) -> UInt32 {
  let momentum =
    switch phase {
    case .began: GHOSTTY_MOUSE_MOMENTUM_BEGAN
    case .stationary: GHOSTTY_MOUSE_MOMENTUM_STATIONARY
    case .changed: GHOSTTY_MOUSE_MOMENTUM_CHANGED
    case .ended: GHOSTTY_MOUSE_MOMENTUM_ENDED
    case .cancelled: GHOSTTY_MOUSE_MOMENTUM_CANCELLED
    case .mayBegin: GHOSTTY_MOUSE_MOMENTUM_MAY_BEGIN
    default: GHOSTTY_MOUSE_MOMENTUM_NONE
    }
  return momentum.rawValue
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

/// AppKit's button numbers in libghostty's spelling, as Ghostty's app maps them
/// (Ghostty.Input.swift, `MouseButton(fromNSEventButtonNumber:)`): the back and forward buttons
/// are eight and nine, as X11 numbers them.
private func ghosttyButton(_ number: Int) -> ghostty_input_mouse_button_e {
  switch number {
  case 0: GHOSTTY_MOUSE_LEFT
  case 1: GHOSTTY_MOUSE_RIGHT
  case 2: GHOSTTY_MOUSE_MIDDLE
  case 3: GHOSTTY_MOUSE_EIGHT
  case 4: GHOSTTY_MOUSE_NINE
  case 5: GHOSTTY_MOUSE_SIX
  case 6: GHOSTTY_MOUSE_SEVEN
  case 7: GHOSTTY_MOUSE_FOUR
  case 8: GHOSTTY_MOUSE_FIVE
  case 9: GHOSTTY_MOUSE_TEN
  case 10: GHOSTTY_MOUSE_ELEVEN
  default: GHOSTTY_MOUSE_UNKNOWN
  }
}
