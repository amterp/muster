import CGhosttyVt

/// A terminal with no screen: bytes in, grid out.
///
/// This is the production VT engine running headless - the same code the daemon's own
/// terminals and every ghostty surface run. `docs/testing.md` asks for the user-facing
/// oracle to be the terminal grid computed by that engine rather than by a second
/// implementation written to agree with it, and this is where that comes from.
///
/// It is also how the input path learns what a pane would do with a sequence, which is
/// why this lives in its own module rather than inside the test support code.
public final class Terminal {
  private let terminal: GhosttyTerminal

  public enum Failure: Error {
    case creationFailed(GhosttyResult)
    case resizeFailed(GhosttyResult)
  }

  /// - Parameter graphemeClustering: whether a multi-codepoint cluster occupies one
  ///   cell (DEC mode 2027). Defaults on, because the panes Muster mirrors have it on:
  ///   herdr patches its vendored libghostty-vt to make 2027 the default
  ///   (`vendor/libghostty-vt.patches.md`, `0001-default-grapheme-cluster-mode`), and
  ///   stock libghostty-vt does not. Left off, a ZWJ emoji renders across several cells
  ///   here and one cell in the daemon, so a grid read here would describe a screen the
  ///   user never saw. Found by the cross-oracle test rather than by reading the patch.
  public init(columns: UInt16, rows: UInt16, graphemeClustering: Bool = true) throws {
    var handle: GhosttyTerminal?
    let result = ghostty_terminal_new(nil, &handle, columns, rows)
    guard result == GHOSTTY_SUCCESS, let handle else { throw Failure.creationFailed(result) }
    self.terminal = handle

    // No option on ghostty_terminal_set reaches DEC modes, so this goes in the way any
    // program would set it.
    if graphemeClustering { write(Array("\u{1b}[?2027h".utf8)) }
  }

  deinit {
    ghostty_terminal_free(terminal)
  }

  /// Feeds bytes through the VT parser.
  ///
  /// Never fails, by libghostty's own contract: this input is untrusted by definition,
  /// so malformed sequences are logged and dropped rather than propagated. A frame
  /// stream that has gone wrong shows up as a wrong grid, which is what the snapshot
  /// then catches.
  public func write(_ bytes: some Collection<UInt8>) {
    let buffer = Array(bytes)
    guard !buffer.isEmpty else { return }
    buffer.withUnsafeBufferPointer { ghostty_terminal_vt_write(terminal, $0.baseAddress, $0.count) }
  }

  public func resize(columns: UInt16, rows: UInt16) throws {
    // Cell pixel dimensions feed image protocols and size reports, neither of which a
    // headless grid reader has any use for.
    let result = ghostty_terminal_resize(terminal, columns, rows, 0, 0)
    guard result == GHOSTTY_SUCCESS else { throw Failure.resizeFailed(result) }
  }

  /// Reads the visible screen.
  ///
  /// The viewport rather than the active area, because the viewport is what a user is
  /// looking at, and that is the thing tests are supposed to assert on.
  public func viewport(columns: UInt16, rows: UInt16) -> Grid {
    Grid(
      rows: (0..<rows).map { y in
        Grid.Row(cells: (0..<columns).compactMap { x in cell(column: x, row: y) })
      },
      cursor: cursor)
  }

  /// Where the cursor sits, and whether the user can see it.
  ///
  /// Part of the screen for snapshot purposes: a frame that paints the right glyphs and
  /// leaves the cursor in the wrong cell is a real rendering bug, and a grid-only oracle
  /// would pass it.
  public var cursor: Grid.Cursor {
    var x: UInt16 = 0
    var y: UInt16 = 0
    var visible = true
    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_X, &x)
    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_Y, &y)
    ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE, &visible)
    return Grid.Cursor(column: x, row: y, isVisible: visible)
  }

  private func cell(column: UInt16, row: UInt16) -> Grid.Cell? {
    var point = GhosttyPoint()
    point.tag = GHOSTTY_POINT_TAG_VIEWPORT
    point.value.coordinate = GhosttyPointCoordinate(x: column, y: UInt32(row))

    var ref = GhosttyGridRef(size: MemoryLayout<GhosttyGridRef>.size, node: nil, x: 0, y: 0)
    guard ghostty_terminal_grid_ref(terminal, point, &ref) == GHOSTTY_SUCCESS else { return nil }

    var raw: GhosttyCell = 0
    guard ghostty_grid_ref_cell(&ref, &raw) == GHOSTTY_SUCCESS else { return nil }

    var wide = GHOSTTY_CELL_WIDE_NARROW
    ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide)

    return Grid.Cell(text: graphemes(at: &ref), width: Grid.Cell.Width(wide))
  }

  /// The cell's whole grapheme cluster, not just its first codepoint.
  ///
  /// A snapshot that dropped combining marks would render an agent's output as
  /// something the user never saw, and would do it silently.
  private func graphemes(at ref: inout GhosttyGridRef) -> String {
    var codepoints = [UInt32](repeating: 0, count: 8)
    var count = 0

    var result = codepoints.withUnsafeMutableBufferPointer {
      ghostty_grid_ref_graphemes(&ref, $0.baseAddress, $0.count, &count)
    }
    if result == GHOSTTY_OUT_OF_SPACE {
      codepoints = [UInt32](repeating: 0, count: count)
      result = codepoints.withUnsafeMutableBufferPointer {
        ghostty_grid_ref_graphemes(&ref, $0.baseAddress, $0.count, &count)
      }
    }
    guard result == GHOSTTY_SUCCESS else { return "" }

    let scalars = codepoints.prefix(count).compactMap(Unicode.Scalar.init)
    return String(String.UnicodeScalarView(scalars))
  }
}
