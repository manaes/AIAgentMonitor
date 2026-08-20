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
    /// 인가 후에야 구독한다 — 그 전에 구독하면 미인가 상태에서도 데이터가 요청되는
    /// 모양이 되어 스펙 5.1 의 "인가된 central 에만 notify" 전제와 어긋난다.
    private var snapshotCharacteristic: CBCharacteristic?
    /// HELLO/CODE/AUTH/PROOF 쓰기와 재연결에 계속 쓴다.
    private var authCharacteristic: CBCharacteristic?
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

    /// 사용자가 페어링 화면에서 6자리 코드를 입력하고 확인을 눌렀을 때 호출한다.
    /// 코드 자체는 이 파일이 만들거나 검증하지 않는다 — 이미 열려 있는 Mac 쪽 창에
    /// 제출할 뿐이다(창을 여는 것은 여전히 Mac 사용자 제스처 전용이다, 스펙 5.1).
    public func submitPairingCode(_ code: String) {
        guard let peripheral, let authCh = authCharacteristic else { return }
        peripheral.writeValue(PairingClient.codeFrame(code), for: authCh, type: .withResponse)
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
    /// Mac 이 stop() 에서 removeAllServices() 를 부르더라도 LE 링크는 끊기지 않는다 —
    /// CBPeripheralManager 에는 central 연결을 끊는 API 가 없기 때문이다. 대신 GATT
    /// Service Changed 표시가 도착해 CoreBluetooth 가 서비스를 무효화한다.
    /// 이 콜백이 없으면 앱은 무효가 된 CBCharacteristic 을 쥔 채 .streaming 에 머물고,
    /// Mac 이 공유를 다시 켜 서비스를 올려도 재검색·재구독을 하지 않는다. Mac 쪽 구독자
    /// 목록이 영원히 비어 데이터가 흐르지 않는데 화면만 "연결됨" 인 거짓말이 된다.
    public func peripheral(_ peripheral: CBPeripheral, didModifyServices invalidated: [CBService]) {
        guard invalidated.contains(where: { $0.uuid == MirrorUUIDs.service }) else { return }
        // 이미 놓아준 주변기기의 늦은 콜백이 현재 연결을 건드리지 않게 한다.
        guard self.peripheral === peripheral else { return }

        // 무효화 시점에 절반만 도착해 있던 프레임의 잔여 청크가, 재구독 뒤 새 프레임의
        // 첫 청크와 이어붙지 않도록 여기서 버린다. didUpdateValueFor 에 이르는 모든
        // 경로는 그 앞에서 재조립기를 초기화한다는 규약을 이 새 경로도 지킨다.
        reassembler = FrameReassembler()

        // 재검색 → 특성 검색 → 재구독 체인을 처음부터 다시 타야 하므로 아직 스트리밍이
        // 아니다. .connecting 으로 내려두면 라벨이 정직해질 뿐 아니라, 체인이 영영
        // 끝나지 않는 경우(사용자가 Mac 공유를 계속 꺼둔 경우)를 connectTimeout 이 잡아
        // 연결을 정리하고 재스캔으로 돌려보낸다.
        stateSubject.send(.connecting)
        scheduleConnectTimeout(for: peripheral)
        peripheral.discoverServices([MirrorUUIDs.service])
    }

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
        peripheral.discoverCharacteristics([MirrorUUIDs.snapshot, MirrorUUIDs.auth], for: service)
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
        guard let snapshotCh = service.characteristics?.first(where: { $0.uuid == MirrorUUIDs.snapshot }),
              let authCh = service.characteristics?.first(where: { $0.uuid == MirrorUUIDs.auth }) else {
            stateSubject.send(.disconnected(reason: "필수 특성을 찾지 못함"))
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        snapshotCharacteristic = snapshotCh
        authCharacteristic = authCh
        peripheral.setNotifyValue(true, for: authCh)
        if let token = TokenStore.load() {
            // 토큰 자체는 보내지 않는다. 논스를 받아 서명해 답한다.
            peripheral.writeValue(PairingClient.authFrame(), for: authCh, type: .withResponse)
        } else {
            peripheral.writeValue(PairingClient.helloFrame(), for: authCh, type: .withResponse)
        }
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateNotificationStateFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        if characteristic.uuid == MirrorUUIDs.auth {
            if let error {
                NSLog("Auth 구독 실패: \(error)")
            }
            return
        }
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
        if characteristic.uuid == MirrorUUIDs.auth {
            handleAuthReply(characteristic.value ?? Data())
            return
        }
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

    /// Auth 특성 notify 를 해석한다. `reply.nonce` 를 가장 먼저 확인해야 한다 —
    /// `Nonce` 응답은 `ok:false` 라서 다른 `ok:false` 갈래(Denied/Rejected/
    /// AwaitingCode)와 섞이면 안 된다. `Granted`(최초 인가, 토큰 저장)와
    /// `Authorized`(재인증 성공, 토큰 없음, 저장 안 함)도 서로 다른 동작이다.
    private func handleAuthReply(_ data: Data) {
        guard let peripheral, let authCh = authCharacteristic,
              let snapshotCh = snapshotCharacteristic,
              let reply = PairingClient.parse(data) else { return }

        if let nonce = reply.nonce {
            // AUTH 요청에 대한 응답 — 저장된 토큰으로 서명해 되돌린다.
            guard let token = TokenStore.load(),
                  let proof = PairingClient.proofFrame(token: token, nonce: nonce) else {
                // 토큰이 없거나 서명을 못 만들었다 — 코드 페어링으로 되돌아간다.
                // 여기서 HELLO 를 자동으로 다시 쓰지 않는다(전체 브랜치 리뷰 C-1) —
                // 창이 안 열려 있으면 Mac 은 이것도 Rejected 로 답하고, 아래 else
                // 갈래가 그걸 또 HELLO 로 되돌리면 연결이 끊길 때까지 멈추지 않는
                // write/notify 루프가 된다. 사용자가 [확인] 으로 코드를 제출하면
                // submitPairingCode 가 CODE: 를 직접 쓴다 — HELLO 없이도 통과한다
                // (pairing.rs: code_without_prior_hello_still_grants).
                TokenStore.clear()
                stateSubject.send(.needsPairing)
                return
            }
            peripheral.writeValue(proof, for: authCh, type: .withResponse)
        } else if reply.ok, let token = reply.token {
            // 최초 코드 인가 성공. 저장이 실패해도(디스크 꽉 참 등) 스트리밍은
            // 계속한다 — 지금 세션은 인가된 상태다. 다만 다음 재연결부터는 저장된
            // 토큰이 없어 코드를 다시 요구하게 되므로 로그를 남긴다(TokenStore.save
            // 가 이제 SecItemAdd 결과를 그대로 돌려준다, Task 6 리뷰 반영).
            if !TokenStore.save(token) {
                NSLog("페어링 토큰 저장 실패 — 다음 재연결부터 코드를 다시 요구합니다")
            }
            peripheral.setNotifyValue(true, for: snapshotCh)   // 여기서 비로소 데이터가 흐른다
        } else if reply.ok {
            // 재인증(PROOF) 성공 — 되돌릴 토큰이 없다(스펙 5.1). 이미 Keychain 에 있는 걸 쓴다.
            peripheral.setNotifyValue(true, for: snapshotCh)
        } else if let left = reply.left {
            stateSubject.send(.pairingFailed(left: left))
        } else if reply.awaiting == "code" {
            stateSubject.send(.needsPairing)
        } else {
            // Rejected — Mac 은 이 응답 하나로 네 가지 경우를 전부 가리킨다: 창이 안
            // 열려 있는 HELLO, 시도가 소진된 HELLO/CODE, 또는 PROOF 실패. 어느
            // 경우든 토큰을 지우고 코드 페어링을 기다린다. **여기서 HELLO 를 자동으로
            // 다시 쓰지 않는다** — 창이 안 열려 있으면 재전송도 다시 Rejected 를
            // 받고, 그게 다시 이 갈래로 와서 또 HELLO 를 쓰는 무한 루프가 된다
            // (전체 브랜치 리뷰 C-1 — 실제로 확인된 결함이었다). 사용자가 코드를
            // 제출하면 submitPairingCode 가 CODE: 를 직접 쓴다.
            TokenStore.clear()
            stateSubject.send(.needsPairing)
        }
    }
}
