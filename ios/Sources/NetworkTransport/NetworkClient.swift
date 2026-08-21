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
        beginConnecting(endpointIdHex: endpointHex, code: nil)
    }

    public func stop() {
        wantsRunning = false
        runTask?.cancel()
        runTask = nil
        stateSubject.send(.idle)
    }

    /// QR 스캐너가 디코딩한 문자열(`aim://pair?endpoint=<hex>&code=<code>`)을 넘긴다.
    /// 스캔 한 번으로 dial 과 `CODE:` 제출이 자동으로 끝난다 — 사용자가 코드를 따로
    /// 입력할 필요가 없다(설계 결정, 계획 문서 참고).
    public func pair(qrPayload: String) {
        guard let parsed = Self.parseQrPayload(qrPayload) else {
            stateSubject.send(.disconnected(reason: "QR 코드를 인식하지 못했습니다"))
            return
        }
        NetworkTokenStore.saveEndpointIdHex(parsed.endpointIdHex)
        beginConnecting(endpointIdHex: parsed.endpointIdHex, code: parsed.code)
    }

    nonisolated static func parseQrPayload(_ payload: String) -> (endpointIdHex: String, code: String)? {
        guard let components = URLComponents(string: payload),
              components.scheme == "aim", components.host == "pair",
              let items = components.queryItems,
              let endpointIdHex = items.first(where: { $0.name == "endpoint" })?.value,
              let code = items.first(where: { $0.name == "code" })?.value else {
            return nil
        }
        return (endpointIdHex, code)
    }

    private func beginConnecting(endpointIdHex: String, code: String?) {
        wantsRunning = true
        runTask?.cancel()
        runTask = Task { [weak self] in
            await self?.runConnection(endpointIdHex: endpointIdHex, code: code)
        }
    }

    private func runConnection(endpointIdHex: String, code: String?) async {
        stateSubject.send(.connecting)
        do {
            guard let idBytes = Data(hexString: endpointIdHex) else {
                stateSubject.send(.disconnected(reason: "잘못된 페어링 정보"))
                return
            }
            let endpointId = try EndpointId.fromBytes(bytes: idBytes)
            // QR 은 EndpointId 만 담는다 — 주소/relay 는 n0 디스커버리(applyN0)가
            // 연결 시점에 알아낸다("dial by key, not IP"), 계획 문서에서 확인됨.
            let addr = EndpointAddr(id: endpointId, relayUrl: nil, addresses: [])

            let builder = EndpointBuilder()
            builder.applyN0()
            builder.alpns(alpns: [Self.alpn])
            let ep = try await builder.bind()
            endpoint = ep

            let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
            try await authenticate(conn: conn, code: code)
            guard wantsRunning else { return }
            try await listenForSnapshots(conn: conn)
        } catch {
            guard wantsRunning else { return }
            stateSubject.send(.disconnected(reason: "\(error)"))
            // BLE 의 beginScan() 재시도 루프와 같은 목적 — 잠깐 쉬었다가 저장된
            // EndpointId 로 재연결을 시도한다(사용자가 QR 을 또 스캔할 필요 없음).
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            guard wantsRunning else { return }
            beginConnecting(endpointIdHex: endpointIdHex, code: nil)
        }
    }

    /// `BLEClient.decide(_:)` 와 동일한 결정을 그대로 쓴다 — 전송만 다를 뿐 상태
    /// 기계는 하나다. QR 로 코드를 이미 받았으므로 `AwaitingCode` 에서 사용자
    /// 입력을 기다리지 않고 즉시 제출한다(재연결 경로에서는 `code`가 nil이라
    /// `needsPairing` 으로 빠진다 — 저장된 토큰이 거부됐다는 뜻이므로 QR 재스캔이
    /// 맞다).
    private func authenticate(conn: Connection, code: String?) async throws {
        let initialFrame = NetworkTokenStore.loadToken() != nil
            ? PairingClient.authFrame()
            : PairingClient.helloFrame()
        var reply = try await sendControl(conn, initialFrame)

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
