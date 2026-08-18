import ProjectDescription

let bundlePrefix = "com.dgitx.aiagentmonitor.mirror"
let iOS: DeploymentTargets = .iOS("17.0")

func framework(_ name: String, deps: [TargetDependency] = []) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .framework,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: iOS,
        sources: ["Sources/\(name)/**"],
        dependencies: deps
    )
}

func unitTests(_ name: String, for target: String) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .unitTests,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: iOS,
        sources: ["Tests/\(name)/**"],
        resources: ["../docs/ble-protocol/golden/**"],
        dependencies: [.target(name: target)]
    )
}

let project = Project(
    name: "AIAgentMonitorMirror",
    packages: [],
    targets: [
        framework("MirrorFormat"),
        unitTests("MirrorFormatTests", for: "MirrorFormat"),
        framework("Wire"),
        unitTests("WireTests", for: "Wire"),
        framework("BLETransport", deps: [.target(name: "Wire")]),
        unitTests("BLETransportTests", for: "BLETransport"),
        framework("DesignSystem", deps: [.target(name: "MirrorFormat"), .external(name: "SnapKit")]),
        unitTests("DesignSystemTests", for: "DesignSystem"),
        framework("MirrorFeature", deps: [
            .target(name: "BLETransport"),
            .target(name: "DesignSystem"),
            .target(name: "MirrorFormat"),
            .external(name: "SnapKit"),
        ]),
        unitTests("MirrorFeatureTests", for: "MirrorFeature"),
        .target(
            name: "App",
            destinations: .iOS,
            product: .app,
            bundleId: bundlePrefix,
            deploymentTargets: iOS,
            infoPlist: .extendingDefault(with: [
                "UILaunchScreen": [:],
                "NSBluetoothAlwaysUsageDescription":
                    "Mac 의 AI Agent Monitor 와 연결해 모니터링 화면을 표시합니다.",
                "UIApplicationSceneManifest": [
                    "UIApplicationSupportsMultipleScenes": false,
                    "UISceneConfigurations": [
                        "UIWindowSceneSessionRoleApplication": [[
                            "UISceneConfigurationName": "Default",
                            "UISceneDelegateClassName": "$(PRODUCT_MODULE_NAME).SceneDelegate",
                        ]]
                    ],
                ],
            ]),
            sources: ["Sources/App/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .external(name: "SnapKit"),
            ]
        ),
    ],
    schemes: [
        // Tuist 4.158.2 는 테스트 타겟용 스킴을 자동 생성하지 않고 의존 대상(Wire)의
        // 스킴에 테스트 액션으로 묶는다. CI/리뷰에서 `WireTests` 스킴을 직접 지정해
        // 실행할 수 있도록 명시적으로 선언한다.
        .scheme(
            name: "MirrorFormatTests",
            buildAction: .buildAction(targets: [.target("MirrorFormatTests")]),
            testAction: .targets([.testableTarget(target: .target("MirrorFormatTests"))])
        ),
        .scheme(
            name: "WireTests",
            buildAction: .buildAction(targets: [.target("WireTests")]),
            testAction: .targets([.testableTarget(target: .target("WireTests"))])
        ),
        .scheme(
            name: "BLETransportTests",
            buildAction: .buildAction(targets: [.target("BLETransportTests")]),
            testAction: .targets([.testableTarget(target: .target("BLETransportTests"))])
        ),
        .scheme(
            name: "DesignSystemTests",
            buildAction: .buildAction(targets: [.target("DesignSystemTests")]),
            testAction: .targets([.testableTarget(target: .target("DesignSystemTests"))])
        ),
        .scheme(
            name: "MirrorFeatureTests",
            buildAction: .buildAction(targets: [.target("MirrorFeatureTests")]),
            testAction: .targets([.testableTarget(target: .target("MirrorFeatureTests"))])
        ),
    ]
)
