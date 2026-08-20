import Foundation
import CryptoKit

/// Mac 의 `src-tauri/src/ble/pairing.rs` 가 해석하는 형식과 정확히 맞춰야 한다.
/// 프레임은 평문이고 짧다 — 이 채널로 오가는 것은 코드와 토큰뿐이다.
///
/// 실제로 나가는 여섯 모양(정본, 개정 참고): `{"ok":false,"await":"code"}`,
/// `{"ok":false,"nonce":"<hex>"}`, `{"ok":true}`(재인증 성공, 토큰 없음),
/// `{"ok":true,"token":"<hex>"}`(최초 인가), `{"ok":false,"left":<n>}`, `{"ok":false}`.
public struct AuthReplyPayload: Decodable, Equatable, Sendable {
    public let ok: Bool
    public let token: String?
    public let left: Int?
    /// Rust 가 보내는 키는 `await` 인데 Swift 예약어라 이름을 바꿔 받는다.
    public let awaiting: String?
    /// 재인증 2단계(AUTH 응답)로 받는 논스. 이 값을 `proofFrame` 에 그대로 넘긴다.
    public let nonce: String?

    private enum CodingKeys: String, CodingKey {
        case ok, token, left, nonce
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

extension Data {
    /// 소문자/대문자 hex 문자열을 원시 바이트로 디코드한다. 길이가 홀수이거나
    /// hex 가 아닌 문자가 섞이면 nil — 여기서 패닉하면 안 된다(Rust 쪽
    /// `hex_decode` 가 원격 패닉을 냈던 것과 같은 실수를 반복하지 않는다).
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
