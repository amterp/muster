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

private func rendererCloseSurface(_ userdata: UnsafeMutableRawPointer?, _ processAlive: Bool) {}

/// One libghostty runtime. Owns the app handle every surface hangs off.
///
/// libghostty calls back when it has work to do rather than being polled, so the host's
/// whole obligation is to forward that wakeup to the main queue.
@MainActor
public final class Renderer {
  private let app: ghostty_app_t
  private let config: ghostty_config_t

  public init() throws {
    // libghostty parses the real argv here - it is how `ghostty +action` works - so it
    // wants the process's own, not an empty stand-in. Handing it argc 0 exits the
    // process before any of our error handling can say why.
    let rc = ghostty_init(UInt(CommandLine.argc), CommandLine.unsafeArgv)
    if rc != GHOSTTY_SUCCESS { throw RendererError.initFailed(rc) }

    guard let config = ghostty_config_new() else { throw RendererError.appCreationFailed }
    // The user's own ghostty config decides fonts, colors, and cursor style. Muster has
    // no opinion on those yet, and inheriting them means panes look like the terminal
    // this developer already tuned.
    ghostty_config_load_default_files(config)
    ghostty_config_finalize(config)
    self.config = config

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

  fileprivate func tick() {
    ghostty_app_tick(app)
  }

  public func setFocus(_ focused: Bool) {
    ghostty_app_set_focus(app, focused)
  }

  /// Creates a surface that renders into `view`, running `command`.
  ///
  /// The command is the only way bytes reach a surface - libghostty exposes no way to
  /// feed one directly (see docs/observations/libghostty-9f9b8d1d.md section 2), which
  /// is why the pane bridge is a subprocess rather than a function call.
  public func makeSurface(in view: NSView, command: String?) throws -> Surface {
    var config = ghostty_surface_config_new()
    config.platform_tag = GHOSTTY_PLATFORM_MACOS
    config.platform = ghostty_platform_u(
      macos: ghostty_platform_macos_s(nsview: Unmanaged.passUnretained(view).toOpaque()))
    let scale = view.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor
    config.scale_factor = Double(scale ?? 2)
    config.font_size = 0  // inherit from the user's config

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
    return Surface(surface)
  }
}

/// One rendered pane. Disposable: it owns no truth, and closing it touches no session.
@MainActor
public final class Surface {
  private let surface: ghostty_surface_t

  init(_ surface: ghostty_surface_t) {
    self.surface = surface
  }

  isolated deinit {
    ghostty_surface_free(surface)
  }

  public func setSize(width: UInt32, height: UInt32) {
    ghostty_surface_set_size(surface, width, height)
  }

  public func setFocus(_ focused: Bool) {
    ghostty_surface_set_focus(surface, focused)
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
}
