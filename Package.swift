// swift-tools-version: 6.2

import PackageDescription

// The shell/core split is drawn here rather than discovered later: `MusterCore` is
// headless and OS-free, `muster` is the thin per-OS shell, and each dependency we do not
// own sits behind its own module. Which language the core ends up in is still open (see
// the kan board), and keeping the boundary visible from the first commit is what keeps
// that decision a package swap instead of a rewrite.
//
// GhosttyKit is not in the repo: `./dev -d` builds it from the commit in
// deps/ghostty.pin. A missing xcframework surfaces here as a SwiftPM error naming the
// path.
let package = Package(
  name: "muster",
  platforms: [.macOS(.v14)],
  targets: [
    .target(name: "MusterCore"),
    .binaryTarget(name: "GhosttyKit", path: "deps/ghostty/macos/GhosttyKit.xcframework"),
    .target(
      name: "MusterRenderer",
      dependencies: ["GhosttyKit", "MusterCore"],
      linkerSettings: [
        // A static library declares no dependencies of its own, so libghostty's are ours
        // to name. This list is the linker's, discovered by building.
        .linkedFramework("AppKit"),
        .linkedFramework("Metal"),
        .linkedFramework("MetalKit"),
        .linkedFramework("CoreText"),
        .linkedFramework("CoreGraphics"),
        .linkedFramework("QuartzCore"),
        .linkedFramework("UniformTypeIdentifiers"),
        // Text Input Services, for the keyboard-layout lookups libghostty does when
        // translating keys.
        .linkedFramework("Carbon"),
        .linkedLibrary("c++"),
      ]
    ),
    .executableTarget(name: "muster", dependencies: ["MusterCore", "MusterRenderer"]),
    .testTarget(name: "MusterCoreTests", dependencies: ["MusterCore"]),
  ]
)
