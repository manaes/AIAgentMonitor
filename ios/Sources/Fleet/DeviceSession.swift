import Foundation
import Wire

/// 장치 한 대의 연결 수명주기. 상태 전이는 `DeviceStatusMachine` 이 정하고,
/// 이 타입은 전송 호출과 세대 관리만 한다.
@MainActor
public final class DeviceSession {
    /// 장치 하나에 붙어보는 예산. dial(hole-punch 또는 릴레이) + 인증 왕복 여러 번 +
    /// 첫 스냅샷까지가 이 안에 들어가야 한다. 처음 3초로 잡았다가 실기에서 전부 실패했다 —
    /// 같은 일을 하는 페어링 경로가 10초를 쓰는 데는 이유가 있었다. Endpoint 바인드는
    /// 이 예산 밖으로 뺐으므로(`NetworkClient.endpointBindTimeoutSeconds`) 순수 dial 비용만 재면 된다.
    /// 16대가 전부 꺼져 있어도 동시성 4라 목록이 자리잡는 최악은 4라운드 × 이 값이다.
    public static let probeTimeoutSeconds: Double = 8

    /// `sortIndex` 만 드래그 재정렬로 바뀐다(`updateSortIndex`). 나머지 필드는 세션
    /// 수명 동안 고정이다 — 연결 정보가 바뀌면 fleet 을 다시 만든다.
    public private(set) var device: Device
    public private(set) var status: DeviceStatus = .idle
    public private(set) var latest: MirrorSnapshot?
    /// `latest` 가 도착한 시각. 오프라인 장치도 마지막 스냅샷을 계속 들고 있으므로,
    /// 목록 화면이 "방금 전"으로 잘못 표시하지 않으려면 스냅샷 자체의 수신 시각이 필요하다.
    public private(set) var latestAt: Date?
    public var onChange: ((DeviceSession) -> Void)?

    private let transport: DeviceTransport
    /// 취소/정리 시점 이후에 도착한 결과가 상태를 덮어쓰는 걸 막는다.
    /// 완료 지점마다 이 값을 검사한다.
    private var generation = 0
    /// stop() 이후 세션은 죽은 것으로 취급한다. generation 만으로는 "이미 멈춘 세션에
    /// 나중에 새로 걸어온 probeNow() 호출"까지 막지 못한다 — generation 은 그 호출
    /// 자체를 새 세대로 인정해버리기 때문이다. 재사용하려면 새 DeviceSession 을 만든다.
    private var isStopped = false
    /// 진행 중인 probe/consume 작업. `stop()` 과 재-probe 가 취소해 스트림을 **실제로** 닫는다 —
    /// 취소는 `for try await` 에서 CancellationError 로 튀어나오고, 그 취소가 스트림의
    /// onTermination 까지 내려가 QUIC 연결을 닫는다. generation 가드는 늦은 결과를 **무시**만
    /// 할 수 있고 연결을 닫지는 못한다(다음 스냅샷이 와야 가드에 걸리는데, 조용한 Mac 은
    /// 그 스냅샷을 영원히 안 보낸다).
    private var probeTask: Task<Void, Never>?

    public init(device: Device, transport: DeviceTransport) {
        self.device = device
        self.transport = transport
    }

    /// 지금 한 번 **붙어보기만** 한다. 종단 상태(재페어링 필요·버전 불일치)면 아무것도 하지 않는다.
    ///
    /// 스트림 소비를 여기서 기다리지 않는 게 핵심이다. 실제 전송의 스냅샷 스트림은 연결이
    /// 살아 있는 한 끝나지 않으므로, 소비까지 await 하면 이 함수는 online 인 동안 영영
    /// 반환하지 않는다. 그러면 `DeviceFleet.runRound` 의 동시성 슬롯(4)을 온라인 세션이
    /// 영구 점유해 5대째부터는 probe 조차 시작하지 못하고, 당겨서 새로고침은 장치가
    /// 실제로 복구되는 순간 영원히 끝나지 않는다. 슬롯은 dial/인증에만 쓰고, 소비는
    /// 세션이 자기 `probeTask` 로 들고 간다.
    public func probeNow() async {
        guard !isStopped, !isTerminal else { return }
        // 포어그라운드 복귀처럼 online 인 세션에 다시 probe 가 걸리면, 죽었을 옛 스트림을
        // 기다리지 말고 바로 끊는다. 아래 generation 증가까지 await 가 없으므로, 취소당한
        // 옛 작업이 다시 깨어날 때는 이미 새 세대라 가드에 걸린다.
        probeTask?.cancel()

        generation += 1
        let current = generation
        apply(.probeStarted, generation: current)

        do {
            let stream = try await transport.probe(
                device: device, timeoutSeconds: Self.probeTimeoutSeconds
            )
            // 늦게 도착한 스트림은 여기서 버려진다 — 아무도 소비하지 않은 스트림은 해제되면서
            // onTermination 이 불려 연결이 닫힌다.
            guard current == generation else { return }
            apply(.probeSucceeded, generation: current)
            // 여기서 nil 로 되돌리지 않는다 — 새 probe 가 들어와 probeTask 를 갈아끼웠을 수
            // 있고, 그걸 지워버리면 취소 대상을 잃는다.
            probeTask = Task { [weak self] in
                guard let self else { return }
                await self.consume(stream, generation: current)
            }
        } catch let error as DeviceTransportError {
            guard current == generation else { return }
            switch error {
            case .authRejected: apply(.authRejected, generation: current)
            case .versionMismatch: apply(.versionRejected, generation: current)
            case .unreachable: apply(.probeFailed, generation: current)
            }
        } catch {
            guard current == generation else { return }
            apply(.probeFailed, generation: current)
        }
    }

    /// 타이머(3분)/포어그라운드 복귀/당겨서 새로고침이 부른다.
    public func retrigger() async {
        guard !isStopped, !isTerminal else { return }
        // 이미 probing 중이면 재요청은 무시한다. DeviceStatusMachine.next(.probing, on:
        // .retriggered) 는 항등 매핑으로 .probing 을 그대로 돌려주는데, 이는 offline→probing
        // 전이와 값이 같아서 다음 줄의 가드만으로는 두 경우를 구분할 수 없다. 여기서 걸러내지
        // 않으면 probeNow() 가 다시 호출되어 generation 이 올라가고, 지금 한창 진행 중인 probe
        // 자체가 "늦게 도착한 결과"로 취급돼 버려 이 태스크가 막으려는 버그를 스스로 일으킨다.
        guard status != .probing else { return }
        // 재탐색 대상 판정은 `DeviceStatus.isRetriggerable` 한 곳에만 둔다 — 같은 규칙을
        // 여기와 `DeviceFleet.retriggerOffline()` 에 따로 적어 두 곳이 어긋난 것이
        // "`.unstable` 에서 영영 못 빠져나오는" 버그였다(Ruling 33).
        // `.probing` 은 isRetriggerable 이 false 라 위 가드와 중복이지만, 이 함수의 계약을
        // 한 줄로 읽히게 두는 편이 낫다.
        guard status.isRetriggerable else { return }
        await probeNow()
    }

    /// 드래그 재정렬이 저장한 순서를 반영한다. 표시 순서만 바꾸는 것이라 연결·상태는
    /// 건드리지 않는다 — 여기서 세션을 다시 만들면 16대의 QUIC 연결이 전부 끊긴다.
    public func updateSortIndex(_ newValue: Int) {
        device.sortIndex = newValue
    }

    public func stop() {
        generation += 1
        isStopped = true
        probeTask?.cancel()
        probeTask = nil
        status = .idle
        latest = nil
        latestAt = nil
        onChange?(self)
    }

    private var isTerminal: Bool {
        status == .needsRepairing || status == .versionMismatch
    }

    private func consume(_ stream: AsyncThrowingStream<MirrorSnapshot, Error>, generation current: Int) async {
        do {
            for try await snapshot in stream {
                guard current == generation else { return }
                latest = snapshot
                latestAt = Date()
                apply(.streamEstablished, generation: current)
            }
            // 스트림이 에러 없이 끝났다 — 실제 전송은 연결이 살아있는 한 계속 스냅샷을
            // 흘려보내므로, 정상 종료는 "이번 probe 로 받을 수 있는 만큼 다 받았다"는
            // 뜻이지 연결이 끊겼다는 뜻이 아니다. 마지막으로 도달한 상태(대개 online)를
            // 그대로 둔다. 실제로 연결이 끊기면 스트림은 에러를 던지고, 그건 catch 가 처리한다.
        } catch let error as DeviceTransportError {
            // 스트림 도중에 도착한 종단 상태를 재시도 가능한 상태로 격하하면 안 된다 —
            // 타입을 버리고 전부 connectionLost 로 뭉개면, 이미 online 이던 장치가
            // 버전 불일치·인증 거부를 만나도 unstable → offline 로만 가서 성공할 수 없는
            // 재시도를 영원히 반복한다(probeNow 의 catch 와 같은 분기를 쓴다).
            guard current == generation else { return }
            switch error {
            case .authRejected: apply(.authRejected, generation: current)
            case .versionMismatch: apply(.versionRejected, generation: current)
            case .unreachable: apply(.connectionLost, generation: current)
            }
        } catch {
            guard current == generation else { return }
            apply(.connectionLost, generation: current)
        }
    }

    private func apply(_ event: DeviceEvent, generation current: Int) {
        guard current == generation else { return }
        status = DeviceStatusMachine.next(status, on: event)
        onChange?(self)
    }
}
