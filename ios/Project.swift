import ProjectDescription

let bundlePrefix = "com.dgitx.aiagentmonitor.mirror"
// iroh-ffi(IrohLib) SwiftPM 매니페스트가 iOS 17.5+ 를 요구해서 전체 배포
// 타깃을 17.5로 올렸다(기존 17.0). 네트워크 전송 추가 이전에는 17.0으로
// 충분했다 — IrohSpike 사전 스파이크에서 이 제약이 처음 드러났다.
//
// 시도했던 "App 은 16.0, NetworkTransport 만 optional(weak-link)" 방식은 안 된다 —
// Swift 는 모듈 자체의 최소 배포 타깃이 임포트하는 쪽보다 높으면 `import` 문 자체를
// 컴파일 거부한다("compiling for iOS 16.0, but module 'NetworkTransport' has a
// minimum deployment target of iOS 17.5"). `@available`/`@_weakLinked` 는 이미
// 임포트된 모듈 안의 개별 심볼에만 적용되고, 모듈 전체의 최소 버전은 못 낮춘다.
//
// 그래서 앱을 통째로 둘로 나눈다 — 같은 소스 폴더(Sources/MirrorFeature, Sources/App)
// 를 가리키는 타깃을 하나씩 더 만들고(MirrorFeatureBLE/AppBLE), 공유 소스 안에서
// NetworkTransport 를 쓰는 부분만 `#if NETWORK_TRANSPORT` 로 감싼다. 이 플래그는
// "전체지원" 타깃(MirrorFeature/App)에만 켜져 있다 — BLE 전용 타깃은 그 블록이 아예
// 컴파일되지 않으므로 NetworkTransport 를 링크할 필요조차 없다.
let iOS: DeploymentTargets = .iOS("17.5")
/// BLE 전용 변형(MirrorFeatureBLE/AppBLE)의 배포 타깃. NetworkTransport 를 전혀
/// 링크하지 않으므로 iroh-ffi 의 17.5 하한과 무관하다.
let iOSBLE: DeploymentTargets = .iOS("16.0")

// 기본값을 iOSBLE(16.0)로 둔다 — MirrorFormat/Wire/BLETransport/DesignSystem 은
// NetworkTransport 를 전혀 모르고 17.5 전용 API 도 안 쓰므로, App/AppBLE 양쪽에서
// 공유할 수 있어야 한다. NetworkTransport(및 그 테스트)만 명시적으로 iOS(17.5) 를
// 넘겨 예외로 둔다.
func framework(_ name: String, deps: [TargetDependency] = [], deploymentTargets: DeploymentTargets = iOSBLE) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .framework,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: deploymentTargets,
        sources: ["Sources/\(name)/**"],
        dependencies: deps
    )
}

func unitTests(_ name: String, for target: String, deploymentTargets: DeploymentTargets = iOSBLE) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .unitTests,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: deploymentTargets,
        sources: ["Tests/\(name)/**"],
        resources: ["../docs/ble-protocol/golden/**"],
        dependencies: [.target(name: target)],
        // 실기기에서 테스트를 돌리려면 XCTest 번들도 서명이 필요하다 — App 과
        // 같은 팀으로 맞추지 않으면 기기 빌드가 실패한다.
        settings: .settings(base: [
            "DEVELOPMENT_TEAM": "LC8PY3D283",
            "CODE_SIGN_STYLE": "Automatic",
        ])
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
        // 전체지원(iOS 17.5+) 변형. 기존 이름/모듈을 그대로 유지한다 —
        // MirrorFeatureTests 의 `@testable import MirrorFeature` 가 이걸 가리킨다.
        .target(
            name: "MirrorFeature",
            destinations: .iOS,
            product: .framework,
            bundleId: "\(bundlePrefix).mirrorfeature",
            deploymentTargets: iOS,
            sources: ["Sources/MirrorFeature/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "NetworkTransport"),
                .target(name: "DesignSystem"),
                .target(name: "MirrorFormat"),
                .external(name: "SnapKit"),
            ],
            settings: .settings(base: ["SWIFT_ACTIVE_COMPILATION_CONDITIONS": "$(inherited) NETWORK_TRANSPORT"])
        ),
        unitTests("MirrorFeatureTests", for: "MirrorFeature", deploymentTargets: iOS),
        // BLE 전용(iOS 16+) 변형. **같은 소스 폴더**(Sources/MirrorFeature/**)를
        // 가리키지만 NETWORK_TRANSPORT 가 꺼져 있어 공유 소스 안의 그 블록이
        // 컴파일되지 않는다 — 그래서 NetworkTransport 를 아예 링크하지 않는다.
        .target(
            name: "MirrorFeatureBLE",
            destinations: .iOS,
            product: .framework,
            bundleId: "\(bundlePrefix).mirrorfeatureble",
            deploymentTargets: iOSBLE,
            sources: ["Sources/MirrorFeature/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "DesignSystem"),
                .target(name: "MirrorFormat"),
                .external(name: "SnapKit"),
            ]
        ),
        framework(
            "NetworkTransport",
            deps: [.target(name: "Wire"), .target(name: "BLETransport"), .external(name: "IrohLib")],
            deploymentTargets: iOS
        ),
        unitTests("NetworkTransportTests", for: "NetworkTransport", deploymentTargets: iOS),
        .target(
            name: "App",
            destinations: .iOS,
            product: .app,
            bundleId: bundlePrefix,
            deploymentTargets: iOS,
            infoPlist: .extendingDefault(with: [
                "UILaunchScreen": [:],
                "CFBundleDisplayName": "AI Monitor",
                "NSBluetoothAlwaysUsageDescription":
                    "Mac 의 AI Agent Monitor 와 연결해 모니터링 화면을 표시합니다.",
                "NSCameraUsageDescription":
                    "Mac 화면에 뜬 페어링 QR 코드를 스캔해 네트워크로 연결합니다.",
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
            resources: ["Sources/App/Resources/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "NetworkTransport"),
                .target(name: "MirrorFeature"),
                .external(name: "SnapKit"),
            ],
            // 실기기 디버그 빌드에 매번 Xcode 에서 Team 을 고르지 않도록 고정한다.
            // "Juwan Park" 이름으로 로컬에 팀이 두 개 있다(4Z3DSP9QUS / LC8PY3D283).
            // ktkpsmobile@gmail.com 계정으로 Xcode 에 로그인한 뒤에는 LC8PY3D283 이
            // Automatic 서명으로 Development 인증서를 즉석에서 발급받아 통과한다 —
            // 실제로 확인된 쪽은 이 팀이다.
            settings: .settings(base: [
                "DEVELOPMENT_TEAM": "LC8PY3D283",
                "CODE_SIGN_STYLE": "Automatic",
                "ASSETCATALOG_COMPILER_APPICON_NAME": "AppIcon",
                "SWIFT_ACTIVE_COMPILATION_CONDITIONS": "$(inherited) NETWORK_TRANSPORT",
            ])
        ),
        // BLE 전용(iOS 16+) 변형. **같은 소스 폴더**(Sources/App/**)를 가리키지만
        // NETWORK_TRANSPORT 가 꺼져 있어 SceneDelegate.swift 의 그 블록이 컴파일되지
        // 않는다 — QR 스캔(카메라)도 네트워크 전송 전용 기능이라 권한 문구를 안 둔다.
        .target(
            name: "AppBLE",
            destinations: .iOS,
            product: .app,
            bundleId: "\(bundlePrefix).ble",
            deploymentTargets: iOSBLE,
            infoPlist: .extendingDefault(with: [
                "UILaunchScreen": [:],
                "CFBundleDisplayName": "AI Monitor (BLE)",
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
            resources: ["Sources/App/Resources/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "MirrorFeatureBLE"),
                .external(name: "SnapKit"),
            ],
            settings: .settings(base: [
                "DEVELOPMENT_TEAM": "LC8PY3D283",
                "CODE_SIGN_STYLE": "Automatic",
                "ASSETCATALOG_COMPILER_APPICON_NAME": "AppIcon",
            ])
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
        .scheme(
            name: "NetworkTransportTests",
            buildAction: .buildAction(targets: [.target("NetworkTransportTests")]),
            testAction: .targets([.testableTarget(target: .target("NetworkTransportTests"))])
        ),
    ]
)
