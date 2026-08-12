// swift-tools-version: 6.2

import PackageDescription

// The shell/core split is drawn here rather than discovered later: `MusterCore` is
// headless and OS-free, `muster` is the thin per-OS shell. Which language the core
// ends up in is still open (see the kan board), and keeping the boundary visible from
// the first commit is what keeps that decision a package swap instead of a rewrite.
let package = Package(
  name: "muster",
  platforms: [.macOS(.v14)],
  targets: [
    .target(name: "MusterCore"),
    .executableTarget(name: "muster", dependencies: ["MusterCore"]),
    .testTarget(name: "MusterCoreTests", dependencies: ["MusterCore"]),
  ]
)
