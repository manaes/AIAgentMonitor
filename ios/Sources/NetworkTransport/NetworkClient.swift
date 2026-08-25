import BLETransport
import Combine
import Foundation
import IrohLib
import Wire

/// iroh(QUIC) 기반 미러 전송. `BLEClient` 와 같은 `MirrorTransport` 모양을 갖지만
/// GATT 대신 QR 로 전달받은 `EndpointId` 로 직접 dial 한다. 페어링 인증 프로토콜은
/// `BLEClient.decide`/`PairingClient` 를 그대로 재사용한다 — 전송만 다를 뿐 Mac
/// 쪽 `pairing.rs` 상태 기계는 두 전송이 동일하게 취급한다.
@MainActor
public final class NetworkClient: NSObject {
    private static let alpn = Data("aim/mirror/1".utf8)
    /// 제어 메시지 하나의 최대 크기. 실제 응답은 수십 바이트 수준이라 넉넉히 잡는다.
    private static let controlSizeLimit: UInt32 = 4096
    private static let snapshotChunkSizeLimit: UInt32 = 65536

    private var endpoint: Endpoint?
    private var wantsRunning = false
    private var runTask: Task<Void, Never>?

    private let stateSubject = CurrentValueSubject<ConnectionState, Never>(.idle)
    private let snapshotSubject = PassthroughSubject<MirrorSnapshot, Never>()

    public var state: AnyPublisher<ConnectionState, Never> { stateSubject.eraseToAnyPublisher() }
    public var snapshots: AnyPublisher<MirrorSnapshot, Never> { snapshotSubject.eraseToAnyPublisher() }

    public override init() {
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

    nonisolated static func parseQrPayload(
        _ payload: String
    ) -> (endpointIdHex: String, code: String, relayUrl: String?, addresses: [String])? {
        guard let components = URLComponents(string: payload),
              components.scheme == "aim", components.host == "pair",
              let items = components.queryItems,
              let endpointIdHex = items.first(where: { $0.name == "endpoint" })?.value,
              let code = items.first(where: { $0.name == "code" })?.value else {
            return nil
        }
        // relay/addr 는 Rust 쪽에서 URL 인코딩을 피하려고 hex 로 실어 보낸다
        // (Data(hexString:) 는 이 파일이 이미 BLE 쪽에서 재사용하고 있다).
        func decodeHex(_ value: String?) -> String? {
            guard let value, let data = Data(hexString: value) else { return nil }
            return String(data: data, encoding: .utf8)
        }
        let relayUrl = decodeHex(items.first(where: { $0.name == "relay" })?.value)
        let addresses = items
            .filter { $0.name == "addr" }
            .compactMap { decodeHex($0.value) }
        return (endpointIdHex, code, relayUrl, addresses)
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

            let builder = EndpointBuilder()
            builder.applyN0()
            builder.alpns(alpns: [Self.alpn])
            let ep = try await builder.bind()
            endpoint = ep

            let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
            try await authenticate(conn: conn, code: code)
            guard wantsRunning else { return }
            try await listenForSnapshots(conn: conn)
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

    /// 인증을 어떤 프레임으로 시작할지 고른다.
    ///
    /// **코드가 있으면 저장된 토큰보다 코드를 우선한다.** QR 을 방금 스캔했다는
    /// 것은 "새로 페어링하겠다" 는 명시적 의사이기 때문이다. 초안은 토큰을
    /// 우선했는데, Mac 에서 전체 해제로 토큰이 폐기된 뒤에는 그 재인증이 반드시
    /// 거부되어(`Rejected` → `resetAndAwaitCode`) 방금 스캔한 코드가 쓰이지도
    /// 못한 채 `needsPairing` 으로 떨어졌다.
    ///
    /// 코드가 있으면 `HELLO` 를 건너뛰고 바로 `CODE:` 를 낸다 — Mac 은 HELLO
    /// 없이 온 CODE: 도 받는다(`pairing.rs: code_without_prior_hello_still_grants`).
    nonisolated static func initialFrame(hasToken: Bool, code: String?) -> Data {
        if let code {
            return PairingClient.codeFrame(code)
        }
        return hasToken ? PairingClient.authFrame() : PairingClient.helloFrame()
    }

    /// `BLEClient.decide(_:)` 와 동일한 결정을 그대로 쓴다 — 전송만 다를 뿐 상태
    /// 기계는 하나다. QR 로 코드를 이미 받았으므로 `AwaitingCode` 에서 사용자
    /// 입력을 기다리지 않고 즉시 제출한다(재연결 경로에서는 `code`가 nil이라
    /// `needsPairing` 으로 빠진다 — 저장된 토큰이 거부됐다는 뜻이므로 QR 재스캔이
    /// 맞다).
    private func authenticate(conn: Connection, code: String?) async throws {
        let hasToken = NetworkTokenStore.loadToken() != nil
        var reply = try await sendControl(conn, Self.initialFrame(hasToken: hasToken, code: code))

        while true {
            switch BLEClient.decide(reply) {
            case .signNonce(let nonce):
                guard let token = NetworkTokenStore.loadToken(),
                      let proof = PairingClient.proofFrame(token: token, nonce: nonce) else {
                    NetworkTokenStore.clearToken()
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                reply = try await sendControl(conn, proof)
            case .storeTokenAndSubscribe(let token):
                if !NetworkTokenStore.saveToken(token) {
                    NSLog("네트워크 페어링 토큰 저장 실패 — 다음 재연결부터 코드를 다시 요구합니다")
                }
                return
            case .subscribe:
                return
            case .failed(let left):
                stateSubject.send(.pairingFailed(left: left))
                throw NetworkClientError.authFailed
            case .awaitCode:
                guard let code else {
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                reply = try await sendControl(conn, PairingClient.codeFrame(code))
            case .resetAndAwaitCode:
                NetworkTokenStore.clearToken()
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

    /// 인가된 뒤 Mac 이 여는 장수명 uni-stream 을 NDJSON 으로 읽는다 — 스냅샷
    /// JSON 은 raw 개행을 포함하지 않으므로 줄 단위 분리만으로 프레이밍이 끝난다.
    private func listenForSnapshots(conn: Connection) async throws {
        stateSubject.send(.streaming)
        let recv = try await conn.acceptUni()
        var buffer = Data()
        while wantsRunning {
            let chunk = try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit)
            buffer.append(chunk)
            while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                let lineData = buffer[..<newlineIndex]
                buffer.removeSubrange(buffer.startIndex...newlineIndex)
                guard let snap = try? JSONDecoder().decode(MirrorSnapshot.self, from: lineData) else {
                    continue
                }
                guard snap.isSupportedVersion else {
                    wantsRunning = false
                    stateSubject.send(.versionMismatch)
                    return
                }
                snapshotSubject.send(snap)
            }
        }
    }
}

enum NetworkClientError: Error {
    case malformedReply
    case authFailed
    case needsPairing
}

extension NetworkClient: MirrorTransport {}
