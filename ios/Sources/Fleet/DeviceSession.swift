import Foundation
import Wire

/// 장치 한 대의 연결 수명주기. 상태 전이는 `DeviceStatusMachine` 이 정하고,
/// 이 타입은 전송 호출과 세대 관리만 한다.
@MainActor
public final class DeviceSession {
    public static let probeTimeoutSeconds: Double = 3

    public let device: Device
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

    /// 지금 한 번 붙어본다. 종단 상태(재페어링 필요·버전 불일치)면 아무것도 하지 않는다.
    public func probeNow() async {
        guard !isStopped, !isTerminal else { return }
        // 포어그라운드 복귀처럼 online 인 세션에 다시 probe 가 걸리면, 죽었을 옛 스트림을
        // 기다리지 말고 바로 끊는다. 아래 generation 증가까지 await 가 없으므로, 취소당한
        // 옛 작업이 다시 깨어날 때는 이미 새 세대라 가드에 걸린다.
        probeTask?.cancel()

        generation += 1
        let current = generation
        apply(.probeStarted, generation: current)

        // `await self?.runProbe(...)` 는 결과 타입이 `()?` 라 `Task<Void, Never>` 에 못 담는다.
        let task = Task { [weak self] in
            guard let self else { return }
            await self.runProbe(generation: current)
        }
        // 여기서 nil 로 되돌리지 않는다 — 이 호출이 기다리는 동안 새 probe 가 들어와
        // probeTask 를 갈아끼웠을 수 있고, 그걸 지워버리면 취소 대상을 잃는다.
        probeTask = task
        await task.value
    }

    /// `probeNow()` 의 본문. 취소(CancellationError)는 아래 범용 catch 로 떨어지는데,
    /// 취소한 쪽이 generation 을 이미 올렸으므로 `guard current == generation` 에서
    /// 조용히 빠져나간다.
    private func runProbe(generation current: Int) async {
        do {
            let stream = try await transport.probe(
                device: device, timeoutSeconds: Self.probeTimeoutSeconds
            )
            guard current == generation else { return }
            apply(.probeSucceeded, generation: current)
            await consume(stream, generation: current)
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
        let nextStatus = DeviceStatusMachine.next(status, on: .retriggered)
        // 오프라인이 아니면 재탐색 대상이 아니다(이미 붙어 있거나 붙는 중).
        guard nextStatus == .probing || status == .idle else { return }
        await probeNow()
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
