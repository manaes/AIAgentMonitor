import Foundation
import MirrorFormat
import Wire

public struct QuotaWindowText: Equatable, Sendable {
    public let percentText: String
    public let percent: Float
}

public struct AgentRowModel: Equatable, Sendable {
    public let name: String
    /// 오프라인이면 nil — tok/s 는 지금 이 순간의 값이라 낡으면 의미가 없다.
    public let rateText: String?
    /// 아래 다섯은 `QuotaBarView.configure` 가 그대로 받는 **원값**이다. 리셋 직후 0% 처리와
    /// 조회 실패 시 % 숨김은 `QuotaDisplay`/`QuotaBarView` 가 이미 갖고 있는 규칙이므로
    /// 여기서 다시 계산하지 않는다 — 1:1 앱 `AgentCardView` 와 같은 호출이 되게 한다.
    public let tokens5h: UInt32
    public let usedPct5h: Float?
    public let usedPctWeekly: Float?
    /// 5h 창이 이미 리셋됐는가. 항상 false 로 넘기면 리셋된 에이전트가 리셋 전 낡은 %를
    /// 계속 보여준다.
    public let isReset5h: Bool
    /// 사용량 조회 자체가 실패 중인가(`quotaError`). 값이 단순히 없는 것과는 다르다.
    public let unreadable: Bool
    /// 아래 둘은 % 텍스트가 필요한 곳(테스트·요약 표시)용 파생값이다.
    public let fiveHour: QuotaWindowText?
    public let weekly: QuotaWindowText?
}

/// 목록에서 셀을 탭했을 때 어디로 갈지.
public enum DeviceTapDestination: Equatable, Sendable {
    case detail
    /// 그 장치를 다시 스캔하는 화면. 재페어링 필요 상태에서만 나온다.
    case rePair
}

public struct DeviceRowModel: Equatable, Sendable {
    public let id: String
    public let title: String
    public let statusText: String
    /// 오프라인일 때만 채운다("12분 전").
    public let freshnessText: String?
    public let agents: [AgentRowModel]
    public let hiddenAgentCount: Int
}

public enum DeviceListPresentation {
    public static let visibleAgentLimit = 2

    public static func row(
        device: Device, status: DeviceStatus, cached: CachedSnapshot?, now: Date
    ) -> DeviceRowModel {
        let isLive = (status == .online)
        let ordered = orderedForDisplay(cached?.snapshot.agents ?? [])
        let shown = ordered.prefix(visibleAgentLimit).map { agent in
            let unreadable = agent.quotaError != nil
            return AgentRowModel(
                name: agentName(agent.kind),
                rateText: isLive ? "\(MirrorFormat.tokensPerSec(agent.ratePerSec)) tok/s" : nil,
                tokens5h: agent.tokens5h,
                usedPct5h: agent.usedPct5h,
                usedPctWeekly: agent.usedPctWeekly,
                isReset5h: QuotaDisplay.isReset5h(resetAt: agent.r5, now: now),
                unreadable: unreadable,
                fiveHour: window(percent: agent.usedPct5h, hasError: unreadable),
                weekly: window(percent: agent.usedPctWeekly, hasError: unreadable)
            )
        }

        return DeviceRowModel(
            id: device.endpointIdHex,
            title: device.displayName,
            statusText: statusText(status),
            freshnessText: isLive ? nil : freshness(cached?.fetchedAt, now: now),
            agents: Array(shown),
            hiddenAgentCount: max(0, ordered.count - visibleAgentLimit)
        )
    }

    /// 셀을 탭했을 때 상세로 갈지, 재페어링(재스캔)으로 갈지. 스펙 §7 — "재페어링 필요는
    /// 탭하면 그 장치용 QR 스캐너". 토큰이 폐기된 종단 상태라 상세로 보내봐야 재시도조차
    /// 막혀 있는 막다른 화면이 된다.
    ///
    /// `.versionMismatch` 는 여기 넣지 않는다 — 같은 종단 상태지만 다시 스캔해도 Mac 앱
    /// 버전이 바뀌지 않으므로, 상세에서 무엇이 문제인지 안내하는 편이 맞다.
    ///
    /// 화면 밖으로 뺀 이유는 화면 코드에 자동 테스트를 걸 수단이 없어서다(AppMultiTests 타겟 없음).
    public static func tapDestination(for status: DeviceStatus) -> DeviceTapDestination {
        status == .needsRepairing ? .rePair : .detail
    }

    /// 스캔한 QR 의 장치가 지금 고치려는 장치가 맞는지. `expected` 가 nil 이면(`+` 버튼으로
    /// 연 일반 추가 경로) 무엇이든 통과시킨다.
    ///
    /// 재페어링 스캐너를 범용으로 두면, 다른 Mac 의 QR 을 스캔했을 때 고치려던 장치는 망가진
    /// 채 그대로 두고 엉뚱한 장치가 새로 추가된다. 16대가 찬 상태라면 그 오스캔이 한도 거절로
    /// 끝나 사용자는 영문도 모른 채 실패만 본다(스펙 §7 "그 장치용").
    ///
    /// 비교 전에 소문자로 맞춘다. 이 저장소는 endpointIdHex 를 소문자로 쓰는 전제이지만
    /// (`DeviceSnapshotCache` 의 파일명), 단순 `==` 로 두면 대문자로 실려온 같은 장치의 QR 을
    /// 남의 것으로 거절하게 된다.
    ///
    /// `tapDestination` 과 같은 이유로 화면 밖에 있다 — AppMultiTests 타겟이 없어 화면 안에
    /// 두면 자동 테스트를 걸 수단이 없다.
    public static func acceptsScannedDevice(expected: String?, scanned: String) -> Bool {
        guard let expected else { return true }
        return expected.lowercased() == scanned.lowercased()
    }

    /// `온라인 → 불안정 → 오프라인` 그룹, 그룹 안에서는 sortIndex 순.
    public static func sorted(_ rows: [(Device, DeviceStatus)]) -> [Device] {
        rows.sorted { lhs, rhs in
            let l = groupRank(lhs.1), r = groupRank(rhs.1)
            if l != r { return l < r }
            return lhs.0.sortIndex < rhs.0.sortIndex
        }.map(\.0)
    }

    /// 드래그로 옮길 수 있는 자리인가(스펙 §6.1). 표시 순서는 상태 그룹이 1차 키고
    /// `sortIndex` 는 2차 키라, 그룹을 넘는 이동은 놓는 순간 `sorted` 가 곧바로 원래
    /// 그룹으로 돌려보낸다 — 사용자에게는 "드래그가 먹지 않는" 것으로 보인다.
    /// 그래서 놓기 전에 막는다.
    ///
    /// `tapDestination`·`acceptsScannedDevice` 와 같은 이유로 화면 밖에 있다 —
    /// AppMultiTests 타겟이 없어 화면 안에 두면 자동 테스트를 걸 수단이 없다.
    public static func allowsReorder(from: DeviceStatus, to: DeviceStatus) -> Bool {
        groupRank(from) == groupRank(to)
    }

    private static func groupRank(_ status: DeviceStatus) -> Int {
        switch status {
        case .online: return 0
        case .probing, .unstable: return 1
        case .idle, .offline: return 2
        case .needsRepairing, .versionMismatch: return 3
        }
    }

    private static func window(percent: Float?, hasError: Bool) -> QuotaWindowText? {
        guard !hasError, let percent else { return nil }
        let clamped = min(100, percent)
        return QuotaWindowText(
            percentText: MirrorFormat.toFixed(Double(clamped), 0) + "%",
            percent: clamped
        )
    }

    private static func statusText(_ status: DeviceStatus) -> String {
        switch status {
        case .idle: return "대기"
        case .probing: return "확인 중"
        case .online: return "연결됨"
        case .unstable: return "재연결 중"
        case .offline: return "오프라인"
        case .needsRepairing: return "재페어링 필요"
        case .versionMismatch: return "버전 불일치"
        }
    }

    private static func freshness(_ fetchedAt: Date?, now: Date) -> String? {
        guard let fetchedAt else { return nil }
        return MirrorFormat.relativeTime(UInt64(max(0, fetchedAt.timeIntervalSince1970)), now: now)
    }

    private static func agentName(_ kind: AgentKindCode) -> String {
        switch kind {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .antigravity: return "Antigravity"
        case .unknown: return "Agent"
        }
    }

    /// 기존 앱 `MirrorViewController.orderedForDisplay` 와 같은 순서. 상세 화면도 같은 순서로
    /// 카드를 쌓아야 목록과 상세에서 에이전트 자리가 뒤바뀌지 않으므로 공개한다.
    public static func orderedForDisplay(_ agents: [MirrorAgent]) -> [MirrorAgent] {
        let claude = agents.filter { $0.kind == .claude }
        let codex = agents.filter { $0.kind == .codex }
        let others = agents.filter { $0.kind != .claude && $0.kind != .codex }
        return claude + codex + others
    }
}
