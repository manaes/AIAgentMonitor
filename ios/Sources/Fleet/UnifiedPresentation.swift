import Foundation
import MirrorFormat
import Wire

/// 통합 보기 상단 한 줄 = 에이전트 종류 하나.
public struct UnifiedAgentRow: Equatable, Sendable {
    /// `DeviceListPresentation` 과 같은 표기("Claude Code" 등).
    public let name: String
    /// 이 종류를 보고한 장치 수.
    public let deviceCount: Int
    /// 아래 다섯은 `QuotaBarView.configure` 가 그대로 받는 **원값**이다 — `AgentRowModel` 과
    /// 같은 규약이다. 리셋 직후 0% 처리와 조회 실패 시 % 숨김은 `QuotaDisplay`/`QuotaBarView`
    /// 가 이미 갖고 있으므로 여기서 다시 계산하지 않는다.
    public let tokens5h: UInt32
    public let usedPct5h: Float?
    public let usedPctWeekly: Float?
    public let isReset5h: Bool
    public let unreadable: Bool

    public init(
        name: String,
        deviceCount: Int,
        tokens5h: UInt32,
        usedPct5h: Float?,
        usedPctWeekly: Float?,
        isReset5h: Bool,
        unreadable: Bool
    ) {
        self.name = name
        self.deviceCount = deviceCount
        self.tokens5h = tokens5h
        self.usedPct5h = usedPct5h
        self.usedPctWeekly = usedPctWeekly
        self.isReset5h = isReset5h
        self.unreadable = unreadable
    }
}

/// 통합 보기 하단의 에이전트 한 줄. 튜플 배열로 두면 `UnifiedDeviceRow` 의 `Equatable`
/// 자동 합성이 안 되므로 작은 구조체로 둔다.
public struct UnifiedRate: Equatable, Sendable {
    public let name: String
    public let rateText: String

    public init(name: String, rateText: String) {
        self.name = name
        self.rateText = rateText
    }
}

/// 통합 보기 하단 한 줄 = 장치 하나. tok/s 만 보여준다.
public struct UnifiedDeviceRow: Equatable, Sendable {
    public let id: String
    public let title: String
    public let statusText: String
    /// 오프라인이면 빈 배열 — tok/s 는 지금 이 순간의 값이라 낡으면 의미가 없다.
    public let rates: [UnifiedRate]

    public init(id: String, title: String, statusText: String, rates: [UnifiedRate]) {
        self.id = id
        self.title = title
        self.statusText = statusText
        self.rates = rates
    }
}

/// 같은 계정의 Claude/Codex 를 여러 Mac 에서 쓸 때를 위한 보기.
///
/// **Ruling 40 — 합칠 값과 하나만 고를 값이 다르다.** 와이어 모델(`MirrorAgent`)에서
/// 값의 성격이 갈린다:
/// - `p5`·`pw`(%)와 `r5`·`rw`(리셋 시각)는 **계정 단위** 서버 한도다. 같은 계정이면 어느
///   Mac 에서 보든 같은 값이므로 **합치지 않고** 가장 신선한 스냅샷의 값 하나를 쓴다.
///   더하면 42%+42%=84% 같은 거짓이 된다 — 같은 한도를 두 번 세는 것이다.
/// - `t5`(5h 토큰 수)는 그 Mac 에서 쓴 **로컬** 사용량이라 **합산한다**.
/// - `r`(tok/s)는 그 Mac 의 순간값이라 상단 통합에는 넣지 않는다(하단 장치별 섹션의 몫).
///
/// 계정이 서로 다르면 이 보기는 맞지 않다 — 그때는 개별 보기를 쓴다.
public enum UnifiedPresentation {

    /// 상단 "한도 (통합)" 섹션. 종류 하나당 한 줄.
    public static func agentRows(
        sources: [(status: DeviceStatus, cached: CachedSnapshot?)], now: Date
    ) -> [UnifiedAgentRow] {
        // 종류 순서는 개별 보기와 같은 기준(`orderedForDisplay`)을 쓴다 — 목록과 통합에서
        // 에이전트 자리가 뒤바뀌면 사용자는 같은 값을 다른 줄에서 찾게 된다.
        let flattened = sources.compactMap(\.cached).flatMap { $0.snapshot.agents }
        var order: [AgentKindCode] = []
        for agent in DeviceListPresentation.orderedForDisplay(flattened) where !order.contains(agent.kind) {
            order.append(agent.kind)
        }

        return order.map { kind in
            let reports = sources.compactMap { source in report(for: kind, source: source) }
            // `order` 가 실제로 보고된 종류만 담으므로 여기서 reports 는 비지 않는다.
            let adopted = adopt(reports)
            let totalTokens = reports.reduce(UInt64(0)) { $0 + $1.tokens5h }

            return UnifiedAgentRow(
                name: DeviceListPresentation.agentName(kind),
                deviceCount: reports.count,
                // 16대가 각각 UInt32 상한 가까이 쓰면 넘칠 수 있으므로 UInt64 로 더한 뒤 자른다.
                tokens5h: UInt32(clamping: totalTokens),
                usedPct5h: adopted?.usedPct5h,
                usedPctWeekly: adopted?.usedPctWeekly,
                isReset5h: QuotaDisplay.isReset5h(resetAt: adopted?.r5, now: now),
                // 조회 실패 여부도 값을 채택한 스냅샷의 것을 따른다 — 신선한 쪽이 실패
                // 중인데 낡은 쪽이 멀쩡하다고 정상으로 보여주면 안 된다.
                unreadable: adopted?.quotaError != nil
            )
        }
    }

    /// 하단 "장치별 속도" 섹션. 장치 하나당 한 줄.
    public static func deviceRow(
        device: Device, status: DeviceStatus, cached: CachedSnapshot?
    ) -> UnifiedDeviceRow {
        let isLive = (status == .online)
        let agents = isLive ? DeviceListPresentation.orderedForDisplay(cached?.snapshot.agents ?? []) : []
        return UnifiedDeviceRow(
            id: device.endpointIdHex,
            title: device.displayName,
            statusText: DeviceListPresentation.statusText(status),
            rates: agents.map { agent in
                UnifiedRate(
                    name: DeviceListPresentation.agentName(agent.kind),
                    rateText: "\(MirrorFormat.tokensPerSec(agent.ratePerSec)) tok/s"
                )
            }
        )
    }

    /// 장치 한 대가 그 종류에 대해 보고한 것. 한 장치가 같은 종류를 두 줄로 보내는 일은
    /// 없어야 하지만, 토큰까지 잃지 않도록 합쳐 둔다.
    private struct Report {
        let fetchedAt: Date
        /// %·리셋 시각·조회 실패를 가져올 대표 항목.
        let representative: MirrorAgent
        let tokens5h: UInt64
    }

    private static func report(
        for kind: AgentKindCode, source: (status: DeviceStatus, cached: CachedSnapshot?)
    ) -> Report? {
        guard let cached = source.cached else { return nil }
        let matching = cached.snapshot.agents.filter { $0.kind == kind }
        guard let representative = matching.first else { return nil }
        return Report(
            fetchedAt: cached.fetchedAt,
            representative: representative,
            tokens5h: matching.reduce(UInt64(0)) { $0 + UInt64($1.tokens5h) }
        )
    }

    /// 계정 단위 값을 가져올 스냅샷 하나를 고른다 — **오직 `fetchedAt` 이 가장 최근인 것**.
    ///
    /// 장치 상태로 거르지 않는다. 계정 한도는 그 Mac 이 꺼졌다고 틀려지지 않으므로, 2초 전에
    /// 꺼진 Mac 이 보낸 42% 는 그대로 유효하다. 반대로 온라인을 우선하면 스트림이 느린 장치의
    /// **더 오래된** 값을 고르게 된다. 온라인 장치는 초당 갱신돼 실제로는 어차피 가장 최근이라,
    /// 상태 조건은 이득 없이 규칙만 복잡하게 만든다.
    private static func adopt(_ reports: [Report]) -> MirrorAgent? {
        reports.max(by: { $0.fetchedAt < $1.fetchedAt })?.representative
    }
}
