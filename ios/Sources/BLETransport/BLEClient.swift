import Combine
import CoreBluetooth
import Foundation
import Wire

/// CBCentralManager 래퍼. 서비스 UUID 로 스캔 → 연결 → Snapshot 특성 구독 → 청크 재조립.
///
/// CoreBluetooth 콜백이 메인 큐로 오도록 만들고 클래스 전체를 @MainActor 로 고정한다.
/// Swift 6 엄격 동시성에서 델리게이트 상태 접근을 안전하게 만드는 가장 단순한 방법이다.
@MainActor
public final class BLEClient: NSObject {
    private var central: CBCentralManager?
    private var peripheral: CBPeripheral?
    private var reassembler = FrameReassembler()
    /// 사용자가 `start()`/`stop()` 중 어느 쪽을 마지막으로 불렀는지. `stop()` 으로 인한
    /// 연쇄 콜백(didDisconnectPeripheral 등)이 다시 스캔을 시작하지 않도록 막는 데 쓴다.
    private var wantsRunning = false
    /// `connect()` 가 콜백 없이 무한정 걸리는 경우를 위한 타임아웃. 연결 성공/실패가
    /// 확정되면 반드시 취소해서 이후 연결 시도에 잘못 발동하지 않게 한다.
    private var connectTimeout: Task<Void, Never>?

    private let stateSubject = CurrentValueSubject<ConnectionState, Never>(.idle)
    private let snapshotSubject = PassthroughSubject<MirrorSnapshot, Never>()
    private let rawSubject = PassthroughSubject<String, Never>()

    public var state: AnyPublisher<ConnectionState, Never> { stateSubject.eraseToAnyPublisher() }
    public var snapshots: AnyPublisher<MirrorSnapshot, Never> { snapshotSubject.eraseToAnyPublisher() }
    /// 1단계 확인용 원본 JSON 스트림
    public var rawMessages: AnyPublisher<String, Never> { rawSubject.eraseToAnyPublisher() }

    public override init() {
        super.init()
    }

    public func start() {
        // 이미 실행 중이면 아무것도 하지 않는다. 그렇지 않으면 streaming 중에도
        // beginScan() 이 재조립기를 초기화하고 스캔을 다시 시작해, 데이터는 계속
        // 오는데 화면은 "Mac 찾는 중…" 에 멈춰 거짓말을 하게 된다.
        guard !wantsRunning else { return }
        wantsRunning = true
        if central == nil {
            central = CBCentralManager(delegate: self, queue: .main)
        } else {
            beginScan()
        }
    }

    public func stop() {
        // 연쇄로 도착할 didDisconnectPeripheral 등이 다시 스캔을 시작하지 않도록
        // 취소보다 먼저 false 로 내려둔다.
        wantsRunning = false
        cancelConnectTimeout()
        central?.stopScan()
        if let p = peripheral {
            central?.cancelPeripheralConnection(p)
        }
        peripheral = nil
        stateSubject.send(.idle)
    }

    /// 스캔을 (다시) 시작하는 유일한 경로. 재연결마다 이전 연결에서 시작된
    /// 미완성 프레임이 새 연결로 새어 들어가지 않도록 여기서 재조립기를 초기화한다.
    private func beginScan() {
        guard let central, central.state == .poweredOn else { return }
        reassembler = FrameReassembler()
        stateSubject.send(.scanning)
        central.scanForPeripherals(withServices: [MirrorUUIDs.service])
    }

    private func cancelConnectTimeout() {
        connectTimeout?.cancel()
        connectTimeout = nil
    }

    /// 연결 요청 후 일정 시간 내에 성공/실패 콜백이 오지 않으면 강제로 취소해
    /// "연결 중…" 에서 영영 못 빠져나오는 경우를 막는다.
    ///
    /// `cancelPeripheralConnection` 이 (아직 didConnect 가 온 적 없는) 대기 중인 연결에
    /// 대해 didDisconnectPeripheral/didFailToConnect 중 무엇을 부르는지는 문서화되어
    /// 있지 않다. 그 콜백에 의존하지 않도록 이 타이머 자체가 상태 정리와 재스캔까지
    /// 전부 끝맺는다 — 이후 콜백이 실제로 오더라도 beginScan() 이 한 번 더 불릴 뿐
    /// 무해하다.
    private func scheduleConnectTimeout(for peripheral: CBPeripheral) {
        cancelConnectTimeout()
        connectTimeout = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 10_000_000_000)
            guard !Task.isCancelled else { return }
            guard let self, self.peripheral === peripheral, self.stateSubject.value == .connecting else { return }
            self.central?.cancelPeripheralConnection(peripheral)
            self.peripheral = nil
            self.stateSubject.send(.disconnected(reason: "연결 시간 초과"))
            self.beginScan()
        }
    }
}

extension BLEClient: @preconcurrency CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn:
            if wantsRunning { beginScan() }
        case .poweredOff:
            // poweredOff/resetting 전환 시 CoreBluetooth 는 didDisconnectPeripheral 를
            // 호출하지 않고 연결을 그냥 무효화한다. 여기서 peripheral 을 비우지 않으면
            // didDiscover 의 "guard self.peripheral == nil" 가 낡은 참조에 막혀 이후
            // 어떤 기기도 다시 채택하지 못하고 영원히 스캔만 하게 된다.
            peripheral = nil
            cancelConnectTimeout()
            stateSubject.send(.bluetoothOff)
        case .unauthorized:
            peripheral = nil
            cancelConnectTimeout()
            stateSubject.send(.disconnected(reason: "블루투스 권한 거부됨"))
        default:
            peripheral = nil
            cancelConnectTimeout()
            stateSubject.send(.disconnected(reason: "블루투스 사용 불가"))
        }
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        // 스캔 중지가 이미 큐에 들어온 콜백을 취소하지 않으므로, 이미 붙잡은 주변기기가
        // 있으면 두 번째 발견을 무시한다. 그렇지 않으면 서로 다른 두 기기의 청크가
        // 같은 FrameReassembler 로 섞여 들어갈 수 있다.
        guard self.peripheral == nil else { return }
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self
        stateSubject.send(.connecting)
        central.connect(peripheral)
        scheduleConnectTimeout(for: peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.discoverServices([MirrorUUIDs.service])
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral peripheral: CBPeripheral,
        error: Error?
    ) {
        // stop() 직후 곧바로 start() 가 호출되면 이전 주변기기의 늦은 콜백이 방금
        // 채택한 새 주변기기의 참조를 지워버릴 수 있다. 다른 기기 얘기면 무시한다.
        if let current = self.peripheral, current !== peripheral { return }
        self.peripheral = nil
        cancelConnectTimeout()
        if wantsRunning {
            stateSubject.send(.disconnected(reason: error?.localizedDescription ?? "Mac 연결 종료"))
            beginScan()
        } else if stateSubject.value != .versionMismatch {
            // stop() 으로 인한 연쇄 콜백이라면 "연결 끊김" 이 아니라 "대기 중" 이어야 한다.
            // 단, 버전 불일치로 우리가 스스로 끊은 경우엔 그 화면을 덮어쓰지 않는다.
            stateSubject.send(.idle)
        }
    }

    public func centralManager(
        _ central: CBCentralManager,
        didFailToConnect peripheral: CBPeripheral,
        error: Error?
    ) {
        if let current = self.peripheral, current !== peripheral { return }
        self.peripheral = nil
        cancelConnectTimeout()
        if wantsRunning {
            stateSubject.send(.disconnected(reason: error?.localizedDescription ?? "연결 실패"))
            beginScan()
        } else {
            stateSubject.send(.idle)
        }
    }
}

extension BLEClient: @preconcurrency CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if let error {
            stateSubject.send(.disconnected(reason: "서비스 검색 실패 · \(error.localizedDescription)"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        guard let service = peripheral.services?.first(where: { $0.uuid == MirrorUUIDs.service }) else {
            stateSubject.send(.disconnected(reason: "미러 서비스를 찾지 못함"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        peripheral.discoverCharacteristics([MirrorUUIDs.snapshot], for: service)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        if let error {
            stateSubject.send(.disconnected(reason: "특성 검색 실패 · \(error.localizedDescription)"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        guard let ch = service.characteristics?.first(where: { $0.uuid == MirrorUUIDs.snapshot }) else {
            stateSubject.send(.disconnected(reason: "Snapshot 특성을 찾지 못함"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        peripheral.setNotifyValue(true, for: ch)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateNotificationStateFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard characteristic.uuid == MirrorUUIDs.snapshot else { return }
        if let error {
            stateSubject.send(.disconnected(reason: "구독 실패 · \(error.localizedDescription)"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        guard characteristic.isNotifying else {
            stateSubject.send(.disconnected(reason: "Snapshot 구독이 비활성 상태"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        // 구독이 실제로 걸린 시점에만 "연결됨" 을 알린다. 미리 알리면 구독이 실패해도
        // 화면은 계속 연결됨으로 남아 데이터가 안 오는 이유를 알 수 없게 된다.
        cancelConnectTimeout()
        stateSubject.send(.streaming)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        // 버전 불일치로 이미 종료를 결정했다면, 정리 중에 큐에 남아 있던 프레임이
        // 뒤늦게 도착해도 재조립/디코딩을 다시 돌리지 않는다.
        guard stateSubject.value != .versionMismatch else { return }
        guard characteristic.uuid == MirrorUUIDs.snapshot, let data = characteristic.value else { return }
        guard let message = reassembler.push(data) else { return }

        if let text = String(data: message, encoding: .utf8) {
            rawSubject.send(text)
        }
        do {
            let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: message)
            guard snap.isSupportedVersion else {
                // 클라이언트가 지원하지 못하는 버전이다. 재시도로는 해결되지 않으므로
                // 스캔을 재개하지 않고 연결을 끊는다. setNotifyValue(false) 는 일부러
                // 부르지 않는다 — cancelPeripheralConnection 이 어차피 구독을
                // 정리하는데, 먼저 부르면 그 완료 콜백이 이 상태를 덮어쓸 수 있다.
                wantsRunning = false
                stateSubject.send(.versionMismatch)
                central?.cancelPeripheralConnection(peripheral)
                return
            }
            snapshotSubject.send(snap)
        } catch {
            // 디코딩 실패는 연결을 끊을 사유가 아니다. 다음 프레임에서 회복될 수 있다.
            NSLog("스냅샷 디코딩 실패: \(error)")
        }
    }
}
