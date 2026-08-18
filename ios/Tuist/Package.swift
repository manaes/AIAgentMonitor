// swift-tools-version: 6.0
import PackageDescription

#if TUIST
import ProjectDescription
let packageSettings = PackageSettings(productTypes: ["SnapKit": .framework])
#endif

let package = Package(
    name: "AIAgentMonitorMirrorDeps",
    dependencies: [
        .package(url: "https://github.com/SnapKit/SnapKit", from: "5.7.1")
    ]
)
