import AppKit
import Testing

@testable import MusterMac

// The banner stack against the room the panes leave it. What it draws inside is the card's
// (`BannerTests`); these pin the numbers that keep a stack of long messages inside the window:
// the hosting view never reports more height than the panes have, and the layout tells it how much
// that is. Whether it scrolls under a trackpad and how it looks needs a real window.

/// A banner whose message is `paragraphs` paragraphs long, each wrapping to several drawn lines at
/// the stack's width - the shape an agent's numbered list or closing paragraph takes.
private func aLongBanner(_ group: String, seq: UInt64, paragraphs: Int = 8) -> Muster_Banner {
  var banner = aBanner(group, seq: seq)
  banner.body = (0..<paragraphs).map { index in
    var run = Muster_MarkdownRun()
    run.text = String(
      repeating: "Paragraph \(index) says something long enough to wrap onto several lines. ",
      count: 6)
    var paragraph = Muster_MarkdownParagraph()
    paragraph.runs = [run]
    var block = Muster_MarkdownBlock()
    block.paragraph = paragraph
    return block
  }
  return banner
}

@MainActor
private func host(showing banners: Muster_Banner...) -> BannerHost {
  let board = BannerBoard()
  board.apply(bannerStack(banners[0], banners[1], banners[2]))
  let host = BannerHost(board: board)
  host.show(true)
  return host
}

/// Lets SwiftUI take a change of the room, which it applies on the run loop's next turn.
@MainActor
private func settle(_ host: BannerHost) {
  RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
  host.hosting.layoutSubtreeIfNeeded()
}

@MainActor
@Suite("the banner stack is held to the window")
struct BannerLayoutTests {
  @Test("the stack reports no more height than the room it was given, and all of it when it fits")
  func theStackIsBounded() {
    let host = host(
      showing: aLongBanner("a", seq: 1), aLongBanner("b", seq: 2), aLongBanner("c", seq: 3))
    host.room.height = 10_000
    settle(host)
    let whole = host.hosting.intrinsicContentSize.height
    #expect(whole > 240, "the three banners are taller than the room under test: \(whole)")

    host.room.height = 240
    settle(host)
    #expect(host.hosting.intrinsicContentSize.height <= 240)
    #expect(host.hosting.intrinsicContentSize.height > 0)

    host.room.height = 10_000
    settle(host)
    #expect(host.hosting.intrinsicContentSize.height == whole, "room to spare shrinks nothing")
  }

  @Test("the layout gives the stack the panes' height less the insets, and keeps it inside them")
  func theLayoutBoundsTheStack() throws {
    let host = host(
      showing: aLongBanner("a", seq: 1), aLongBanner("b", seq: 2), aLongBanner("c", seq: 3))
    let layout = WindowLayout(frame: NSRect(x: 0, y: 0, width: 1000, height: 260))
    let panes = NSView()
    layout.attach(sidebar: NSView(), strip: panes)
    layout.attach(banners: host.hosting, room: host.room)
    layout.layoutSubtreeIfNeeded()
    layout.layoutSubtreeIfNeeded()

    let inset = BannerStackView.edgeInset
    #expect(host.room.height == panes.frame.height - 2 * inset)
    let frame = host.hosting.frame
    #expect(frame.minY == panes.frame.minY + inset, "the top sits at the panes' top: \(frame)")
    #expect(frame.maxX == panes.frame.maxX - inset, "and the right edge at theirs: \(frame)")
    #expect(frame.maxY <= panes.frame.maxY - inset, "the bottom stays inside the panes: \(frame)")
    #expect(
      host.hosting.intrinsicContentSize.height <= panes.frame.height - 2 * inset,
      "so the stack never asks for more than the constraints give it")
  }

  @Test("a taller window gives the stack more room")
  func theRoomFollowsTheWindow() {
    let host = host(
      showing: aLongBanner("a", seq: 1), aLongBanner("b", seq: 2), aLongBanner("c", seq: 3))
    let layout = WindowLayout(frame: NSRect(x: 0, y: 0, width: 1000, height: 260))
    layout.attach(sidebar: NSView(), strip: NSView())
    layout.attach(banners: host.hosting, room: host.room)
    layout.layoutSubtreeIfNeeded()
    let short = host.room.height

    layout.setFrameSize(NSSize(width: 1000, height: 600))
    layout.layoutSubtreeIfNeeded()
    #expect(host.room.height == short + 340)
  }
}
