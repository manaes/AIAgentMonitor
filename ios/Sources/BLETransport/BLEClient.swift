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

    /// 이번 연결의 v2 핸드셰이크. 임시 키가 한 번만 쓰이므로 연결마다 새로 만든다.
    private var handshake: V2Handshake?
    /// 방금 보낸 v2 동사. 응답을 이걸로 가른다(`decideV2`).
    private var sentVerb: V2Verb?
    /// 인가 후의 봉인 채널. 이게 nil 인 동안 도착하는 스냅샷은 전부 버린다 —
    /// 평문 스냅샷을 받아주는 것이 곧 다운그레이드다.
    private var channel: SealedChannel?
    /// `AwaitingCode2` 를 받아 transcript 까지 확정된 상태인가. `CODE2` 는 이
    /// 상태에서만 낼 수 있다.
    private var awaitingUserCode = false
    /// 사용자가 낸 코드를 아직 못 쓴 경우 들고 있는다. `HELLO2` 가 거절당한 뒤
    /// (맥 화면의 페어링 창이 아직 안 열렸을 때) 사용자가 창을 열고 코드를
    /// 입력하면, 새 `HELLO2` 로 다시 시작해 그 응답에 이 코드를 바로 낸다.
    private var pendingCode: String?

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
        resetV2State()
        pendingCode = nil
        stateSubject.send(.idle)
    }

    /// 사용자가 페어링 화면에서 6자리 코드를 입력하고 확인을 눌렀을 때 호출한다.
    /// 코드 자체는 링크를 건너지 않는다 — transcript 를 코드로 MAC 한 값만 나간다.
    ///
    /// **v1 과 달리 코드를 곧바로 낼 수 없다.** v1 은 `CODE:` 를 `HELLO` 없이도
    /// 낼 수 있었지만(`pairing.rs: code_without_prior_hello_still_grants`), v2 의
    /// 바인딩은 `HELLO2` 가 만든 transcript 위에서만 계산되고 맥도 핸드셰이크가
    /// 없으면 즉시 거절한다(`pairing.rs: Code2`). 그래서 쓸 수 있는 핸드셰이크가
    /// 없으면 코드를 들고 `HELLO2` 부터 다시 시작한다 — 맥 화면의 창이 연결
    /// 시점에는 닫혀 있다가 사용자가 그 뒤에 여는 것이 정상 순서이므로, 이
    /// 경로가 오히려 흔하다.
    public func submitPairingCode(_ code: String) {
        guard let peripheral, let authCh = authCharacteristic else { return }
        guard awaitingUserCode, let handshake,
              let binding = handshake.codeBinding(code: code) else {
            pendingCode = code
            beginV2Handshake(peripheral, authCh)
            return
        }
        // 핸드셰이크는 맥에서도 CODE2 한 번으로 소비된다 — 틀렸으면 HELLO2 부터다.
        awaitingUserCode = false
        pendingCode = nil
        sentVerb = .code2
        peripheral.writeValue(PairingClient.code2Frame(binding: binding), for: authCh, type: .withResponse)
    }

    /// 새 임시 키로 v2 핸드셰이크를 시작한다.
    ///
    /// **방금 받은 코드가 저장된 토큰보다 우선한다.** 맥에서 전체 해제로 토큰이
    /// 폐기된 뒤에는 `AUTH2` 재인증이 반드시 거부되고, 그때 코드는 쓰이지도 못한
    /// 채 `needsPairing` 으로 떨어진다(v1 에서 실제로 겪은 버그).
    private func beginV2Handshake(_ peripheral: CBPeripheral, _ authCh: CBCharacteristic) {
        let hs = V2Handshake()
        handshake = hs
        channel = nil
        awaitingUserCode = false
        let frame: Data
        if pendingCode != nil || TokenStore.load() == nil {
            sentVerb = .hello2
            frame = PairingClient.hello2Frame(clientPub: hs.clientPub)
        } else {
            // 토큰 자체는 보내지 않는다. 논스를 받아 서명해 답한다.
            sentVerb = .auth2
            frame = PairingClient.auth2Frame(clientPub: hs.clientPub)
        }
        peripheral.writeValue(frame, for: authCh, type: .withResponse)
    }

    /// 연결이 끊기거나 서비스가 무효화될 때 v2 상태를 통째로 버린다. 임시 키도
    /// 세션 카운터도 연결 하나에 매인 값이라 다음 연결로 넘기면 안 된다.
    /// `pendingCode` 는 남긴다 — 사용자가 낸 코드는 재연결 뒤에도 여전히 유효하다.
    private func resetV2State() {
        handshake = nil
        sentVerb = nil
        channel = nil
        awaitingUserCode = false
    }

    /// 스캔을 (다시) 시작하는 유일한 경로. 재연결마다 이전 연결에서 시작된
    /// 미완성 프레임이 새 연결로 새어 들어가지 않도록 여기서 재조립기를 초기화한다.
    private func beginScan() {
        guard let central, central.state == .poweredOn else { return }
        reassembler = FrameReassembler()
        // 임시 키도 세션 카운터도 연결 하나에 매인 값이다. 다음 연결로 넘기면
        // 봉인 채널의 카운터가 맥과 어긋나 스냅샷이 한 장도 안 열린다.
        resetV2State()
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
        // 맥이 서비스를 내렸다 올리면 그쪽 세션도 끝나 있다 — 이쪽 봉인 채널도
        // 버리고 특성 재검색부터 핸드셰이크를 다시 탄다.
        resetV2State()

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
        beginV2Handshake(peripheral, authCh)
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

        // BLE 는 재조립한 프레임이 **곧 봉인 프레임**이다 —
        // `counter(8B BE) || ciphertext || tag(16)` 를 그대로 청킹해 보내기
        // 때문이다(`ble/mod.rs`: 봉인은 청킹 직전에 한다). 네트워크 전송과
        // 달리 hex 도 줄 단위 프레이밍도 끼지 않는다.
        guard let channel else {
            // 세션이 아직 안 열렸는데 스냅샷이 왔다 — 봉인되지 않은 평문이다.
            // 받아주면 그게 다운그레이드다(스펙 8장).
            NSLog("v2 세션 전에 도착한 스냅샷을 버립니다")
            return
        }
        let plaintext: Data
        do {
            plaintext = try channel.open(message)
        } catch {
            // 프레임 하나를 버릴 뿐 연결은 끊지 않는다. 카운터에 빈 칸이 생기는
            // 것은 정상이고(청크가 어긋난 프레임은 재조립기가 버린다) 수신 측은
            // 그걸 견디도록 만들어져 있다 — 다음 프레임에서 회복된다.
            NSLog("봉인 프레임 열기 실패: \(error)")
            return
        }

        if let text = String(data: plaintext, encoding: .utf8) {
            rawSubject.send(text)
        }
        do {
            let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: plaintext)
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

    /// `handleAuthReply` 가 실제로 무엇을 할지 결정하는 순수 함수. `reply.nonce` 를
    /// 가장 먼저 확인해야 한다 — `Nonce` 응답은 `ok:false` 라서 다른 `ok:false` 갈래
    /// (Denied/Rejected/AwaitingCode)와 섞이면 안 된다. `Granted`(최초 인가, 토큰
    /// 저장)와 `Authorized`(재인증 성공, 토큰 없음, 저장 안 함)도 서로 다른 동작이다.
    ///
    /// 이 결정을 `handleAuthReply` 본문에서 분리해둔 이유: 전체 브랜치 리뷰가 찾은
    /// C-1(Rejected 를 HELLO 자동 재전송으로 잘못 연결해 무한 루프를 만든 버그)이
    /// 5번의 개별 태스크 리뷰를 전부 통과한 직접 원인이, 이 분기가 `CBPeripheral`/
    /// `CBCharacteristic` 을 직접 잡고 있어 호스트 없는 로직 테스트로 검증할 수
    /// 없었다는 것이다(`macos.rs` 의 `targets_for`/`without_revoked` 와 같은 계열의
    /// 사각지대). 결정만 뽑아내면 CoreBluetooth 없이 6가지 응답 전부를 고정할 수 있다.
    /// `NetworkClient`(다른 전송)도 같은 인증 상태 기계를 재사용한다 — 이 결정
    /// 자체는 CoreBluetooth 와 무관하므로 `public`으로 노출해 모듈 경계를 넘긴다.
    public enum AuthAction: Equatable {
        /// AUTH 요청에 대한 응답 — 저장된 토큰으로 서명해 되돌려야 한다.
        case signNonce(nonce: String)
        /// 최초 코드 인가 성공 — 토큰을 저장하고 스트리밍을 시작한다.
        case storeTokenAndSubscribe(token: String)
        /// 재인증(PROOF) 성공 — 되돌릴 토큰이 없다(스펙 5.1). 그대로 스트리밍을 시작한다.
        case subscribe
        case failed(left: Int)
        case awaitCode
        /// Rejected — Mac 은 이 응답 하나로 네 가지 경우를 전부 가리킨다: 창이 안
        /// 열려 있는 HELLO, 시도가 소진된 HELLO/CODE, 또는 PROOF 실패. **여기서
        /// HELLO 를 자동으로 다시 쓰면 안 된다** — 창이 안 열려 있으면 재전송도
        /// 다시 Rejected 를 받고, 그게 다시 여기로 와서 또 HELLO 를 쓰는 무한
        /// 루프가 된다(C-1). 사용자가 코드를 제출하면 `submitPairingCode` 가
        /// CODE: 를 직접 쓴다 — HELLO 없이도 통과한다
        /// (pairing.rs: code_without_prior_hello_still_grants).
        case resetAndAwaitCode
    }

    /// v1 상태 기계. **더 이상 이 앱의 연결 경로에 있지 않다** — 클라이언트는
    /// v2 만 말한다(아래 `decideV2` 의 다운그레이드 설명). 그래도 남겨 두는
    /// 이유는 이 함수와 그 여섯 테스트가 맥이 지금도 보내는 여섯 가지 v1 응답
    /// 모양의 정본이기 때문이다 — `decideV2` 의 거절 규칙("`v:2` 없는 `ok:true`
    /// 는 평문 인가다")은 정확히 이 표를 근거로 정의된다. 전환이 끝나 맥이 v1
    /// 응답을 더 이상 만들지 않게 되면 이 함수와 테스트를 함께 지운다.
    public static func decide(_ reply: AuthReplyPayload) -> AuthAction {
        if let nonce = reply.nonce {
            return .signNonce(nonce: nonce)
        } else if reply.ok, let token = reply.token {
            return .storeTokenAndSubscribe(token: token)
        } else if reply.ok {
            return .subscribe
        } else if let left = reply.left {
            return .failed(left: left)
        } else if reply.awaiting == "code" {
            return .awaitCode
        } else {
            return .resetAndAwaitCode
        }
    }

    // MARK: - v2 상태 기계

    /// 방금 보낸 v2 동사. **응답을 이걸로 가른다.**
    ///
    /// `AwaitingCode2` 와 `Nonce2` 는 `await` 하나만 다르고 필드 구성이 같다:
    /// ```
    /// AwaitingCode2 → {"ok":false,"v":2,"await":"code","epk":"…","nonce":"…"}
    /// Nonce2        → {"ok":false,"v":2,            "epk":"…","nonce":"…"}
    /// ```
    /// "epk 와 nonce 가 있으니 X 다" 로 판단하면 페어링 경로와 재연결 경로가
    /// 뒤섞인다 — 그 결과는 크래시도 로그도 없는 빈 화면이다.
    public enum V2Verb: Equatable, Sendable {
        case hello2, code2, auth2, proof2
    }

    public enum V2Action: Equatable, Sendable {
        /// `AwaitingCode2` — 합의하고 사용자 코드로 바인딩을 만들어 `CODE2` 를 낸다.
        case bindCode(epk: String, nonce: String)
        /// `Nonce2` — 합의하고 저장된 토큰으로 증명을 만들어 `PROOF2` 를 낸다.
        case signSessionProof(epk: String, nonce: String)
        /// `Granted2` — 봉인된 토큰을 열어 저장하고 세션을 연다.
        case openSealedToken(sealed: String)
        /// `Authorized2` — 되돌아온 토큰이 없다. 이미 저장된 토큰으로 세션을 연다.
        case openSession
        /// `Denied` — 코드가 틀렸다. 남은 시도를 그대로 보여준다.
        case failed(left: Int)
        /// `Rejected`, 또는 이 동사에 올 수 없는 응답. **재시도하지 않고 멈춘다.**
        case needsPairing
    }

    /// v2 응답 하나를 다음 행동으로 옮긴다.
    ///
    /// **다운그레이드하지 않는다.** 이 규칙의 실질은 딱 한 줄이다 — `"v":2` 없는
    /// `ok:true` 를 인가로 받아들이지 않는 것. `{"ok":true}` 와
    /// `{"ok":true,"token":…}` 는 v1 이 인가를 알리는 두 모양이고, 이걸 성공으로
    /// 읽으면 그 뒤 스냅샷을 평문으로 받게 된다. 공격자가 v2 를 방해해 평문으로
    /// 끌어내리는 경로가 정확히 여기다(스펙 8장). 거절당하면 `needsPairing` 으로
    /// 멈출 뿐 v1 프레임은 만들지 않는다.
    ///
    /// 반대로 `"v":2` 를 **모든** 응답에 요구해서도 안 된다. 맥은 `Denied` 와
    /// `Rejected` 를 두 세대에 그대로 쓰므로(`to_json_bytes`) v2 흐름에서도 이
    /// 두 모양이 `"v"` 없이 도착한다 — 전부 `needsPairing` 으로 뭉개면 사용자는
    /// 코드가 틀렸다는 사실도, 몇 번 남았는지도 알 수 없다.
    public static func decideV2(sent: V2Verb, reply: AuthReplyPayload) -> V2Action {
        if reply.ok {
            // 인가만은 v2 임이 증명돼야 한다.
            guard reply.v == 2 else { return .needsPairing }
            switch sent {
            case .code2:
                guard let sealed = reply.sealed else { return .needsPairing }
                return .openSealedToken(sealed: sealed)
            case .proof2:
                return .openSession
            case .hello2, .auth2:
                // 맥은 이 두 동사에 성공을 돌려주지 않는다.
                return .needsPairing
            }
        }
        if let left = reply.left { return .failed(left: left) }
        guard reply.v == 2, let epk = reply.epk, let nonce = reply.nonce else {
            return .needsPairing
        }
        switch sent {
        case .hello2: return .bindCode(epk: epk, nonce: nonce)
        case .auth2: return .signSessionProof(epk: epk, nonce: nonce)
        case .code2, .proof2: return .needsPairing
        }
    }

    private func handleAuthReply(_ data: Data) {
        guard let peripheral, let authCh = authCharacteristic,
              let snapshotCh = snapshotCharacteristic,
              let handshake, let sentVerb else { return }
        guard let reply = PairingClient.parse(data) else {
            // v2 응답은 v1 보다 훨씬 길다(`AwaitingCode2` 가 148바이트). Auth 특성은
            // 청킹 없이 notify 한 장으로 나가므로 MTU 협상이 낮게 끝나면 여기서
            // 잘린 JSON 이 도착한다 — 조용히 return 하면 화면이 이유 없이 멈춘다.
            NSLog("Auth 응답을 해석하지 못했습니다(\(data.count)바이트)")
            return
        }

        switch Self.decideV2(sent: sentVerb, reply: reply) {
        case .bindCode(let epk, let nonce):
            guard handshake.agree(epkHex: epk, nonceHex: nonce) else {
                // 저차 점이거나 형식이 깨진 epk — 재시도해도 같은 맥이면 같은
                // 결과다. resetAndAwaitCode 와 같은 이유로 자동 재전송은 없다.
                failV2()
                return
            }
            guard let code = pendingCode, let binding = handshake.codeBinding(code: code) else {
                // 아직 코드가 없다. 사용자가 맥 화면의 6자리를 입력하면
                // `submitPairingCode` 가 이 핸드셰이크 위에서 CODE2 를 낸다.
                awaitingUserCode = true
                stateSubject.send(.needsPairing)
                return
            }
            pendingCode = nil
            self.sentVerb = .code2
            peripheral.writeValue(PairingClient.code2Frame(binding: binding), for: authCh, type: .withResponse)
        case .signSessionProof(let epk, let nonce):
            guard handshake.agree(epkHex: epk, nonceHex: nonce),
                  let token = TokenStore.load(),
                  let proof = handshake.sessionProof(tokenHex: token) else {
                // 토큰이 없거나 증명을 못 만들었다 — 코드 페어링으로 되돌아간다.
                failV2()
                return
            }
            self.sentVerb = .proof2
            peripheral.writeValue(PairingClient.proof2Frame(proof: proof), for: authCh, type: .withResponse)
        case .openSealedToken(let sealed):
            guard let token = handshake.openSealedToken(sealedHex: sealed),
                  let channel = handshake.sessionChannel(tokenHex: token) else {
                // 봉인이 안 열렸다는 건 우리가 만든 키가 맥의 키와 다르다는 뜻이다.
                failV2()
                return
            }
            // 저장이 실패해도(디스크 꽉 참 등) 스트리밍은 계속한다 — 지금 세션은
            // 인가된 상태다. 다만 다음 재연결부터는 저장된 토큰이 없어 코드를
            // 다시 요구하게 되므로 로그를 남긴다(TokenStore.save 가 이제
            // SecItemAdd 결과를 그대로 돌려준다, Task 6 리뷰 반영).
            if !TokenStore.save(token) {
                NSLog("페어링 토큰 저장 실패 — 다음 재연결부터 코드를 다시 요구합니다")
            }
            // 채널을 먼저 세운다 — 구독보다 늦으면 첫 스냅샷이 채널 없이 도착해
            // 버려진다.
            self.channel = channel
            self.sentVerb = nil
            peripheral.setNotifyValue(true, for: snapshotCh)   // 여기서 비로소 데이터가 흐른다
        case .openSession:
            // 재인증 성공 — 되돌아온 토큰이 없다(스펙 5.1). 이미 저장된 토큰으로
            // 세션 키를 만든다.
            guard let token = TokenStore.load(),
                  let channel = handshake.sessionChannel(tokenHex: token) else {
                failV2()
                return
            }
            self.channel = channel
            self.sentVerb = nil
            peripheral.setNotifyValue(true, for: snapshotCh)
        case .failed(let left):
            // 맥은 CODE2 하나로 핸드셰이크를 소비했다 — 다시 넣으려면 HELLO2
            // 부터다. `submitPairingCode` 가 그 판단을 한다.
            awaitingUserCode = false
            pendingCode = nil
            self.sentVerb = nil
            stateSubject.send(.pairingFailed(left: left))
        case .needsPairing:
            failV2()
        }
    }

    /// v2 인증이 더 진행될 수 없을 때의 유일한 종착점. **여기서 프레임을 다시
    /// 쓰지 않는다** — 창이 안 열려 있으면 재전송도 다시 거절당하고, 그게 다시
    /// 여기로 와 무한 루프가 된다(v1 에서 겪은 C-1). v1 로 물러서지도 않는다.
    /// 사용자가 코드를 내면 `submitPairingCode` 가 HELLO2 부터 다시 시작한다.
    private func failV2() {
        TokenStore.clear()
        awaitingUserCode = false
        sentVerb = nil
        // 들고 있던 코드도 버린다. 여기까지 왔다는 건 창이 닫혔거나 만료됐다는
        // 뜻이고(HELLO2 거절), 맥의 코드는 120초짜리라 나중에 쓰면 어차피 틀린다.
        pendingCode = nil
        stateSubject.send(.needsPairing)
    }
}
