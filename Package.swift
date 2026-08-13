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
//
// muster-bridge is not here either: it moved to the Rust workspace (MIP-1), and `./dev`
// stages the built binary beside these so a surface still finds it next to the app.
let package = Package(
  name: "muster",
  platforms: [.macOS(.v14)],
  dependencies: [
    // The seam's runtime, and - as `swift run protoc` and `swift run protoc-gen-swift` -
    // its code generator too. It vendors protoc's own C++ source, so `./dev --proto`
    // needs nothing installed and cannot pick up whichever protoc happens to be first on
    // PATH. Nothing it generates is committed; see proto/muster.proto.
    .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.38.0")
  ],
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
    // The portable core, reached through the one seam symbol. Built by cargo, not by
    // SwiftPM - `./dev` runs the Rust build first, and a missing dylib surfaces here as a
    // linker error naming libmuster.
    .systemLibrary(name: "CMuster", path: "Sources/CMuster"),
    // What the shell still needs from libghostty-vt: the key encoder, on the path from an
    // NSEvent to a socket. The grid oracle and the terminal it reads moved to the Rust
    // crate of the same name (MIP-1); this shrinks to nothing when the input path follows.
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
    // The macOS shell, as a library rather than as part of the executable. Everything here
    // assumes an OS, which is allowed - it is the per-OS layer - but an executable whose
    // entry point is top-level code cannot be imported by a test target, and this layer
    // has shipped bugs. A library keeps it reachable (docs/testing.md, thin shell).
    .target(
      name: "MusterMac",
      dependencies: [
        "CMuster", "MusterCore", "MusterRenderer",
        .product(name: "SwiftProtobuf", package: "swift-protobuf"),
      ],
      linkerSettings: [
        .unsafeFlags([
          "-Ltarget/debug",
          // Loader-relative, so a checkout works without installing anything: the first
          // depth resolves for executables in .build/<triple>/<config>/, the second for a
          // test bundle's deeper Contents/MacOS. Same arrangement as MusterVT's.
          "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../../../target/debug",
          "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../../../../../../target/debug",
        ])
      ]
    ),
    .executableTarget(
      name: "muster",
      dependencies: ["MusterCore", "MusterHerdr", "MusterMac", "MusterRenderer", "MusterVT"]),
    // Test plumbing, shared rather than duplicated.
    .target(name: "TestSupport", dependencies: ["MusterCore"], path: "Tests/Support"),
    .testTarget(name: "MusterCoreTests", dependencies: ["MusterCore", "TestSupport"]),
    .testTarget(name: "MusterHerdrTests", dependencies: ["MusterHerdr", "TestSupport"]),
    .testTarget(name: "MusterMacTests", dependencies: ["MusterMac", "MusterCore", "TestSupport"]),
    .testTarget(
      name: "MusterVTTests",
      dependencies: ["MusterVT", "TestSupport"]),
  ]
)
