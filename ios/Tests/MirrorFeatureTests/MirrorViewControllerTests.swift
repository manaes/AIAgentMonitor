import BLETransport
import UIKit
import Wire
import XCTest
@testable import MirrorFeature

/// `MirrorViewController` 는 조립·1Hz 타이머·두 번째 뷰 풀(`agentCards`)·표시 순서를
/// 모두 소유하면서 테스트가 하나도 없었다. BLE 는 시뮬레이터에서 **동작**하지 않을 뿐
/// 인스턴스화는 되므로, `BLEClient()` 로 만들고 `loadViewIfNeeded()` 로 뷰를 세운 뒤
/// BLE 스트림 대신 `configure(snapshot:now:)` 로 직접 구동해 검증한다.
@MainActor
final class MirrorViewControllerTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    private func snapshot(_ json: String) -> MirrorSnapshot {
        // swiftlint:disable:next force_try
        try! JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
    }

    private func loaded() -> MirrorViewController {
        let vc = MirrorViewController(client: BLEClient())
        vc.loadViewIfNeeded()
        return vc
    }

    /// JSON 순서를 일부러 codex → claude 로 뒤집어 둔다. 그냥 스냅샷 순서대로
    /// 그리면 이 테스트가 실패한다.
    private let codexFirstThenClaude = #"""
    {"v":1,"t":0,"a":[
      {"k":1,"r":10,"t5":1000,"pj":[{"id":2,"n":"codex-proj","m":"gpt-x","r":10,"t":999000,"s":0}]},
      {"k":0,"r":20,"t5":2000,"pj":[{"id":1,"n":"claude-proj","m":"claude-opus-5","r":20,"t":999990,"s":0}]}
    ]}
    """#

    private let claudeOnly = #"""
    {"v":1,"t":0,"a":[
      {"k":0,"r":20,"t5":2000,"pj":[{"id":1,"n":"claude-proj","m":"claude-opus-5","r":20,"t":999990,"s":0}]}
    ]}
    """#

    // MARK: - 카드 조립과 표시 순서

    func testTwoAgentsProduceTwoCardsInClaudeThenCodexOrder() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(codexFirstThenClaude), now: now)

        XCTAssertEqual(vc.visibleAgentCardCount, 2)
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Claude Code",
                       "스냅샷 순서와 무관하게 claude 카드가 항상 먼저 — 매 프레임 자리가 바뀌면 안 된다")
        XCTAssertEqual(vc.visibleAgentCardName(at: 1), "Codex")
    }

    /// `orderedForDisplay` 의 세 번째 갈래(claude/codex 가 아닌 것)까지 덮는다.
    func testUnknownKindAgentIsOrderedAfterClaudeAndCodex() {
        let json = #"""
        {"v":1,"t":0,"a":[
          {"k":2,"r":1,"t5":10,"pj":[]},
          {"k":1,"r":10,"t5":1000,"pj":[]},
          {"k":0,"r":20,"t5":2000,"pj":[]}
        ]}
        """#
        let vc = loaded()
        vc.configure(snapshot: snapshot(json), now: now)

        XCTAssertEqual(vc.visibleAgentCardCount, 3)
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Claude Code")
        XCTAssertEqual(vc.visibleAgentCardName(at: 1), "Codex")
        XCTAssertEqual(vc.visibleAgentCardName(at: 2), "알 수 없음", "모르는 종류는 맨 뒤")
    }

    func testSingleAgentProducesExactlyOneCard() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(claudeOnly), now: now)

        XCTAssertEqual(vc.visibleAgentCardCount, 1)
        XCTAssertEqual(vc.pooledAgentCardCount, 1, "필요 없는 카드를 미리 만들지 않는다")
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Claude Code")
        XCTAssertNil(vc.visibleAgentCardName(at: 1))
    }

    // MARK: - 재사용 풀

    /// 세션 목록 풀과 같은 위험이 카드 풀에도 있다 — 에이전트가 2 → 1 로 줄었을 때
    /// 남는 카드가 이전 프레임의 내용을 그대로 들고 화면에 남아 있으면 안 된다.
    func testFewerAgentsHidesTheStaleCardWithoutDestroyingThePool() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(codexFirstThenClaude), now: now)
        XCTAssertEqual(vc.visibleAgentCardCount, 2)
        XCTAssertEqual(vc.pooledAgentCardCount, 2)

        vc.configure(snapshot: snapshot(claudeOnly), now: now)
        XCTAssertEqual(vc.visibleAgentCardCount, 1, "Codex 가 사라졌으면 카드도 화면에서 사라져야 한다")
        XCTAssertEqual(vc.pooledAgentCardCount, 2, "풀은 유지된다 — 매 프레임 뷰를 새로 만들지 않는다")
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Claude Code")
        XCTAssertNil(vc.visibleAgentCardName(at: 1), "숨은 카드가 보이는 목록에 섞이면 안 된다")
    }

    /// 카드가 재사용되므로, 같은 인덱스에 다른 에이전트가 오면 내용이 통째로 갈려야 한다.
    func testReusedCardShowsTheNewAgentNotTheOldOne() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(#"""
        {"v":1,"t":0,"a":[
          {"k":1,"r":10,"t5":1000,"pj":[{"id":2,"n":"codex-proj","m":"gpt-x","r":10,"t":999000,"s":0}]}
        ]}
        """#), now: now)
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Codex")

        vc.configure(snapshot: snapshot(claudeOnly), now: now)
        XCTAssertEqual(vc.pooledAgentCardCount, 1, "같은 카드가 재사용됐는지 먼저 확인")
        XCTAssertEqual(vc.visibleAgentCardName(at: 0), "Claude Code", "재사용된 카드에 이전 잔상이 남으면 안 된다")
    }

    // MARK: - 세션 목록

    func testSessionListReflectsSnapshotProjectsInRecentOrder() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(codexFirstThenClaude), now: now)

        XCTAssertEqual(vc.sessionRowCount, 2, "두 에이전트의 프로젝트가 한 목록으로 펼쳐진다")
        XCTAssertTrue(vc.sessionRowText(at: 0)?.contains("claude-proj") == true,
                      "t 가 더 큰 쪽(999990)이 위")
        XCTAssertTrue(vc.sessionRowText(at: 1)?.contains("codex-proj") == true)
    }

    func testSessionListEmptiesWhenSnapshotHasNoProjects() {
        let vc = loaded()
        vc.configure(snapshot: snapshot(codexFirstThenClaude), now: now)
        XCTAssertEqual(vc.sessionRowCount, 2)

        vc.configure(snapshot: snapshot(#"{"v":1,"t":0,"a":[]}"#), now: now)
        XCTAssertEqual(vc.sessionRowCount, 0)
        XCTAssertEqual(vc.visibleAgentCardCount, 0, "에이전트가 없으면 카드도 전부 숨는다")
    }

    // MARK: - 상태 라벨

    /// 상단 상태 라벨이 `ConnectionState.label` 을 그대로 쓴다는 것을 고정한다.
    /// 시뮬레이터에는 BLE 가 없어 `.idle` 이후의 전이는 여기서 만들 수 없다 —
    /// 각 상태의 문구 자체는 `ConnectionStateTests` 가 전수 검증한다.
    func testStatusLabelShowsConnectionStateLabel() {
        let vc = loaded()
        XCTAssertEqual(vc.statusText, ConnectionState.idle.label)
        XCTAssertEqual(vc.statusText, "대기 중")
    }

    // MARK: - now 주입

    /// 1Hz 로 다시 그리는 이유가 카운트다운/상대 시각이므로, 주입한 `now` 가
    /// 카드 안쪽까지 실제로 전달되는지 확인한다(같은 스냅샷에 다른 now).
    func testInjectedNowReachesCardCountdown() {
        let json = #"""
        {"v":1,"t":0,"a":[
          {"k":0,"r":20,"t5":2000,"r5":1003725,"pj":[]}
        ]}
        """#
        let vc = loaded()
        vc.configure(snapshot: snapshot(json), now: now)
        let first = vc.visibleAgentCardCountdown(at: 0)

        vc.configure(snapshot: snapshot(json), now: now.addingTimeInterval(60))
        let later = vc.visibleAgentCardCountdown(at: 0)

        XCTAssertNotNil(first)
        XCTAssertNotEqual(first, later, "같은 스냅샷이라도 now 가 흐르면 카운트다운이 줄어야 한다")
    }

    // MARK: - 페어링 시트 present/dismiss 결정 (전체 브랜치 리뷰 I-3)

    /// `present`/`dismiss` 자체(UIKit)는 호스트 없는 로직 테스트 번들에서 신뢰성 있게
    /// 검증하기 어렵다 — 그래서 `pairingAction(for:)` 을 순수 함수로 뽑아 그 결정만 고정한다.
    func testPairingSheetPresentsWhenCodeIsNeeded() {
        XCTAssertEqual(
            MirrorViewController.pairingAction(for: .needsPairing),
            .present(attemptsRemaining: nil)
        )
        XCTAssertEqual(
            MirrorViewController.pairingAction(for: .pairingFailed(left: 3)),
            .present(attemptsRemaining: 3)
        )
    }

    /// 시트가 떠 있는 동안 연결이 어떤 이유로든 끊기면 닫혀야 한다 — 안 그러면
    /// `isModalInPresentation = true` 인 시트 뒤에 상태 라벨이 가려진 채 사용자가
    /// 이유를 알 방법 없이 갇힌다(전체 브랜치 리뷰 I-3, 실제로 발견된 결함).
    func testPairingSheetDismissesOnAnyDisconnectionReason() {
        for state: ConnectionState in [
            .streaming, .disconnected(reason: "아무 이유"), .bluetoothOff, .idle, .versionMismatch,
        ] {
            XCTAssertEqual(MirrorViewController.pairingAction(for: state), .dismiss, "\(state) 는 닫혀야 한다")
        }
    }

    func testPairingSheetIgnoresTransientConnectionStates() {
        XCTAssertEqual(MirrorViewController.pairingAction(for: .scanning), .none)
        XCTAssertEqual(MirrorViewController.pairingAction(for: .connecting), .none)
    }
}
