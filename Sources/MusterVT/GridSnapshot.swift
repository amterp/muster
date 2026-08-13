/// A terminal grid as text a reviewer can read in a diff.
///
/// `docs/testing.md` asks for cases that are data rather than code, so this is the
/// format those files are in. Two properties matter more than looking nice:
///
/// Trailing blanks are cut from every row. A grid is mostly empty space, and a file
/// carrying 80 columns of trailing whitespace per line is one save-with-trim away from a
/// spurious diff - which would train a reviewer to ignore snapshot changes, the one
/// habit that makes the whole approach worthless.
///
/// Row numbers are on every line. Without them, a diff of a mostly-blank screen shows
/// two identical-looking hunks and no way to tell which row moved.
public enum GridSnapshot {
  public static func render(_ grid: Grid) -> String {
    let columns = grid.rows.first?.cells.count ?? 0
    let width = String(grid.rows.count).count

    var lines = [
      "grid \(columns)x\(grid.rows.count)",
      "cursor \(grid.cursor.column),\(grid.cursor.row)"
        + (grid.cursor.isVisible ? "" : " (hidden)"),
      "",
    ]

    for (index, row) in grid.rows.enumerated() {
      let number = String(index).leftPadded(to: width)
      let text = row.text.trimmedTrailingSpaces()
      // A separator even on empty rows, so a row that gained a single leading space
      // shows up as a changed line rather than as an invisible one.
      lines.append(text.isEmpty ? "\(number) |" : "\(number) | \(text)")
    }

    return lines.joined(separator: "\n") + "\n"
  }
}

extension String {
  fileprivate func leftPadded(to width: Int) -> String {
    String(repeating: " ", count: max(0, width - count)) + self
  }

  fileprivate func trimmedTrailingSpaces() -> String {
    var text = self
    while text.last == " " { text.removeLast() }
    return text
  }
}
