import Foundation

/// Notices that the config file was saved, and asks for it to be read again.
///
/// A trigger rather than a second path: what it does is dispatch the same `reload_config` action
/// the menu item and a chord dispatch, so there is one place a reload happens and one place to
/// look when it goes wrong ("one action path", `docs/architecture.md`).
///
/// **It watches the directory, not the file.** Every editor worth using saves by writing a
/// temporary file and renaming it over the original, which replaces the inode - so a watch on
/// the file itself fires once, for the deletion, and then watches something nothing will ever
/// write to again. Watching the directory survives that, at the cost of waking on any change
/// inside it; there are two files in there and reading one of them is cheap.
///
/// The debounce is not about frequency, it is about torn reads: a save is often a write followed
/// by a rename, and reading between the two gets half a file. A tenth of a second is below
/// noticing and comfortably past both.
@MainActor
public final class ConfigWatcher {
  private var source: DispatchSourceFileSystemObject?
  private var descriptor: CInt = -1
  private var pending: DispatchWorkItem?
  private let onChange: @MainActor () -> Void

  /// The file whose directory is watched. Held rather than re-derived so that a reload and a
  /// watch cannot disagree about which file they are about.
  private let path: String

  public init(path: String, onChange: @escaping @MainActor () -> Void) {
    self.path = path
    self.onChange = onChange
  }

  /// Starts watching, and says whether it could.
  ///
  /// False is not an error worth stopping for - the reload action still works, and this is the
  /// convenience on top of it - but it is worth a line, because somebody whose saves stopped
  /// taking effect has no other way to find out.
  @discardableResult
  public func start() -> Bool {
    stop()
    let directory = URL(fileURLWithPath: path).deletingLastPathComponent().path
    descriptor = open(directory, O_EVTONLY)
    guard descriptor >= 0 else { return false }

    let source = DispatchSource.makeFileSystemObjectSource(
      fileDescriptor: descriptor, eventMask: [.write, .rename, .delete], queue: .main)
    source.setEventHandler { [weak self] in self?.settle() }
    source.setCancelHandler { [descriptor] in close(descriptor) }
    self.source = source
    source.resume()
    return true
  }

  public func stop() {
    pending?.cancel()
    pending = nil
    source?.cancel()
    source = nil
    // Closing is the cancel handler's, so that a descriptor is never closed while the source
    // still holds it - which is a crash rather than a leak.
    descriptor = -1
  }

  /// Isolated, because both handles are main-actor state and cancelling a dispatch source from
  /// another thread is the kind of teardown crash that only shows up on quit - the same reason
  /// `Renderer`'s is.
  isolated deinit {
    stop()
  }

  /// Waits for the writing to stop before asking anybody to read.
  private func settle() {
    pending?.cancel()
    let work = DispatchWorkItem { [weak self] in
      MainActor.assumeIsolated { self?.onChange() }
    }
    pending = work
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: work)
  }
}
