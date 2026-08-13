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
    // The one herdr-shaped module. Frame decoding lives here rather than in the bridge
    // executable so it can be tested: executables cannot be imported by a test target.
    .target(name: "MusterHerdr", dependencies: ["MusterCore"]),
    .binaryTarget(name: "GhosttyKit", path: "deps/ghostty/macos/GhosttyKit.xcframework"),
    // libghostty-vt: the same engine, headless. It ships an xcframework of its own next
    // to the dylib, and Muster uses the *dylib* instead - deliberately.
    //
    // The two libraries are separate builds of one commit (-Demit-lib-vt turns the
    // xcframework off), and both statically embed Zig's runtime. Linking both archives
    // into one binary fails on 35 duplicate symbols - ubsan handlers defined by each
    // Zig compilation unit. The dylib exports only libghostty-vt's 192 public functions
    // and keeps its runtime private, so it composes with GhosttyKit where the archive
    // cannot. Revisit if upstream ever emits one library carrying both APIs.
    .systemLibrary(name: "CGhosttyVt", path: "Sources/CGhosttyVt"),
    // The terminal Muster reasons with rather than shows: tests read grids from it, and
    // the input path encodes keys with it. Kept apart from MusterRenderer because
    // nothing here needs a GPU, a window, or a running app.
    .target(
      name: "MusterVT",
      dependencies: ["CGhosttyVt", "MusterCore"],
      swiftSettings: [.unsafeFlags(["-Xcc", "-Ideps/ghostty/zig-out/include"])],
      linkerSettings: [
        .unsafeFlags([
          "-Ldeps/ghostty/zig-out/lib",
          // Both rpaths are relative to the loading binary, so a checkout works without
          // installing anything: the first resolves for executables in .build/<triple>/
          // <config>/, the second for a test bundle's deeper Contents/MacOS.
          "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../../../deps/ghostty/zig-out/lib",
          "-Xlinker", "-rpath", "-Xlinker",
          "@loader_path/../../../../../../deps/ghostty/zig-out/lib",
        ])
      ]
    ),
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
    // Spawned as a surface's command, one per visible pane. Its own executable because
    // that is the only shape libghostty can be fed by.
    .executableTarget(name: "muster-bridge", dependencies: ["MusterHerdr"]),
    .executableTarget(
      name: "muster", dependencies: ["MusterCore", "MusterHerdr", "MusterRenderer", "MusterVT"]),
    // Test plumbing, shared rather than duplicated: the snapshot cases the grid oracle
    // writes and the ones the input path will write are the same mechanism.
    .target(name: "TestSupport", path: "Tests/Support"),
    .testTarget(name: "MusterCoreTests", dependencies: ["MusterCore"]),
    .testTarget(name: "MusterHerdrTests", dependencies: ["MusterHerdr"]),
    .testTarget(
      name: "MusterVTTests",
      dependencies: ["MusterVT", "MusterHerdr", "TestSupport"],
      // Snapshots are read from the source tree by path, not from a bundle, so that
      // regenerating one and reading its diff are the same file. Declared here because
      // SwiftPM otherwise warns about them as unhandled resources.
      exclude: ["snapshots"]),
  ]
)
