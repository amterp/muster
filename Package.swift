// swift-tools-version: 6.2

import PackageDescription

// The shell/core split is drawn here rather than discovered later. Everything left is the
// per-OS shell: a window, a renderer, and the translation from an NSEvent into the
// vocabulary the seam speaks. The core it talks to is Rust and lives in crates/ (MIP-1),
// reached through the one symbol CMuster declares.
//
// GhosttyKit is not in the repo: `./dev -d` builds it from the commit in
// deps/ghostty.pin. A missing xcframework surfaces here as a SwiftPM error naming the
// path.
//
// Neither is muster-bridge, nor libmuster: both come from the Rust workspace, and `./dev`
// stages them where the app and the linker look.
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
    .binaryTarget(name: "GhosttyKit", path: "deps/ghostty/macos/GhosttyKit.xcframework"),
    // The portable core, reached through the one seam symbol. Built by cargo, not by
    // SwiftPM - `./dev` runs the Rust build first, and a missing dylib surfaces here as a
    // linker error naming libmuster.
    .systemLibrary(name: "CMuster", path: "Sources/CMuster"),
    .target(
      name: "MusterRenderer",
      dependencies: ["GhosttyKit"],
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
        "CMuster", "MusterRenderer",
        .product(name: "SwiftProtobuf", package: "swift-protobuf"),
      ],
      linkerSettings: [
        .unsafeFlags([
          "-Ltarget/debug",
          // Loader-relative, so a checkout works without installing anything: the first
          // depth resolves for executables in .build/<triple>/<config>/, the second for a
          // test bundle's deeper Contents/MacOS.
          "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../../../target/debug",
          "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../../../../../../target/debug",
        ])
      ]
    ),
    .executableTarget(name: "muster", dependencies: ["MusterMac", "MusterRenderer"]),
    .testTarget(name: "MusterMacTests", dependencies: ["MusterMac"]),
  ]
)
