# iOS 홈 화면 위젯 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `App`(네트워크 지원 iOS 타겟)에 홈 화면 위젯을 추가해, Mac AI Agent Monitor의 tok/s·쿼터 사용량을 앱을 안 열어도 보여준다.

**Architecture:** 새 프레임워크(`WidgetShared`)가 App Group 캐시를 담당하고, 메인 앱은 스냅샷을 받을 때마다 그 캐시에 쓰고 위젯을 리로드한다. 위젯은 캐시를 즉시 보여주면서, `NetworkTransport`에 추가한 단발성 `fetchSnapshotOnce(timeoutSeconds:)`로 짧게 최신값을 시도한다(자동 스케줄 + AppIntent 수동 버튼 두 경로 다 이 메서드 하나를 쓴다).

**Tech Stack:** Swift, Tuist(ProjectDescription), WidgetKit, AppIntents, IrohLib(iroh), Keychain(Security), UserDefaults(App Group)

**Spec:** `docs/superpowers/specs/2026-09-17-ios-widget-design.md`


## 작업 정리 (2026-09-17)

이 문서는 초기 구현 계획을 보존한 기록이다. 아래 체크박스와 코드 예시는 당시 계획이며, 현재 완료 여부나 최신 소스 전체를 보증하지 않는다. 최종 레이아웃은 [설계 문서의 최종 레이아웃 정리](../specs/2026-09-17-ios-widget-design.md#최종-레이아웃-정리-2026-09-17)를 기준으로 한다.

이번 정리에는 Small/Medium 카드 상단 정렬, 여백 조정, 주간 리셋 시간 위치 이동과 빈 줄 예약, 상단 갱신 시간 우측 정렬, 사용량 없는 행의 높이 유지가 포함된다. 사용량 계산·정렬·동기화 로직은 변경하지 않았다. 위젯/포함 앱 빌드와 실제 크기의 SwiftUI 렌더링 검증을 완료했다. 기기 설치·배포는 이번 정리 범위에 포함하지 않는다.

## Global Constraints

- 이번 스코프는 `App` 타겟에만 적용한다. `AppBLE`은 건드리지 않는다(스펙 §1).
- App Group id: `group.co.kr.wannypark.aiagentmirror`.
- Keychain access group(엔타이틀먼트 문자열, Xcode가 빌드 시 `$(AppIdentifierPrefix)`를 팀ID로 치환): `$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared`. **런타임 Swift 코드에서는 이미 치환된 값을 직접 써야 한다**: `"LC8PY3D283.co.kr.wannypark.aiagentmirror.shared"`(팀ID `LC8PY3D283`는 Project.swift에 이미 쓰이고 있다).
- 위젯 익스텐션 bundle id: `co.kr.wannypark.aiagentmirror.widget`, 배포 타깃은 `iOS`(기존 `iOS` 상수, 17.5) 재사용.
- Tuist 4.208.0(`.mise.toml`에 이미 고정됨). `Project.swift`를 고칠 때마다 `mise exec -- tuist generate --no-open`을 다시 실행해야 `.xcodeproj`에 반영된다.
- 위젯이 `NetworkTransport → BLETransport`에 의존하는 한 위젯 바이너리에 CoreBluetooth 심볼이 딸려온다 — `AppBLE`의 `NSCameraUsageDescription`(ITMS-90683)과 같은 이유로, 위젯 Info.plist에도 `NSBluetoothAlwaysUsageDescription`을 미사용이어도 선언한다(스펙 §6, 순수 로직 분리는 이번 스코프 아님).
- 빌드 검증은 시뮬레이터 `platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23`(iPhone 16 Pro, 이번 세션에서 계속 써 온 것)를 그대로 쓴다.
- 커밋은 각 태스크 끝에 있는 대로 진행한다(태스크 단위 검증 통과 후).

---

## 파일 구조 개요

| 파일 | 책임 |
|---|---|
| `ios/Sources/Wire/MirrorSnapshot.swift` (수정) | `Decodable` → `Codable`로 확장 — 위젯 캐시가 인코딩도 해야 함 |
| `ios/Sources/WidgetShared/UsageCacheStore.swift` (신규) | App Group UserDefaults 읽기/쓰기 |
| `ios/Sources/WidgetShared/WidgetKind.swift` (신규) | 위젯 kind 문자열 상수(앱·위젯 공용, 오타 드리프트 방지) |
| `ios/Sources/NetworkTransport/NetworkTokenStore.swift` (수정) | Keychain access group 추가 |
| `ios/Sources/NetworkTransport/NetworkClient.swift` (수정) | `fetchSnapshotOnce(timeoutSeconds:)` + 새 에러 케이스 추가 |
| `ios/Sources/AIMonitorWidget/RefreshUsageIntent.swift` (신규) | 수동 새로고침 AppIntent |
| `ios/Sources/AIMonitorWidget/UsageTimelineProvider.swift` (신규) | 자동 새로고침 TimelineProvider |
| `ios/Sources/AIMonitorWidget/UsageWidgetView.swift` (신규) | SwiftUI 위젯 뷰(Small/Medium/Large) |
| `ios/Sources/AIMonitorWidget/AIMonitorWidgetBundle.swift` (신규) | 익스텐션 진입점 |
| `ios/Sources/MirrorFeature/MirrorViewController.swift` (수정) | 스냅샷 수신 시 캐시 쓰기 + 위젯 리로드 훅 |
| `ios/Project.swift` (수정) | `WidgetShared`/`WidgetSharedTests`/`AIMonitorWidget` 타겟, `App` 엔타이틀먼트·의존성 추가 |

---

### Task 1: Wire — MirrorSnapshot 계열을 Codable로 확장

**Files:**
- Modify: `ios/Sources/Wire/MirrorSnapshot.swift:75` (`MirrorProject`), `:90` (`MirrorAgent`), `:117` (`MirrorSnapshot`)
- Test: `ios/Tests/WireTests/MirrorSnapshotTests.swift`

**Interfaces:**
- Produces: `MirrorSnapshot`/`MirrorAgent`/`MirrorProject`가 `Encodable`도 만족(기존 `Decodable, Equatable, Sendable`에 인코딩 능력 추가) — Task 2의 `UsageCacheStore`가 이걸 그대로 쓴다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/WireTests/MirrorSnapshotTests.swift` 맨 끝에 추가:

```swift
    /// 위젯 캐시(WidgetShared)가 스냅샷을 App Group에 저장하려면 인코딩도
    /// 가능해야 한다 — 지금은 Decodable뿐이라 컴파일이 안 된다.
    func testSnapshotRoundTripsThroughEncodingForWidgetCache() throws {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-sample", withExtension: "json")
        )
        let original = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(contentsOf: url))
        let encoded = try JSONEncoder().encode(original)
        let decoded = try JSONDecoder().decode(MirrorSnapshot.self, from: encoded)
        XCTAssertEqual(decoded, original)
    }
```

- [ ] **Step 2: 테스트가 실패하는지 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WireTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: `error: type 'MirrorSnapshot' does not conform to protocol 'Encodable'`로 컴파일 실패.

- [ ] **Step 3: 최소 구현 — Decodable을 Codable로**

`ios/Sources/Wire/MirrorSnapshot.swift`에서 세 곳을 바꾼다(각각 `Decodable` → `Codable`, 다른 프로토콜 목록은 그대로):

```swift
public struct MirrorProject: Codable, Equatable, Sendable {
```
```swift
public struct MirrorAgent: Codable, Equatable, Sendable {
```
```swift
public struct MirrorSnapshot: Codable, Equatable, Sendable {
```

- [ ] **Step 4: 테스트 통과 확인 + 기존 테스트 회귀 없는지 확인**

```bash
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WireTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: 새 테스트 포함 전부 통과. `Codable`은 `Decodable`의 상위집합이라 기존 디코딩 전용 테스트는 영향받지 않는다.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Wire/MirrorSnapshot.swift ios/Tests/WireTests/MirrorSnapshotTests.swift
git commit -m "feat: MirrorSnapshot 계열을 Codable로 확장 (위젯 캐시 인코딩용)"
```

---

### Task 2: WidgetShared 프레임워크 — App Group 캐시

**Files:**
- Create: `ios/Sources/WidgetShared/UsageCacheStore.swift`
- Create: `ios/Sources/WidgetShared/WidgetKind.swift`
- Create: `ios/Tests/WidgetSharedTests/UsageCacheStoreTests.swift`
- Modify: `ios/Project.swift` (새 타겟 2개 + 스킴 1개)

**Interfaces:**
- Consumes: `Wire.MirrorSnapshot`(Task 1에서 Codable이 됨)
- Produces:
  - `public enum UsageCacheStore { static func save(_ snapshot: MirrorSnapshot, fetchedAt: Date); static func load() -> (snapshot: MirrorSnapshot, fetchedAt: Date)?; static func clear() }`
  - `public let widgetKind: String`(값 `"AIMonitorWidget"`)

- [ ] **Step 1: Project.swift에 WidgetShared/WidgetSharedTests 타겟 추가**

`ios/Project.swift`의 `targets: [` 배열에서, `framework("DesignSystem", ...)`/`unitTests("DesignSystemTests", ...)` 바로 다음(그리고 `MirrorFeature` 타겟 정의 이전)에 추가:

```swift
        framework("WidgetShared", deps: [.target(name: "Wire")]),
        unitTests("WidgetSharedTests", for: "WidgetShared"),
```

`schemes: [` 배열 끝(`NetworkTransportTests` 스킴 다음)에 추가:

```swift
        .scheme(
            name: "WidgetSharedTests",
            buildAction: .buildAction(targets: [.target("WidgetSharedTests")]),
            testAction: .targets([.testableTarget(target: .target("WidgetSharedTests"))])
        ),
```

- [ ] **Step 2: 실패하는 테스트 작성**

`ios/Tests/WidgetSharedTests/UsageCacheStoreTests.swift`:

```swift
import XCTest
import Wire
@testable import WidgetShared

final class UsageCacheStoreTests: XCTestCase {

    override func tearDown() {
        UsageCacheStore.clear()
        super.tearDown()
    }

    private func loadGoldenSnapshot() throws -> MirrorSnapshot {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-sample", withExtension: "json")
        )
        return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(contentsOf: url))
    }

    func testLoadReturnsNilWhenNothingSaved() {
        XCTAssertNil(UsageCacheStore.load())
    }

    func testSavedSnapshotRoundTrips() throws {
        let snap = try loadGoldenSnapshot()
        let fetchedAt = Date(timeIntervalSince1970: 1_755_500_100)

        UsageCacheStore.save(snap, fetchedAt: fetchedAt)
        let loaded = UsageCacheStore.load()

        XCTAssertEqual(loaded?.snapshot, snap)
        XCTAssertEqual(loaded?.fetchedAt, fetchedAt)
    }

    func testClearRemovesSavedValue() throws {
        let snap = try loadGoldenSnapshot()
        UsageCacheStore.save(snap, fetchedAt: Date())
        UsageCacheStore.clear()
        XCTAssertNil(UsageCacheStore.load())
    }

    func testWidgetKindIsStableIdentifier() {
        XCTAssertEqual(widgetKind, "AIMonitorWidget")
    }
}
```

- [ ] **Step 3: 프로젝트 재생성 후 테스트가 실패하는지 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WidgetSharedTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -40
```

기대 결과: `UsageCacheStore`/`widgetKind`가 없어 컴파일 실패.

- [ ] **Step 4: 최소 구현**

`ios/Sources/WidgetShared/WidgetKind.swift`:

```swift
/// App(메인 앱)과 AIMonitorWidget(익스텐션)이 같은 문자열을 쓰기 위한 상수.
/// 위젯 kind는 `Widget.body`의 `StaticConfiguration(kind:)`와
/// `WidgetCenter.shared.reloadTimelines(ofKind:)` 양쪽에 정확히 같은 값이어야
/// 하므로, 문자열 리터럴을 두 곳에 따로 적지 않고 이 상수 하나로 통일한다.
public let widgetKind = "AIMonitorWidget"
```

`ios/Sources/WidgetShared/UsageCacheStore.swift`:

```swift
import Foundation
import Wire

/// App Group으로 App(메인 앱)과 AIMonitorWidget(위젯 익스텐션)이 공유하는
/// 마지막 스냅샷 캐시. 스냅샷과 수신 시각을 한 구조체로 묶어 저장한다 —
/// 따로 저장하면 "새 스냅샷인데 옛날 시각" 같은 불일치가 생길 수 있다
/// (설계 §3.1).
public enum UsageCacheStore {
    private static let suiteName = "group.co.kr.wannypark.aiagentmirror"
    private static let key = "latestSnapshot"

    private struct Cached: Codable {
        let snapshot: MirrorSnapshot
        let fetchedAt: Date
    }

    public static func save(_ snapshot: MirrorSnapshot, fetchedAt: Date) {
        guard let defaults = UserDefaults(suiteName: suiteName) else { return }
        guard let data = try? JSONEncoder().encode(Cached(snapshot: snapshot, fetchedAt: fetchedAt)) else { return }
        defaults.set(data, forKey: key)
    }

    public static func load() -> (snapshot: MirrorSnapshot, fetchedAt: Date)? {
        guard let defaults = UserDefaults(suiteName: suiteName),
              let data = defaults.data(forKey: key),
              let cached = try? JSONDecoder().decode(Cached.self, from: data) else { return nil }
        return (cached.snapshot, cached.fetchedAt)
    }

    /// "연결 재설정" 등으로 캐시를 지워야 할 때.
    public static func clear() {
        UserDefaults(suiteName: suiteName)?.removeObject(forKey: key)
    }
}
```

- [ ] **Step 5: 테스트 통과 확인**

```bash
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WidgetSharedTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -40
```

기대 결과: 4개 테스트 전부 통과. (App Group 엔타이틀먼트가 아직 없어도 `UserDefaults(suiteName:)`는 테스트 러너에서 일반 suite처럼 동작한다 — 설계 §5.)

- [ ] **Step 6: 커밋**

```bash
git add ios/Project.swift ios/Sources/WidgetShared ios/Tests/WidgetSharedTests
git commit -m "feat: WidgetShared 프레임워크 추가 — App Group 사용량 캐시"
```

---

### Task 3: App 타겟 엔타이틀먼트 — App Group + Keychain Sharing

**Files:**
- Modify: `ios/Project.swift:120` 근처(`App` 타겟 정의)

**Interfaces:**
- Consumes: 없음(엔타이틀먼트만 추가)
- Produces: `App` 타겟이 `group.co.kr.wannypark.aiagentmirror` App Group과
  `$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared` Keychain access group을 가짐 — Task 4/6이 이걸 전제로 한다.

- [ ] **Step 1: `App` 타겟에 `entitlements:` 파라미터 추가**

`ios/Project.swift`의 `App` 타겟(`name: "App"`) 정의에서, **`resources: ["Sources/App/Resources/**"],` 다음, `dependencies: [` 앞**에 추가한다. ⚠️ Swift는 라벨 붙은 인자라도 함수 선언 순서를 지켜야 한다 —
`Target.target(...)`의 실제 선언 순서가 `... infoPlist, sources, resources, buildableFolders, copyFiles, headers, entitlements, scripts, ..., dependencies, settings ...`라서, `entitlements:`는 `resources:`보다 뒤, `dependencies:`보다 앞에 와야 컴파일된다(`infoPlist`와 `sources` 사이에 넣으면 "argument 'sources' must precede argument 'entitlements'" 컴파일 에러가 난다):

```swift
            entitlements: .dictionary([
                "com.apple.security.application-groups": ["group.co.kr.wannypark.aiagentmirror"],
                "keychain-access-groups": ["$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared"],
            ]),
```

- [ ] **Step 2: 재생성 + 빌드로 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대 결과: `BUILD SUCCEEDED`. (엔타이틀먼트가 실제로 반영됐는지는 생성된
`ios/AIAgentMonitorMirror.xcodeproj`가 아니라 `codesign -d --entitlements - <App.app>`
로도 확인 가능하지만, 이 태스크에서는 빌드 성공만 확인하고 실제 access group
동작 검증은 Task 4/9에서 한다 — 지금은 아직 이걸 쓰는 코드가 없다.)

- [ ] **Step 3: 커밋**

```bash
git add ios/Project.swift
git commit -m "chore: App 타겟에 App Group + Keychain Sharing 엔타이틀먼트 추가"
```

---

### Task 4: NetworkTokenStore — Keychain Access Group 공유

**Files:**
- Modify: `ios/Sources/NetworkTransport/NetworkTokenStore.swift:20-26`(`baseQuery`)
- Test: `ios/Tests/NetworkTransportTests/NetworkClientTests.swift` (기존 파일에 추가)

**Interfaces:**
- Produces: `NetworkTokenStore`의 모든 Keychain 쿼리가 `kSecAttrAccessGroup`을 포함 — Task 6에서 만들 위젯 익스텐션이 같은 값을 저장/조회할 때 이게 있어야 항목을 찾는다.

**⚠️ 주의:** 이 변경 이후, 이 access group이 없던 **기존 빌드로 저장된 페어링 토큰은 더 이상 안 보인다**(2026-09-17 번들 ID 변경 때와 같은 종류의 1회성 비용 — 설계 §3.3). 재페어링이 필요하다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/NetworkTransportTests/NetworkClientTests.swift` 파일 끝에 추가(access group이 쿼리에 들어가는지는 직접 관측할 수 없으므로, 저장→로드 왕복이 access group을 넣은 뒤에도 깨지지 않는다는 걸 확인하는 회귀 테스트):

```swift
    // MARK: - Keychain access group 공유(위젯과)

    /// 위젯 익스텐션과 토큰을 공유하려면 access group이 필요하다(설계 §3.3).
    /// 이 테스트 자체는 access group 유무를 직접 못 보지만, 그걸 추가한 뒤에도
    /// 저장/조회 왕복이 이 프로세스(테스트 러너) 안에서 깨지지 않는지 확인한다 —
    /// 실제 "위젯에서도 보이는지"는 실기 검증(Task 9) 몫이다.
    func testTokenRoundTripsAfterAccessGroupChange() {
        NetworkTokenStore.clearAll()
        defer { NetworkTokenStore.clearAll() }

        XCTAssertTrue(NetworkTokenStore.saveToken("test-token-value"))
        XCTAssertEqual(NetworkTokenStore.loadToken(), "test-token-value")

        XCTAssertTrue(NetworkTokenStore.saveEndpointIdHex("abcdef01"))
        XCTAssertEqual(NetworkTokenStore.loadEndpointIdHex(), "abcdef01")
    }
```

- [ ] **Step 2: 재생성 + 테스트 실행(변경 전 — 통과해야 정상, baseline 확인용)**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: 새 테스트 포함 통과(access group을 아직 안 넣었으니 지금은 baseline).

- [ ] **Step 3: 구현 — access group 추가**

`ios/Sources/NetworkTransport/NetworkTokenStore.swift`의 `private static let service = "co.kr.wannypark.aiagentmirror"` 바로 다음 줄에 추가:

```swift
    /// Keychain access group. 엔타이틀먼트(`keychain-access-groups`)에는
    /// `$(AppIdentifierPrefix)co.kr.wannypark.aiagentmirror.shared`로 적지만,
    /// `$(AppIdentifierPrefix)`는 Xcode가 빌드 시 치환하는 매크로라 런타임
    /// Swift 코드에서는 이미 치환된 값(팀ID `LC8PY3D283`, Project.swift 참고)을
    /// 그대로 써야 한다 — 매크로 문자열을 그대로 넘기면 Keychain이 그런
    /// access group을 못 찾아 항상 실패한다.
    private static let accessGroup = "LC8PY3D283.co.kr.wannypark.aiagentmirror.shared"
```

그리고 `baseQuery(account:)` 함수를 다음으로 바꾼다(기존 딕셔너리에 한 줄 추가):

```swift
    private static func baseQuery(account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: accessGroup,
        ]
    }
```

- [ ] **Step 4: 재생성 + 테스트 재실행**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: 전부 통과. **만약 Keychain 관련 테스트가 `errSecMissingEntitlement`류로 실패하면**, `NetworkTransportTests` 유닛 테스트 타겟(Project.swift의 `unitTests()` 헬퍼) 자체에도 같은 Keychain Sharing 엔타이틀먼트가 필요하다는 뜻이다 — 그 경우 `unitTests(_:for:deploymentTargets:)` 헬퍼 함수(Project.swift)의 `settings:` 블록에 이 태스크의 Step 1과 같은 `entitlements:` 파라미터를 추가한다. 시뮬레이터는 이 제약이 실기기보다 느슨해서 보통은 그냥 통과한다.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/NetworkTransport/NetworkTokenStore.swift ios/Tests/NetworkTransportTests/NetworkClientTests.swift
git commit -m "feat: NetworkTokenStore에 Keychain Access Group 추가 (위젯과 페어링 정보 공유)"
```

---

### Task 5: NetworkClient — 위젯용 단발성 fetch

**Files:**
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift` (새 public 메서드 + 새 에러 케이스 2개)
- Test: `ios/Tests/NetworkTransportTests/NetworkClientTests.swift`

**Interfaces:**
- Consumes: 기존 `private func authenticate(conn:code:) async throws -> SealedChannel`, `Self.classifyLine(_:) -> SnapshotLine`, `Self.alpn`, `Self.snapshotChunkSizeLimit`(모두 같은 파일 안, 그대로 재사용)
- Produces: `public func fetchSnapshotOnce(timeoutSeconds: Double) async throws -> MirrorSnapshot` — Task 7(TimelineProvider)과 Task 7(AppIntent)이 둘 다 이 메서드 하나만 호출한다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/NetworkTransportTests/NetworkClientTests.swift` 끝에 추가(실제 iroh dial은 CI에서 재현 불가 — 페어링 정보가 아예 없을 때 즉시 `needsPairing`으로 떨어지는, 네트워크 없이도 검증 가능한 경로만 자동화한다. 실제 성공 경로는 Task 9 실기 검증 몫이다):

```swift
    // MARK: - fetchSnapshotOnce (위젯 전용 단발성 fetch)

    /// 저장된 페어링 정보가 없으면 iroh를 아예 건드리지 않고 즉시 실패해야
    /// 한다 — 위젯의 짧은 타임아웃 예산을 존재하지도 않는 연결 시도로
    /// 낭비하면 안 된다.
    func testFetchSnapshotOnceThrowsNeedsPairingWithoutStoredEndpoint() async {
        NetworkTokenStore.clearAll()
        let client = await NetworkClient()

        do {
            _ = try await client.fetchSnapshotOnce(timeoutSeconds: 5)
            XCTFail("페어링 정보가 없으면 반드시 던져야 한다")
        } catch let error as NetworkClientError {
            XCTAssertEqual(error, .needsPairing)
        } catch {
            XCTFail("NetworkClientError.needsPairing 을 기대했는데 \(error)")
        }
    }
```

`NetworkClientError`는 아직 `Equatable`이 아니라 `XCTAssertEqual(error, .needsPairing)`이 컴파일 안 된다 — 같은 테스트 파일 수정 안에서 Step 3과 함께 `NetworkClientError`에 `Equatable`을 추가한다(Step 3 참고).

- [ ] **Step 2: 재생성 + 테스트 실행해서 실패 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4055-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: `fetchSnapshotOnce`가 없어 컴파일 실패.

- [ ] **Step 3: 최소 구현**

`ios/Sources/NetworkTransport/NetworkClient.swift` 맨 끝의 `enum NetworkClientError` 정의를 찾아서(현재):

```swift
enum NetworkClientError: Error {
    case malformedReply
    case authFailed
    case needsPairing
}
```

다음으로 바꾼다(케이스 2개 추가 + 테스트에서 비교할 수 있게 `Equatable` 추가):

```swift
enum NetworkClientError: Error, Equatable {
    case malformedReply
    case authFailed
    case needsPairing
    /// 위젯 전용 — `fetchSnapshotOnce`가 타임아웃 예산 안에 못 끝났을 때.
    case fetchTimedOut
    /// 위젯 전용 — 단발성 fetch 중 받은 스냅샷이 지원 버전이 아닐 때.
    case versionMismatch
}
```

그 다음, `pair(qrPayload:)` 메서드 바로 다음(같은 파일)에 새 public 메서드와 private 헬퍼를 추가한다:

```swift
    /// 위젯 전용. 저장된 페어링 정보로 짧게 한 번만 dial → 인증 → 스냅샷
    /// 한 장을 받고 끝낸다. `runConnection`(스트리밍, 실패 시 3초 후 무한
    /// 재시도)과 달리 **재시도하지 않는다** — 실패하면 그대로 던지고,
    /// 호출부(TimelineProvider/AppIntent)가 캐시 폴백을 결정한다. 이
    /// 인스턴스의 `state`/`snapshots` 퍼블리셔는 건드리지 않는다 — 위젯
    /// 프로세스는 화면에 붙이지 않으므로 구독자가 없다.
    public func fetchSnapshotOnce(timeoutSeconds: Double) async throws -> MirrorSnapshot {
        guard let endpointHex = NetworkTokenStore.loadEndpointIdHex() else {
            throw NetworkClientError.needsPairing
        }
        let relayUrl = NetworkTokenStore.loadRelayUrl()
        let addresses = NetworkTokenStore.loadAddresses()

        let fetchTask = Task { [weak self] () -> MirrorSnapshot in
            guard let self else { throw NetworkClientError.needsPairing }
            return try await self.dialAuthenticateAndReadOne(
                endpointIdHex: endpointHex, relayUrl: relayUrl, addresses: addresses
            )
        }
        let watchdog = Task {
            try? await Task.sleep(nanoseconds: UInt64(timeoutSeconds * 1_000_000_000))
            fetchTask.cancel()
        }
        defer { watchdog.cancel() }

        do {
            return try await fetchTask.value
        } catch is CancellationError {
            throw NetworkClientError.fetchTimedOut
        }
    }

    /// `runConnection`/`listenForSnapshots`와 같은 저수준 조각(`authenticate`,
    /// `classifyLine`)을 재사용하되, 무한 루프 대신 **첫 유효 프레임 하나**를
    /// 받으면 바로 돌려준다.
    private func dialAuthenticateAndReadOne(
        endpointIdHex: String, relayUrl: String?, addresses: [String]
    ) async throws -> MirrorSnapshot {
        guard let idBytes = Data(hexString: endpointIdHex) else {
            throw NetworkClientError.needsPairing
        }
        let endpointId = try EndpointId.fromBytes(bytes: idBytes)
        let addr = EndpointAddr(id: endpointId, relayUrl: relayUrl, addresses: addresses)

        let builder = EndpointBuilder()
        builder.applyN0()
        builder.alpns(alpns: [Self.alpn])
        let ep = try await builder.bind()

        let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
        // code: nil — 위젯은 항상 재연결 경로다(이미 저장된 토큰으로 인증),
        // 새 페어링(코드 입력)은 여기서 절대 일어나지 않는다.
        let channel = try await authenticate(conn: conn, code: nil)

        let recv = try await conn.acceptUni()
        var buffer = Data()
        while true {
            let chunk = try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit)
            buffer.append(chunk)
            while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                let lineData = Data(buffer[..<newlineIndex])
                buffer.removeSubrange(buffer.startIndex...newlineIndex)

                guard case .sealed(let frame) = Self.classifyLine(lineData) else { continue }
                guard let plaintext = try? channel.open(frame) else { continue }
                guard let snap = try? JSONDecoder().decode(MirrorSnapshot.self, from: plaintext) else { continue }
                guard snap.isSupportedVersion else { throw NetworkClientError.versionMismatch }
                return snap
            }
        }
    }
```

- [ ] **Step 4: 재생성 + 테스트 통과 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: 새 테스트 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/NetworkTransport/NetworkClient.swift ios/Tests/NetworkTransportTests/NetworkClientTests.swift
git commit -m "feat: NetworkClient에 위젯용 단발성 fetchSnapshotOnce 추가"
```

---

### Task 6: AIMonitorWidget 타겟 생성 (Project.swift)

**Files:**
- Modify: `ios/Project.swift` (새 타겟 1개, `App`의 `dependencies`에 추가)

**Interfaces:**
- Produces: `AIMonitorWidget`이라는 `.appExtension` 타겟(아직 소스 없음 — Task 7에서 채운다). `App` 타겟이 이걸 의존성으로 가져 자동으로 embed된다.

- [ ] **Step 1: Project.swift에 위젯 타겟 추가**

`ios/Project.swift`의 `targets: [` 배열에서 `AppBLE` 타겟 정의 다음(배열 마지막)에 추가:

```swift
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
                "NSBluetoothAlwaysUsageDescription":
                    "이 위젯은 블루투스를 쓰지 않지만, 공유 코드에 Bluetooth API가 포함돼 있어 시스템이 이 문구를 요구합니다.",
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
            ],
            settings: .settings(base: [
                "DEVELOPMENT_TEAM": "LC8PY3D283",
                "CODE_SIGN_STYLE": "Automatic",
                "MARKETING_VERSION": marketingVersion,
                "CURRENT_PROJECT_VERSION": currentProjectVersion,
            ])
        ),
```

`App` 타겟(`name: "App"`)의 `dependencies: [` 배열에 한 줄 추가(끝에):

```swift
                .target(name: "WidgetShared"),
                .target(name: "AIMonitorWidget"),
```

- [ ] **Step 2: `Sources/AIMonitorWidget/` 자리표시 파일 하나만 두고 재생성 확인**

아직 실제 위젯 코드가 없으면 `sources: ["Sources/AIMonitorWidget/**"]`가 빈 글롭이라 Tuist가 경고할 수 있다 — 임시로 최소 파일 하나를 만든다(Task 7에서 실제 내용으로 교체됨):

```bash
mkdir -p /Users/wannypark/Desktop/@Projects/2_App/4_AIAgentMonitor/ios/Sources/AIMonitorWidget
```

`ios/Sources/AIMonitorWidget/_Placeholder.swift`:

```swift
// Task 7에서 실제 위젯 구현(RefreshUsageIntent/UsageTimelineProvider/
// UsageWidgetView/AIMonitorWidgetBundle)으로 교체된다.
```

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -30
```

기대 결과: `BUILD SUCCEEDED`(위젯 익스텐션이 빈 채로 App에 embed됨).

- [ ] **Step 3: 커밋**

```bash
git add ios/Project.swift ios/Sources/AIMonitorWidget
git commit -m "chore: AIMonitorWidget 익스텐션 타겟 추가 (아직 빈 채)"
```

---

### Task 7: 위젯 구현 — TimelineProvider, AppIntent, 뷰, 번들

**Files:**
- Create: `ios/Sources/AIMonitorWidget/RefreshUsageIntent.swift`
- Create: `ios/Sources/AIMonitorWidget/UsageTimelineProvider.swift`
- Create: `ios/Sources/AIMonitorWidget/UsageWidgetView.swift`
- Create: `ios/Sources/AIMonitorWidget/AIMonitorWidgetBundle.swift`
- Delete: `ios/Sources/AIMonitorWidget/_Placeholder.swift`

**Interfaces:**
- Consumes: `WidgetShared.UsageCacheStore`, `WidgetShared.widgetKind`, `NetworkTransport.NetworkClient.fetchSnapshotOnce(timeoutSeconds:)`(Task 5), `Wire.MirrorSnapshot`/`MirrorAgent`/`AgentKindCode`
- Produces: 위젯 익스텐션의 진입점(`@main struct AIMonitorWidgetBundle`) — Task 8(메인 앱 훅)이 참조하는 `widgetKind`는 이미 `WidgetShared`에 있으므로 이 태스크가 그걸 바꾸진 않는다.

이 태스크는 UI 익스텐션이라 자동화 테스트를 붙이기 어렵다(설계 §5) — 대신 실기 확인은 Task 9에서 한다. 여기서는 **빌드 성공**이 곧 이 태스크의 검증이다.

- [ ] **Step 1: `_Placeholder.swift` 삭제**

```bash
rm /Users/wannypark/Desktop/@Projects/2_App/4_AIAgentMonitor/ios/Sources/AIMonitorWidget/_Placeholder.swift
```

- [ ] **Step 2: RefreshUsageIntent**

`ios/Sources/AIMonitorWidget/RefreshUsageIntent.swift`:

```swift
import AppIntents
import NetworkTransport
import WidgetKit
import WidgetShared

/// 위젯 안의 새로고침 버튼이 실행하는 액션. 위젯 익스텐션 프로세스 안에서
/// 바로 실행되고 메인 앱은 열리지 않는다(`openAppWhenRun`의 기본값이 이미
/// false이지만, 이 설계의 핵심이라 명시적으로 적어 둔다).
struct RefreshUsageIntent: AppIntent {
    static var title: LocalizedStringResource = "사용량 새로고침"
    static var openAppWhenRun: Bool { false }

    func perform() async throws -> some IntentResult {
        let client = NetworkClient()
        // 실패해도(타임아웃·페어링 없음 등) 캐시는 그대로 두고 위젯만
        // 다시 그린다 — "새로고침 실패" 표시는 UsageWidgetView가 신선도
        // 문구로 대신한다(설계 §4).
        if let snapshot = try? await client.fetchSnapshotOnce(timeoutSeconds: 7) {
            UsageCacheStore.save(snapshot, fetchedAt: Date())
        }
        WidgetCenter.shared.reloadTimelines(ofKind: widgetKind)
        return .result()
    }
}
```

- [ ] **Step 3: UsageTimelineProvider**

`ios/Sources/AIMonitorWidget/UsageTimelineProvider.swift`:

```swift
import NetworkTransport
import WidgetKit
import WidgetShared
import Wire

struct UsageEntry: TimelineEntry {
    let date: Date
    let snapshot: MirrorSnapshot?
    let fetchedAt: Date?
}

struct UsageTimelineProvider: TimelineProvider {
    func placeholder(in context: Context) -> UsageEntry {
        UsageEntry(date: Date(), snapshot: nil, fetchedAt: nil)
    }

    func getSnapshot(in context: Context, completion: @escaping (UsageEntry) -> Void) {
        let cached = UsageCacheStore.load()
        completion(UsageEntry(date: Date(), snapshot: cached?.snapshot, fetchedAt: cached?.fetchedAt))
    }

    /// 캐시를 먼저 즉시 보여줄 준비를 하고, 짧은 타임아웃으로 최신값을
    /// 시도한다 — 성공하면 캐시도 갱신, 실패하면 기존 캐시값 그대로
    /// 엔트리를 만든다(설계 §3.4). 다음 갱신은 15분 뒤로 요청하지만 실제
    /// 실행 시점은 iOS 스케줄러가 정한다(설계 §1 성공기준 3).
    func getTimeline(in context: Context, completion: @escaping (Timeline<UsageEntry>) -> Void) {
        Task {
            let cached = UsageCacheStore.load()
            var snapshot = cached?.snapshot
            var fetchedAt = cached?.fetchedAt

            let client = NetworkClient()
            if let fresh = try? await client.fetchSnapshotOnce(timeoutSeconds: 7) {
                let now = Date()
                UsageCacheStore.save(fresh, fetchedAt: now)
                snapshot = fresh
                fetchedAt = now
            }

            let entry = UsageEntry(date: Date(), snapshot: snapshot, fetchedAt: fetchedAt)
            let nextRefresh = Date().addingTimeInterval(15 * 60)
            completion(Timeline(entries: [entry], policy: .after(nextRefresh)))
        }
    }
}
```

- [ ] **Step 4: UsageWidgetView**

`ios/Sources/AIMonitorWidget/UsageWidgetView.swift`:

```swift
import SwiftUI
import WidgetKit
import Wire

struct UsageWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: UsageEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                content(for: snapshot)
            } else {
                emptyState
            }
        }
        // iOS 17+ 위젯은 이 modifier가 없으면 배경이 제대로 안 그려진다.
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Text("Mac 앱에서 먼저 연결하세요")
                .font(.caption)
                .multilineTextAlignment(.center)
            Button(intent: RefreshUsageIntent()) {
                Image(systemName: "arrow.clockwise")
            }
        }
        .padding()
        .widgetURL(URL(string: "aim://open"))
    }

    private func content(for snapshot: MirrorSnapshot) -> some View {
        let ordered = orderedForDisplay(snapshot.agents)
        let shown = family == .systemSmall ? Array(ordered.prefix(1)) : ordered

        return VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(freshnessText)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Spacer()
                Button(intent: RefreshUsageIntent()) {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.plain)
            }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, agent in
                agentRow(agent)
            }
        }
        .padding()
    }

    private func agentRow(_ agent: MirrorAgent) -> some View {
        HStack {
            Text(agentName(agent.kind))
                .font(.subheadline.bold())
            Spacer()
            Text("\(Int(agent.ratePerSec)) tok/s")
                .font(.caption)
            if let pct = agent.usedPct5h {
                Text("· 5h \(Int(pct))%")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var freshnessText: String {
        guard let fetchedAt = entry.fetchedAt else { return "" }
        let minutes = max(0, Int(Date().timeIntervalSince(fetchedAt) / 60))
        return minutes < 1 ? "방금 갱신" : "\(minutes)분 전"
    }

    private func agentName(_ kind: AgentKindCode) -> String {
        switch kind {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .antigravity: return "Antigravity"
        case .unknown: return "Agent"
        }
    }

    /// `MirrorViewController.orderedForDisplay`와 같은 순서(설계 §3.4) —
    /// 위젯만의 새 선택 알고리즘을 만들지 않는다.
    private func orderedForDisplay(_ agents: [MirrorAgent]) -> [MirrorAgent] {
        let claude = agents.filter { $0.kind == .claude }
        let codex = agents.filter { $0.kind == .codex }
        let others = agents.filter { $0.kind != .claude && $0.kind != .codex }
        return claude + codex + others
    }
}
```

- [ ] **Step 5: Widget + WidgetBundle(진입점)**

`ios/Sources/AIMonitorWidget/AIMonitorWidgetBundle.swift`:

```swift
import SwiftUI
import WidgetKit
import WidgetShared

struct AIMonitorWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: widgetKind, provider: UsageTimelineProvider()) { entry in
            UsageWidgetView(entry: entry)
        }
        .configurationDisplayName("AI Agent 사용량")
        .description("Mac AI Agent Monitor의 tok/s·쿼터 사용량을 보여줍니다.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

@main
struct AIMonitorWidgetBundle: WidgetBundle {
    var body: some Widget {
        AIMonitorWidget()
    }
}
```

- [ ] **Step 6: 재생성 + 빌드 확인**

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -40
```

기대 결과: `BUILD SUCCEEDED`(App이 위젯 익스텐션까지 같이 빌드·embed).

- [ ] **Step 7: 커밋**

```bash
git add ios/Sources/AIMonitorWidget
git commit -m "feat: AIMonitorWidget 구현 — TimelineProvider, 수동 새로고침 AppIntent, SwiftUI 뷰"
```

---

### Task 8: MirrorViewController — 캐시 쓰기 훅

**Files:**
- Modify: `ios/Sources/MirrorFeature/MirrorViewController.swift:1-15`(import), `:275-280`(`bind(to:)`의 snapshots sink), `:328-339`(`resetConnection`)

**Interfaces:**
- Consumes: `WidgetShared.UsageCacheStore.save(_:fetchedAt:)`, `WidgetShared.widgetKind`, `WidgetKit.WidgetCenter`
- Produces: 없음(이 파일을 쓰는 다른 코드 없음 — 훅만 추가)

이 파일은 `MirrorFeature`(App, NETWORK_TRANSPORT 켜짐)와 `MirrorFeatureBLE`(AppBLE, 꺼짐) 둘 다 컴파일하는 공유 소스다 — `WidgetShared`/`WidgetKit` import와 이 훅은 반드시 `#if NETWORK_TRANSPORT`로 감싼다(설계 §1: 위젯은 App 전용).

- [ ] **Step 1: import 추가**

`ios/Sources/MirrorFeature/MirrorViewController.swift` 맨 위, 기존 `#if NETWORK_TRANSPORT / import NetworkTransport / #endif` 블록을 다음으로 바꾼다:

```swift
#if NETWORK_TRANSPORT
import NetworkTransport
import WidgetKit
import WidgetShared
#endif
```

- [ ] **Step 2: 스냅샷 sink에 캐시 쓰기 추가**

`bind(to:)` 안의 다음 블록을 찾는다:

```swift
        transport.snapshots
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snap in
                self?.configure(snapshot: snap, now: Date())
            }
            .store(in: &cancellables)
```

다음으로 바꾼다:

```swift
        transport.snapshots
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snap in
                #if NETWORK_TRANSPORT
                // BLE로 받았든 네트워크로 받았든 캐시는 항상 갱신한다 — BLE만
                // 쓰는 사용자도 위젯에 값은 보여야 한다(위젯 자체 새로고침만
                // 안 될 뿐, 설계 §3.1). AppBLE 빌드에는 이 블록 자체가 없다.
                let now = Date()
                UsageCacheStore.save(snap, fetchedAt: now)
                WidgetCenter.shared.reloadTimelines(ofKind: widgetKind)
                #endif
                self?.configure(snapshot: snap, now: Date())
            }
            .store(in: &cancellables)
```

- [ ] **Step 3: "연결 재설정"에서 위젯 캐시도 같이 지우기**

`resetConnection()`의 다음 줄:

```swift
        TokenStore.clear()
        #if NETWORK_TRANSPORT
        NetworkTokenStore.clearAll()
        #endif
```

을:

```swift
        TokenStore.clear()
        #if NETWORK_TRANSPORT
        NetworkTokenStore.clearAll()
        UsageCacheStore.clear()
        WidgetCenter.shared.reloadTimelines(ofKind: widgetKind)
        #endif
```

로 바꾼다.

- [ ] **Step 4: 재생성 + 두 타겟(App/AppBLE) 모두 빌드 확인**

`MirrorFeatureBLE`가 이 파일을 그대로 컴파일하는데 `#if NETWORK_TRANSPORT`로 안 감싼 게 없는지가 핵심 확인 포인트다:

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppBLE \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대 결과: 둘 다 `BUILD SUCCEEDED`. `AppBLE`가 실패하면 `#if NETWORK_TRANSPORT` 가드가 빠진 곳이 있다는 뜻이다.

- [ ] **Step 5: MirrorFeatureTests 회귀 확인**

```bash
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대 결과: 기존 47개 테스트 전부 통과(이 훅은 순수 부수효과 추가라 기존 로직/렌더링 테스트에 영향 없어야 한다).

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/MirrorFeature/MirrorViewController.swift
git commit -m "feat: 스냅샷 수신 시 위젯 캐시 갱신 + WidgetCenter 리로드 훅"
```

---

### Task 9: 통합 검증 (실기)

이 태스크는 코드 변경이 없다 — 시뮬레이터로는 위젯의 실제 백그라운드 새로고침 스케줄링을 신뢰성 있게 재현할 수 없으므로(배터리/사용 패턴 휴리스틱, 설계 §5) 실기기 확인이 필수다.

- [ ] **Step 1: 빌드 번호를 올리고 실기기에 설치**

`ios/Project.swift`의 `currentProjectVersion`을 다음 정수로 올린 뒤:

```bash
cd ios && mise exec -- tuist generate --no-open
xcodebuild archive -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -configuration Release -destination "generic/platform=iOS" \
  -archivePath /tmp/App-widget.xcarchive
```

(실기기에 직접 설치하려면 TestFlight 재배포 대신 Xcode에서 실기기를 선택해 Run 하는 쪽이 더 빠르다 — 이 단계는 사용자가 Xcode GUI로 직접 진행.)

- [ ] **Step 2: 네트워크로 페어링 확인**

앱을 열어 설정에서 "네트워크"로 전환 → Mac의 QR을 스캔해 정상 연결되는지 확인(기존 기능, 회귀 없어야 함). 만약 Task 4에서 access group 문제로 기존 토큰이 무효화됐다면 여기서 자연스럽게 재페어링된다.

- [ ] **Step 3: 위젯 추가 + 캐시 표시 확인**

홈 화면 길게 눌러 위젯 추가 → "AI Agent 사용량" 위젯을 Small/Medium/Large 각각 추가. 앱이 최근에 스냅샷을 받은 적 있으면 위젯에 그 값이 바로 보여야 한다(앱을 완전히 종료한 뒤에도).

- [ ] **Step 4: 수동 새로고침 버튼 확인**

위젯의 새로고침 아이콘을 탭 → 앱이 열리지 않고 그 자리에서 값/신선도 문구가 갱신되는지 확인. Mac 쪽 네트워크 공유를 잠깐 꺼서 실패 케이스도 확인(캐시값 + "새로고침 실패" 없이 그냥 기존 값 유지 — Step 확인 후 Mac 네트워크 공유 다시 켜기).

- [ ] **Step 5: 빈 상태 확인**

설정 → "연결 재설정" → 위젯이 "Mac 앱에서 먼저 연결하세요"로 바뀌는지 확인(Task 8의 캐시 clear 훅 검증).

- [ ] **Step 6: AppBLE 회귀 없음 확인**

`AppBLE`를 실기기에 설치해 평소처럼 동작하는지 확인(위젯 없음이 정상 — 애초에 embed 안 됨).

- [ ] **Step 7: 문제 없으면 최종 커밋(버전 변경분)**

```bash
cd /Users/wannypark/Desktop/@Projects/2_App/4_AIAgentMonitor
git add ios/Project.swift
git commit -m "chore: 위젯 실기 검증용 빌드 번호 갱신"
```
