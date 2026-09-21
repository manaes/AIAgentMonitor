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
    }

    /// 타이머(3분)/당겨서 새로고침. **붙어 있지 않은 모든 세션**(미탐색·재연결 중·오프라인)을
    /// 다시 본다. 판정은 `DeviceStatus.isRetriggerable` 한 곳에만 있다 — 여기와
    /// `DeviceSession.retrigger()` 에 같은 규칙을 따로 적어 어긋난 것이 Ruling 33 의 버그였다.
    public func retriggerOffline() async {
        let targets = sessions.filter { $0.status.isRetriggerable }
        await runRound(targets) { session in await session.retrigger() }
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

    /// 화면이 이 fleet 을 버릴 때(레지스트리 변경으로 다시 만들 때) 부른다. 모든 세션의
    /// 진행 중 작업을 취소해 연결을 닫는다 — 안 부르면 옛 fleet 의 스트림이 계속 살아
    /// 삭제된 장치의 연결이 누수되고 남은 장치는 이중 연결이 된다.
    public func stopAll() {
        for session in sessions { session.stop() }
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
