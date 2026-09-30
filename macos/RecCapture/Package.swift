// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "rec-capture",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "rec-capture", targets: ["RecCapture"])],
    targets: [
        .executableTarget(name: "RecCapture", path: "Sources", linkerSettings: [
            .linkedFramework("ScreenCaptureKit"), .linkedFramework("AVFoundation"),
            .linkedFramework("CoreMedia"), .linkedFramework("CoreVideo"),
            .linkedFramework("CoreGraphics"), .linkedFramework("AppKit"), .linkedFramework("AudioToolbox"),
        ]),
        .testTarget(name: "RecCaptureTests", dependencies: ["RecCapture"], path: "Tests"),
    ]
)
