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

    public private(set) var sessions: [DeviceSession] = []
    public var onChange: (() -> Void)?

    private let cache: DeviceSnapshotCache?

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

    /// 타이머(3분)/당겨서 새로고침. 오프라인·미탐색 세션만 다시 본다.
    public func retriggerOffline() async {
        let targets = sessions.filter { $0.status == .offline || $0.status == .idle }
        await runRound(targets) { session in await session.retrigger() }
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
            cache?.save(snapshot, fetchedAt: Date(), endpointIdHex: session.device.endpointIdHex)
        }
        onChange?()
    }
}
