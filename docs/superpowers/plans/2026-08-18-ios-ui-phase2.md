# iOS UI 미러링 (2단계) 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iPhone 화면을 macOS Detail 창과 시각적으로 일치시킨다 — 에이전트 카드, 사용량 바, 세션 목록.

**Architecture:** 순수 포맷 로직(`MirrorFormat`)과 디자인 토큰·재사용 뷰(`DesignSystem`)를 의존성 없는 모듈로 분리해 시뮬레이터에서 전부 검증한다. 그 위에 `MirrorFeature`가 `BLETransport`의 Combine 스트림을 구독해 뷰를 갱신하고, `App`은 조립만 한다. 1초 타이머로 카운트다운·상대시간을 갱신하되 추가 전송은 없다.

**Tech Stack:** Swift 6 · UIKit · SnapKit · Combine · Tuist 4.158.2

**Spec:** `docs/superpowers/specs/2026-08-18-ble-ios-mirror-design.md` (§7.1 모듈 그래프, §7.4 화면 대응)

## Global Constraints

- iOS 배포 타깃 **17.0**. 번들 ID 접두사 `com.dgitx.aiagentmonitor.mirror`.
- 한글 주석, 한글 사용자 문구. Swift API 이름은 영어.
- `@unchecked Sendable`, `nonisolated(unsafe)`, `@MainActor` 제거 **금지**.
- 시뮬레이터 목적지는 항상 `'platform=iOS Simulator,name=iPhone 16,OS=18.5'`.
- Tuist 4.158.2는 테스트 타깃의 독립 스킴을 자동 생성하지 않는다. 새 테스트 타깃마다 `Project.swift`의 `schemes:` 배열에 항목을 추가해야 `xcodebuild -scheme`이 찾는다.
- 생성된 Xcode 산출물(`.xcodeproj`/`.xcworkspace`/`Derived/`)은 커밋하지 않는다.
- **Swift 튜플은 `Equatable` 을 만족할 수 없어 `XCTAssertEqual(튜플, 튜플)` 이 컴파일되지 않는다.** 튜플 비교는 `XCTAssertTrue(a == b)` 로 쓴다 (Task 3 실측).
- **Tuist 4.158.2 는 타깃의 소스 글롭 디렉토리가 없으면 `tuist generate` 자체가 실패한다.** 새 모듈을 추가할 때는 `Project.swift` 수정 전에 `mkdir -p ios/Sources/<모듈>` 로 디렉토리를 먼저 만든다 (Task 1 실측).
- **BLE는 시뮬레이터에서 동작하지 않는다.** 이 계획의 모든 자동 테스트는 순수 로직만 다루고, 실제 데이터 표시는 Task 7의 실기기 확인에서 검증한다.

### 미러링 대상의 정확한 값 (macOS 원본에서 그대로 옮긴다)

색상:
- claude 점 `#30d158`, codex 점 `#ff9f0a`
- 카드 배경 `#2c2c2e`, 바 트랙 `#1c1c1e`, 구분선 `#3a3a3c`
- 흐린 텍스트 `#8e8e93`, 더 흐린 텍스트 `#636366`, 기본 텍스트 `#f2f2f7`
- 사용률 숫자 `#30d158`, 카운트다운 `#ff9f0a`, 속도 `#0a84ff`
- 세션 목록 점: dormant `#636366`, idle `#ff9f0a`, active는 에이전트 색

사용량 바 그라디언트 임계치 (`QuotaBar.svelte`와 동일):
- `>= 90` → `#ff9f0a` → `#ff453a`
- `>= 70` → `#30d158` → `#ff9f0a`
- 그 외 → `#30d158` → `#34c759`

치수: 카드 라운드 10, 목록 라운드 8, 바 높이 6·라운드 3, 에이전트 점 8pt, 세션 점 6pt, tok/s 22pt bold, 사용률 13pt bold, **에이전트 이름 12pt semibold(크기 미지정 → body 상속)**, 본문 11pt, 라벨 10pt, 목록 제목 9pt.

> 원본에 실재하는 font-size 는 9·10·11·12·22px 뿐이고 13px 는 `QuotaBar` 의 `.pct` 하나다. 이 목록에 없는 크기를 쓰면 두 화면이 어긋난다.

---

## File Structure

| 파일 | 책임 | 의존 |
|---|---|---|
| `ios/Sources/MirrorFormat/MirrorFormat.swift` | 숫자·시간 포맷 (`format.ts` 이식) | **없음** — 순수 |
| `ios/Sources/MirrorFormat/QuotaDisplay.swift` | 사용률 표시값·그라디언트 색 결정 | 없음 — 순수 |
| `ios/Sources/DesignSystem/Palette.swift` | 색 토큰 | UIKit |
| `ios/Sources/DesignSystem/Typography.swift` | 폰트 토큰 | UIKit |
| `ios/Sources/DesignSystem/DotView.swift` | 원형 점 | UIKit, SnapKit |
| `ios/Sources/DesignSystem/QuotaBarView.swift` | 5h·주간 2단 바 | DesignSystem, MirrorFormat, SnapKit |
| `ios/Sources/MirrorFeature/AgentCardView.swift` | 에이전트 카드 | DesignSystem, MirrorFormat, Wire, SnapKit |
| `ios/Sources/MirrorFeature/SessionRowView.swift` | 세션 목록 한 줄 | DesignSystem, MirrorFormat, Wire, SnapKit |
| `ios/Sources/MirrorFeature/SessionListView.swift` | 세션 목록 컨테이너 | 위 + SnapKit |
| `ios/Sources/MirrorFeature/MirrorViewController.swift` | 화면 조립 · 구독 · 1초 타이머 | BLETransport, 위 전부 |

**수정**: `ios/Project.swift`(타깃 4개 추가), `ios/Sources/App/SceneDelegate.swift`(루트 교체)
**삭제**: `ios/Sources/App/RawDumpViewController.swift` (1단계 진단용, 역할 종료)

---

## Task 1: 포맷 로직 (`MirrorFormat`)

`src/lib/format.ts`를 Swift로 옮긴다. **표기가 한 글자라도 다르면 두 화면이 달라 보인다.**

**Files:**
- Create: `ios/Sources/MirrorFormat/MirrorFormat.swift`
- Create: `ios/Tests/MirrorFormatTests/MirrorFormatTests.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: 없음
- Produces: `enum MirrorFormat` with `static func tokensPerSec(_ v: Float) -> String`, `static func tokensTotal(_ n: UInt32) -> String`, `static func relativeTime(_ epochSecs: UInt64, now: Date) -> String`, `static func countdown(resetAt: UInt64, now: Date) -> String?`

- [ ] **Step 1: Project.swift 에 타깃과 스킴을 추가한다**

`targets:` 배열의 `framework("Wire"),` **앞에** 추가:
```swift
        framework("MirrorFormat"),
        unitTests("MirrorFormatTests", for: "MirrorFormat"),
```

`schemes:` 배열에 추가:
```swift
        .scheme(
            name: "MirrorFormatTests",
            buildAction: .buildAction(targets: [.target("MirrorFormatTests")]),
            testAction: .targets([.testableTarget(target: .target("MirrorFormatTests"))])
        ),
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`ios/Tests/MirrorFormatTests/MirrorFormatTests.swift`:
```swift
import XCTest
@testable import MirrorFormat

final class MirrorFormatTests: XCTestCase {

    // MARK: tokensPerSec — format.ts 의 formatTokensPerSec 와 동일해야 한다

    func testTokensPerSecUnderOneIsZero() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(0), "0")
        XCTAssertEqual(MirrorFormat.tokensPerSec(0.9), "0")
    }

    func testTokensPerSecUnderThousandHasNoDecimals() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(1), "1")
        XCTAssertEqual(MirrorFormat.tokensPerSec(123.5), "124", "toFixed(0) 는 반올림한다")
        XCTAssertEqual(MirrorFormat.tokensPerSec(999.4), "999")
    }

    func testTokensPerSecThousandAndAboveUsesK() {
        XCTAssertEqual(MirrorFormat.tokensPerSec(1000), "1.0k")
        XCTAssertEqual(MirrorFormat.tokensPerSec(1234), "1.2k")
        XCTAssertEqual(MirrorFormat.tokensPerSec(15678), "15.7k")
    }

    // MARK: tokensTotal — formatTokensTotal 와 동일

    func testTokensTotalBoundaries() {
        XCTAssertEqual(MirrorFormat.tokensTotal(0), "0")
        XCTAssertEqual(MirrorFormat.tokensTotal(999), "999")
        XCTAssertEqual(MirrorFormat.tokensTotal(1000), "1.0k")
        XCTAssertEqual(MirrorFormat.tokensTotal(999_999), "1000.0k", "100만 미만은 k 로 유지된다")
        XCTAssertEqual(MirrorFormat.tokensTotal(1_000_000), "1.00M")
        XCTAssertEqual(MirrorFormat.tokensTotal(2_500_000), "2.50M")
    }

    // MARK: relativeTime — relativeTime 와 동일 (영문 그대로)

    func testRelativeTimeBuckets() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        func at(_ agoSecs: UInt64) -> String {
            MirrorFormat.relativeTime(1_000_000 - agoSecs, now: now)
        }
        XCTAssertEqual(at(0), "just now")
        XCTAssertEqual(at(4), "just now")
        XCTAssertEqual(at(5), "5s ago")
        XCTAssertEqual(at(59), "59s ago")
        XCTAssertEqual(at(60), "1m ago")
        XCTAssertEqual(at(3599), "59m ago")
        XCTAssertEqual(at(3600), "1h ago")
        XCTAssertEqual(at(7200), "2h ago")
    }

    func testRelativeTimeFutureDoesNotUnderflow() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(
            MirrorFormat.relativeTime(1_000_050, now: now),
            "just now",
            "미래 시각이 와도 UInt64 언더플로로 크래시하면 안 된다"
        )
    }

    // MARK: countdown — AgentCard.svelte 의 countdown 파생과 동일

    func testCountdownFormats() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 3661, now: now), "약 1시간 1분 1초 남음")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 61, now: now), "약 1분 1초 남음",
                       "1시간 미만이면 시간 부분을 생략한다")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000 + 1, now: now), "약 0분 1초 남음")
    }

    func testCountdownAtOrPastResetSaysReset() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 1_000_000, now: now), "리셋됨")
        XCTAssertEqual(MirrorFormat.countdown(resetAt: 999_000, now: now), "리셋됨")
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFormatTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'MirrorFormat' in scope` 또는 모듈 미해결

실제 출력을 보고서에 그대로 붙인다.

- [ ] **Step 4: 최소 구현을 쓴다**

`ios/Sources/MirrorFormat/MirrorFormat.swift`:
```swift
import Foundation

/// macOS 쪽 `src/lib/format.ts` 를 그대로 옮긴 것.
/// 두 화면이 나란히 놓였을 때 같은 숫자가 같은 모양으로 보여야 하므로
/// 반올림 방식과 경계값을 원본과 정확히 맞춘다.
public enum MirrorFormat {

    /// formatTokensPerSec: 1 미만은 "0", 1000 미만은 정수, 그 이상은 "N.Nk"
    public static func tokensPerSec(_ v: Float) -> String {
        if v < 1 { return "0" }
        if v < 1000 { return String(format: "%.0f", v) }
        return String(format: "%.1fk", v / 1000)
    }

    /// formatTokensTotal: 1000 미만 정수, 100만 미만 "N.Nk", 그 이상 "N.NNM"
    public static func tokensTotal(_ n: UInt32) -> String {
        if n < 1000 { return String(n) }
        if n < 1_000_000 { return String(format: "%.1fk", Double(n) / 1000) }
        return String(format: "%.2fM", Double(n) / 1_000_000)
    }

    /// relativeTime: 원본이 영문이므로 영문 그대로 둔다(두 화면 일치가 목적).
    public static func relativeTime(_ epochSecs: UInt64, now: Date) -> String {
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        // 미래 시각이 오면 UInt64 뺄셈이 언더플로하므로 0 으로 clamp 한다.
        let elapsed = nowSecs > epochSecs ? nowSecs - epochSecs : 0
        if elapsed < 5 { return "just now" }
        if elapsed < 60 { return "\(elapsed)s ago" }
        if elapsed < 3600 { return "\(elapsed / 60)m ago" }
        return "\(elapsed / 3600)h ago"
    }

    /// AgentCard.svelte 의 countdown 파생과 동일. 리셋 시각이 없으면 호출부가 nil 을 넘기지 않는다.
    public static func countdown(resetAt: UInt64, now: Date) -> String? {
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        if resetAt <= nowSecs { return "리셋됨" }
        let rem = resetAt - nowSecs
        let h = rem / 3600
        let m = (rem % 3600) / 60
        let s = rem % 60
        return h > 0 ? "약 \(h)시간 \(m)분 \(s)초 남음" : "약 \(m)분 \(s)초 남음"
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFormatTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 8 tests, with 0 failures` · `** TEST SUCCEEDED **`

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 숫자·시간 포맷 모듈 추가 (macOS format.ts 이식)"
```

---

## Task 2: 사용량 표시 로직 (`MirrorFormat/QuotaDisplay.swift`)

`QuotaBar.svelte`의 파생 로직 — 표시할 퍼센트와 그라디언트 색 선택 — 을 순수 함수로 뽑는다. 뷰가 아니라 여기서 결정해야 색 임계치를 테스트할 수 있다.

**Files:**
- Create: `ios/Sources/MirrorFormat/QuotaDisplay.swift`
- Create: `ios/Tests/MirrorFormatTests/QuotaDisplayTests.swift`

**Interfaces:**
- Consumes: 없음
- Produces: `struct QuotaGradient { public let startHex: UInt32; public let endHex: UInt32 }`, `enum QuotaDisplay` with `static func gradient(forPercent p: Float) -> QuotaGradient`, `static func displayPercent(autoPct: Float?, isReset: Bool) -> Float?`, `static func isReset5h(resetAt: UInt64?, now: Date) -> Bool`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`ios/Tests/MirrorFormatTests/QuotaDisplayTests.swift`:
```swift
import XCTest
@testable import MirrorFormat

final class QuotaDisplayTests: XCTestCase {

    // QuotaBar.svelte 의 color() 임계치와 정확히 일치해야 한다
    func testGradientThresholds() {
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 0).startHex, 0x30d158)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 0).endHex, 0x34c759)

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 69.9).endHex, 0x34c759, "70 미만은 녹색 계열")

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 70).startHex, 0x30d158, "70 이상은 녹→주황")
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 70).endHex, 0xff9f0a)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 89.9).endHex, 0xff9f0a)

        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 90).startHex, 0xff9f0a, "90 이상은 주황→빨강")
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 90).endHex, 0xff453a)
        XCTAssertEqual(QuotaDisplay.gradient(forPercent: 100).endHex, 0xff453a)
    }

    func testDisplayPercentClampsToHundred() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: 137.0, isReset: false), 100,
                       "원본이 Math.min(100, …) 으로 자른다")
    }

    func testDisplayPercentIsZeroRightAfterReset() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: 62.0, isReset: true), 0,
                       "리셋 직후에는 백엔드 갱신 전까지 0% 로 보여준다")
    }

    func testDisplayPercentNilBeforeSync() {
        XCTAssertNil(QuotaDisplay.displayPercent(autoPct: nil, isReset: false),
                     "동기화 전이면 바 대신 토큰 합계를 보여줘야 하므로 nil")
    }

    func testDisplayPercentResetWinsOverNil() {
        XCTAssertEqual(QuotaDisplay.displayPercent(autoPct: nil, isReset: true), 0,
                       "원본은 reset_5h 를 먼저 평가한다")
    }

    func testIsReset5h() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertFalse(QuotaDisplay.isReset5h(resetAt: nil, now: now), "리셋 시각을 모르면 리셋 아님")
        XCTAssertFalse(QuotaDisplay.isReset5h(resetAt: 1_000_001, now: now))
        XCTAssertTrue(QuotaDisplay.isReset5h(resetAt: 1_000_000, now: now), "남은 시간 0 이하면 리셋됨")
        XCTAssertTrue(QuotaDisplay.isReset5h(resetAt: 999_999, now: now))
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFormatTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'QuotaDisplay' in scope`

- [ ] **Step 3: 최소 구현을 쓴다**

`ios/Sources/MirrorFormat/QuotaDisplay.swift`:
```swift
import Foundation

/// 사용량 바 그라디언트 양끝 색. 뷰가 아니라 값이므로 여기서 결정하고 테스트한다.
public struct QuotaGradient: Equatable, Sendable {
    public let startHex: UInt32
    public let endHex: UInt32

    public init(startHex: UInt32, endHex: UInt32) {
        self.startHex = startHex
        self.endHex = endHex
    }
}

/// `QuotaBar.svelte` 의 파생 로직을 그대로 옮긴 것.
public enum QuotaDisplay {

    /// color(p) 와 동일한 임계치: 90 이상 주황→빨강, 70 이상 녹→주황, 그 외 녹→녹
    public static func gradient(forPercent p: Float) -> QuotaGradient {
        if p >= 90 { return QuotaGradient(startHex: 0xff9f0a, endHex: 0xff453a) }
        if p >= 70 { return QuotaGradient(startHex: 0x30d158, endHex: 0xff9f0a) }
        return QuotaGradient(startHex: 0x30d158, endHex: 0x34c759)
    }

    /// pct 파생: 리셋 직후면 0, 아니면 min(100, autoPct), 동기화 전이면 nil.
    /// 원본이 reset_5h 를 먼저 평가하므로 순서를 지킨다.
    public static func displayPercent(autoPct: Float?, isReset: Bool) -> Float? {
        if isReset { return 0 }
        guard let autoPct else { return nil }
        return min(100, autoPct)
    }

    /// isReset5h 파생: 리셋 시각을 알고 남은 시간이 0 이하일 때만 true.
    public static func isReset5h(resetAt: UInt64?, now: Date) -> Bool {
        guard let resetAt else { return false }
        let nowSecs = UInt64(max(0, now.timeIntervalSince1970))
        return resetAt <= nowSecs
    }
}
```

- [ ] **Step 4: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFormatTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 14 tests, with 0 failures` (Task 1 의 8개 + 6개)

- [ ] **Step 5: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 사용량 표시 로직과 그라디언트 임계치 추가"
```

---

## Task 3: 디자인 토큰과 점 뷰 (`DesignSystem` 1/2)

**Files:**
- Create: `ios/Sources/DesignSystem/Palette.swift`
- Create: `ios/Sources/DesignSystem/Typography.swift`
- Create: `ios/Sources/DesignSystem/DotView.swift`
- Create: `ios/Tests/DesignSystemTests/PaletteTests.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: `MirrorFormat.QuotaGradient`
- Produces: `enum Palette` (아래 정적 프로퍼티), `extension UIColor { convenience init(hex: UInt32) }`, `enum Typography`, `final class DotView: UIView` with `init(diameter: CGFloat)` and `var color: UIColor`

- [ ] **Step 1: Project.swift 에 타깃과 스킴을 추가한다**

`targets:` 배열에서 `unitTests("BLETransportTests", for: "BLETransport"),` **뒤에** 추가:
```swift
        framework("DesignSystem", deps: [.target(name: "MirrorFormat"), .external(name: "SnapKit")]),
        unitTests("DesignSystemTests", for: "DesignSystem"),
```

`schemes:` 배열에 추가:
```swift
        .scheme(
            name: "DesignSystemTests",
            buildAction: .buildAction(targets: [.target("DesignSystemTests")]),
            testAction: .targets([.testableTarget(target: .target("DesignSystemTests"))])
        ),
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`ios/Tests/DesignSystemTests/PaletteTests.swift`:
```swift
import UIKit
import XCTest
@testable import DesignSystem

final class PaletteTests: XCTestCase {

    private func rgb(_ c: UIColor) -> (Int, Int, Int) {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        c.getRed(&r, green: &g, blue: &b, alpha: &a)
        return (Int((r * 255).rounded()), Int((g * 255).rounded()), Int((b * 255).rounded()))
    }

    func testHexInitProducesExactChannels() {
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).0, 0x30)
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).1, 0xd1)
        XCTAssertEqual(rgb(UIColor(hex: 0x30d158)).2, 0x58)
        XCTAssertTrue(rgb(UIColor(hex: 0x000000)) == (0, 0, 0))
        XCTAssertTrue(rgb(UIColor(hex: 0xffffff)) == (255, 255, 255))
    }

    /// macOS Detail 창과 같은 색이어야 한다. 값이 바뀌면 두 화면이 달라진다.
    func testPaletteMatchesMacOS() {
        XCTAssertTrue(rgb(Palette.claudeDot) == rgb(UIColor(hex: 0x30d158)))
        XCTAssertTrue(rgb(Palette.codexDot) == rgb(UIColor(hex: 0xff9f0a)))
        XCTAssertTrue(rgb(Palette.cardBackground) == rgb(UIColor(hex: 0x2c2c2e)))
        XCTAssertTrue(rgb(Palette.barTrack) == rgb(UIColor(hex: 0x1c1c1e)))
        XCTAssertTrue(rgb(Palette.separator) == rgb(UIColor(hex: 0x3a3a3c)))
        XCTAssertTrue(rgb(Palette.subtle) == rgb(UIColor(hex: 0x8e8e93)))
        XCTAssertTrue(rgb(Palette.fainter) == rgb(UIColor(hex: 0x636366)))
        XCTAssertTrue(rgb(Palette.primaryText) == rgb(UIColor(hex: 0xf2f2f7)))
        XCTAssertTrue(rgb(Palette.percent) == rgb(UIColor(hex: 0x30d158)))
        XCTAssertTrue(rgb(Palette.countdown) == rgb(UIColor(hex: 0xff9f0a)))
        XCTAssertTrue(rgb(Palette.rate) == rgb(UIColor(hex: 0x0a84ff)))
        XCTAssertTrue(rgb(Palette.dormantDot) == rgb(UIColor(hex: 0x636366)))
        XCTAssertTrue(rgb(Palette.idleDot) == rgb(UIColor(hex: 0xff9f0a)))
    }

    func testDotViewIsCircularAfterLayout() {
        let dot = DotView(diameter: 8)
        dot.frame = CGRect(x: 0, y: 0, width: 8, height: 8)
        dot.layoutIfNeeded()
        XCTAssertEqual(dot.layer.cornerRadius, 4, accuracy: 0.01, "지름의 절반이어야 원이 된다")
    }

    func testDotViewColorIsApplied() {
        let dot = DotView(diameter: 6)
        dot.color = Palette.idleDot
        XCTAssertTrue(rgb(dot.backgroundColor ?? .clear) == rgb(Palette.idleDot))
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme DesignSystemTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'Palette' in scope`

- [ ] **Step 4: 최소 구현을 쓴다**

`ios/Sources/DesignSystem/Palette.swift`:
```swift
import UIKit

public extension UIColor {
    /// 0xRRGGBB 정수로 색을 만든다. macOS 쪽 CSS 값을 그대로 옮기기 위한 것.
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat((hex >> 16) & 0xff) / 255,
            green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255,
            alpha: 1
        )
    }
}

/// macOS Detail 창의 색을 그대로 옮긴 토큰. 두 화면을 나란히 놓았을 때
/// 같아 보이는 것이 목적이므로 값을 임의로 바꾸지 않는다.
public enum Palette {
    public static let claudeDot = UIColor(hex: 0x30d158)
    public static let codexDot = UIColor(hex: 0xff9f0a)
    public static let idleDot = UIColor(hex: 0xff9f0a)
    public static let dormantDot = UIColor(hex: 0x636366)

    public static let cardBackground = UIColor(hex: 0x2c2c2e)
    public static let barTrack = UIColor(hex: 0x1c1c1e)
    public static let separator = UIColor(hex: 0x3a3a3c)

    public static let primaryText = UIColor(hex: 0xf2f2f7)
    public static let subtle = UIColor(hex: 0x8e8e93)
    public static let fainter = UIColor(hex: 0x636366)

    public static let percent = UIColor(hex: 0x30d158)
    public static let countdown = UIColor(hex: 0xff9f0a)
    public static let rate = UIColor(hex: 0x0a84ff)
}
```

`ios/Sources/DesignSystem/Typography.swift`:
```swift
import UIKit

/// macOS 쪽 폰트 크기를 그대로 옮긴다. 숫자는 자릿수가 흔들리지 않도록
/// tabular figures 를 쓴다(원본의 font-variant-numeric: tabular-nums).
public enum Typography {
    public static let bigRate = monospacedDigit(ofSize: 22, weight: .bold)
    public static let percent = monospacedDigit(ofSize: 13, weight: .bold)
    public static let body = UIFont.systemFont(ofSize: 11)
    public static let rate = monospacedDigit(ofSize: 11, weight: .semibold)
    public static let countdown = monospacedDigit(ofSize: 11, weight: .semibold)
    /// AgentCard.svelte:90 `.unit` 과 SessionList.svelte:53 `.proj` 의 font-weight: 500
    public static let medium = UIFont.systemFont(ofSize: 11, weight: .medium)
    /// SessionList 한 줄의 `<strong>` — 행 font-size 11px 를 상속한 굵은 글씨
    public static let strong = UIFont.systemFont(ofSize: 11, weight: .bold)
    public static let label = UIFont.systemFont(ofSize: 10)
    public static let sectionLabel = UIFont.systemFont(ofSize: 9)
    /// AgentCard.svelte:88 의 `.name` 은 font-weight: 600 만 지정하고 크기는
    /// app.css:8 의 body 12px 를 상속한다. 13pt 가 아니다.
    public static let name = UIFont.systemFont(ofSize: 12, weight: .semibold)

    private static func monospacedDigit(ofSize size: CGFloat, weight: UIFont.Weight) -> UIFont {
        UIFont.monospacedDigitSystemFont(ofSize: size, weight: weight)
    }
}
```

`ios/Sources/DesignSystem/DotView.swift`:
```swift
import UIKit

/// 상태 표시용 원형 점. 지름이 고정이라 레이아웃 후 코너를 반지름으로 맞춘다.
public final class DotView: UIView {
    private let diameter: CGFloat

    public var color: UIColor = .clear {
        didSet { backgroundColor = color }
    }

    public init(diameter: CGFloat) {
        self.diameter = diameter
        super.init(frame: .zero)
        layer.cornerRadius = diameter / 2
        layer.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override var intrinsicContentSize: CGSize {
        CGSize(width: diameter, height: diameter)
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme DesignSystemTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 4 tests, with 0 failures`

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 디자인 토큰과 상태 점 뷰 추가"
```

---

## Task 4: 사용량 바 뷰 (`DesignSystem` 2/2)

`QuotaBar.svelte`를 옮긴다. 5h 바, 주간 바, 그리고 동기화 전 폴백 세 가지 상태를 가진다.

**Files:**
- Create: `ios/Sources/DesignSystem/QuotaBarView.swift`
- Create: `ios/Tests/DesignSystemTests/QuotaBarViewTests.swift`

**Interfaces:**
- Consumes: `Palette`, `Typography`, `MirrorFormat.QuotaDisplay`, `MirrorFormat.MirrorFormat`
- Produces: `final class QuotaBarView: UIView` with `func configure(tokens5h: UInt32, autoPct: Float?, weeklyPct: Float?, isReset5h: Bool)`, and for tests `var fivePercentText: String?`, `var weeklyPercentText: String?`, `var fallbackText: String?`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`ios/Tests/DesignSystemTests/QuotaBarViewTests.swift`:
```swift
import UIKit
import XCTest
@testable import DesignSystem

final class QuotaBarViewTests: XCTestCase {

    func testShowsBarsWhenSynced() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: 62.4, weeklyPct: 31.5, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "62%", "원본은 toFixed(0)")
        XCTAssertEqual(v.weeklyPercentText, "32%", "31.5 는 반올림되어 32")
        XCTAssertNil(v.fallbackText, "동기화됐으면 폴백 문구는 없다")
    }

    func testShowsFallbackBeforeSync() {
        let v = QuotaBarView()
        v.configure(tokens5h: 48210, autoPct: nil, weeklyPct: nil, isReset5h: false)
        XCTAssertNil(v.fivePercentText)
        XCTAssertEqual(v.fallbackText, "5h 토큰: 48.2k · 동기화 전")
    }

    func testWeeklyRowHiddenWhenWeeklyMissing() {
        let v = QuotaBarView()
        v.configure(tokens5h: 0, autoPct: 50, weeklyPct: nil, isReset5h: false)
        XCTAssertEqual(v.fivePercentText, "50%")
        XCTAssertNil(v.weeklyPercentText, "주간 값이 없으면 주간 줄 자체가 없다")
    }

    func testResetShowsZeroPercentNotFallback() {
        let v = QuotaBarView()
        v.configure(tokens5h: 100, autoPct: 62, weeklyPct: nil, isReset5h: true)
        XCTAssertEqual(v.fivePercentText, "0%")
        XCTAssertNil(v.fallbackText)
    }

    func testResetWithoutPriorSyncStillShowsZero() {
        let v = QuotaBarView()
        v.configure(tokens5h: 100, autoPct: nil, weeklyPct: nil, isReset5h: true)
        XCTAssertEqual(v.fivePercentText, "0%", "원본은 reset_5h 를 먼저 평가한다")
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme DesignSystemTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'QuotaBarView' in scope`

- [ ] **Step 3: 최소 구현을 쓴다**

`ios/Sources/DesignSystem/QuotaBarView.swift`:
```swift
import MirrorFormat
import SnapKit
import UIKit

/// `QuotaBar.svelte` 이식. 세 가지 표시 상태를 가진다.
/// 1) 동기화 후: 5h 바 (+ 주간 값이 있으면 주간 바)
/// 2) 리셋 직후: 5h 를 0% 로
/// 3) 동기화 전: 바 대신 "5h 토큰: N · 동기화 전"
public final class QuotaBarView: UIView {

    private let fiveRow = PercentRow(title: "5h")
    private let weeklyRow = PercentRow(title: "주간")
    private let fallbackLabel = UILabel()
    private let stack = UIStackView()

    /// 테스트에서 표시 결과를 확인하기 위한 읽기 전용 창구.
    public var fivePercentText: String? { fiveRow.isHidden ? nil : fiveRow.percentText }
    public var weeklyPercentText: String? { weeklyRow.isHidden ? nil : weeklyRow.percentText }
    public var fallbackText: String? { fallbackLabel.isHidden ? nil : fallbackLabel.text }

    public init() {
        super.init(frame: .zero)
        stack.axis = .vertical
        stack.spacing = 6
        addSubview(stack)
        stack.snp.makeConstraints { $0.edges.equalToSuperview() }

        fallbackLabel.font = Typography.label
        fallbackLabel.textColor = Palette.subtle

        [fiveRow, weeklyRow, fallbackLabel].forEach(stack.addArrangedSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(tokens5h: UInt32, autoPct: Float?, weeklyPct: Float?, isReset5h: Bool) {
        let pct = QuotaDisplay.displayPercent(autoPct: autoPct, isReset: isReset5h)

        if let pct {
            fiveRow.isHidden = false
            fallbackLabel.isHidden = true
            fiveRow.apply(percent: pct)

            if let weeklyPct {
                weeklyRow.isHidden = false
                weeklyRow.apply(percent: min(100, weeklyPct))
            } else {
                weeklyRow.isHidden = true
            }
        } else {
            fiveRow.isHidden = true
            weeklyRow.isHidden = true
            fallbackLabel.isHidden = false
            // 원본은 tokens_in + tokens_out 을 합쳐 보여주는데, 전송 DTO 의 t5 가 이미 그 합이다.
            fallbackLabel.text = "5h 토큰: \(MirrorFormat.tokensTotal(tokens5h)) · 동기화 전"
        }
    }
}

/// 라벨 + 퍼센트 + 진행 바 한 세트.
private final class PercentRow: UIView {
    private let titleLabel = UILabel()
    private let percentLabel = UILabel()
    private let track = UIView()
    private let fill = UIView()
    private let gradient = CAGradientLayer()
    private var ratio: CGFloat = 0

    var percentText: String? { percentLabel.text }

    init(title: String) {
        super.init(frame: .zero)
        titleLabel.text = title
        titleLabel.font = Typography.label
        titleLabel.textColor = Palette.subtle

        percentLabel.font = Typography.percent
        percentLabel.textColor = Palette.percent
        percentLabel.textAlignment = .right

        track.backgroundColor = Palette.barTrack
        track.layer.cornerRadius = 3
        track.layer.masksToBounds = true

        gradient.startPoint = CGPoint(x: 0, y: 0.5)
        gradient.endPoint = CGPoint(x: 1, y: 0.5)
        fill.layer.addSublayer(gradient)
        fill.layer.cornerRadius = 3
        fill.layer.masksToBounds = true

        [titleLabel, percentLabel, track].forEach(addSubview)
        track.addSubview(fill)

        titleLabel.snp.makeConstraints { make in
            make.leading.top.equalToSuperview()
        }
        percentLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview()
            make.firstBaseline.equalTo(titleLabel.snp.firstBaseline)
        }
        track.snp.makeConstraints { make in
            make.top.equalTo(percentLabel.snp.bottom).offset(3)
            make.leading.trailing.bottom.equalToSuperview()
            make.height.equalTo(6)
        }
        fill.snp.makeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(0).priority(.high)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func apply(percent: Float) {
        percentLabel.text = String(format: "%.0f%%", percent)
        ratio = CGFloat(max(0, min(100, percent)) / 100)
        let g = QuotaDisplay.gradient(forPercent: percent)
        gradient.colors = [UIColor(hex: g.startHex).cgColor, UIColor(hex: g.endHex).cgColor]
        fill.snp.remakeConstraints { make in
            make.leading.top.bottom.equalToSuperview()
            make.width.equalToSuperview().multipliedBy(max(ratio, 0.0001)).priority(.high)
        }
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        gradient.frame = fill.bounds
    }
}
```

- [ ] **Step 4: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme DesignSystemTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 9 tests, with 0 failures` (Task 3 의 4개 + 5개)

- [ ] **Step 5: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 5h·주간 사용량 바 뷰 추가"
```

---

## Task 5: 에이전트 카드와 세션 행 (`MirrorFeature` 1/2)

**Files:**
- Create: `ios/Sources/MirrorFeature/AgentCardView.swift`
- Create: `ios/Sources/MirrorFeature/SessionRowView.swift`
- Create: `ios/Tests/MirrorFeatureTests/AgentCardViewTests.swift`
- Create: `ios/Tests/MirrorFeatureTests/SessionRowViewTests.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: `Wire.MirrorAgent`, `Wire.MirrorProject`, `Wire.AgentKindCode`, `Wire.ActivityStatusCode`, `DesignSystem` 전부, `MirrorFormat` 전부
- Produces: `final class AgentCardView: UIView` with `func configure(agent: MirrorAgent, now: Date)` and test 창구 `var nameText: String?`, `var modelText: String?`, `var rateText: String?`, `var projectText: String?`, `var countdownText: String?`, `var dotColor: UIColor?`; `final class SessionRowView: UIView` with `func configure(project: MirrorProject, kind: AgentKindCode, now: Date)` and `var leftText: String?`, `var rightText: String?`, `var relativeText: String?`, `var dotColor: UIColor?`

- [ ] **Step 1: Project.swift 에 타깃과 스킴을 추가한다**

`targets:` 배열에서 `unitTests("DesignSystemTests", for: "DesignSystem"),` **뒤에** 추가:
```swift
        framework("MirrorFeature", deps: [
            .target(name: "BLETransport"),
            .target(name: "DesignSystem"),
            .target(name: "MirrorFormat"),
            .external(name: "SnapKit"),
        ]),
        unitTests("MirrorFeatureTests", for: "MirrorFeature"),
```

`schemes:` 배열에 추가:
```swift
        .scheme(
            name: "MirrorFeatureTests",
            buildAction: .buildAction(targets: [.target("MirrorFeatureTests")]),
            testAction: .targets([.testableTarget(target: .target("MirrorFeatureTests"))])
        ),
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`ios/Tests/MirrorFeatureTests/AgentCardViewTests.swift`:
```swift
import UIKit
import Wire
import XCTest
@testable import DesignSystem
@testable import MirrorFeature

/// 테스트용 스냅샷 조각을 JSON 으로 만든다.
/// Wire 의 DTO 는 Decodable 전용이라 이 경로로만 인스턴스를 얻을 수 있다.
enum Fixture {
    static func agent(
        k: UInt8 = 0,
        r: Float = 123.5,
        t5: UInt32 = 3000,
        p5: Float? = 62,
        r5: UInt64? = nil,
        pw: Float? = nil,
        rw: UInt64? = nil,
        projects: [(id: UInt32, n: String, m: String, r: Float, t: UInt64, s: UInt8)] = []
    ) -> MirrorAgent {
        func opt<T: CustomStringConvertible>(_ key: String, _ v: T?) -> String {
            v.map { ",\"\(key)\":\($0)" } ?? ""
        }
        let pj = projects.map {
            "{\"id\":\($0.id),\"n\":\"\($0.n)\",\"m\":\"\($0.m)\",\"r\":\($0.r),\"t\":\($0.t),\"s\":\($0.s)}"
        }.joined(separator: ",")
        let json = """
        {"v":1,"t":0,"a":[{"k":\(k),"r":\(r),"t5":\(t5)\
        \(opt("p5", p5))\(opt("r5", r5))\(opt("pw", pw))\(opt("rw", rw))\
        ,"pj":[\(pj)]}]}
        """
        // swiftlint:disable:next force_try
        return try! JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8)).a[0]
    }
}

final class AgentCardViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    func testClaudeHeaderAndRate() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(k: 0, r: 1234), now: now)
        XCTAssertEqual(v.nameText, "Claude Code")
        XCTAssertEqual(v.rateText, "1.2k")
        XCTAssertEqual(v.dotColor, Palette.claudeDot)
    }

    func testCodexHeader() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(k: 1), now: now)
        XCTAssertEqual(v.nameText, "Codex")
        XCTAssertEqual(v.dotColor, Palette.codexDot)
    }

    func testEmDashAndPlaceholderWhenNoProjects() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: []), now: now)
        XCTAssertEqual(v.modelText, "—", "원본은 모델이 없으면 em dash")
        XCTAssertEqual(v.projectText, "no active session")
    }

    func testPrimaryProjectPrefersActive() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: [
            (1, "idle-one", "model-idle", 0, 999_000, 1),
            (2, "active-one", "model-active", 50, 999_990, 0),
        ]), now: now)
        XCTAssertEqual(v.projectText, "active-one", "active 가 있으면 그것이 대표")
        XCTAssertEqual(v.modelText, "model-active")
    }

    func testFallsBackToFirstProjectWhenNoneActive() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(projects: [
            (1, "first-idle", "model-a", 0, 999_000, 1),
            (2, "second-dormant", "model-b", 0, 998_000, 2),
        ]), now: now)
        XCTAssertEqual(v.projectText, "first-idle", "active 가 없으면 첫 번째")
    }

    func testCountdownHiddenWithoutResetTime() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(r5: nil), now: now)
        XCTAssertNil(v.countdownText)
    }

    func testCountdownShownWithResetTime() {
        let v = AgentCardView()
        v.configure(agent: Fixture.agent(r5: 1_000_000 + 3661), now: now)
        XCTAssertEqual(v.countdownText, "약 1시간 1분 1초 남음")
    }
}
```

`ios/Tests/MirrorFeatureTests/SessionRowViewTests.swift`:
```swift
import UIKit
import Wire
import XCTest
@testable import DesignSystem
@testable import MirrorFeature

final class SessionRowViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func project(n: String, m: String, r: Float, t: UInt64, s: UInt8) -> MirrorProject {
        Fixture.agent(projects: [(1, n, m, r, t, s)]).pj[0]
    }

    func testActiveRowShowsRate() {
        let v = SessionRowView()
        v.configure(project: project(n: "foo", m: "claude-opus-5", r: 98.25, t: 999_990, s: 0),
                    kind: .claude, now: now)
        XCTAssertEqual(v.leftText, "Claude · foo claude-opus-5")
        XCTAssertEqual(v.rightText, "98 tok/s", "active 면 속도를 보여준다")
        XCTAssertEqual(v.relativeText, "10s ago")
        XCTAssertEqual(v.dotColor, Palette.claudeDot)
    }

    func testIdleRowShowsStatusWordAndAmberDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "bar", m: "m", r: 50, t: 999_900, s: 1),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightText, "idle", "active 가 아니면 속도 대신 상태 단어")
        XCTAssertEqual(v.dotColor, Palette.idleDot)
    }

    func testDormantRowUsesGreyDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "baz", m: "m", r: 0, t: 900_000, s: 2),
                    kind: .codex, now: now)
        XCTAssertEqual(v.rightText, "dormant")
        XCTAssertEqual(v.dotColor, Palette.dormantDot, "dormant 는 에이전트 색이 아니라 회색")
    }

    func testCodexActiveUsesCodexColor() {
        let v = SessionRowView()
        v.configure(project: project(n: "qux", m: "m", r: 10, t: 999_999, s: 0),
                    kind: .codex, now: now)
        XCTAssertEqual(v.leftText, "Codex · qux m")
        XCTAssertEqual(v.dotColor, Palette.codexDot)
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'AgentCardView' in scope`

- [ ] **Step 4: 최소 구현을 쓴다**

`ios/Sources/MirrorFeature/AgentCardView.swift`:
```swift
import DesignSystem
import MirrorFormat
import SnapKit
import UIKit
import Wire

/// `AgentCard.svelte` 이식. 위에서부터 헤더(점·이름·모델), tok/s, 대표 프로젝트·카운트다운,
/// 사용량 바 순서로 쌓는다. macOS 의 🔄 동기화 버튼은 읽기 전용 미러이므로 옮기지 않는다.
public final class AgentCardView: UIView {

    private let dot = DotView(diameter: 8)
    private let nameLabel = UILabel()
    private let modelLabel = UILabel()
    private let rateLabel = UILabel()
    private let unitLabel = UILabel()
    private let projectLabel = UILabel()
    private let countdownLabel = UILabel()
    private let quotaBar = QuotaBarView()

    public var nameText: String? { nameLabel.text }
    public var modelText: String? { modelLabel.text }
    public var rateText: String? { rateLabel.text }
    public var projectText: String? { projectLabel.text }
    public var countdownText: String? { countdownLabel.isHidden ? nil : countdownLabel.text }
    public var dotColor: UIColor? { dot.color }

    public init() {
        super.init(frame: .zero)
        backgroundColor = Palette.cardBackground
        layer.cornerRadius = 10

        nameLabel.font = Typography.name
        nameLabel.textColor = Palette.primaryText
        modelLabel.font = Typography.body
        modelLabel.textColor = Palette.subtle
        rateLabel.font = Typography.bigRate
        rateLabel.textColor = Palette.primaryText
        unitLabel.font = Typography.medium
        unitLabel.textColor = Palette.subtle
        unitLabel.text = "tok/s"
        projectLabel.font = Typography.body
        projectLabel.textColor = Palette.subtle
        countdownLabel.font = Typography.countdown
        countdownLabel.textColor = Palette.countdown
        countdownLabel.textAlignment = .right

        [dot, nameLabel, modelLabel, rateLabel, unitLabel,
         projectLabel, countdownLabel, quotaBar].forEach(addSubview)

        dot.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.centerY.equalTo(nameLabel)
            make.width.height.equalTo(8)
        }
        nameLabel.snp.makeConstraints { make in
            make.leading.equalTo(dot.snp.trailing).offset(6)
            make.top.equalToSuperview().offset(10)
        }
        modelLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview().offset(-12)
            make.centerY.equalTo(nameLabel)
            make.leading.greaterThanOrEqualTo(nameLabel.snp.trailing).offset(8)
        }
        rateLabel.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.top.equalTo(nameLabel.snp.bottom).offset(4)
        }
        unitLabel.snp.makeConstraints { make in
            make.leading.equalTo(rateLabel.snp.trailing).offset(4)
            make.firstBaseline.equalTo(rateLabel.snp.firstBaseline)
        }
        projectLabel.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.top.equalTo(rateLabel.snp.bottom).offset(2)
        }
        countdownLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview().offset(-12)
            make.centerY.equalTo(projectLabel)
            make.leading.greaterThanOrEqualTo(projectLabel.snp.trailing).offset(8)
        }
        quotaBar.snp.makeConstraints { make in
            make.leading.equalToSuperview().offset(12)
            make.trailing.equalToSuperview().offset(-12)
            make.top.equalTo(projectLabel.snp.bottom).offset(6)
            make.bottom.equalToSuperview().offset(-10)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(agent: MirrorAgent, now: Date) {
        switch agent.kind {
        case .claude:
            nameLabel.text = "Claude Code"
            dot.color = Palette.claudeDot
        case .codex:
            nameLabel.text = "Codex"
            dot.color = Palette.codexDot
        case .unknown:
            nameLabel.text = "알 수 없음"
            dot.color = Palette.dormantDot
        }

        // 원본과 동일: active 를 우선하고, 없으면 첫 번째를 대표로 삼는다.
        let primary = agent.projects.first(where: { $0.status == .active }) ?? agent.projects.first
        modelLabel.text = primary?.model ?? "—"
        projectLabel.text = primary?.name ?? "no active session"
        rateLabel.text = MirrorFormat.tokensPerSec(agent.ratePerSec)

        if let resetAt = agent.r5 {
            countdownLabel.isHidden = false
            countdownLabel.text = MirrorFormat.countdown(resetAt: resetAt, now: now)
        } else {
            countdownLabel.isHidden = true
            countdownLabel.text = nil
        }

        quotaBar.configure(
            tokens5h: agent.tokens5h,
            autoPct: agent.usedPct5h,
            weeklyPct: agent.usedPctWeekly,
            isReset5h: QuotaDisplay.isReset5h(resetAt: agent.r5, now: now)
        )
    }
}
```

`ios/Sources/MirrorFeature/SessionRowView.swift`:
```swift
import DesignSystem
import MirrorFormat
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 의 한 줄. 왼쪽은 점·에이전트·프로젝트·모델,
/// 오른쪽은 속도(또는 상태 단어)와 상대 시각.
public final class SessionRowView: UIView {

    private let dot = DotView(diameter: 6)
    private let leftLabel = UILabel()
    private let rightLabel = UILabel()
    private let relativeLabel = UILabel()

    /// attributedText 로 구간을 나눠 그리므로 평문은 여기서 꺼낸다.
    public var leftText: String? { leftLabel.attributedText?.string ?? leftLabel.text }
    public var rightText: String? { rightLabel.text }
    public var relativeText: String? { relativeLabel.text }
    public var dotColor: UIColor? { dot.color }

    public init() {
        super.init(frame: .zero)
        // 아래 configure 에서 attributedText 로 구간별 스타일을 지정한다.
        leftLabel.font = Typography.body
        leftLabel.textColor = Palette.primaryText
        rightLabel.font = Typography.rate
        relativeLabel.font = Typography.body
        relativeLabel.textColor = Palette.subtle

        [dot, leftLabel, rightLabel, relativeLabel].forEach(addSubview)

        dot.snp.makeConstraints { make in
            make.leading.equalToSuperview()
            make.centerY.equalToSuperview()
            make.width.height.equalTo(6)
        }
        leftLabel.snp.makeConstraints { make in
            make.leading.equalTo(dot.snp.trailing).offset(6)
            make.top.bottom.equalToSuperview().inset(6)
        }
        relativeLabel.snp.makeConstraints { make in
            make.trailing.equalToSuperview()
            make.centerY.equalToSuperview()
        }
        rightLabel.snp.makeConstraints { make in
            make.trailing.equalTo(relativeLabel.snp.leading).offset(-12)
            make.centerY.equalToSuperview()
            make.leading.greaterThanOrEqualTo(leftLabel.snp.trailing).offset(8)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(project: MirrorProject, kind: AgentKindCode, now: Date) {
        let agentName: String
        let agentColor: UIColor
        switch kind {
        case .claude: agentName = "Claude"; agentColor = Palette.claudeDot
        case .codex: agentName = "Codex"; agentColor = Palette.codexDot
        case .unknown: agentName = "?"; agentColor = Palette.dormantDot
        }

        // 원본 dotColor(): dormant 는 회색, idle 은 주황, active 는 에이전트 색.
        // .unknown 은 프로토콜이 앞서 나간 경우이므로 dormant 와 같이 조용히 취급한다.
        switch project.status {
        case .dormant, .unknown: dot.color = Palette.dormantDot
        case .idle: dot.color = Palette.idleDot
        case .active: dot.color = agentColor
        }

        // 원본은 한 줄 안에 세 가지 스타일이 공존한다(SessionList.svelte).
        //   <strong>Claude</strong>          → 굵게
        //   <span class="proj">· 이름</span>  → weight 500
        //   <span class="model subtle">모델</span> → 흐린 색
        // 단일 라벨로 뭉개면 전부 같게 보이므로 attributed string 으로 구간을 나눈다.
        let line = NSMutableAttributedString(
            string: agentName,
            attributes: [.font: Typography.strong, .foregroundColor: Palette.primaryText]
        )
        line.append(NSAttributedString(
            string: " · \(project.name)",
            attributes: [.font: Typography.medium, .foregroundColor: Palette.primaryText]
        ))
        line.append(NSAttributedString(
            string: "  \(project.model)",
            attributes: [.font: Typography.body, .foregroundColor: Palette.subtle]
        ))
        leftLabel.attributedText = line

        switch project.status {
        case .active:
            rightLabel.text = "\(MirrorFormat.tokensPerSec(project.ratePerSec)) tok/s"
            rightLabel.textColor = Palette.rate
        case .idle:
            rightLabel.text = "idle"
            rightLabel.textColor = Palette.subtle
        case .dormant, .unknown:
            rightLabel.text = "dormant"
            rightLabel.textColor = Palette.subtle
        }

        relativeLabel.text = MirrorFormat.relativeTime(project.t, now: now)
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 11 tests, with 0 failures`

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 에이전트 카드와 세션 행 뷰 추가"
```

---

## Task 6: 세션 목록과 화면 조립 (`MirrorFeature` 2/2)

**Files:**
- Create: `ios/Sources/MirrorFeature/SessionListView.swift`
- Create: `ios/Sources/MirrorFeature/MirrorViewController.swift`
- Create: `ios/Tests/MirrorFeatureTests/SessionListViewTests.swift`

**Interfaces:**
- Consumes: `SessionRowView`, `AgentCardView`, `BLETransport.BLEClient`, `BLETransport.ConnectionState`, `Wire.MirrorSnapshot`
- Produces: `final class SessionListView: UIView` with `func configure(snapshot: MirrorSnapshot, now: Date)` and `var rowCount: Int`, `var isEmptyMessageVisible: Bool`, `func rowText(at index: Int) -> String?`; `@MainActor final class MirrorViewController: UIViewController` with `init(client: BLEClient)`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`ios/Tests/MirrorFeatureTests/SessionListViewTests.swift`:
```swift
import UIKit
import Wire
import XCTest
@testable import MirrorFeature

final class SessionListViewTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func snapshot(_ json: String) -> MirrorSnapshot {
        // swiftlint:disable:next force_try
        try! JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
    }

    func testEmptyMessageWhenNoSessions() {
        let v = SessionListView()
        v.configure(snapshot: snapshot(#"{"v":1,"t":0,"a":[]}"#), now: now)
        XCTAssertEqual(v.rowCount, 0)
        XCTAssertTrue(v.isEmptyMessageVisible)
    }

    func testSortsByMostRecentActivityAcrossAgents() {
        let json = #"""
        {"v":1,"t":0,"a":[
          {"k":0,"r":0,"t5":0,"pj":[
            {"id":1,"n":"older-claude","m":"m","r":0,"t":999000,"s":1}]},
          {"k":1,"r":0,"t5":0,"pj":[
            {"id":2,"n":"newest-codex","m":"m","r":0,"t":999990,"s":1},
            {"id":3,"n":"middle-codex","m":"m","r":0,"t":999500,"s":1}]}
        ]}
        """#
        let v = SessionListView()
        v.configure(snapshot: snapshot(json), now: now)

        XCTAssertEqual(v.rowCount, 3)
        XCTAssertFalse(v.isEmptyMessageVisible)
        XCTAssertTrue(v.rowText(at: 0)?.contains("newest-codex") == true, "가장 최근 활동이 맨 위")
        XCTAssertTrue(v.rowText(at: 1)?.contains("middle-codex") == true)
        XCTAssertTrue(v.rowText(at: 2)?.contains("older-claude") == true)
    }

    func testRowsAreReusedNotAccumulatedAcrossConfigures() {
        let json = #"{"v":1,"t":0,"a":[{"k":0,"r":0,"t5":0,"pj":[{"id":1,"n":"a","m":"m","r":0,"t":1,"s":0}]}]}"#
        let v = SessionListView()
        v.configure(snapshot: snapshot(json), now: now)
        v.configure(snapshot: snapshot(json), now: now)
        v.configure(snapshot: snapshot(json), now: now)
        XCTAssertEqual(v.rowCount, 1, "1Hz 로 계속 들어오므로 행이 쌓이면 안 된다")
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'SessionListView' in scope`

- [ ] **Step 3: 최소 구현을 쓴다**

`ios/Sources/MirrorFeature/SessionListView.swift`:
```swift
import DesignSystem
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 이식. 모든 에이전트의 프로젝트를 한 줄씩 펼쳐
/// 최근 활동순으로 정렬한다.
public final class SessionListView: UIView {

    private let titleLabel = UILabel()
    private let emptyLabel = UILabel()
    private let stack = UIStackView()
    private var rows: [SessionRowView] = []

    public var rowCount: Int { rows.filter { !$0.isHidden }.count }
    public var isEmptyMessageVisible: Bool { !emptyLabel.isHidden }
    public func rowText(at index: Int) -> String? {
        let visible = rows.filter { !$0.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].leftText
    }

    public init() {
        super.init(frame: .zero)
        backgroundColor = Palette.cardBackground
        layer.cornerRadius = 8

        titleLabel.text = "ACTIVE SESSIONS · SORTED BY RECENT ACTIVITY"
        titleLabel.font = Typography.sectionLabel
        titleLabel.textColor = Palette.subtle

        emptyLabel.text = "No sessions yet."
        emptyLabel.font = Typography.body
        emptyLabel.textColor = Palette.subtle

        stack.axis = .vertical
        stack.spacing = 0

        [titleLabel, emptyLabel, stack].forEach(addSubview)

        titleLabel.snp.makeConstraints { make in
            make.top.equalToSuperview().offset(10)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        emptyLabel.snp.makeConstraints { make in
            make.top.equalTo(titleLabel.snp.bottom).offset(6)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        stack.snp.makeConstraints { make in
            make.top.equalTo(emptyLabel.snp.bottom)
            make.leading.trailing.equalToSuperview().inset(12)
            make.bottom.equalToSuperview().offset(-10)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(snapshot: MirrorSnapshot, now: Date) {
        // 에이전트별 프로젝트를 한 줄로 펼치고 최근 활동순 정렬 — 원본과 동일.
        let entries = snapshot.agents
            .flatMap { agent in agent.projects.map { (project: $0, kind: agent.kind) } }
            .sorted { $0.project.t > $1.project.t }

        emptyLabel.isHidden = !entries.isEmpty

        // 스냅샷이 1Hz 로 들어오므로 행을 재사용한다. 매번 새로 만들면 뷰가 쌓인다.
        while rows.count < entries.count {
            let row = SessionRowView()
            rows.append(row)
            stack.addArrangedSubview(row)
        }

        for (i, row) in rows.enumerated() {
            if i < entries.count {
                row.isHidden = false
                row.configure(project: entries[i].project, kind: entries[i].kind, now: now)
            } else {
                row.isHidden = true
            }
        }
    }
}
```

`ios/Sources/MirrorFeature/MirrorViewController.swift`:
```swift
import BLETransport
import Combine
import DesignSystem
import SnapKit
import UIKit
import Wire

/// Detail 창을 미러링하는 화면. 연결 상태를 항상 상단에 노출해
/// 화면이 비어 있을 때 원인이 미궁이 되지 않게 한다(스펙 7.3).
@MainActor
public final class MirrorViewController: UIViewController {

    private let client: BLEClient
    private var cancellables = Set<AnyCancellable>()
    private var tick: Timer?
    private var latest: MirrorSnapshot?

    private let statusLabel = UILabel()
    private let scrollView = UIScrollView()
    private let contentStack = UIStackView()
    private let claudeCard = AgentCardView()
    private let codexCard = AgentCardView()
    private let sessionList = SessionListView()

    public init(client: BLEClient) {
        self.client = client
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override func viewDidLoad() {
        super.viewDidLoad()
        title = "AI Agent Monitor"
        view.backgroundColor = .black

        statusLabel.font = Typography.body
        statusLabel.textColor = Palette.subtle
        statusLabel.text = ConnectionState.idle.label

        contentStack.axis = .vertical
        contentStack.spacing = 8

        view.addSubview(statusLabel)
        view.addSubview(scrollView)
        scrollView.addSubview(contentStack)
        [claudeCard, codexCard, sessionList].forEach(contentStack.addArrangedSubview)

        statusLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(12)
            make.leading.trailing.equalToSuperview().inset(16)
        }
        scrollView.snp.makeConstraints { make in
            make.top.equalTo(statusLabel.snp.bottom).offset(12)
            make.leading.trailing.equalToSuperview()
            make.bottom.equalTo(view.safeAreaLayoutGuide)
        }
        contentStack.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 0, left: 16, bottom: 16, right: 16))
            make.width.equalTo(scrollView).offset(-32)
        }

        // 스냅샷이 오기 전에는 카드를 감춰 빈 껍데기를 보여주지 않는다.
        claudeCard.isHidden = true
        codexCard.isHidden = true

        client.state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.statusLabel.text = $0.label }
            .store(in: &cancellables)

        client.snapshots
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snap in
                self?.latest = snap
                self?.render()
            }
            .store(in: &cancellables)

        // 카운트다운과 상대 시각은 클라이언트가 계산하므로 추가 전송 없이 1초마다 다시 그린다.
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.render() }
        }

        client.start()
    }

    deinit {
        tick?.invalidate()
    }

    private func render() {
        guard let snap = latest else { return }
        let now = Date()

        if let claude = snap.agents.first(where: { $0.kind == .claude }) {
            claudeCard.isHidden = false
            claudeCard.configure(agent: claude, now: now)
        } else {
            claudeCard.isHidden = true
        }

        if let codex = snap.agents.first(where: { $0.kind == .codex }) {
            codexCard.isHidden = false
            codexCard.configure(agent: codex, now: now)
        } else {
            codexCard.isHidden = true
        }

        sessionList.configure(snapshot: snap, now: now)
    }
}
```

- [ ] **Step 4: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 14 tests, with 0 failures`

- [ ] **Step 5: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 세션 목록과 미러 화면 조립"
```

---

## Task 7: 앱을 미러 화면으로 교체 · 실기기 확인

**Files:**
- Modify: `ios/Sources/App/SceneDelegate.swift`
- Delete: `ios/Sources/App/RawDumpViewController.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: `MirrorFeature.MirrorViewController`, `BLETransport.BLEClient`
- Produces: 실행 가능한 앱

- [ ] **Step 1: App 타깃 의존성에 MirrorFeature 를 추가한다**

`ios/Project.swift` 의 `App` 타깃 `dependencies:` 를 다음으로 바꾼다:
```swift
            dependencies: [
                .target(name: "BLETransport"),
                .target(name: "MirrorFeature"),
                .external(name: "SnapKit"),
            ]
```

- [ ] **Step 2: SceneDelegate 의 루트를 교체한다**

`ios/Sources/App/SceneDelegate.swift` 의 `rootViewController` 줄을 바꾼다.

import 에 추가:
```swift
import BLETransport
import MirrorFeature
```

루트 생성부:
```swift
        let root = MirrorViewController(client: BLEClient())
        w.rootViewController = UINavigationController(rootViewController: root)
```

- [ ] **Step 3: 1단계 진단 화면을 지운다**

```bash
git rm ios/Sources/App/RawDumpViewController.swift
```

원본 JSON 덤프는 전송 계층 검증용이었고 그 역할이 끝났다. 남겨두면 쓰이지 않는 코드가 된다.

- [ ] **Step 4: 빌드와 전체 테스트를 확인한다**

Run:
```bash
cd ios && tuist generate --no-open
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "BUILD SUCCEEDED|BUILD FAILED|error:"
```
Expected: `** BUILD SUCCEEDED **`

Run 각 테스트 스킴 (전부 통과해야 한다):
```bash
cd ios && for S in WireTests BLETransportTests MirrorFormatTests DesignSystemTests MirrorFeatureTests; do
  printf "%s: " "$S"
  xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme $S \
    -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 \
    | grep -E "Executed [0-9]+ tests?, with" | tail -1
done
```
Expected: Wire 3, BLETransport 12, MirrorFormat 14, DesignSystem 9, MirrorFeature 14 — 실패 0

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: **76 passed** — Rust 는 이 계획에서 건드리지 않는다

- [ ] **Step 5: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 앱 루트를 미러 화면으로 교체하고 원본 덤프 화면 제거"
```

- [ ] **Step 6: 실기기에서 확인한다 (사람이 수행)**

`docs/ble-protocol/DEVICE-TEST.md` 의 절차대로 Mac 공유를 켜고 iPhone 앱을 실행한 뒤:

- [ ] 상단에 연결 상태가 보이고 `연결됨` 으로 바뀐다
- [ ] Claude Code / Codex 카드가 각각 나타난다 (해당 에이전트가 활동 중일 때)
- [ ] tok/s 숫자가 macOS Detail 창과 **같은 값·같은 표기**다
- [ ] 5h 사용률 퍼센트와 바 색이 macOS 와 일치한다 (70%/90% 경계에서 색이 바뀐다)
- [ ] 주간 바는 주간 값이 있을 때만 나타난다
- [ ] 리셋 카운트다운이 1초마다 줄어들고 문구가 `약 X시간 Y분 Z초 남음` 이다
- [ ] 세션 목록이 최근 활동순으로 정렬되고 상대 시각(`10s ago`)이 갱신된다
- [ ] idle/dormant 행의 점 색과 상태 단어가 macOS 와 같다
- [ ] **두 화면을 나란히 놓고 비교했을 때 시각적으로 일치한다** ← 2단계 완료 판정

---

## Self-Review

**스펙 커버리지 (§7 중 2단계 범위)**

| 스펙 항목 | 태스크 |
|---|---|
| §7.1 `MirrorFormat`(신규 분리) · `DesignSystem` · `MirrorFeature` 모듈 | Task 1·2 / 3·4 / 5·6 |
| §7.1 `Wire` 의존성 0 유지 | 변경 없음 — `MirrorFormat` 도 의존성 0 |
| §7.2 Combine 으로 상태 전파 | Task 6 (`MirrorViewController`) |
| §7.3 연결 상태를 UI 에 항상 노출 | Task 6 (`statusLabel`) |
| §7.4 `AgentCard` → `AgentCardView` | Task 5 |
| §7.4 `QuotaBar` → `QuotaBarView` | Task 4 |
| §7.4 `SessionList` → `SessionListVC` | Task 6 (`SessionListView` — VC 가 아니라 뷰로 구현) |
| §7.4 카운트다운을 클라이언트가 계산 | Task 1 (`countdown`) + Task 6 (1초 타이머) |
| §9 2단계 완료 판정 "나란히 놓고 시각적으로 일치" | Task 7 Step 6 |

**의도한 스펙 이탈 3건**
1. **`MirrorCore` 모듈을 만들지 않는다.** 스펙 7.1 은 `MirrorStore` 를 담는 `MirrorCore` 를 그렸으나, 1단계에서 `BLEClient` 가 이미 Combine 퍼블리셔로 상태를 노출한다. 그 위에 저장소를 한 겹 더 두면 값을 그대로 통과시키는 층이 하나 늘 뿐이다(YAGNI). 대신 순수 포맷 로직을 `MirrorFormat` 으로 분리해 테스트 가능성을 얻었다.
2. **`SessionListVC` 가 아니라 `SessionListView`.** 세션 목록은 독자적 화면 생명주기가 없어 뷰 컨트롤러일 이유가 없다. 스크롤은 부모가 담당한다.
3. **`TriggerList` 는 범위 밖.** 트리거 특성 전송이 Rust 쪽에 아직 없어 3단계로 미룬다. 사용자가 이 범위를 명시적으로 선택했다.

**`AgentCard.svelte` 의 🔄 동기화 버튼은 옮기지 않는다** — 1단계에서 확정한 읽기 전용 미러 범위 밖이다.

**타입 일관성 확인**
- `MirrorFormat.{tokensPerSec, tokensTotal, relativeTime, countdown}` — Task 1 정의, Task 4·5 사용 일치
- `QuotaDisplay.{gradient, displayPercent, isReset5h}` · `QuotaGradient{startHex,endHex}` — Task 2 정의, Task 4·5 사용 일치
- `Palette.*` · `Typography.*` · `DotView(diameter:)`/`.color` — Task 3 정의, Task 4·5·6 사용 일치
- `QuotaBarView.configure(tokens5h:autoPct:weeklyPct:isReset5h:)` — Task 4 정의, Task 5 호출 일치
- `AgentCardView.configure(agent:now:)` · `SessionRowView.configure(project:kind:now:)` — Task 5 정의, Task 6 호출 일치
- `SessionListView.configure(snapshot:now:)` — Task 6 정의, 같은 태스크 내 사용
- `MirrorViewController(client:)` — Task 6 정의, Task 7 호출 일치
- Wire 접근자 확인: `agent.kind`·`agent.ratePerSec`·`agent.tokens5h`·`agent.usedPct5h`·`agent.usedPctWeekly`·`agent.r5`·`agent.projects`, `project.name`·`project.model`·`project.ratePerSec`·`project.status`·`project.t` — 전부 1단계 `MirrorSnapshot.swift` 에 존재

---

## Execution Handoff

계획 완료. 실행 방식 두 가지 중 선택한다.

1. **Subagent-Driven (권장)** — 태스크마다 새 서브에이전트를 붙이고 사이사이 리뷰.
2. **Inline Execution** — 이 세션에서 체크포인트를 두고 배치 실행.
