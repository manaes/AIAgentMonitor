import ProjectDescription

let bundlePrefix = "co.kr.wannypark.aiagentmirror"
// App / AppBLE 를 짝으로 계속 배포하므로 버전·빌드를 한 곳에서만 관리한다 —
// 상수를 안 뽑아두면 둘 중 하나만 올리는 실수가 나기 쉽다(2026-09-17).
// SettingValue 로 타입을 못 박아 둔다 — 그냥 String 이면 settings 딕셔너리 리터럴의
// 타입 추론이 깨진다(다른 원소들의 String 리터럴은 SettingValue 로 암묵 변환되지만,
// 이미 String 으로 확정된 변수는 안 된다).
let marketingVersion: SettingValue = "1.0.0"
let currentProjectVersion: SettingValue = "5"
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
        framework("WidgetShared", deps: [.target(name: "Wire"), .target(name: "MirrorFormat")]),
        unitTests("WidgetSharedTests", for: "WidgetShared"),
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
                .target(name: "WidgetShared"),
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
                // 없으면 iOS 가 로컬 네트워크(사설 IP) 소켓 연결마다 물어보는 권한
                // 팝업 자체가 제대로 안 뜬다 — NetworkClient 가 QR 로 받은 LAN
                // 주소로 iroh 직접 dial 을 시도하는데(재연결 시 discovery 가 안 돼
                // relay/direct 주소를 그대로 쓴다, NetworkClient.swift 참고), 이
                // 키가 없으면 그 첫 연결 시도 도중에 권한 팝업이 뒤늦게(비동기로)
                // 떠서 시도 자체가 타임아웃난다(2026-09-17 실기 확인 — 삭제 후
                // 재설치해도 최초 1회는 항상 재현됨, 권한을 허용한 뒤 재시도하면
                // 바로 성공).
                "NSLocalNetworkUsageDescription":
                    "Mac과 같은 네트워크에서 QUIC(iroh)로 직접 연결하기 위해 필요합니다.",
                "UIApplicationSceneManifest": [
                    "UIApplicationSupportsMultipleScenes": false,
                    "UISceneConfigurations": [
                        "UIWindowSceneSessionRoleApplication": [[
                            "UISceneConfigurationName": "Default",
                            "UISceneDelegateClassName": "$(PRODUCT_MODULE_NAME).SceneDelegate",
                        ]]
                    ],
                ],
                // 개인용 미러 앱이라 자체 암호화(HTTPS/OS 표준 API 외 커스텀 암호화)를
                // 앱에 새로 추가하지 않는다 — TestFlight/App Store Connect 의 수출
                // 규정 준수 질문을 빌드마다 다시 안 받도록 미리 선언해 둔다.
                "ITSAppUsesNonExemptEncryption": false,
                // Tuist 의 기본 Info.plist 는 이 두 키를 리터럴 "1.0"/"1" 로 박아 두고
                // MARKETING_VERSION/CURRENT_PROJECT_VERSION 빌드 설정을 보지 않는다
                // (`Derived/InfoPlists/App-Info.plist` 확인) — 빌드 설정을 실제로
                // 반영하려면 여기서 명시적으로 변수 치환을 걸어야 한다.
                "CFBundleShortVersionString": "$(MARKETING_VERSION)",
                "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)",
            ]),
            sources: ["Sources/App/**"],
            resources: ["Sources/App/Resources/**"],
            entitlements: .dictionary([
                "com.apple.security.application-groups": ["group.co.kr.wannypark.aiagentmirror"],
                "keychain-access-groups": ["$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared"],
            ]),
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "NetworkTransport"),
                .target(name: "MirrorFeature"),
                .external(name: "SnapKit"),
                .target(name: "WidgetShared"),
                .target(name: "AIMonitorWidget"),
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
                // 최초 TestFlight 업로드 기준값(2026-09-17).
                "MARKETING_VERSION": marketingVersion,
                "CURRENT_PROJECT_VERSION": currentProjectVersion,
            ])
        ),
        // BLE 전용(iOS 16+) 변형. **같은 소스 폴더**(Sources/App/**)를 가리키지만
        // NETWORK_TRANSPORT 가 꺼져 있어 SceneDelegate.swift 의 그 블록이 컴파일되지
        // 않는다.
        //
        // ⚠️ 그래도 NSCameraUsageDescription 은 필요하다 — "QR 스캔은 네트워크
        // 전용 기능이니 권한 문구를 안 둬도 된다"는 예전 가정이 틀렸다(2026-09-17
        // App Store Connect ITMS-90683 반려로 확인). `MirrorFeatureBLE` 도
        // `MirrorFeature` 와 같은 소스 폴더(Sources/MirrorFeature/**)를 컴파일
        // 하는데, 그 안의 QRScannerViewController.swift(AVFoundation 카메라 API)
        // 자체는 `#if NETWORK_TRANSPORT` 로 감싸져 있지 않아 BLE 전용 바이너리
        // 에도 카메라 API 심볼이 그대로 링크된다 — Apple 의 정적 바이너리 스캔은
        // 실제 실행 경로가 아니라 심볼 존재 여부만 보므로, 실행 중 절대 안 쓰여도
        // 권한 문구가 있어야 통과한다.
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
                "NSCameraUsageDescription":
                    "이 빌드는 QR 페어링을 쓰지 않지만, 공유 코드에 카메라 API가 포함돼 있어 시스템이 이 문구를 요구합니다.",
                "UIApplicationSceneManifest": [
                    "UIApplicationSupportsMultipleScenes": false,
                    "UISceneConfigurations": [
                        "UIWindowSceneSessionRoleApplication": [[
                            "UISceneConfigurationName": "Default",
                            "UISceneDelegateClassName": "$(PRODUCT_MODULE_NAME).SceneDelegate",
                        ]]
                    ],
                ],
                // App 과 짝으로 계속 배포하므로 같은 수출 규정 준수/버전 처리를 맞춘다.
                "ITSAppUsesNonExemptEncryption": false,
                "CFBundleShortVersionString": "$(MARKETING_VERSION)",
                "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)",
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
                // App 과 같은 기준값 — 두 앱을 짝으로 같이 배포한다(2026-09-17).
                "MARKETING_VERSION": marketingVersion,
                "CURRENT_PROJECT_VERSION": currentProjectVersion,
            ])
        ),
        // 홈 화면 위젯. `App`에만 embed한다(`AppBLE`은 iroh 자체가 없어 위젯
        // 자체 새로고침이 불가능 — 설계 §1 범위 밖).
        //
        // ⚠️ NetworkTransport → BLETransport(CoreBluetooth 포함) 의존 때문에,
        // 실제로 안 쓰여도 이 위젯 바이너리에 Bluetooth API 심볼이 딸려온다 —
        // AppBLE의 NSCameraUsageDescription(ITMS-90683)과 같은 메커니즘
        // (설계 §6). 그래서 아래 NSBluetoothAlwaysUsageDescription이 필요하다.
        .target(
            name: "AIMonitorWidget",
            destinations: .iOS,
            product: .appExtension,
            bundleId: "\(bundlePrefix).widget",
            deploymentTargets: iOS,
            infoPlist: .extendingDefault(with: [
                "NSExtension": [
                    "NSExtensionPointIdentifier": "com.apple.widgetkit-extension",
                ],
                // App Store Connect 검증이 익스텐션 번들에도 이 키를 요구한다
                // ("Missing Info.plist value ... CFBundleDisplayName ... .appex",
                // 빌드 5 업로드에서 반려됨). Tuist 기본 plist 에는 없다.
                "CFBundleDisplayName": "AI Monitor",
                "NSBluetoothAlwaysUsageDescription":
                    "이 위젯은 블루투스를 쓰지 않지만, 공유 코드에 Bluetooth API가 포함돼 있어 시스템이 이 문구를 요구합니다.",
                "CFBundleShortVersionString": "$(MARKETING_VERSION)",
                "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)",
            ]),
            sources: ["Sources/AIMonitorWidget/**"],
            entitlements: .dictionary([
                "com.apple.security.application-groups": ["group.co.kr.wannypark.aiagentmirror"],
                "keychain-access-groups": ["$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared"],
            ]),
            dependencies: [
                .target(name: "WidgetShared"),
                .target(name: "NetworkTransport"),
                .target(name: "Wire"),
                .target(name: "DesignSystem"),
                .target(name: "MirrorFormat"),
            ],
            settings: .settings(base: [
                "DEVELOPMENT_TEAM": "LC8PY3D283",
                "CODE_SIGN_STYLE": "Automatic",
                "MARKETING_VERSION": marketingVersion,
                "CURRENT_PROJECT_VERSION": currentProjectVersion,
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
        .scheme(
            name: "WidgetSharedTests",
            buildAction: .buildAction(targets: [.target("WidgetSharedTests")]),
            testAction: .targets([.testableTarget(target: .target("WidgetSharedTests"))])
        ),
    ]
)
