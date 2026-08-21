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
        XCTAssertEqual(v.relativeText, "10초 전")
        XCTAssertEqual(v.dotColor, Palette.claudeDot)
    }

    func testIdleRowShowsStatusWordAndAmberDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "bar", m: "m", r: 50, t: 999_900, s: 1),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightText, "유휴", "active 가 아니면 속도 대신 상태 단어")
        XCTAssertEqual(v.dotColor, Palette.idleDot)
    }

    func testDormantRowUsesGreyDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "baz", m: "m", r: 0, t: 900_000, s: 2),
                    kind: .codex, now: now)
        XCTAssertEqual(v.rightText, "휴면")
        XCTAssertEqual(v.dotColor, Palette.dormantDot, "dormant 는 에이전트 색이 아니라 회색")
    }

    /// 원본은 오른쪽 칸의 **폰트도** 상태에 따라 갈린다.
    ///   active → `.rate { font-weight: 600; tabular-nums }` (SessionList.svelte:55)
    ///   그 외   → `<span class="subtle">` (SessionList.svelte:35) — 색만 지정되고
    ///            굵기는 상속된 normal, 크기는 app.css:22 의 11px.
    /// 폰트가 init 에 있으면 idle/dormant/unknown 이 맥에 없는 semibold 로 그려지므로
    /// 이 단정으로 "switch 안에서 정한다"를 고정한다.
    func testActiveUsesRateFontAndOtherStatusesUseBodyFont() {
        let v = SessionRowView()

        v.configure(project: project(n: "a", m: "m", r: 98.25, t: 999_990, s: 0),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightFont, Typography.rate, "active 의 속도만 11pt semibold tabular")

        for (status, word) in [(UInt8(1), "유휴"), (UInt8(2), "휴면"), (UInt8(3), "알 수 없음")] {
            v.configure(project: project(n: "a", m: "m", r: 0, t: 999_990, s: status),
                        kind: .claude, now: now)
            XCTAssertEqual(v.rightText, word)
            XCTAssertEqual(v.rightFont, Typography.body,
                           "\(word) 은 맥에서 보통 굵기다 — semibold 로 그리면 나란히 놓았을 때 바로 보인다")
        }
    }

    /// 행이 재사용되므로 폰트도 되돌아와야 한다 — active 를 그린 슬롯이 다음
    /// 프레임에 idle 이 되면 semibold 가 남아 있으면 안 된다.
    func testRightFontRevertsWhenActiveRowBecomesIdle() {
        let v = SessionRowView()
        v.configure(project: project(n: "a", m: "m", r: 98.25, t: 999_990, s: 0),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightFont, Typography.rate)

        v.configure(project: project(n: "a", m: "m", r: 0, t: 999_990, s: 1),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightFont, Typography.body, "이전 프레임의 semibold 가 남으면 안 된다")
    }

    func testCodexActiveUsesCodexColor() {
        let v = SessionRowView()
        v.configure(project: project(n: "qux", m: "m", r: 10, t: 999_999, s: 0),
                    kind: .codex, now: now)
        XCTAssertEqual(v.leftText, "Codex · qux m")
        XCTAssertEqual(v.dotColor, Palette.codexDot)
    }

    func testAntigravityActiveUsesAntigravityColor() {
        let v = SessionRowView()
        v.configure(project: project(n: "2_App", m: "gemini-3.7-flash", r: 10, t: 999_999, s: 0),
                    kind: .antigravity, now: now)
        XCTAssertEqual(v.leftText, "Antigravity · 2_App gemini-3.7-flash")
        XCTAssertEqual(v.dotColor, Palette.antigravityDot)
    }

    /// s 가 알려지지 않은 값(3)이면 ActivityStatusCode.unknown 이 된다 — 회색 점은
    /// dormant 와 같이 취급하되, 문구는 "dormant" 로 잘못 표시하지 않고 "unknown" 이어야 한다.
    func testUnknownStatusShowsUnknownWordNotDormant() {
        let v = SessionRowView()
        v.configure(project: project(n: "quux", m: "m", r: 0, t: 900_000, s: 3),
                    kind: .claude, now: now)
        XCTAssertEqual(v.rightText, "알 수 없음", "unknown 을 dormant 로 잘못 표시하면 안 된다")
        XCTAssertEqual(v.dotColor, Palette.dormantDot)
    }

    /// k 가 알려지지 않은 값(2)이면 AgentKindCode.unknown 이 된다.
    func testUnknownAgentKindShowsPlaceholderNameAndDormantDot() {
        let v = SessionRowView()
        v.configure(project: project(n: "corge", m: "m", r: 5, t: 999_999, s: 0),
                    kind: .unknown, now: now)
        XCTAssertEqual(v.leftText, "? · corge m")
        XCTAssertEqual(v.dotColor, Palette.dormantDot, "unknown 에이전트의 active 점도 회색이어야 한다")
    }

    /// 원본 CSS 에는 잘림 규칙이 없지만 실제 기기 폭에서는 셋 중 하나가 잘릴 수 있다.
    /// 우선순위: 에이전트 이름(절대 유지) > 프로젝트 이름 > 모델 이름(가장 먼저 잘림).
    ///
    /// 폭은 실제 iPhone 16 레이아웃값으로 계산한다: 화면 393pt - 좌우 여백 16pt*2 -
    /// 카드 좌우 패딩 12pt*2 = 337pt 가 이 행이 실제로 받는 폭이다. 모델명도 이
    /// 앱에 실제로 존재하는 값(`claude-sonnet-5`, 15자)을 쓴다 — 가상의 31자짜리
    /// 모델명으로 계산하면 없는 위기가 만들어진다(Fix Round 3→4 경위 참고).
    /// 이 조합(프로젝트 "4AIAgentMonitor" + 모델 "claude-sonnet-5")의 실측 초과폭은
    /// 약 10pt 뿐이라, 이름과 프로젝트는 그대로고 모델만 소폭 잘린다.
    func testTruncationAtRealisticDeviceWidthAndRealModelName() {
        let v = SessionRowView()
        v.configure(project: project(n: "4AIAgentMonitor", m: "claude-sonnet-5",
                                      r: 98.25, t: 999_990, s: 0),
                    kind: .claude, now: now)
        v.frame = CGRect(x: 0, y: 0, width: 337, height: 24)
        v.setNeedsLayout()
        v.layoutIfNeeded()

        XCTAssertEqual(
            v.nameLabelWidth, v.nameLabelIntrinsicWidth,
            accuracy: 0.5,
            "에이전트 이름은 실기기 폭에서도 잘리지 않아야 한다"
        )
        XCTAssertEqual(
            v.projLabelWidth, v.projLabelIntrinsicWidth,
            accuracy: 0.5,
            "실제 모델명 길이에서는 프로젝트 이름도 잘릴 필요가 없어야 한다"
        )
        XCTAssertLessThan(
            v.modelLabelWidth, v.modelLabelIntrinsicWidth - 0.5,
            "모델 이름은 폭이 부족하면 셋 중 가장 먼저 잘려야 한다"
        )
        XCTAssertGreaterThan(
            v.modelLabelWidth, v.modelLabelIntrinsicWidth * 0.5,
            "실측 초과폭은 그리 크지 않으므로 모델도 절반 이상은 그대로 보여야 한다(과도한 잘림이면 다른 회귀)"
        )
    }

    /// 위 테스트가 "정상 범위"라면, 이건 그 범위를 벗어난 병적인 입력에서 실제로
    /// 어떤 일이 벌어지는지 기록해두는 테스트다 — 실제로 나올 수 없는 31자짜리
    /// 모델명을 일부러 넣는다. 설계 목표가 아니라 극단값에서의 현재 동작을
    /// 그대로 고정해두는 용도다.
    ///
    /// **알려진 한계 — 이 우선순위(600)에서는 30pt 바닥이 실질적으로 무력하다.**
    /// 바닥(600)이 project 의 저항(700)보다 낮으므로, 필요한 공간을 만들 때
    /// solver 는 project 가 한 치도 양보하기 전에 바닥을 0(모델의 물리적 최소값)
    /// 까지 완전히 희생시킨다 — project 가 이미 자기 목표(98)를 다 채우고도
    /// 남는 자투리 폭이 30pt 에 못 미치면, 바닥은 그 자투리(예: 6.3pt)에서
    /// 멈추지 않고 그대로 통과해 0 까지 내려간다(실측: 215pt 에서 model=0).
    /// project 보다 낮은 우선순위의 바닥은 "0 으로 사라지는 것을 막는다"는
    /// 목표를 이 폭에서는 달성하지 못한다 — 바닥을 실질적으로 만들려면
    /// project(700)보다 높은 우선순위가 필요한데, 그러면 이번엔 이 병적인
    /// 입력에서 project 가 큰 폭으로 양보해야 한다(Fix Round 3 참고). 실사용
    /// 데이터(위 테스트)는 이 트레이드오프에 전혀 걸리지 않으므로 결정은
    /// task-5-report.md 의 "Fix Round 4" 에 기록해 팀 리드 판단에 맡긴다.
    func testPathologicalLongModelNameCurrentlyCanStillReachZero() {
        let v = SessionRowView()
        v.configure(project: project(n: "4AIAgentMonitor",
                                      m: "claude-opus-5-extended-thinking",
                                      r: 10, t: 999_999, s: 0),
                    kind: .claude, now: now)
        v.frame = CGRect(x: 0, y: 0, width: 215, height: 24)
        v.setNeedsLayout()
        v.layoutIfNeeded()

        XCTAssertEqual(
            v.nameLabelWidth, v.nameLabelIntrinsicWidth,
            accuracy: 0.5,
            "에이전트 이름은 이런 극단적인 입력에서도 절대 양보하지 않아야 한다"
        )
        // 의도한 "바닥"이 아니라 현재의 실제 값을 고정한다 — 우선순위가 바뀌면
        // 이 값도 바뀌어야 하므로, 실패하면 위 코멘트의 트레이드오프를 다시 검토한다.
        XCTAssertEqual(
            v.modelLabelWidth, 0, accuracy: 0.5,
            "현재 우선순위(600)에서는 바닥이 무력해 모델이 0 까지 내려간다 — 코멘트 참고"
        )
    }
}
