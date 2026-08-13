import CGhosttyVt

/// What a terminal screen holds, as data.
///
/// Deliberately only text and cell widths. Colors and attributes are real, and a
/// snapshot that carried them would be a better oracle - but it would also be a wall of
/// noise in every diff, and `docs/testing.md` wants cases a reviewer can read. Styling
/// gets added when a test needs it to fail honestly.
public struct Grid: Equatable, Sendable {
  public struct Cell: Equatable, Sendable {
    /// The whole grapheme cluster in this cell. Empty for an unwritten cell.
    public let text: String
    public let width: Width

    public enum Width: Equatable, Sendable {
      case narrow
      case wide
      /// The second half of a wide character. Holds no text of its own.
      case spacerTail
      /// Padding before a wide character that would not fit at the end of a line.
      case spacerHead

      init(_ raw: GhosttyCellWide) {
        self =
          switch raw {
          case GHOSTTY_CELL_WIDE_WIDE: .wide
          case GHOSTTY_CELL_WIDE_SPACER_TAIL: .spacerTail
          case GHOSTTY_CELL_WIDE_SPACER_HEAD: .spacerHead
          default: .narrow
          }
      }
    }

    public init(text: String, width: Width) {
      self.text = text
      self.width = width
    }
  }

  public struct Row: Equatable, Sendable {
    public let cells: [Cell]

    public init(cells: [Cell]) {
      self.cells = cells
    }

    /// The row as a user would read it.
    ///
    /// Spacer tails are dropped rather than rendered as blanks: the wide character
    /// ahead of them already occupies two columns on any terminal showing this text, so
    /// emitting both would widen every CJK line in the snapshot by its own length.
    public var text: String {
      cells
        .filter { $0.width != .spacerTail }
        .map { $0.text.isEmpty ? " " : $0.text }
        .joined()
    }
  }

  public struct Cursor: Equatable, Sendable {
    public let column: UInt16
    public let row: UInt16
    public let isVisible: Bool

    public init(column: UInt16, row: UInt16, isVisible: Bool) {
      self.column = column
      self.row = row
      self.isVisible = isVisible
    }
  }

  public let rows: [Row]
  public let cursor: Cursor

  public init(rows: [Row], cursor: Cursor) {
    self.rows = rows
    self.cursor = cursor
  }
}
