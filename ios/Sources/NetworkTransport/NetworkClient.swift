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
            let channel = try await authenticate(conn: conn, code: code)
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

    /// `BLEClient.decideV2` 와 동일한 결정을 그대로 쓴다 — 전송만 다를 뿐 상태
    /// 기계는 하나다. QR 로 코드를 이미 받았으므로 `AwaitingCode2` 에서 사용자
    /// 입력을 기다리지 않고 즉시 바인딩을 낸다(재연결 경로에서는 `code` 가 nil
    /// 이라 `needsPairing` 으로 빠진다 — 저장된 토큰이 거부됐다는 뜻이므로 QR
    /// 재스캔이 맞다).
    ///
    /// 인가되면 이 연결의 봉인 채널을 돌려준다. 이 값 없이는 스냅샷을 한 장도
    /// 읽을 수 없다.
    private func authenticate(conn: Connection, code: String?) async throws -> SealedChannel {
        let handshake = V2Handshake()
        // 프레임과 동사를 **한 값으로** 받는다 — 따로 계산하면 조건 하나만
        // 고쳤을 때 서로 어긋나고, 그러면 `AwaitingCode2` 를 `Nonce2` 로 오해해
        // 논스 없는 `PROOF2` 를 내고 조용히 `needsPairing` 에 앉는다.
        // "코드가 저장된 토큰을 이긴다" 는 규칙도 이 한 함수에만 있다.
        let first = BLEClient.initialSend(
            hasToken: NetworkTokenStore.loadToken() != nil,
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
                      let token = NetworkTokenStore.loadToken(),
                      let proof = handshake.sessionProof(tokenHex: token) else {
                    NetworkTokenStore.clearToken()
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
                if !NetworkTokenStore.saveToken(token) {
                    NSLog("네트워크 페어링 토큰 저장 실패 — 다음 재연결부터 코드를 다시 요구합니다")
                }
                return channel
            case .openSession:
                guard let token = NetworkTokenStore.loadToken(),
                      let channel = handshake.sessionChannel(tokenHex: token) else {
                    NetworkTokenStore.clearToken()
                    stateSubject.send(.needsPairing)
                    throw NetworkClientError.needsPairing
                }
                return channel
            case .failed(let left):
                stateSubject.send(.pairingFailed(left: left))
                throw NetworkClientError.authFailed
            case .needsPairing:
                // v1 으로 물러서지 않는다. 재시도도 하지 않는다 — 두 결정 모두
                // `runConnection` 의 전용 catch 가 지킨다.
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
    private func listenForSnapshots(conn: Connection, channel: SealedChannel) async throws {
        stateSubject.send(.streaming)
        let recv = try await conn.acceptUni()
        var buffer = Data()
        while wantsRunning {
            let chunk = try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit)
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
