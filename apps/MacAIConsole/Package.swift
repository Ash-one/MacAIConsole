// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "MacAIConsole",
    platforms: [
        .macOS(.v14)
    ],
    targets: [
        .executableTarget(
            name: "MacAIConsole",
            path: "Sources/MacAIConsole"
        ),
        .testTarget(
            name: "MacAIConsoleTests",
            dependencies: ["MacAIConsole"],
            path: "Tests/MacAIConsoleTests"
        )
    ]
)