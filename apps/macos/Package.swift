// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "pprofessor",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "PProfessorKit", targets: ["PProfessorKit"]),
    ],
    targets: [
        .target(
            name: "PProfessorKit",
            path: "Sources/PProfessorKit",
            linkerSettings: [.linkedLibrary("z")]
        ),
        .testTarget(
            name: "PProfessorKitTests",
            dependencies: ["PProfessorKit"],
            path: "Tests/PProfessorKitTests"
        ),
    ]
)
