import Foundation
import CryptoKit

/// Mac 의 `src-tauri/src/ble/pairing.rs` 가 해석하는 형식과 정확히 맞춰야 한다.
/// 프레임은 평문이고 짧다 — 이 채널로 오가는 것은 코드와 토큰뿐이다.
///
/// 실제로 나가는 여섯 모양(정본, 개정 참고): `{"ok":false,"await":"code"}`,
/// `{"ok":false,"nonce":"<hex>"}`, `{"ok":true}`(재인증 성공, 토큰 없음),
/// `{"ok":true,"token":"<hex>"}`(최초 인가), `{"ok":false,"left":<n>}`, `{"ok":false}`.
///
/// v2 는 네 모양을 더한다(`pairing.rs: AuthReply::to_json_bytes`):
/// `{"ok":false,"v":2,"await":"code","epk":"<hex>","nonce":"<hex>"}`(AwaitingCode2),
/// `{"ok":false,"v":2,"epk":"<hex>","nonce":"<hex>"}`(Nonce2),
/// `{"ok":true,"v":2,"sealed":"<hex>"}`(Granted2), `{"ok":true,"v":2}`(Authorized2).
/// **`Denied`/`Rejected` 는 두 세대가 그대로 공유해 `"v"` 가 실리지 않는다** —
/// v2 흐름에서도 이 두 모양이 그대로 도착하므로, `"v":2` 가 없다는 것만으로
/// "v1 이니 되돌아가자" 고 판단하면 안 된다(`BLEClient.decideV2` 참고).
public struct AuthReplyPayload: Decodable, Equatable, Sendable {
    public let ok: Bool
    public let token: String?
    public let left: Int?
    /// Rust 가 보내는 키는 `await` 인데 Swift 예약어라 이름을 바꿔 받는다.
    public let awaiting: String?
    /// 재인증 2단계(AUTH/AUTH2 응답)로 받는 논스. v1 은 `proofFrame` 의 메시지이고,
    /// v2 는 세션 키 파생의 salt 이자 proof 메시지의 앞부분이다.
    public let nonce: String?
    /// 프로토콜 세대. v2 응답에만 `2` 로 실린다.
    public let v: Int?
    /// 맥의 임시 X25519 공개키(hex 64자). `AwaitingCode2`/`Nonce2` 에 실린다.
    public let epk: String?
    /// `Granted2` 가 `k_pair` 로 봉인해 보낸 `{"token":"<hex>"}`(hex).
    public let sealed: String?

    private enum CodingKeys: String, CodingKey {
        case ok, token, left, nonce, v, epk, sealed
        case awaiting = "await"
    }
}

public enum PairingClient {
    public static func helloFrame() -> Data { Data("HELLO".utf8) }
    public static func codeFrame(_ code: String) -> Data { Data("CODE:\(code)".utf8) }
    public static func authFrame() -> Data { Data("AUTH".utf8) }

    /// 논스에 대한 서명. **hex 문자열이 아니라 디코드한 원시 바이트**로 계산한다 —
    /// 이 계약은 docs/ble-protocol/golden/hmac-sample.json 이 고정한다.
    public static func proofFrame(token: String, nonce: String) -> Data? {
        guard let key = Data(hexString: token), let msg = Data(hexString: nonce) else { return nil }
        let mac = HMAC<SHA256>.authenticationCode(for: msg, using: SymmetricKey(data: key))
        let hex = mac.map { String(format: "%02x", $0) }.joined()
        return Data("PROOF:\(hex)".utf8)
    }

    public static func parse(_ data: Data) -> AuthReplyPayload? {
        try? JSONDecoder().decode(AuthReplyPayload.self, from: data)
    }
}

// MARK: - v2 프레임

/// Rust `parse_auth_request` 가 `strip_prefix` 로 읽는 네 동사. 접두사는 v1 과
/// 겹치지 않게 골라져 있다(`HELLO2:` 는 `HELLO` 검사보다 먼저 걸린다).
public extension PairingClient {
    static func hello2Frame(clientPub: Data) -> Data {
        Data("HELLO2:\(clientPub.hexString)".utf8)
    }
    static func code2Frame(binding: Data) -> Data {
        Data("CODE2:\(binding.hexString)".utf8)
    }
    static func auth2Frame(clientPub: Data) -> Data {
        Data("AUTH2:\(clientPub.hexString)".utf8)
    }
    static func proof2Frame(proof: Data) -> Data {
        Data("PROOF2:\(proof.hexString)".utf8)
    }
}

/// v2 핸드셰이크의 **클라이언트 절반**. 임시 X25519 키를 만들고, 맥의 응답에서
/// 공유 비밀·transcript·논스를 확정한 뒤, 그 위에서 코드 바인딩·재연결 증명·
/// 세션 채널을 만든다. Rust 쪽 짝은 `pairing.rs::test_client::V2Client` 와
/// `PendingHandshake` 다.
///
/// 상태를 한 객체에 모아 둔 이유: transcript 는 **클라이언트 공개키가 먼저**이고
/// 논스는 세션 키의 salt 인데, 이 둘을 호출부마다 들고 다니면 순서를 뒤집거나
/// 논스를 잘못 짝지어도 컴파일은 통과한다 — 그러면 연결은 되는데 스냅샷이 한
/// 장도 안 열리는, 로그도 크래시도 없는 실패가 된다. 여기 묶어두면 두 전송이
/// 같은 조립을 공유한다.
///
/// `Sendable` 이 아니다 — 임시 개인키가 한 번만 쓰이는 가변 상태다.
public final class V2Handshake {
    /// 임시 개인키. `agree` 가 소비하고 nil 로 만든다 — 같은 키로 두 번
    /// 합의하면 임시 키가 임시가 아니게 된다.
    private var privateKey: Curve25519.KeyAgreement.PrivateKey?
    /// `HELLO2:`/`AUTH2:` 에 실어 보낸 바로 그 공개키. transcript 의 앞 32바이트다.
    public let clientPub: Data

    private var sharedSecret: Data?
    private var transcript: Data?
    private var nonce: Data?

    public init() {
        let key = Curve25519.KeyAgreement.PrivateKey()
        privateKey = key
        clientPub = key.publicKey.rawRepresentation
    }

    /// `AwaitingCode2`/`Nonce2` 의 `epk`·`nonce` 로 공유 비밀과 transcript 를
    /// 확정한다. 성공 여부를 돌려주며, 실패하면 이 객체는 아무 값도 내놓지 않는다.
    ///
    /// **저차 점을 거부한다.** Rust `crypto::agree` 는 `was_contributory()` 가
    /// 거짓이면 `None` 을 돌려준다. 클라이언트가 이걸 받아주면 양쪽은 공격자가
    /// 미리 아는 상수에 "합의" 한 것이 되고, 그 위에서 파생한 세션 키도 상수가 된다.
    @discardableResult
    public func agree(epkHex: String, nonceHex: String) -> Bool {
        guard let privateKey,
              let serverPub = Data(hexString: epkHex),
              let nonceBytes = Data(hexString: nonceHex),
              let peer = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: serverPub),
              let secret = try? privateKey.sharedSecretFromKeyAgreement(with: peer) else {
            return false
        }
        self.privateKey = nil
        let bytes = secret.withUnsafeBytes { Data($0) }
        // CryptoKit 도 저차 점에서 던지지만, 그 판단을 구현 세부에 맡기지 않는다 —
        // 비교 대상이 공개 상수(전부 0)라 조기 종료로 새는 비밀도 없다.
        guard bytes.contains(where: { $0 != 0 }) else { return false }
        sharedSecret = bytes
        transcript = CryptoV2.transcript(clientPub: clientPub, serverPub: serverPub)
        nonce = nonceBytes
        return true
    }

    /// `CODE2:` 에 실을 코드 바인딩. 코드 자체는 어느 방향으로도 링크를 건너지 않는다.
    public func codeBinding(code: String) -> Data? {
        guard let transcript else { return nil }
        return CryptoV2.codeBinding(code: code, transcript: transcript)
    }

    /// `PROOF2:` 에 실을 재연결 증명. 토큰은 hex 문자열이 아니라 **디코드한 원시
    /// 바이트**를 키로 쓴다(v1 `proofFrame` 과 같은 계약).
    public func sessionProof(tokenHex: String) -> Data? {
        guard let transcript, let nonce, let token = Data(hexString: tokenHex) else { return nil }
        return CryptoV2.sessionProof(token: token, nonce: nonce, transcript: transcript)
    }

    /// `Granted2.sealed` 를 열어 토큰을 꺼낸다. 맥은 페어링 채널을
    /// `SealedChannel::new(k_pair, k_pair)` 로 만든다 — 양방향이 같은 키다.
    public func openSealedToken(sealedHex: String) -> String? {
        guard let sharedSecret, let nonce, let frame = Data(hexString: sealedHex) else { return nil }
        let kPair = CryptoV2.derivePairKey(sharedSecret: sharedSecret, nonce: nonce)
        let channel = SealedChannel(sendKey: kPair, recvKey: kPair)
        guard let plaintext = try? channel.open(frame),
              let payload = try? JSONDecoder().decode(SealedTokenPayload.self, from: plaintext) else {
            return nil
        }
        return payload.token
    }

    /// 인가 직후의 세션 채널. **맥은 `(s2c, c2s)` 로 만들고 클라이언트는 뒤집어
    /// 넣는다**(스펙 6.1) — 이 뒤집기가 전송 계층에서 가장 틀리기 쉬운 지점이라
    /// 두 전송이 이 한 곳만 부르게 한다.
    public func sessionChannel(tokenHex: String) -> SealedChannel? {
        guard let sharedSecret, let nonce, let token = Data(hexString: tokenHex) else { return nil }
        let keys = CryptoV2.deriveSessionKeys(sharedSecret: sharedSecret, token: token, nonce: nonce)
        return SealedChannel(sendKey: keys.c2s, recvKey: keys.s2c)
    }
}

private struct SealedTokenPayload: Decodable {
    let token: String
}

public extension Data {
    /// 소문자/대문자 hex 문자열을 원시 바이트로 디코드한다. 길이가 홀수이거나
    /// hex 가 아닌 문자가 섞이면 nil — 여기서 패닉하면 안 된다(Rust 쪽
    /// `hex_decode` 가 원격 패닉을 냈던 것과 같은 실수를 반복하지 않는다).
    /// `NetworkClient`(다른 모듈)도 QR 로 받은 EndpointId hex 를 디코드하는 데
    /// 이걸 재사용한다.
    init?(hexString: String) {
        guard hexString.count % 2 == 0 else { return nil }
        var bytes = [UInt8]()
        bytes.reserveCapacity(hexString.count / 2)
        var idx = hexString.startIndex
        while idx < hexString.endIndex {
            let next = hexString.index(idx, offsetBy: 2)
            guard let byte = UInt8(hexString[idx..<next], radix: 16) else { return nil }
            bytes.append(byte)
            idx = next
        }
        self = Data(bytes)
    }
}
