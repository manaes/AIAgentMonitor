import Foundation
import Security
import os

/// 장치 레지스트리를 Keychain 항목 하나에 담는다.
///
/// access group 을 **쓰지 않는다** — 위젯이 없어 프로세스 간 공유가 없으므로
/// `$(AppIdentifierPrefix)` 치환값(팀 ID)을 코드에 들고 있을 이유가 없다.
public struct KeychainRegistryStore: DeviceRegistryStore {
    private static let logger = Logger(
        subsystem: "co.kr.wannypark.aiagentmonitor.multi", category: "KeychainRegistryStore"
    )

    private let service: String
    private let account = "device-registry"

    public init(service: String = "co.kr.wannypark.aiagentmonitor.multi") {
        self.service = service
    }

    func baseQuery() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public func read() throws -> Data? {
        var query = baseQuery()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var out: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &out)
        // "저장된 게 없다"와 "읽기 실패"를 구분해야 한다 — 후자를 nil 로 뭉개면
        // Keychain 잠김 등으로 16대 페어링이 조용히 날아간 것처럼 보인다.
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else {
            Self.logger.error("레지스트리 조회 실패 status=\(status, privacy: .public)")
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
        guard let data = out as? Data else {
            Self.logger.error("레지스트리 조회 결과 타입 불일치")
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
        return data
    }

    public func write(_ data: Data) throws {
        SecItemDelete(baseQuery() as CFDictionary)

        var query = baseQuery()
        query[kSecValueData as String] = data
        query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly

        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            Self.logger.error("레지스트리 저장 실패 status=\(status, privacy: .public)")
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
    }
}
