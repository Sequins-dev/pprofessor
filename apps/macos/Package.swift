// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "pprofessor",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "PProfessorKit", targets: ["PProfessorKit"]),
        .library(name: "PProfessorCaptureSupport", targets: ["PProfessorCaptureSupport"]),
    ],
    targets: [
        .target(
            name: "PProfessorKit",
            path: "Sources/PProfessorKit",
            linkerSettings: [.linkedLibrary("z")]
        ),
        .target(
            name: "PProfessorCaptureSupport",
            path: "Sources/PProfessorCaptureSupport"
        ),
        .testTarget(
            name: "PProfessorKitTests",
            dependencies: ["PProfessorKit", "PProfessorCaptureSupport"],
            path: "Tests/PProfessorKitTests"
        ),
    ]
)
