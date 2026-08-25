import CryptoKit
import Foundation

/// 전송 독립 종단 암호화 v2 (스펙 `2026-08-25-e2ee-protocol-v2-design.md`).
///
/// Rust `src-tauri/src/crypto/` 와 **골든 벡터로 묶여 있다**
/// (`docs/ble-protocol/golden/e2ee-v2-sample.json`). 여기서 나오는 값이
/// 그 파일과 다르면 고쳐야 하는 쪽은 이 파일이다 — 벡터를 다시 만들면
/// 이미 나간 클라이언트와 조용히 갈라진다.
///
/// 난수도 시계도 쓰지 않는 순수 함수만 둔다. 그래야 두 언어를 벡터로 묶을 수 있다.
enum CryptoV2 {
    /// HKDF info 문자열. 두 언어가 바이트 단위로 같아야 한다.
    static let infoPair = Data("aim-pair-v2".utf8)
    static let infoS2C = Data("aim-sess-v2-s2c".utf8)
    static let infoC2S = Data("aim-sess-v2-c2s".utf8)
    /// AEAD 부가 인증 데이터. 프로토콜 버전을 태그에 묶는다.
    static let aad = Data("aim-v2".utf8)

    /// 두 임시 공개키를 이어붙인 64바이트. **항상 클라이언트 키가 먼저다** —
    /// 역할과 무관하게 양쪽이 같은 순서로 만들어야 cbind 와 proof 가 일치한다.
    static func transcript(clientPub: Data, serverPub: Data) -> Data {
        clientPub + serverPub
    }

    private static func hkdf32(ikm: Data, salt: Data, info: Data) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm),
            salt: salt,
            info: info,
            outputByteCount: 32
        )
    }

    /// 페어링 단계에서 토큰 전달 한 건만 봉인하는 키.
    /// 이 시점에는 토큰이 없으므로 ikm 이 공유 비밀뿐이다.
    static func derivePairKey(sharedSecret: Data, nonce: Data) -> SymmetricKey {
        hkdf32(ikm: sharedSecret, salt: nonce, info: infoPair)
    }

    /// 세션 키 두 개. `ikm = sharedSecret || token` 이라 **둘 다 있어야** 키가 나온다 —
    /// X25519 가 깨져도 토큰이 필요하고, 토큰이 새도 임시 개인키가 필요하다.
    static func deriveSessionKeys(
        sharedSecret: Data, token: Data, nonce: Data
    ) -> (s2c: SymmetricKey, c2s: SymmetricKey) {
        let ikm = sharedSecret + token
        return (hkdf32(ikm: ikm, salt: nonce, info: infoS2C),
                hkdf32(ikm: ikm, salt: nonce, info: infoC2S))
    }

    /// 6자리 코드를 **키로** 써서 두 임시 공개키를 MAC 한다. 코드 자체는 어느
    /// 방향으로도 링크를 건너지 않는다(v1 은 `CODE:123456` 으로 그대로 보냈다).
    /// 동시에 이 값이 두 임시 공개키를 묶으므로 능동적 중간자가 자기 키를
    /// 끼워넣으면 값이 맞지 않는다.
    static func codeBinding(code: String, transcript: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(
            for: transcript, using: SymmetricKey(data: Data(code.utf8))
        ))
    }

    /// 재연결 증명. v1 의 `HMAC(token, nonce)` 에 transcript 를 붙인 것이다.
    /// 토큰은 hex 문자열이 아니라 **디코드한 원시 바이트**를 키로 쓴다 —
    /// 여기서 `Data(token.utf8)` 를 쓰면 Mac 이 증명을 거부한다.
    static func sessionProof(token: Data, nonce: Data, transcript: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(
            for: nonce + transcript, using: SymmetricKey(data: token)
        ))
    }
}

enum SealedChannelError: Error, Equatable {
    /// 카운터와 태그를 담기에도 짧다.
    case tooShort
    /// 이미 본 카운터 이하 — 재전송이거나 순서 역행이다.
    case replay
    /// 복호·인증 실패. 변조됐거나 키가 다르다.
    case badTag
}

/// 방향별 키와 카운터를 갖는 봉인 채널. Rust `crypto/channel.rs` 의 짝이다.
///
/// **(키, 논스) 쌍은 절대 재사용하지 않는다.** 세션마다 키가 다르므로 카운터를
/// 0 에서 시작해도 안전하고, 카운터는 UInt64 라 실질적으로 순환하지 않는다.
///
/// `Sendable` 이 아니다 — 카운터가 가변 상태라, 한 연결의 채널은 한 곳에서만
/// 만져야 한다.
final class SealedChannel {
    private let sendKey: SymmetricKey
    private let recvKey: SymmetricKey
    private var sendCounter: UInt64 = 0
    /// 마지막으로 **받아들인** 카운터. 첫 프레임 전에는 nil 이다 — 0 으로 두면
    /// 카운터 0 인 첫 프레임을 재전송으로 오인한다.
    private var lastRecv: UInt64?

    /// 클라이언트 입장에서는 서버의 `c2s` 가 송신 키, `s2c` 가 수신 키다.
    init(sendKey: SymmetricKey, recvKey: SymmetricKey) {
        self.sendKey = sendKey
        self.recvKey = recvKey
    }

    private static func nonce(_ counter: UInt64) throws -> ChaChaPoly.Nonce {
        var raw = Data(repeating: 0, count: 4)
        raw.append(counter.bigEndianBytes)
        return try ChaChaPoly.Nonce(data: raw)
    }

    /// 봉인 프레임 = counter(8바이트 BE) || ciphertext || tag(16바이트).
    ///
    /// 카운터를 프레임에 싣는 이유: 수신자가 자기 카운터만 세면 프레임 하나만
    /// 유실돼도 영구히 어긋난다. BLE 청크 재조립은 순서가 어긋나면 프레임을
    /// 버리므로 실제로 일어나는 일이다.
    func seal(_ plaintext: Data) throws -> Data {
        let counter = sendCounter
        // 봉인이 실패하더라도 카운터는 되돌리지 않는다 — 같은 (키, 논스) 를
        // 두 번 쓰는 것보다 프레임 번호 하나를 건너뛰는 편이 낫다.
        sendCounter += 1
        let box = try ChaChaPoly.seal(
            plaintext, using: sendKey,
            nonce: Self.nonce(counter), authenticating: CryptoV2.aad
        )
        var out = counter.bigEndianBytes
        out.append(box.ciphertext)
        out.append(box.tag)
        return out
    }

    func open(_ frame: Data) throws -> Data {
        guard frame.count >= 8 + 16 else { throw SealedChannelError.tooShort }
        let counter = frame.prefix(8).reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
        if let last = lastRecv, counter <= last { throw SealedChannelError.replay }
        let body = frame.dropFirst(8)
        // 슬라이스의 시작 인덱스가 0 이 아니므로 Data 로 다시 감싼다.
        let ct = Data(body.prefix(body.count - 16))
        let tag = Data(body.suffix(16))
        guard let box = try? ChaChaPoly.SealedBox(
            nonce: Self.nonce(counter), ciphertext: ct, tag: tag
        ),
            let plaintext = try? ChaChaPoly.open(box, using: recvKey, authenticating: CryptoV2.aad)
        else { throw SealedChannelError.badTag }
        // 인증에 성공한 뒤에만 전진시킨다 — 그렇지 않으면 카운터가 UInt64.max 인
        // 쓰레기 프레임 하나로 이후 정상 프레임이 전부 막힌다.
        lastRecv = counter
        return plaintext
    }
}

extension UInt64 {
    /// 프레임 헤더와 AEAD 논스가 모두 빅엔디언 8바이트를 쓴다.
    var bigEndianBytes: Data {
        withUnsafeBytes(of: bigEndian) { Data($0) }
    }
}

extension Data {
    /// 소문자 hex. `Data(hexString:)`(PairingClient.swift) 의 역이다 —
    /// 골든 벡터와 대조할 때 쓴다.
    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}

extension SymmetricKey {
    /// 파생 키를 골든 벡터와 대조하기 위한 것이다. 로그에는 절대 넣지 않는다.
    var hexString: String {
        withUnsafeBytes { Data($0).hexString }
    }
}
