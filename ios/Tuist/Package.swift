// swift-tools-version: 6.0
import PackageDescription

#if TUIST
import ProjectDescription
let packageSettings = PackageSettings(productTypes: [
    "SnapKit": .framework,
    "IrohLib": .framework,
])
#endif

let package = Package(
    name: "AIAgentMonitorMirrorDeps",
    dependencies: [
        .package(url: "https://github.com/SnapKit/SnapKit", from: "5.7.1"),
        // 격리된 사전 스파이크(계획 문서 Phase 4) — IrohSpike 타겟에서만 쓴다.
        // SwiftPM 통합/빌드시간/iOS 버전 호환성을 확인하기 전까지는 공유
        // 타겟 그래프(App/MirrorFeature)에 절대 물리지 않는다.
        .package(url: "https://github.com/n0-computer/iroh-ffi", from: "1.1.0"),
    ]
)
