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

    public init(device: Device, transport: DeviceTransport) {
        self.device = device
        self.transport = transport
    }

    /// 지금 한 번 붙어본다. 종단 상태(재페어링 필요·버전 불일치)면 아무것도 하지 않는다.
    public func probeNow() async {
        guard !isStopped, !isTerminal else { return }

        generation += 1
        let current = generation
        apply(.probeStarted, generation: current)

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
