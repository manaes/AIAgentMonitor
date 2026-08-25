import CryptoKit
import XCTest
@testable import BLETransport

/// Rust(`src-tauri/src/crypto/`) 와 Swift 가 **같은 프로토콜을 말하는지**를
/// 고정하는 테스트다. 값이 어긋나면 Swift 를 고친다 — 골든 벡터를 다시 만들면
/// 이미 배포된 클라이언트와 조용히 갈라진다.
final class CryptoV2Tests: XCTestCase {
    /// 골든 벡터. 세 언어가 같은 파일을 읽는다
    /// (`docs/ble-protocol/golden/e2ee-v2-sample.json`).
    private struct Golden: Decodable {
        struct Input: Decodable {
            let clientPub: String
            let serverPub: String
            let sharedSecret: String
            let nonce: String
            let token: String
            let code: String

            private enum CodingKeys: String, CodingKey {
                case clientPub = "client_pub"
                case serverPub = "server_pub"
                case sharedSecret = "shared_secret"
                case nonce, token, code
            }
        }

        let input: Input
        let transcript: String
        let codeBinding: String
        let sessionProof: String
        let pairKey: String
        let kS2C: String
        let kC2S: String
        let sealedFrame0: String
        let sealedFrame1: String

        private enum CodingKeys: String, CodingKey {
            case input, transcript
            case codeBinding = "code_binding"
            case sessionProof = "session_proof"
            case pairKey = "pair_key"
            case kS2C = "k_s2c"
            case kC2S = "k_c2s"
            case sealedFrame0 = "sealed_frame_0"
            case sealedFrame1 = "sealed_frame_1"
        }
    }

    /// 기존 골든 테스트들(`hmac-sample`, `frames-sample`)과 같은 방식으로 번들을 찾는다.
    private func golden() throws -> Golden {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "e2ee-v2-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다"
        )
        return try JSONDecoder().decode(Golden.self, from: Data(contentsOf: url))
    }

    private func bytes(_ hex: String) throws -> Data {
        try XCTUnwrap(Data(hexString: hex), "hex 디코드 실패: \(hex)")
    }

    // MARK: - 골든 벡터 대조

    func testTranscriptMatchesGolden() throws {
        let g = try golden()
        let cpk = try bytes(g.input.clientPub)
        let spk = try bytes(g.input.serverPub)
        XCTAssertEqual(
            CryptoV2.transcript(clientPub: cpk, serverPub: spk).hexString,
            g.transcript,
            "클라이언트 키가 먼저다"
        )
    }

    func testCodeBindingMatchesGolden() throws {
        let g = try golden()
        let tr = try bytes(g.transcript)
        XCTAssertEqual(
            CryptoV2.codeBinding(code: g.input.code, transcript: tr).hexString,
            g.codeBinding,
            "코드는 HMAC 의 키로 쓰인다 — UTF-8 바이트 그대로다"
        )
    }

    func testSessionProofMatchesGolden() throws {
        let g = try golden()
        let token = try bytes(g.input.token)
        let nonce = try bytes(g.input.nonce)
        let tr = try bytes(g.transcript)
        XCTAssertEqual(
            CryptoV2.sessionProof(token: token, nonce: nonce, transcript: tr).hexString,
            g.sessionProof,
            "토큰은 hex 문자열이 아니라 디코드한 원시 바이트를 키로 쓴다"
        )
    }

    func testPairKeyMatchesGolden() throws {
        let g = try golden()
        let ss = try bytes(g.input.sharedSecret)
        let nonce = try bytes(g.input.nonce)
        XCTAssertEqual(
            CryptoV2.derivePairKey(sharedSecret: ss, nonce: nonce).hexString,
            g.pairKey,
            "페어링 키의 ikm 은 공유 비밀뿐이다 — 이 시점에는 토큰이 없다"
        )
    }

    func testSessionKeysMatchGolden() throws {
        let g = try golden()
        let ss = try bytes(g.input.sharedSecret)
        let token = try bytes(g.input.token)
        let nonce = try bytes(g.input.nonce)
        let keys = CryptoV2.deriveSessionKeys(sharedSecret: ss, token: token, nonce: nonce)
        XCTAssertEqual(keys.s2c.hexString, g.kS2C)
        XCTAssertEqual(keys.c2s.hexString, g.kC2S)
    }

    /// Rust 가 봉인한 프레임을 Swift 가 열 수 있어야 한다. 이 하나가
    /// 실패하면 두 구현이 다른 프로토콜을 말하는 것이다.
    ///
    /// 두 장을 **한 채널로 순서대로** 연다. 카운터 0 짜리 한 장만으로는
    /// 논스 조립을 고정하지 못한다 — 0 에서는 `[0,0,0,0] || BE(counter)` 와
    /// `BE(counter) || [0,0,0,0]` 이 둘 다 12바이트 0 이고, 카운터를 리틀엔디언으로
    /// 읽고 쓰는 구현도 자기들끼리는 일관돼서 왕복 테스트를 통과한다.
    /// 카운터 1 에서 셋 다 갈라진다.
    func testOpensGoldenSealedFrames() throws {
        let g = try golden()
        let keys = try sessionKeys(g)
        // 클라이언트 입장: 서버의 s2c 가 내 수신 키다.
        let ch = SealedChannel(sendKey: keys.c2s, recvKey: keys.s2c)
        XCTAssertEqual(try ch.open(try bytes(g.sealedFrame0)), Data(#"{"v":2}"#.utf8))
        XCTAssertEqual(
            try ch.open(try bytes(g.sealedFrame1)), Data(#"{"v":2}"#.utf8),
            "카운터 1 프레임이 논스 조립(패딩 위치와 바이트 순서)을 고정한다"
        )
    }

    /// ChaCha20-Poly1305 은 (키, 논스) 가 같으면 결정적이다 — 그래서 서버 역할로
    /// 봉인한 결과가 골든 프레임과 **바이트까지** 같아야 한다. 여는 것만 맞추면
    /// 프레임 배치가 틀려도 우연히 통과할 여지가 있어서 반대 방향도 고정한다.
    func testSealsByteIdenticalGoldenFrames() throws {
        let g = try golden()
        let keys = try sessionKeys(g)
        // 서버 입장: s2c 가 내 송신 키다.
        let server = SealedChannel(sendKey: keys.s2c, recvKey: keys.c2s)
        XCTAssertEqual(try server.seal(Data(#"{"v":2}"#.utf8)).hexString, g.sealedFrame0)
        XCTAssertEqual(
            try server.seal(Data(#"{"v":2}"#.utf8)).hexString, g.sealedFrame1,
            "같은 평문이라도 카운터가 1 이면 논스가 달라 암호문이 달라야 한다"
        )
    }

    private func sessionKeys(_ g: Golden) throws -> (s2c: SymmetricKey, c2s: SymmetricKey) {
        CryptoV2.deriveSessionKeys(
            sharedSecret: try bytes(g.input.sharedSecret),
            token: try bytes(g.input.token),
            nonce: try bytes(g.input.nonce)
        )
    }

    // MARK: - 채널 동작 (Rust channel.rs 의 테스트와 짝을 이룬다)

    /// 맥과 클라이언트를 흉내낸다 — 한쪽의 송신 키가 다른 쪽의 수신 키다.
    private func pair() -> (mac: SealedChannel, client: SealedChannel) {
        let s2c = SymmetricKey(data: Data(repeating: 1, count: 32))
        let c2s = SymmetricKey(data: Data(repeating: 2, count: 32))
        return (SealedChannel(sendKey: s2c, recvKey: c2s),
                SealedChannel(sendKey: c2s, recvKey: s2c))
    }

    func testRoundTrips() throws {
        let (mac, client) = pair()
        let frame = try mac.seal(Data("hello".utf8))
        XCTAssertEqual(try client.open(frame), Data("hello".utf8))
    }

    /// 첫 프레임의 카운터는 0 이다. `lastRecv` 를 0 으로 초기화하면 이게 재전송으로 막힌다.
    func testAcceptsFirstFrameWithCounterZero() throws {
        let (mac, client) = pair()
        let frame = try mac.seal(Data("first".utf8))
        XCTAssertEqual(frame.prefix(8), Data(repeating: 0, count: 8))
        XCTAssertEqual(try client.open(frame), Data("first".utf8))
    }

    func testCounterIncrementsSoTwoIdenticalMessagesDiffer() throws {
        let (mac, client) = pair()
        let a = try mac.seal(Data("same".utf8))
        let b = try mac.seal(Data("same".utf8))
        XCTAssertNotEqual(a, b, "같은 평문이라도 카운터가 달라 암호문이 달라야 한다")
        XCTAssertEqual(try client.open(a), Data("same".utf8))
        XCTAssertEqual(try client.open(b), Data("same".utf8))
    }

    /// 이 검사가 가장 중요하다 — 같은 (키, 논스) 로 두 번 봉인하면
    /// ChaCha20-Poly1305 의 보장이 통째로 무너진다.
    func testRejectsReplayedFrame() throws {
        let (mac, client) = pair()
        let frame = try mac.seal(Data("once".utf8))
        XCTAssertEqual(try client.open(frame), Data("once".utf8))
        XCTAssertThrowsError(try client.open(frame)) {
            XCTAssertEqual($0 as? SealedChannelError, .replay, "같은 카운터를 두 번 받으면 거부한다")
        }
    }

    func testRejectsOutOfOrderFrame() throws {
        let (mac, client) = pair()
        let first = try mac.seal(Data("1".utf8))
        let second = try mac.seal(Data("2".utf8))
        XCTAssertEqual(try client.open(second), Data("2".utf8))
        XCTAssertThrowsError(try client.open(first)) {
            XCTAssertEqual($0 as? SealedChannelError, .replay, "이미 지나간 카운터는 거부한다")
        }
    }

    /// 프레임이 유실돼도 그 다음 프레임은 열려야 한다 — BLE 청크 재조립은
    /// 순서가 어긋나면 프레임을 버리므로 실제로 일어난다.
    func testToleratesAGapInCounters() throws {
        let (mac, client) = pair()
        _ = try mac.seal(Data("lost".utf8))
        let next = try mac.seal(Data("next".utf8))
        XCTAssertEqual(try client.open(next), Data("next".utf8), "빈 칸을 건너뛸 수 있어야 한다")
    }

    func testRejectsTamperedTag() throws {
        let (mac, client) = pair()
        var frame = try mac.seal(Data("hello".utf8))
        frame[frame.count - 1] ^= 0x01
        XCTAssertThrowsError(try client.open(frame)) {
            XCTAssertEqual($0 as? SealedChannelError, .badTag)
        }
    }

    func testRejectsFrameSealedWithTheWrongDirectionKey() throws {
        let (mac, _) = pair()
        let frame = try mac.seal(Data("hello".utf8))
        // 맥이 자기 송신 키로 봉인한 것을 자기가 열려고 하면 안 된다.
        XCTAssertThrowsError(try mac.open(frame)) {
            XCTAssertEqual($0 as? SealedChannelError, .badTag)
        }
    }

    func testRejectsShortFrame() throws {
        let (_, client) = pair()
        XCTAssertThrowsError(try client.open(Data(repeating: 0, count: 8))) {
            XCTAssertEqual($0 as? SealedChannelError, .tooShort)
        }
    }

    /// 변조된 프레임이 이후 정상 프레임을 막아서는 안 된다. 카운터를 인증
    /// 전에 전진시키면, 공격자가 카운터 UInt64.max 짜리 쓰레기 하나로 세션을
    /// 영구히 죽일 수 있다.
    func testATamperedFrameDoesNotBlockLaterValidFrames() throws {
        let (mac, client) = pair()
        let good = try mac.seal(Data("good".utf8))
        var junk = try mac.seal(Data("junk".utf8))
        junk.replaceSubrange(0..<8, with: withUnsafeBytes(of: UInt64.max.bigEndian) { Data($0) })
        XCTAssertThrowsError(try client.open(junk)) {
            XCTAssertEqual($0 as? SealedChannelError, .badTag)
        }
        XCTAssertEqual(try client.open(good), Data("good".utf8))
    }
}
