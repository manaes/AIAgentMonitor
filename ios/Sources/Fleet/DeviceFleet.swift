import Foundation
import Wire

/// 장치 세션 전체를 소유하고 probe 라운드를 관리한다.
@MainActor
public final class DeviceFleet {
    /// 순차는 꺼진 Mac 의 타임아웃이 직렬로 쌓여 목록이 자리잡는 데 최악 100초가 넘고,
    /// 16개를 한꺼번에 던지면 릴레이에 몰린다.
    public static let probeConcurrency = 4
    /// 포어그라운드 상태에서 오프라인 장치를 다시 확인하는 주기.
    public static let reprobeInterval: TimeInterval = 180
    /// 실패한 직후 다시 붙어보는 간격. 이게 없으면 **한 번의 일시적 실패가 최대 3분간
    /// "재연결 중" 으로 남는다** — 3분 타이머나 당겨서 새로고침 말고는 다시 볼 기회가 없기 때문이다.
    /// 페어링 직후가 가장 잘 보인다. 페어링은 연결을 닫자마자 fleet 이 곧바로 같은 Mac 에
    /// 다시 dial 하는데, 그 순간의 dial 은 3초 안에 안 끝나기 쉽고(맥이 직전 연결을 정리하는 중,
    /// hole-punch 를 다시 뚫는 중) 몇 초 뒤 재시도는 대개 성공한다.
    ///
    /// 이 간격이 "연속 3회 실패 → 오프라인" 을 빠르게 소진시키는 것은 의도한 것이다.
    /// 꺼져 있는 Mac 이 9분이 아니라 십여 초 만에 "오프라인" 으로 정착하는 편이 정직하고,
    /// 오프라인이 된 뒤에도 3분 타이머가 계속 다시 확인한다. 테스트가 줄일 수 있도록 var 다.
    public static var quickRetryDelays: [TimeInterval] = [2, 6]
    /// 장치별 스냅샷 캐시 쓰기 최소 간격. 캐시는 "다음 실행 때 목록을 미리 채우는" 용도라
    /// 최신일 필요가 없는데, 스트리밍 중에는 스냅샷이 초당 들어와 그때마다 원자적 파일
    /// 쓰기가 일어난다. 테스트가 줄일 수 있도록 var 로 둔다.
    public static var cacheWriteInterval: TimeInterval = 30

    public private(set) var sessions: [DeviceSession] = []
    public var onChange: (() -> Void)?
    /// 세션 **하나**의 변화를 보고 싶은 화면(상세)용. `onChange`(목록용, 인자 없음)와 별개로 어느 세션이
    /// 바뀌었는지 넘긴다. 상세 화면이 viewWillAppear 에서 걸고 viewWillDisappear 에서 nil 로 되돌린다.
    /// `DeviceSession.onChange` 자체는 fleet 소유라 화면이 건드리면 안 된다 — 캐시 쓰기가 끊긴다.
    public var onSessionChange: ((DeviceSession) -> Void)?

    private let cache: DeviceSnapshotCache?
    /// 장치별 마지막 캐시 쓰기 시각. `cacheWriteInterval` 스로틀의 기준점.
    private var lastCacheWriteAt: [String: Date] = [:]
    /// 진행 중인 짧은 재시도. 라운드마다 갈아끼우고 `stopAll()` 에서 끊는다.
    private var quickRetryTask: Task<Void, Never>?

    public init(
        devices: [Device],
        transportFactory: @escaping (Device) -> DeviceTransport,
        cache: DeviceSnapshotCache?
    ) {
        self.cache = cache
        self.sessions = devices.map { device in
            DeviceSession(device: device, transport: transportFactory(device))
        }
        for session in sessions {
            session.onChange = { [weak self] changed in
                self?.handleChange(changed)
            }
        }
    }

    /// 앱 시작·포어그라운드 복귀 시. 모든 세션을 동시성 제한 안에서 붙여본다.
    /// 포어그라운드 복귀 때 대상이 "오프라인이던 장치"가 아니라 전부인 이유는,
    /// iOS 가 앱을 suspend 하면 QUIC 연결이 전부 조용히 끊기기 때문이다.
    public func startInitialRound() async {
        await runRound(sessions) { session in await session.probeNow() }
        scheduleQuickRetries()
    }

    /// 타이머(3분)/당겨서 새로고침. **붙어 있지 않은 모든 세션**(미탐색·재연결 중·오프라인)을
    /// 다시 본다. 판정은 `DeviceStatus.isRetriggerable` 한 곳에만 있다 — 여기와
    /// `DeviceSession.retrigger()` 에 같은 규칙을 따로 적어 어긋난 것이 Ruling 33 의 버그였다.
    public func retriggerOffline() async {
        let targets = sessions.filter { $0.status.isRetriggerable }
        await runRound(targets) { session in await session.retrigger() }
        scheduleQuickRetries()
    }

    /// 백그라운드 전환·종료 시점, 그리고 fleet 을 버리기 직전에 부른다. 스로틀을 무시하고
    /// 마지막 스냅샷을 가진 **모든** 세션을 즉시 쓴다 — 스로틀 때문에 최대
    /// `cacheWriteInterval` 만큼 뒤처져 있을 수 있기 때문이다.
    ///
    /// 상태로 거르지 않는다. 방금 unstable·offline 으로 떨어진 장치도 마지막 성공
    /// 스냅샷을 그대로 들고 있고, 목록 화면은 오프라인 장치에 바로 그 캐시를 보여준다.
    /// online 만 쓰면 "연결이 끊긴 직후 백그라운드로 간" 경우에 가장 쓸모 있는 스냅샷이
    /// 통째로 버려진다.
    public func flushCache() {
        let now = Date()
        for session in sessions {
            guard let snapshot = session.latest else { continue }
            let id = session.device.endpointIdHex
            lastCacheWriteAt[id] = now
            cache?.save(snapshot, fetchedAt: session.latestAt ?? now, endpointIdHex: id)
        }
    }

    /// 레지스트리에 새로 매겨진 정렬 순서를 **살아 있는** 세션에 얹는다(스펙 §6.1 드래그
    /// 재정렬). fleet 을 다시 만들면 16대의 QUIC 연결이 전부 끊기므로, 순서만 바꾸는
    /// 재정렬에는 쓸 수 없다. 이걸 안 하면 다음 갱신(스트리밍 중에는 매초)에서 옛
    /// sortIndex 로 다시 정렬돼 방금 옮긴 자리가 튕겨 돌아간다.
    ///
    /// 목록 갱신은 호출부가 한다 — 재정렬은 사용자 조작이라 어차피 그 자리에서 다시 그린다.
    public func applySortOrder(_ devices: [Device]) {
        let indexByHex = Dictionary(
            devices.map { ($0.endpointIdHex, $0.sortIndex) }, uniquingKeysWith: { first, _ in first }
        )
        for session in sessions {
            guard let index = indexByHex[session.device.endpointIdHex] else { continue }
            session.updateSortIndex(index)
        }
    }

    /// 화면이 이 fleet 을 버릴 때(레지스트리 변경으로 다시 만들 때) 부른다. 모든 세션의
    /// 진행 중 작업을 취소해 연결을 닫는다 — 안 부르면 옛 fleet 의 스트림이 계속 살아
    /// 삭제된 장치의 연결이 누수되고 남은 장치는 이중 연결이 된다.
    public func stopAll() {
        quickRetryTask?.cancel()
        quickRetryTask = nil
        for session in sessions { session.stop() }
    }

    /// 방금 실패한 세션(`.unstable`)만 짧은 간격으로 다시 붙어본다.
    /// `.offline` 은 대상이 아니다 — 거긴 이미 3회 실패로 정착한 상태라 3분 타이머의 몫이고,
    /// 여기서까지 계속 두드리면 꺼진 Mac 16대에 끝없이 dial 하게 된다.
    private func scheduleQuickRetries() {
        quickRetryTask?.cancel()
        quickRetryTask = Task { [weak self] in
            for delay in Self.quickRetryDelays {
                try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                guard !Task.isCancelled, let self else { return }
                let targets = self.sessions.filter {
                    if case .unstable = $0.status { return true }
                    return false
                }
                // 더 볼 게 없으면 남은 간격까지 기다리지 않고 끝낸다.
                guard !targets.isEmpty else { return }
                await self.runRound(targets) { session in await session.retrigger() }
            }
        }
    }

    /// 동시성 제한 안에서 `action` 을 실행한다. 처음 `probeConcurrency` 개를 띄우고,
    /// 하나가 끝날 때마다 대기열에서 다음 것을 채워 넣는다 — 4개를 띄운 뒤
    /// 전부 끝나길 기다렸다가 다음 4개를 도는 lockstep 방식과는 다르다.
    private func runRound(_ targets: [DeviceSession], _ action: @escaping (DeviceSession) async -> Void) async {
        await withTaskGroup(of: Void.self) { group in
            var iterator = targets.makeIterator()
            for _ in 0..<Self.probeConcurrency {
                guard let session = iterator.next() else { break }
                group.addTask { await action(session) }
            }
            while await group.next() != nil {
                guard let session = iterator.next() else { continue }
                group.addTask { await action(session) }
            }
        }
    }

    private func handleChange(_ session: DeviceSession) {
        if let snapshot = session.latest, session.status == .online {
            let id = session.device.endpointIdHex
            let now = Date()
            // 스냅샷마다 원자적 파일 쓰기를 하면 16대 × 초당 1장이 그대로 디스크로 간다.
            if let last = lastCacheWriteAt[id], now.timeIntervalSince(last) < Self.cacheWriteInterval {
                // 스로틀 구간. 백그라운드 전환 때 flushCache() 가 마지막 상태를 마저 쓴다.
            } else {
                lastCacheWriteAt[id] = now
                // 쓴 시각이 아니라 **스냅샷을 받은 시각**을 남긴다 — 스로틀 때문에 둘이
                // 최대 cacheWriteInterval 만큼 벌어지고, 신선도 표시는 후자여야 맞다.
                cache?.save(snapshot, fetchedAt: session.latestAt ?? now, endpointIdHex: id)
            }
        }
        onSessionChange?(session)
        onChange?()
    }
}
