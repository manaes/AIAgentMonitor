import BLETransport
import Combine
import Foundation
import IrohLib
import Wire

/// 토큰 전역 슬롯 접근의 이음매. 프로덕션은 Keychain(`NetworkTokenStore`) 그대로다.
/// 테스트가 "`.ephemeral`/`.fixed` 는 전역 슬롯을 읽지도 쓰지도 지우지도 않는다" 를
/// **관측**할 수 있어야 한다 — 값만 보면 `.ephemeral` 이 전역에 써도 load 가 nil 이라 통과해버린다.
protocol SharedTokenStoring {
    func load() -> String?
    func save(_ token: String) -> Bool
    func clear()
}

/// 프로덕션 구현. 기존 호출을 그대로 포워딩할 뿐 동작을 바꾸지 않는다.
struct KeychainSharedTokenStore: SharedTokenStoring {
    func load() -> String? { NetworkTokenStore.loadToken() }
    func save(_ token: String) -> Bool { NetworkTokenStore.saveToken(token) }
    func clear() { NetworkTokenStore.clearToken() }
}

/// 전역 슬롯 구현체의 주입 지점. 바꾸는 쪽은 테스트뿐이고, `TokenSlot` 은 메인 액터 밖에서도
/// 불리므로 `nonisolated(unsafe)` 로 둔다. setter 는 이 파일 밖으로 나가지 않는다 —
/// 모듈 어디서나 대입할 수 있으면 프로덕션 코드가 실수로 갈아끼울 수 있다.
nonisolated(unsafe) private(set) var sharedTokenStore: SharedTokenStoring = KeychainSharedTokenStore()

extension NetworkClient {
    /// 테스트 전용. 프로덕션 코드가 이걸 부르면 1:1 앱의 토큰 저장소가 통째로 바뀐다.
    /// 부르는 쪽은 반드시 `defer` 로 원래 저장소를 되돌린다.
    nonisolated static func _setSharedTokenStoreForTesting(_ store: SharedTokenStoring) {
        sharedTokenStore = store
    }
}

/// iroh(QUIC) 기반 미러 전송. `BLEClient` 와 같은 `MirrorTransport` 모양을 갖지만
/// GATT 대신 QR 로 전달받은 `EndpointId` 로 직접 dial 한다. 페어링 인증 프로토콜은
/// `BLEClient.decide`/`PairingClient` 를 그대로 재사용한다 — 전송만 다를 뿐 Mac
/// 쪽 `pairing.rs` 상태 기계는 두 전송이 동일하게 취급한다.
@MainActor
public final class NetworkClient: NSObject {
    /// `IrohEndpointProvider` 도 같은 값으로 bind 해야 하므로 모듈 내부에 공개한다.
    /// `nonisolated` 인 이유: 이 타입은 `@MainActor` 라 static 도 메인 액터에 격리되는데,
    /// `IrohEndpointProvider`(별도 액터)가 bind 할 때 이 값을 읽는다. 격리된 채로 두면
    /// Swift 6 언어 모드에서 컴파일 에러다. 둘 다 상수라 격리가 필요 없다.
    nonisolated static let alpnData = Data("aim/mirror/1".utf8)
    nonisolated private static var alpn: Data { alpnData }
    /// 제어 메시지 하나의 최대 크기. 실제 응답은 수십 바이트 수준이라 넉넉히 잡는다.
    private static let controlSizeLimit: UInt32 = 4096
    private static let snapshotChunkSizeLimit: UInt32 = 65536

    private var endpoint: Endpoint?
    /// 주입되면 Endpoint 를 여기서 받아 쓴다(여러 장치가 공유). nil 이면 기존처럼
    /// 자기 것을 만든다 — 기존 App/AppBLE 의 동작을 바꾸지 않기 위한 기본값이다.
    private let endpointProvider: IrohEndpointProvider?
    private var wantsRunning = false
    private var runTask: Task<Void, Never>?

    private let stateSubject = CurrentValueSubject<ConnectionState, Never>(.idle)
    private let snapshotSubject = PassthroughSubject<MirrorSnapshot, Never>()

    public var state: AnyPublisher<ConnectionState, Never> { stateSubject.eraseToAnyPublisher() }
    public var snapshots: AnyPublisher<MirrorSnapshot, Never> { snapshotSubject.eraseToAnyPublisher() }

    public init(endpointProvider: IrohEndpointProvider? = nil) {
        self.endpointProvider = endpointProvider
        super.init()
    }

    /// 저장된 EndpointId 가 있으면 QR 을 다시 스캔하지 않고 바로 연결을 시도한다.
    /// 없으면 `needsPairing` 으로 화면이 QR 스캐너를 띄우게 한다.
    public func start() {
        guard !wantsRunning else { return }
        guard let endpointHex = NetworkTokenStore.loadEndpointIdHex() else {
            stateSubject.send(.needsPairing)
            return
        }
        let relayUrl = NetworkTokenStore.loadRelayUrl()
        let addresses = NetworkTokenStore.loadAddresses()
        beginConnecting(endpointIdHex: endpointHex, relayUrl: relayUrl, addresses: addresses, code: nil)
    }

    /// 저장된 페어링 정보를 지우고 QR 스캐너가 다시 뜨게 한다. 설정에서
    /// "네트워크"를 고를 때마다 호출한다 — `start()` 처럼 이전 연결 정보로
    /// 조용히 재연결을 시도하면 카메라 화면이 아예 안 뜬다(사용자 확인).
    public func resetPairing() {
        wantsRunning = false
        runTask?.cancel()
        runTask = nil
        NetworkTokenStore.clearAll()
        stateSubject.send(.needsPairing)
    }

    public func stop() {
        wantsRunning = false
        runTask?.cancel()
        runTask = nil
        stateSubject.send(.idle)
    }

    /// QR 스캐너가 디코딩한 문자열
    /// (`aim://pair?endpoint=<hex>&code=<code>&relay=<hex>&addr=<hex>...`)을 넘긴다.
    /// 스캔 한 번으로 dial 과 `CODE:` 제출이 자동으로 끝난다 — 사용자가 코드를 따로
    /// 입력할 필요가 없다(설계 결정, 계획 문서 참고).
    public func pair(qrPayload: String) {
        guard let parsed = Self.parseQrPayload(qrPayload) else {
            stateSubject.send(.disconnected(reason: "QR 코드를 인식하지 못했습니다"))
            return
        }
        NetworkTokenStore.saveEndpointIdHex(parsed.endpointIdHex)
        NetworkTokenStore.saveRelayUrl(parsed.relayUrl)
        NetworkTokenStore.saveAddresses(parsed.addresses)
        beginConnecting(
            endpointIdHex: parsed.endpointIdHex,
            relayUrl: parsed.relayUrl,
            addresses: parsed.addresses,
            code: parsed.code
        )
    }

    /// 위젯 전용. probe 로 연결을 열고 첫 스냅샷만 받은 뒤 **즉시 닫는다**.
    /// `runConnection`(스트리밍, 실패 시 3초 후 무한 재시도)과 달리
    /// **재시도하지 않는다** — 실패하면 그대로 던지고, 호출부
    /// (TimelineProvider/AppIntent)가 캐시 폴백을 결정한다. 이 인스턴스의
    /// `state`/`snapshots` 퍼블리셔는 건드리지 않는다 — 위젯 프로세스는
    /// 화면에 붙이지 않으므로 구독자가 없다.
    public func fetchSnapshotOnce(timeoutSeconds: Double) async throws -> MirrorSnapshot {
        guard let endpointHex = NetworkTokenStore.loadEndpointIdHex() else {
            throw NetworkClientError.needsPairing
        }
        let result = try await probe(
            endpointIdHex: endpointHex,
            relayUrl: NetworkTokenStore.loadRelayUrl(),
            addresses: NetworkTokenStore.loadAddresses(),
            timeoutSeconds: timeoutSeconds
        )
        // 스냅샷은 이미 받았다 — 닫기 실패가 위젯 결과에 영향을 주면 안 된다.
        try? result.connection.close(errorCode: 0, reason: Data())
        return result.firstSnapshot
    }

    /// probe 결과. 연결을 **닫지 않고** 돌려준다 — 호출부가 그대로 스트리밍으로
    /// 이어가거나(AppMulti), 즉시 닫는다(위젯).
    public struct ProbeResult {
        public let connection: Connection
        public let channel: SealedChannel
        public let firstSnapshot: MirrorSnapshot
        /// probe 가 **이미 연** 수신 스트림에서 다음 덩어리를 읽는다.
        ///
        /// 맥은 연결당 uni 스트림을 **하나만** 연다. probe 가 그걸 받아 첫 스냅샷을 읽고도
        /// 스트림을 넘겨주지 않으면, 이어받는 쪽이 `acceptUni()` 를 다시 불러 **오지 않을
        /// 두 번째 스트림을 영원히 기다린다** — 첫 스냅샷만 찍히고 tok/s 가 멈춘 채로
        /// 화면이 얼어붙는다(실기에서 관찰).
        let readChunk: () async throws -> Data
        /// 첫 스냅샷 줄을 떼어내고 남은 바이트. 버리면 다음 줄의 앞부분이 잘려
        /// 그 스냅샷 하나를 통째로 잃는다.
        let pendingBuffer: Data
        /// 이번 인증에서 Mac 이 **새로 발급한** 토큰. 코드로 페어링했을 때(`.openSealedToken`)만
        /// non-nil 이다. 토큰으로 재연결한 경우엔 nil — 프로토콜상 재연결 경로엔 토큰 회전이 없다.
        /// AppMulti 는 이 값을 `Device.token` 에 저장한다(QR 의 `code` 는 6자리 페어링 코드일 뿐
        /// 토큰이 아니라서, 그걸 저장하면 재연결이 항상 needsPairing 으로 거부된다).
        public let issuedToken: String?
    }

    /// dial → 인증 → 첫 스냅샷까지 하고 **연결을 살려둔 채** 반환한다.
    /// 실패 시 재시도하지 않는다 — 재시도 정책은 호출부(DeviceSession)가 정한다.
    ///
    /// `token` 을 넘기면 그 토큰 하나로만 인증하고 전역 Keychain 슬롯은 읽지도 지우지도
    /// 않는다(AppMulti 는 장치마다 토큰이 다르다). 토큰이 없고 `code` 만 있으면 **코드 페어링**
    /// 이다 — 발급된 토큰은 어디에도 저장하지 않고 `ProbeResult.issuedToken` 으로 돌려준다.
    /// 둘 다 nil 이면 기존처럼 전역 슬롯을 쓴다 — 위젯·1:1 앱의 호출부는 수정 없이 동작한다.
    public func probe(
        endpointIdHex: String,
        relayUrl: String?,
        addresses: [String],
        timeoutSeconds: Double,
        token: String? = nil,
        code: String? = nil
    ) async throws -> ProbeResult {
        // 토큰이 코드를 이긴다 — 이미 페어링된 장치는 재연결 경로가 맞다.
        let tokens: TokenSlot = token.map(TokenSlot.fixed) ?? (code != nil ? .ephemeral : .shared)
        let work = Task { () throws -> ProbeResult in
            try await dialAuthenticateAndOpen(
                endpointIdHex: endpointIdHex, relayUrl: relayUrl, addresses: addresses,
                tokens: tokens, code: code
            )
        }
        let watchdog = Task {
            try? await Task.sleep(nanoseconds: UInt64(timeoutSeconds * 1_000_000_000))
            work.cancel()
        }
        defer { watchdog.cancel() }
        do {
            return try await work.value
        } catch is CancellationError {
            throw NetworkClientError.fetchTimedOut
        }
    }

    /// `endpointProvider` 가 주입돼 있으면 공유 Endpoint 를 빌려 쓰고, 없으면
    /// 기존처럼 이 인스턴스 전용 Endpoint 를 새로 bind 한다.
    private func resolveEndpoint() async throws -> Endpoint {
        if let endpointProvider { return try await endpointProvider.endpoint() }
        let builder = EndpointBuilder()
        builder.applyN0()
        builder.alpns(alpns: [Self.alpn])
        return try await builder.bind()
    }

    /// dial → 인증 → 첫 스냅샷까지 하고 **연결을 살려둔 채** 반환한다.
    /// `runConnection`/`listenForSnapshots`와 같은 저수준 조각(`authenticate`,
    /// `classifyLine`)을 재사용하되, 무한 루프 대신 **첫 유효 프레임 하나**를
    /// 받으면 연결을 닫지 않고 바로 돌려준다 — 호출부가 그대로 스트리밍을
    /// 이어가거나(AppMulti), 즉시 닫는다(위젯, `fetchSnapshotOnce`).
    private func dialAuthenticateAndOpen(
        endpointIdHex: String, relayUrl: String?, addresses: [String], tokens: TokenSlot,
        code: String? = nil
    ) async throws -> ProbeResult {
        guard let idBytes = Data(hexString: endpointIdHex) else {
            throw NetworkClientError.needsPairing
        }
        let endpointId = try EndpointId.fromBytes(bytes: idBytes)
        let addr = EndpointAddr(id: endpointId, relayUrl: relayUrl, addresses: addresses)

        let ep = try await resolveEndpoint()
        let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
        // 연결이 열린 뒤로는 어떤 경로로 실패해도 연결을 닫고 나가야 한다. 인증 거부·버전
        // 불일치는 장치마다 **반드시** 이 경로를 밟으므로, 안 닫으면 16대에서 실패가
        // 장치 수만큼 쌓인다. 성공 경로는 연결을 살려둔 채 반환하므로 여기서 닫지 않는다.
        do {
            // code 가 nil 이면 재연결 경로(이미 저장된 토큰으로 인증), non-nil 이면 QR 로 받은
            // 6자리 코드로 새로 페어링하는 경로다.
            let (channel, issuedToken) = try await authenticate(conn: conn, code: code, tokens: tokens)

            let recv = try await conn.acceptUni()
            var buffer = Data()
            while true {
                // 워치독이 cancel() 을 걸어도 uniffi 브리지가 자체적으로 취소를 감지한다는
                // 보장이 없다 — 매 반복 최소 한 번은 취소 지점을 만들어 무한정 도는 걸 막는다.
                try Task.checkCancellation()
                let chunk = try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit)
                buffer.append(chunk)
                while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                    let lineData = Data(buffer[..<newlineIndex])
                    buffer.removeSubrange(buffer.startIndex...newlineIndex)
                    if let snapshot = try decodeSnapshotLine(lineData, channel: channel) {
                        return ProbeResult(
                            connection: conn, channel: channel, firstSnapshot: snapshot,
                            readChunk: { try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit) },
                            pendingBuffer: buffer,
                            issuedToken: issuedToken
                        )
                    }
                }
            }
        } catch {
            try? conn.close(errorCode: 0, reason: Data())
            throw error
        }
    }

    /// 스냅샷 스트림 한 줄을 복호화·디코딩한다. 유효한 스냅샷이 아니면(다른
    /// 형식의 줄, 열기 실패, 디코딩 실패) `nil` 을 돌려줘 호출부가 다음 줄로
    /// 넘어가게 한다 — 지원하지 않는 버전만 예외로 던진다.
    private func decodeSnapshotLine(_ line: Data, channel: SealedChannel) throws -> MirrorSnapshot? {
        guard case .sealed(let frame) = Self.classifyLine(line) else { return nil }
        guard let plaintext = try? channel.open(frame) else { return nil }
        guard let snap = try? JSONDecoder().decode(MirrorSnapshot.self, from: plaintext) else { return nil }
        guard snap.isSupportedVersion else { throw NetworkClientError.versionMismatch }
        return snap
    }

    /// QR 로 받은 페어링 정보. 이름은 표시용이라 optional 이다 — 구버전 Mac 은 보내지 않는다.
    public struct ParsedPairingPayload: Equatable, Sendable {
        public let endpointIdHex: String
        public let code: String
        public let relayUrl: String?
        public let addresses: [String]
        public let macName: String?
    }

    nonisolated public static func parseQrPayload(_ payload: String) -> ParsedPairingPayload? {
        guard let components = URLComponents(string: payload),
              components.scheme == "aim", components.host == "pair",
              let items = components.queryItems,
              let endpointIdHex = items.first(where: { $0.name == "endpoint" })?.value,
              let code = items.first(where: { $0.name == "code" })?.value else {
            return nil
        }
        // relay/addr/name 은 Rust 쪽에서 URL 인코딩을 피하려고 hex 로 실어 보낸다
        // (Data(hexString:) 는 이 파일이 이미 BLE 쪽에서 재사용하고 있다).
        func decodeHex(_ value: String?) -> String? {
            guard let value, let data = Data(hexString: value) else { return nil }
            return String(data: data, encoding: .utf8)
        }
        return ParsedPairingPayload(
            endpointIdHex: endpointIdHex,
            code: code,
            relayUrl: decodeHex(items.first(where: { $0.name == "relay" })?.value),
            addresses: items.filter { $0.name == "addr" }.compactMap { decodeHex($0.value) },
            macName: decodeHex(items.first(where: { $0.name == "name" })?.value)
        )
    }

    private func beginConnecting(endpointIdHex: String, relayUrl: String?, addresses: [String], code: String?) {
        wantsRunning = true
        runTask?.cancel()
        runTask = Task { [weak self] in
            await self?.runConnection(endpointIdHex: endpointIdHex, relayUrl: relayUrl, addresses: addresses, code: code)
        }
    }

    private func runConnection(endpointIdHex: String, relayUrl: String?, addresses: [String], code: String?) async {
        stateSubject.send(.connecting)
        do {
            guard let idBytes = Data(hexString: endpointIdHex) else {
                stateSubject.send(.disconnected(reason: "잘못된 페어링 정보"))
                return
            }
            let endpointId = try EndpointId.fromBytes(bytes: idBytes)
            // EndpointId 만으로는 discovery 에 Mac 이 등록돼 있지 않아 dial 이 안 된다
            // (실기기에서 `IrohError: no addressing information` 로 확인) — QR 에
            // 같이 실려온 relay/direct 주소를 그대로 넣어준다.
            let addr = EndpointAddr(id: endpointId, relayUrl: relayUrl, addresses: addresses)

            let ep = try await resolveEndpoint()
            // 공유 Endpoint 를 받은 경우에는 보관하지 않는다 — 이 인스턴스가
            // 소유한 게 아니라서 정리(teardown)할 권한이 없기 때문이다.
            if endpointProvider == nil { endpoint = ep }

            let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
            // 1:1 앱은 예전 그대로 전역 Keychain 슬롯 하나를 쓴다.
            // 1:1 앱은 발급 토큰을 전역 슬롯(.shared)에 그대로 저장하므로 반환값의 채널만 쓴다.
            let channel = try await authenticate(conn: conn, code: code, tokens: .shared).channel
            guard wantsRunning else { return }
            try await listenForSnapshots(conn: conn, channel: channel)
        } catch NetworkClientError.needsPairing, NetworkClientError.authFailed {
            // 사용자가 QR 을 다시 스캔해야 풀리는 상태다. 이미 needsPairing/
            // pairingFailed 를 보냈으니 그 화면을 그대로 두고 멈춘다.
            //
            // 여기서 재시도하면 안 된다 — 재시도는 code 가 nil 이라 코드 없이는
            // 절대 통과할 수 없고, 그때마다 .disconnected 를 보내 QR 스캐너가
            // 떴다 사라졌다를 반복한다(MirrorViewController 는 needsPairing 에
            // 스캐너를 띄우고 disconnected 에 닫는다). 성공할 수 없는 재시도로
            // 화면만 깜빡이던 버그였다.
            wantsRunning = false
        } catch {
            guard wantsRunning else { return }
            stateSubject.send(.disconnected(reason: "\(error)"))
            // BLE 의 beginScan() 재시도 루프와 같은 목적 — 잠깐 쉬었다가 저장된
            // EndpointId 로 재연결을 시도한다(사용자가 QR 을 또 스캔할 필요 없음).
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            guard wantsRunning else { return }
            beginConnecting(endpointIdHex: endpointIdHex, relayUrl: relayUrl, addresses: addresses, code: nil)
        }
    }

    /// 인증에 쓸 토큰의 출처. `.shared` 는 1:1 앱·위젯의 전역 Keychain 슬롯(기존 동작 그대로),
    /// `.fixed` 는 호출부가 넘긴 토큰 하나 — AppMulti 는 장치마다 다른 토큰을 레지스트리에 들고 있고,
    /// 전역 슬롯을 읽거나 지우면 1:1 앱의 페어링을 망가뜨리므로 저장/삭제가 모두 no-op 이다.
    enum TokenSlot {
        case shared
        case fixed(String)
        /// 코드 페어링 전용. 저장된 토큰이 없고(load → nil), Mac 이 발급한 토큰도 여기엔
        /// **저장하지 않는다**(save/clear 모두 no-op) — 발급 토큰은 `ProbeResult.issuedToken`
        /// 으로 호출부에 돌려준다. 전역 슬롯(`.shared`)을 쓰면 1:1 앱의 페어링을 덮어쓴다.
        case ephemeral

        func load() -> String? {
            switch self {
            case .shared: return sharedTokenStore.load()
            case .fixed(let token): return token
            case .ephemeral: return nil
            }
        }

        /// `.fixed` 는 no-op. 여기서 전역 슬롯을 지우면 장치 B 의 인증 실패가
        /// 1:1 앱의 페어링 토큰까지 같이 날려버린다.
        func clear() {
            switch self {
            case .shared: sharedTokenStore.clear()
            case .fixed, .ephemeral: break
            }
        }

        /// `.fixed` 는 저장하지 않고 true 를 돌려준다 — 저장할 곳이 없는 게 정상이라
        /// false 를 내면 호출부가 쓸데없이 "토큰 저장 실패" 경고를 남긴다.
        func save(_ token: String) -> Bool {
            switch self {
            case .shared: return sharedTokenStore.save(token)
            case .fixed, .ephemeral: return true
            }
        }
    }

    /// `BLEClient.decideV2` 와 동일한 결정을 그대로 쓴다 — 전송만 다를 뿐 상태
    /// 기계는 하나다. QR 로 코드를 이미 받았으므로 `AwaitingCode2` 에서 사용자
    /// 입력을 기다리지 않고 즉시 바인딩을 낸다(재연결 경로에서는 `code` 가 nil
    /// 이라 `needsPairing` 으로 빠진다 — 저장된 토큰이 거부됐다는 뜻이므로 QR
    /// 재스캔이 맞다).
    ///
    /// 인가되면 이 연결의 봉인 채널을 돌려준다. 이 값 없이는 스냅샷을 한 장도
    /// 읽을 수 없다. 코드로 페어링한 경우(`.openSealedToken`)에는 Mac 이 발급한 토큰을
    /// 함께 돌려준다 — `.ephemeral` 슬롯은 그 토큰을 아무데도 저장하지 않으므로,
    /// 호출부가 여기서 받아 자기 레지스트리에 넣지 않으면 영영 잃어버린다.
    private func authenticate(
        conn: Connection, code: String?, tokens: TokenSlot
    ) async throws -> (channel: SealedChannel, issuedToken: String?) {
        let handshake = V2Handshake()
        // 프레임과 동사를 **한 값으로** 받는다 — 따로 계산하면 조건 하나만
        // 고쳤을 때 서로 어긋나고, 그러면 `AwaitingCode2` 를 `Nonce2` 로 오해해
        // 논스 없는 `PROOF2` 를 내고 조용히 `needsPairing` 에 앉는다.
        // "코드가 저장된 토큰을 이긴다" 는 규칙도 이 한 함수에만 있다.
        let first = BLEClient.initialSend(
            hasToken: tokens.load() != nil,
            code: code,
            clientPub: handshake.clientPub
        )
        var sent = first.verb
        var reply = try await sendControl(conn, first.frame)

        while true {
            switch BLEClient.decideV2(sent: sent, reply: reply) {
            case .bindCode(let epk, let nonce):
                guard let code, handshake.agree(epkHex: epk, nonceHex: nonce),
                      let binding = handshake.codeBinding(code: code) else {
                    // 코드가 없다(재연결 경로) 또는 합의 자체가 실패했다.
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                sent = .code2
                reply = try await sendControl(conn, PairingClient.code2Frame(binding: binding))
            case .signSessionProof(let epk, let nonce):
                guard handshake.agree(epkHex: epk, nonceHex: nonce),
                      let token = tokens.load(),
                      let proof = handshake.sessionProof(tokenHex: token) else {
                    tokens.clear()
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                sent = .proof2
                reply = try await sendControl(conn, PairingClient.proof2Frame(proof: proof))
            case .openSealedToken(let sealed):
                guard let token = handshake.openSealedToken(sealedHex: sealed),
                      let channel = handshake.sessionChannel(tokenHex: token) else {
                    // 봉인이 안 열렸다 = 우리가 만든 키가 맥의 키와 다르다.
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                if !tokens.save(token) {
                    NSLog("네트워크 페어링 토큰 저장 실패 — 다음 재연결부터 코드를 다시 요구합니다")
                }
                return (channel: channel, issuedToken: token)
            case .openSession:
                guard let token = tokens.load(),
                      let channel = handshake.sessionChannel(tokenHex: token) else {
                    tokens.clear()
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                // 재연결 경로 — 프로토콜상 토큰 회전이 없으므로 새로 발급된 토큰은 없다.
                return (channel: channel, issuedToken: nil)
            case .failed(let left):
                stateSubject.send(.pairingFailed(left: left))
                throw NetworkClientError.authFailed
            case .needsPairing:
                // v1 으로 물러서지 않는다. 재시도도 하지 않는다 — 두 결정 모두
                // `runConnection` 의 전용 catch 가 지킨다.
                tokens.clear()
                stateSubject.send(.needsPairing)
                throw NetworkClientError.needsPairing
            }
        }
    }

    /// 제어 메시지 하나 = bi-stream 하나(요청 쓰기 후 finish, 응답을 끝까지 읽기).
    /// Mac 쪽 `NetworkBridge::handle_auth` 와 대칭 — 스트림 종료 자체가 메시지 경계라
    /// BLE 식 청크 헤더가 필요 없다.
    private func sendControl(_ conn: Connection, _ frame: Data) async throws -> AuthReplyPayload {
        let bi = try await conn.openBi()
        _ = try await bi.send().write(buf: frame)
        try await bi.send().finish()
        let data = try await bi.recv().readToEnd(sizeLimit: Self.controlSizeLimit)
        guard let reply = PairingClient.parse(data) else {
            throw NetworkClientError.malformedReply
        }
        return reply
    }

    /// 스냅샷 스트림 한 줄의 정체.
    ///
    /// **BLE 와 달리 네트워크는 봉인 프레임을 hex 문자열로 싣는다.** 이 스트림은
    /// 0x0A 로 프레임을 나누는데(NDJSON) 봉인 프레임은 임의의 이진 바이트라 0x0A
    /// 를 그대로 담을 수 있어, 날 것으로 흘리면 프레임 하나가 여러 줄로 쪼개진다.
    /// 스냅샷 한 건 크기라면 사실상 매번 일어난다. 그래서 맥이 hex 로 감싼다
    /// (`network/mod.rs: snapshot_line` 의 doc). hex 는 `{` 로 시작하지 않으므로
    /// v1 평문 줄과도 한눈에 구분된다.
    enum SnapshotLine: Equatable {
        /// hex 를 디코드한 봉인 프레임. `SealedChannel.open` 으로 간다.
        case sealed(Data)
        /// `{` 로 시작 — 맥이 평문 JSON 을 보냈다. v2 세션에서는 일어날 수 없고,
        /// 일어났다면 다운그레이드다.
        case plaintextJSON
        /// 빈 줄이거나 hex 도 JSON 도 아니다.
        case unusable
    }

    nonisolated static func classifyLine(_ line: Data) -> SnapshotLine {
        guard let first = line.first else { return .unusable }
        if first == UInt8(ascii: "{") { return .plaintextJSON }
        guard let text = String(data: line, encoding: .utf8),
              let frame = Data(hexString: text) else { return .unusable }
        return .sealed(frame)
    }

    /// 인가된 뒤 Mac 이 여는 장수명 uni-stream 을 NDJSON 으로 읽는다 — 줄 하나가
    /// 프레임 하나이고, 그 줄은 hex 로 실린 봉인 프레임이다(`classifyLine`).
    ///
    /// 여기서 `try? JSONDecoder().decode(...)` 로 바로 떨어뜨리면 안 된다 —
    /// hex 줄은 JSON 이 아니라 항상 nil 을 내고, `continue` 가 그걸 조용히
    /// 삼켜 "연결은 됐는데 화면이 영영 비어 있는" 무증상 실패가 된다.
    ///
    /// 읽기 루프 자체는 `streamSnapshots`(AppMulti 와 공유)에 맡기고, 여기서는
    /// 이 인스턴스의 Combine 퍼블리셔로 옮겨 담는 것과 `wantsRunning` 기반
    /// 정지, 버전 불일치 시 상태 전송만 담당한다 — 기존 앱의 동작은 그대로다.
    private func listenForSnapshots(conn: Connection, channel: SealedChannel) async throws {
        stateSubject.send(.streaming)
        // 이 경로는 probe 를 거치지 않으므로 여기서 직접 받는다(맥이 여는 그 하나뿐인 스트림).
        let recv = try await conn.acceptUni()
        do {
            try await streamSnapshots(
                readChunk: { try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit) },
                initialBuffer: Data(),
                channel: channel,
                shouldContinue: { [weak self] in self?.wantsRunning ?? false }
            ) { [weak self] snapshot in
                self?.snapshotSubject.send(snapshot)
            }
        } catch NetworkClientError.versionMismatch {
            // AppMulti(`snapshotStream`)는 이 경우를 던져서 알려야 하지만, 이
            // 1:1 앱은 원래부터 상태 전송 후 조용히 멈추는 게 정상 동작이다
            // (runConnection 이 재시도하지 않도록).
            wantsRunning = false
            stateSubject.send(.versionMismatch)
        }
    }

    /// `listenForSnapshots`(1:1 앱)와 `snapshotStream`(AppMulti)이 공유하는
    /// 읽기 루프. 프레이밍(NDJSON)·복호화(`SealedChannel`)·평문 거부·버전 검사가
    /// 전부 여기 한 곳에만 있다 — 호출부는 유효한 스냅샷을 어디로 흘려보낼지만
    /// (`onSnapshot`) 정한다.
    ///
    /// 버전 불일치는 **던진다.** 예전 `listenForSnapshots`처럼 `wantsRunning = false;
    /// return`으로 조용히 끝내면, `AppMulti`의 `DeviceSession`은 스트림의 정상 종료를
    /// "지금 당장 더 보낼 스냅샷이 없다"로만 해석해 마지막 상태(`.online`)에 그대로
    /// 얼어붙는다(`DeviceTransport`의 스트림 종료 규약). 호출부가 필요하면 이 에러를
    /// 잡아 자기 방식대로 처리한다(`listenForSnapshots`는 흡수하고, `snapshotStream`은
    /// 그대로 던진다).
    private func streamSnapshots(
        readChunk: () async throws -> Data,
        initialBuffer: Data,
        channel: SealedChannel,
        shouldContinue: () -> Bool,
        onSnapshot: (MirrorSnapshot) -> Void
    ) async throws {
        // `Connection` 을 받지 않는다 — 여기서 `acceptUni()` 를 부를 수 있으면 probe 가 이미
        // 연 스트림을 두고 두 번째 스트림을 기다리는 실수를 다시 저지를 수 있다. 읽기 경로는
        // 호출부가 넘긴다.
        var buffer = initialBuffer
        while shouldContinue() {
            // `dialAuthenticateAndOpen` 과 같은 이유 — uniffi 브리지가 취소를 스스로
            // 감지한다는 보장이 없어서, onTermination 의 task.cancel() 만으로는
            // recv.read() 에서 빠져나온다는 보장이 없다. 1:1 경로에도 무해하다
            // (`wantsRunning` 이 먼저 걸린다).
            try Task.checkCancellation()
            let chunk = try await readChunk()
            buffer.append(chunk)
            while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                let lineData = Data(buffer[..<newlineIndex])
                buffer.removeSubrange(buffer.startIndex...newlineIndex)

                let frame: Data
                switch Self.classifyLine(lineData) {
                case .sealed(let f):
                    frame = f
                case .plaintextJSON:
                    // 인가된 v2 세션에 평문이 올 수 없다 — 받아주면 그게
                    // 다운그레이드다(스펙 8장). 연결을 끊을 만큼 확실한 공격
                    // 신호는 아니므로 줄만 버리고 남긴다.
                    NSLog("v2 세션에 평문 스냅샷이 도착해 버립니다")
                    continue
                case .unusable:
                    continue
                }

                let plaintext: Data
                do {
                    plaintext = try channel.open(frame)
                } catch {
                    // 프레임 하나를 버릴 뿐 연결은 끊지 않는다 — 다음 프레임에서
                    // 회복될 수 있다(수신 측은 카운터의 빈 칸을 견딘다).
                    NSLog("봉인 프레임 열기 실패: \(error)")
                    continue
                }
                guard let snap = try? JSONDecoder().decode(MirrorSnapshot.self, from: plaintext) else {
                    continue
                }
                guard snap.isSupportedVersion else {
                    throw NetworkClientError.versionMismatch
                }
                onSnapshot(snap)
            }
        }
    }

    /// `probe(...)`로 연 연결을 그대로 스냅샷 스트림으로 잇는다(`Fleet`의
    /// `IrohDeviceTransport` 전용). 첫 스냅샷을 먼저 흘리고, 이후
    /// `listenForSnapshots`와 같은 방식(`streamSnapshots`)으로 계속 읽는다.
    ///
    /// 스트림 소비가 멈추면(`DeviceSession`이 세션을 정리하는 등)
    /// `continuation.onTermination`에서 읽기 태스크를 취소하고 연결을 닫는다 —
    /// 다시 dial 하면 hole-punch·QUIC·인증 왕복을 또 내야 하므로, 살아있는
    /// 연결을 여기서 재사용하는 것 자체가 이 함수의 존재 이유다.
    public func snapshotStream(from result: ProbeResult) -> AsyncThrowingStream<MirrorSnapshot, Error> {
        AsyncThrowingStream { continuation in
            // self 를 **강하게** 잡는다. probe 호출부는 이 클라이언트를 지역변수로만
            // 들고 있다 — weak 로 잡으면 메인 액터가 양보하는 순간 해제돼 첫 스냅샷도
            // 못 흘리고 빈 스트림으로 끝난다(리뷰에서 실측). 이 태스크를 클라이언트가
            // 저장하지 않으므로 순환은 생기지 않는다 — 스트림이 끝나면 태스크가 끝나고
            // 클라이언트가 풀린다.
            let task = Task {
                continuation.yield(result.firstSnapshot)
                do {
                    try await self.streamSnapshots(
                        readChunk: result.readChunk,
                        initialBuffer: result.pendingBuffer,
                        channel: result.channel,
                        shouldContinue: { true }
                    ) { snapshot in
                        continuation.yield(snapshot)
                    }
                    continuation.finish()
                } catch {
                    // 버전 불일치를 포함해 여기서 잡히는 모든 에러를 그대로
                    // 던진다 — DeviceSession 이 종단 상태를 판별할 수 있어야 한다.
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in
                task.cancel()
                Task { try? result.connection.close(errorCode: 0, reason: Data()) }
            }
        }
    }
}

public enum NetworkClientError: Error, Equatable {
    case malformedReply
    case authFailed
    case needsPairing
    /// 위젯 전용 — `fetchSnapshotOnce`가 타임아웃 예산 안에 못 끝났을 때.
    case fetchTimedOut
    /// 스냅샷이 지원 버전이 아닐 때(위젯 단발성 fetch, 스트리밍 도중 모두 해당).
    case versionMismatch
}

extension NetworkClient: MirrorTransport {}
